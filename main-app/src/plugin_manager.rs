//! PluginManager actor — loads / unloads / reloads plugin cdylibs at runtime,
//! broadcasts events to plugins, collects snapshots, proxies API requests,
//! and manages the shared ToolRegistry.

use actix::prelude::*;
use plugin_interface::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── LoadedPlugin ─────────────────────────────────────────────────────────────

struct LoadedPlugin {
    info: PluginInfo,
    path: String,
    instance: Box<dyn Plugin>,
    _library: libloading::Library,
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
}

impl PluginManager {
    pub fn new(
        event_bus: Addr<EventBus>,
        llm_recipient: Option<Recipient<LlmRequest>>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        plugin_dir: String,
    ) -> Self {
        Self {
            event_bus,
            llm_recipient,
            tool_registry,
            plugins: HashMap::new(),
            snapshots: Arc::new(Mutex::new(Vec::new())),
            host_ctx: None,
            plugin_dir,
        }
    }

    pub fn snapshots_arc(&self) -> Arc<Mutex<Vec<String>>> {
        self.snapshots.clone()
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
            let plugin_name = path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| {
                    let base = s.strip_suffix("_plugin").unwrap_or(s).replace('_', "-");
                    if base.ends_with("-plugin") { base } else { format!("{}-plugin", base) }
                })
                .unwrap_or_default();

            if !skip_plugins.is_empty() && skip_plugins.contains(&plugin_name) {
                log::info!("[PluginManager] skipping '{}' (PLUGIN_SKIP)", plugin_name);
                continue;
            }
            if !load_only.is_empty() && !load_only.contains(&plugin_name) {
                log::info!("[PluginManager] skipping '{}' (not in PLUGIN_LOAD)", plugin_name);
                continue;
            }

            match load_plugin_file(
                &path,
                self.event_bus.clone(),
                self.llm_recipient.clone(),
                self.tool_registry.clone(),
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
        log::info!("[PluginManager] actor started — scanning plugins...");

        // Subscribe to ALL events on EventBus, forwarding to plugins via on_event().
        // This is the only way DLL-based plugins can receive events (they can't
        // start their own actix actors, which would need a different tokio runtime).
        self.event_bus.do_send(Subscribe {
            topic: "*".into(),
            recipient: ctx.address().recipient(),
        });
        log::info!("[PluginManager] subscribed to '*' on EventBus");

        self.auto_scan();
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        log::info!("[PluginManager] stopping — unloading {} plugin(s)", self.plugins.len());
        for (_name, loaded) in self.plugins.iter_mut() {
            loaded.instance.stop();
        }
        if let Ok(mut reg) = self.tool_registry.lock() {
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
                    info.name, info.version, info.description
                );

                let ctx = PluginContext {
                    event_bus,
                    plugin_name: plugin_name.clone(),
                    llm: llm_recipient,
                    tool_registry: Some(tool_registry),
                };
                plugin.start(ctx).map_err(|e| format!("plugin.start() failed: {}", e))?;

                Ok(LoadedPlugin { info, path, instance: plugin, _library: library })
            }
        }
        .into_actor(self)
        .map(|result: Result<LoadedPlugin, String>, this: &mut Self, _ctx| match result {
            Ok(loaded) => {
                let info = loaded.info.clone();
                let name = info.name.clone();
                this.plugins.insert(name, loaded);
                Ok(info)
            }
            Err(e) => Err(e),
        });

        Box::pin(fut)
    }
}

// ── UnloadPlugin ─────────────────────────────────────────────────────────────

impl Handler<UnloadPlugin> for PluginManager {
    type Result = Result<(), String>;

    fn handle(&mut self, msg: UnloadPlugin, _ctx: &mut Self::Context) -> Self::Result {
        match self.plugins.remove(&msg.name) {
            Some(mut loaded) => {
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
                let fut =
                    async move { Err(format!("plugin '{}' is not loaded", msg.name)) }
                        .into_actor(self);
                return Box::pin(fut);
            }
        };

        if let Some(mut loaded) = self.plugins.remove(&msg.name) {
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
                let ctx = PluginContext {
                    event_bus,
                    plugin_name: plugin_name.clone(),
                    llm: llm_recipient,
                    tool_registry: Some(tool_registry),
                };
                plugin.start(ctx).map_err(|e| format!("plugin.start() failed: {}", e))?;
                Ok(LoadedPlugin { info, path, instance: plugin, _library: library })
            }
        }
        .into_actor(self)
        .map(|result: Result<LoadedPlugin, String>, this: &mut Self, _ctx| match result {
            Ok(loaded) => {
                let info = loaded.info.clone();
                let name = info.name.clone();
                this.plugins.insert(name, loaded);
                Ok(info)
            }
            Err(e) => Err(e),
        });

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
        let mut snap = self.snapshots.lock().unwrap();
        snap.clear();
        for loaded in self.plugins.values() {
            if let Some(s) = loaded.instance.snapshot() {
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

impl Handler<StopAll> for PluginManager {
    type Result = ();
    fn handle(&mut self, _: StopAll, _ctx: &mut Self::Context) {
        for (_name, loaded) in self.plugins.iter_mut() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                loaded.instance.stop();
            }));
        }
        if let Ok(mut reg) = self.tool_registry.lock() {
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
) -> Result<LoadedPlugin, String> {
    unsafe {
        let library = libloading::Library::new(path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;

        let create: libloading::Symbol<PluginCreateFn> = library
            .get(b"plugin_create")
            .map_err(|e| format!("symbol 'plugin_create' not found: {}", e))?;

        let mut plugin: Box<dyn Plugin> = create();
        let info = plugin.info();
        let plugin_name = info.name.clone();

        let ctx = PluginContext {
            event_bus,
            plugin_name,
            llm: llm_recipient,
            tool_registry: Some(tool_registry),
        };

        plugin.start(ctx).map_err(|e| format!("plugin.start() failed: {}", e))?;

        let file_path = path.to_string_lossy().to_string();
        Ok(LoadedPlugin { info, path: file_path, instance: plugin, _library: library })
    }
}
