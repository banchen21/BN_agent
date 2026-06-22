//! PluginManager actor — loads / unloads / reloads plugin cdylibs at runtime,
//! broadcasts events to plugins, collects snapshots, proxies API requests,
//! and manages the shared ToolRegistry.

use actix::prelude::*;
use plugin_interface::*;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::Mutex;

// ── LoadedPlugin ─────────────────────────────────────────────────────────────

struct LoadedPlugin {
    info: PluginInfo,
    path: String,
    instance: Box<dyn Plugin>,
    _library: libloading::Library,
    tool_names: Vec<String>,
}

/// Snapshot all tool names currently in the registry.
fn snapshot_tool_names(registry: &Arc<Mutex<ToolRegistry>>) -> Vec<String> {
    let r = registry.lock();
    r.all_defs().iter().map(|d| d.name.clone()).collect()
}

/// Return tool names that appeared since `before` was taken.
fn diff_tool_names(registry: &Arc<Mutex<ToolRegistry>>, before: &[String]) -> Vec<String> {
    let now = snapshot_tool_names(registry);
    now.into_iter().filter(|n| !before.contains(n)).collect()
}

// ── PluginManager actor ──────────────────────────────────────────────────────

pub struct PluginManager {
    event_bus: Addr<EventBus>,
    llm_recipient: Option<Recipient<LlmRequest>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    plugins: HashMap<String, LoadedPlugin>,
    snapshots: Arc<Mutex<Vec<String>>>,
    host_ctx: Option<PluginContext>,
    plugin_dir: String,
    chat_store: Option<Recipient<ChatStoreMsg>>,
}

impl PluginManager {
    pub fn new(
        event_bus: Addr<EventBus>,
        llm_recipient: Option<Recipient<LlmRequest>>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        plugin_dir: String,
        chat_store: Option<Recipient<ChatStoreMsg>>,
    ) -> Self {
        let pm = Self {
            event_bus,
            llm_recipient,
            tool_registry,
            plugins: HashMap::new(),
            snapshots: Arc::new(Mutex::new(Vec::new())),
            host_ctx: None,
            plugin_dir,
            chat_store,
        };
        pm
    }

    pub fn snapshots_arc(&self) -> Arc<Mutex<Vec<String>>> {
        self.snapshots.clone()
    }

    /// Set the LLM recipient after construction (plugins started before LLM was ready need this).
    pub fn set_llm_recipient(&mut self, llm: Option<Recipient<LlmRequest>>) {
        self.llm_recipient = llm;
    }

    /// Auto-scan the plugin directory and load all plugins found.
    fn auto_scan(&mut self) {
        let dir = std::path::Path::new(&self.plugin_dir);
        if !dir.exists() {
            log::info!("[PluginManager] plugin dir not found: {}", self.plugin_dir);
            return;
        }

        // 环境变量 PLUGIN_SKIP：逗号分隔，跳过指定插件（如 "audio-capture-plugin,webrtc-plugin"）
        let skip_plugins: Vec<String> = std::env::var("PLUGIN_SKIP")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        // 环境变量 PLUGIN_LOAD：逗号分隔，非空时只加载这些插件
        let load_only: Vec<String> = std::env::var("PLUGIN_LOAD")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(e) => e.filter_map(|e| e.ok()).collect(),
            Err(e) => {
                log::warn!("[PluginManager] read_dir failed: {}", e);
                return;
            }
        };

        let mut count = 0;
        for entry in entries {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "dll" && ext != "so" && ext != "dylib" {
                continue;
            }

            // DLL 文件名如 "asr_tts_plugin.dll" → 插件名 "asr-tts-plugin"
            let plugin_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| {
                    let base = s.strip_suffix("_plugin").unwrap_or(s).replace('_', "-");
                    if base.ends_with("-plugin") {
                        base
                    } else {
                        format!("{}-plugin", base)
                    }
                })
                .unwrap_or_default();

            if !skip_plugins.is_empty() && skip_plugins.contains(&plugin_name) {
                log::info!("[PluginManager] skipping '{}' (PLUGIN_SKIP)", plugin_name);
                continue;
            }
            if !load_only.is_empty() && !load_only.contains(&plugin_name) {
                log::info!(
                    "[PluginManager] skipping '{}' (not in PLUGIN_LOAD)",
                    plugin_name
                );
                continue;
            }

            match load_plugin_file(
                &path,
                self.event_bus.clone(),
                self.llm_recipient.clone(),
                self.tool_registry.clone(),
                self.chat_store.clone(),
            ) {
                Ok(loaded) => {
                    log::info!("[PluginManager] auto-loaded '{}'", loaded.info.name);
                    let name = loaded.info.name.clone();
                    self.plugins.insert(name, loaded);
                    count += 1;
                }
                Err(e) => {
                    log::error!("[PluginManager] failed to load {}: {}", path.display(), e);
                }
            }
        }
        log::info!("[PluginManager] auto-loaded {} plugin(s)", count);
    }
}

impl Actor for PluginManager {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Subscribe to ALL events BEFORE loading plugins, so that plugin.start()
        // can use PluginLogger (which sends plugin.log events via EventBus).
        self.event_bus.do_send(Subscribe {
            topic: "*".into(),
            recipient: ctx.address().recipient(),
        });
        log::info!("[PluginManager] subscribed to '*' on EventBus");

        // Some plugins (e.g. claude-bridge-plugin) call Actor::start() for sub-actors.
        // This must happen inside the actix runtime, not during new(),
        // because Cdylib FFI boundary doesn't carry thread-local LocalSet.
        self.auto_scan();
        log::info!(
            "[PluginManager] actor started — {} plugin(s) loaded",
            self.plugins.len()
        );
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        log::info!(
            "[PluginManager] stopping — unloading {} plugin(s)",
            self.plugins.len()
        );
        for (_name, loaded) in self.plugins.iter_mut() {
            loaded.instance.stop();
        }
        {
            let mut reg = self.tool_registry.lock();
            reg.clear();
        }
        self.plugins.clear();
        Running::Stop
    }
}

// ── LoadPlugin ───────────────────────────────────────────────────────────────

impl Handler<LoadPlugin> for PluginManager {
    type Result = ResponseActFuture<Self, Result<PluginInfo, String>>;

    fn handle(&mut self, msg: LoadPlugin, _ctx: &mut Self::Context) -> Self::Result {
        let event_bus = self.event_bus.clone();
        let llm_recipient = self.llm_recipient.clone();
        let tool_registry = self.tool_registry.clone();
        let chat_store = self.chat_store.clone();
        let path = msg.path;

        let fut = async move {
            unsafe {
                let library = libloading::Library::new(&path)
                    .map_err(|e| format!("failed to load library '{}': {}", path, e))?;
                let create: libloading::Symbol<PluginCreateFn> = library
                    .get(b"plugin_create")
                    .map_err(|e| format!("symbol 'plugin_create' not found: {}", e))?;
                let mut plugin: Box<dyn Plugin> = create();
                let info = plugin.info();
                let plugin_name = info.name.clone();

                log::info!(
                    "[PluginManager] loaded plugin '{}' v{} — {}",
                    info.name,
                    info.version,
                    info.description
                );

                let tr_for_ctx = tool_registry.clone();
                let ctx = PluginContext {
                    event_bus: event_bus.clone(),
                    plugin_name: plugin_name.clone(),
                    llm: llm_recipient,
                    tool_registry: Some(tr_for_ctx),
                    logger: PluginLogger::new(event_bus.clone(), plugin_name.clone()),
                    chat_store,
                };
                let before = snapshot_tool_names(&tool_registry);
                plugin
                    .start(ctx)
                    .map_err(|e| format!("plugin.start() failed: {}", e))?;
                let tool_names = diff_tool_names(&tool_registry, &before);

                Ok(LoadedPlugin {
                    info,
                    path,
                    instance: plugin,
                    _library: library,
                    tool_names,
                })
            }
        }
        .into_actor(self)
        .map(
            |result: Result<LoadedPlugin, String>, this: &mut Self, _ctx| match result {
                Ok(loaded) => {
                    let info = loaded.info.clone();
                    let name = info.name.clone();
                    this.plugins.insert(name, loaded);
                    Ok(info)
                }
                Err(e) => Err(e),
            },
        );

        Box::pin(fut)
    }
}

// ── UnloadPlugin ─────────────────────────────────────────────────────────────

impl Handler<UnloadPlugin> for PluginManager {
    type Result = Result<(), String>;

    fn handle(&mut self, msg: UnloadPlugin, _ctx: &mut Self::Context) -> Self::Result {
        match self.plugins.remove(&msg.name) {
            Some(mut loaded) => {
                // Unregister tools before unloading.
                {
                    let mut reg = self.tool_registry.lock();
                    for name in &loaded.tool_names {
                        reg.unregister(name);
                        log::info!("[PluginManager] unregistered tool '{}'", name);
                    }
                }
                log::info!("[PluginManager] stopping plugin '{}'", msg.name);
                loaded.instance.stop();
                unsafe {
                    let destroy: libloading::Symbol<PluginDestroyFn> = loaded
                        ._library
                        .get(b"plugin_destroy")
                        .map_err(|e| format!("symbol 'plugin_destroy' not found: {}", e))?;
                    destroy(loaded.instance);
                }
                drop(loaded._library);
                log::info!("[PluginManager] unloaded plugin '{}'", msg.name);
                Ok(())
            }
            None => Err(format!("plugin '{}' is not loaded", msg.name)),
        }
    }
}

// ── ReloadPlugin ─────────────────────────────────────────────────────────────

impl Handler<ReloadPlugin> for PluginManager {
    type Result = ResponseActFuture<Self, Result<PluginInfo, String>>;

    fn handle(&mut self, msg: ReloadPlugin, _ctx: &mut Self::Context) -> Self::Result {
        let path = match self.plugins.get(&msg.name) {
            Some(loaded) => {
                log::info!("[PluginManager] reloading plugin '{}'", msg.name);
                loaded.path.clone()
            }
            None => {
                let fut = async move { Err(format!("plugin '{}' is not loaded", msg.name)) }
                    .into_actor(self);
                return Box::pin(fut);
            }
        };

        if let Some(mut loaded) = self.plugins.remove(&msg.name) {
            // Unregister old tools before unloading.
            {
                let mut reg = self.tool_registry.lock();
                for name in &loaded.tool_names {
                    reg.unregister(name);
                }
            }
            loaded.instance.stop();
            unsafe {
                let destroy: libloading::Symbol<PluginDestroyFn> =
                    loaded._library.get(b"plugin_destroy").unwrap();
                destroy(loaded.instance);
            }
            drop(loaded._library);
        }

        let event_bus = self.event_bus.clone();
        let llm_recipient = self.llm_recipient.clone();
        let tool_registry = self.tool_registry.clone();
        let chat_store = self.chat_store.clone();
        let fut = async move {
            unsafe {
                let library = libloading::Library::new(&path)
                    .map_err(|e| format!("failed to reload library '{}': {}", path, e))?;
                let create: libloading::Symbol<PluginCreateFn> = library
                    .get(b"plugin_create")
                    .map_err(|e| format!("symbol 'plugin_create' not found: {}", e))?;
                let mut plugin: Box<dyn Plugin> = create();
                let info = plugin.info();
                let plugin_name = info.name.clone();
                let tr_for_ctx = tool_registry.clone();
                let ctx = PluginContext {
                    event_bus: event_bus.clone(),
                    plugin_name: plugin_name.clone(),
                    llm: llm_recipient,
                    tool_registry: Some(tr_for_ctx),
                    logger: PluginLogger::new(event_bus.clone(), plugin_name.clone()),
                    chat_store,
                };
                let before = snapshot_tool_names(&tool_registry);
                plugin
                    .start(ctx)
                    .map_err(|e| format!("plugin.start() failed: {}", e))?;
                let tool_names = diff_tool_names(&tool_registry, &before);
                Ok(LoadedPlugin {
                    info,
                    path,
                    instance: plugin,
                    _library: library,
                    tool_names,
                })
            }
        }
        .into_actor(self)
        .map(
            |result: Result<LoadedPlugin, String>, this: &mut Self, _ctx| match result {
                Ok(loaded) => {
                    let info = loaded.info.clone();
                    let name = info.name.clone();
                    this.plugins.insert(name, loaded);
                    Ok(info)
                }
                Err(e) => Err(e),
            },
        );

        Box::pin(fut)
    }
}

// ── ListPlugins ──────────────────────────────────────────────────────────────

impl Handler<ListPlugins> for PluginManager {
    type Result = Vec<PluginInfo>;
    fn handle(&mut self, _msg: ListPlugins, _ctx: &mut Self::Context) -> Self::Result {
        self.plugins.values().map(|p| p.info.clone()).collect()
    }
}

// ── ScanAndLoad ──────────────────────────────────────────────────────────────

impl Handler<ScanAndLoad> for PluginManager {
    type Result = Result<usize, String>;

    fn handle(&mut self, msg: ScanAndLoad, _ctx: &mut Self::Context) -> Self::Result {
        let dir = std::path::Path::new(&msg.plugin_dir);
        if !dir.exists() {
            return Ok(0);
        }
        self.host_ctx = Some(msg.host_context);

        let mut count = 0;
        let entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
            Err(e) => return Err(format!("read_dir failed: {}", e)),
        };

        for entry in entries {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "dll" && ext != "so" && ext != "dylib" {
                continue;
            }
            match load_plugin_file(
                &path,
                self.event_bus.clone(),
                self.llm_recipient.clone(),
                self.tool_registry.clone(),
                self.chat_store.clone(),
            ) {
                Ok(loaded) => {
                    log::info!("[PluginManager] auto-loaded plugin '{}'", loaded.info.name);
                    let name = loaded.info.name.clone();
                    self.plugins.insert(name, loaded);
                    count += 1;
                }
                Err(e) => {
                    log::error!("[PluginManager] failed to load {}: {}", path.display(), e);
                }
            }
        }

        log::info!("[PluginManager] scan loaded {} plugin(s)", count);
        Ok(count)
    }
}

// ── Event — from EventBus "*" subscription ───────────────────────────────────

impl Handler<Event> for PluginManager {
    type Result = ();
    fn handle(&mut self, event: Event, _ctx: &mut Self::Context) {
        // Intercept plugin.log events and output via host logging.
        if event.topic == "plugin.log" {
            let level = event.data["level"].as_str().unwrap_or("info");
            let plugin = event.data["plugin"].as_str().unwrap_or("plugin");
            let message = event.data["message"].as_str().unwrap_or("");
            match level {
                "error" => log::error!("[{}] {}", plugin, message),
                "warn" => log::warn!("[{}] {}", plugin, message),
                _ => log::info!("[{}] {}", plugin, message),
            }
            return;
        }

        for loaded in self.plugins.values() {
            if !loaded.instance.on_event(&event) {
                break;
            }
        }
    }
}

// ── BroadcastEvent — 不再使用，保留接口以防外部调用 ─────

impl Handler<BroadcastEvent> for PluginManager {
    type Result = ();
    fn handle(&mut self, msg: BroadcastEvent, _ctx: &mut Self::Context) {
        for loaded in self.plugins.values() {
            if !loaded.instance.on_event(&msg.0) {
                break;
            }
        }
    }
}

// ── RefreshSnapshots ─────────────────────────────────────────────────────────

impl Handler<RefreshSnapshots> for PluginManager {
    type Result = ();
    fn handle(&mut self, _: RefreshSnapshots, _: &mut Self::Context) {
        let mut snap = self.snapshots.lock();
        snap.clear();
        for loaded in self.plugins.values() {
            if let Some(s) = loaded.instance.snapshot() {
                snap.push(s);
            }
        }
    }
}

impl Handler<RefreshSnapshotsForPeer> for PluginManager {
    type Result = ();
    fn handle(&mut self, msg: RefreshSnapshotsForPeer, _: &mut Self::Context) {
        let mut snap = self.snapshots.lock();
        snap.clear();
        for loaded in self.plugins.values() {
            if let Some(s) = loaded.instance.snapshot_for_peer(&msg.peer_id) {
                snap.push(s);
            }
        }
    }
}

// ── ApiRequest ───────────────────────────────────────────────────────────────

impl Handler<ApiRequest> for PluginManager {
    type Result = Option<(u16, String)>;
    fn handle(&mut self, msg: ApiRequest, _ctx: &mut Self::Context) -> Self::Result {
        let loaded = self.plugins.get(&msg.plugin)?;
        let api = loaded.instance.api_handler()?;
        api.handle_api(&msg.method, &msg.path, msg.body.as_deref())
    }
}

// ── StopAll ──────────────────────────────────────────────────────────────────

// ── GetLlmBackend ─────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "Option<LlmBackend>")]
pub struct GetLlmBackend;

impl Handler<GetLlmBackend> for PluginManager {
    type Result = Option<LlmBackend>;

    fn handle(&mut self, _: GetLlmBackend, _ctx: &mut Self::Context) -> Self::Result {
        for loaded in self.plugins.values() {
            if let Some(backend) = loaded.instance.llm_backend() {
                log::info!(
                    "[PluginManager] LLM backend found: plugin={}",
                    loaded.info.name
                );
                return Some(backend);
            }
        }
        None
    }
}

// ── StopAll ──────────────────────────────────────────────────────────────────

impl Handler<StopAll> for PluginManager {
    type Result = ();
    fn handle(&mut self, _: StopAll, _ctx: &mut Self::Context) {
        for (_name, loaded) in self.plugins.iter_mut() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                loaded.instance.stop();
            }));
        }
        {
            let mut reg = self.tool_registry.lock();
            reg.clear();
            log::info!("[PluginManager] tool registry cleared");
        }
        let mut libs = Vec::new();
        for (_name, loaded) in self.plugins.drain() {
            drop(loaded.instance);
            libs.push(loaded._library);
        }
        drop(libs);
    }
}

// ── Helper: load a single plugin file (synchronous) ──────────────────────────

fn load_plugin_file(
    path: &std::path::Path,
    event_bus: Addr<EventBus>,
    llm_recipient: Option<Recipient<LlmRequest>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    chat_store: Option<Recipient<ChatStoreMsg>>,
) -> Result<LoadedPlugin, String> {
    unsafe {
        let library =
            libloading::Library::new(path).map_err(|e| format!("{}: {}", path.display(), e))?;

        let create: libloading::Symbol<PluginCreateFn> = library
            .get(b"plugin_create")
            .map_err(|e| format!("symbol 'plugin_create' not found: {}", e))?;

        let mut plugin: Box<dyn Plugin> = create();
        let info = plugin.info();
        let plugin_name = info.name.clone();

        let tr_for_ctx = tool_registry.clone();
        let ctx = PluginContext {
            event_bus: event_bus.clone(),
            plugin_name: plugin_name.clone(),
            llm: llm_recipient,
            tool_registry: Some(tr_for_ctx),
            logger: PluginLogger::new(event_bus.clone(), plugin_name.clone()),
            chat_store,
        };

        let before = snapshot_tool_names(&tool_registry);
        plugin
            .start(ctx)
            .map_err(|e| format!("plugin.start() failed: {}", e))?;
        let tool_names = diff_tool_names(&tool_registry, &before);

        let file_path = path.to_string_lossy().to_string();
        Ok(LoadedPlugin {
            info,
            path: file_path,
            instance: plugin,
            _library: library,
            tool_names,
        })
    }
}
