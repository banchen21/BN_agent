//! PluginManager actor — loads / unloads / reloads plugin cdylibs at runtime,
//! broadcasts events to plugins, collects snapshots, proxies API requests,
//! and manages the shared ToolRegistry.

use crate::plugin_config::PluginConfigEntry;
use actix::prelude::*;
use parking_lot::Mutex;
use plugin_interface::config as pi_config;
use plugin_interface::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// ── LoadedPlugin ─────────────────────────────────────────────────────────────

struct LoadedPlugin {
    info: PluginInfo,
    path: String,
    instance: Box<dyn Plugin>,
    /// 测试注入的桩插件没有动态库；真实插件始终为 Some。
    library: Option<libloading::Library>,
    tool_names: Vec<String>,
}

/// 插件配置条目（schema 随插件动态提供）。
fn config_entry(loaded: &LoadedPlugin) -> PluginConfigEntry {
    let schema = loaded.instance.config_schema();
    let title = schema
        .as_ref()
        .map(|s| s.title.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| loaded.info.name.clone());
    let description = schema
        .as_ref()
        .map(|s| s.description.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| loaded.info.description.clone());
    PluginConfigEntry {
        name: loaded.info.name.clone(),
        title,
        description,
        schema,
    }
}

/// 构造插件上下文：读取 `data/plugin_configs/<name>.json` 并按 schema 合并默认值。
/// 没有 schema / 文件损坏都不会失败，最坏情况是空配置 + 日志警告。
fn build_plugin_context(
    event_bus: Addr<EventBus>,
    llm_recipient: Option<Recipient<LlmRequest>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    chat_store: Option<Recipient<ChatStoreMsg>>,
    plugin_name: &str,
    schema: Option<&PluginConfigSchema>,
) -> PluginContext {
    let config_dir = pi_config::config_dir();
    let (config, warning) = pi_config::load_merged(plugin_name, schema, &config_dir);
    if let Some(warning) = warning {
        log::warn!("[PluginManager] config for '{}': {}", plugin_name, warning);
    }
    PluginContext {
        event_bus: event_bus.clone(),
        plugin_name: plugin_name.to_string(),
        llm: llm_recipient,
        tool_registry: Some(tool_registry),
        logger: PluginLogger::new(event_bus, plugin_name.to_string()),
        chat_store,
        config,
    }
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

/// 期望工具中缺失的部分（reload 后校验用）。
fn missing_tools(expected: &[String], actual: &[String]) -> Vec<String> {
    expected
        .iter()
        .filter(|name| !actual.iter().any(|a| a == *name))
        .cloned()
        .collect()
}

/// 卸载一个已加载实例：注销其工具、stop、销毁动态库。
/// 用于 reload 校验失败后清理"看似成功但实际不可用"的新实例。
fn unload_loaded(mut loaded: LoadedPlugin, tool_registry: &Arc<Mutex<ToolRegistry>>) {
    let name = loaded.info.name.clone();
    log::info!(
        "[PluginManager] unloading failed/invalid instance of '{}' ({} tool(s))",
        name,
        loaded.tool_names.len()
    );
    {
        let mut reg = tool_registry.lock();
        for tool in &loaded.tool_names {
            reg.unregister(tool);
        }
    }
    log::info!("[PluginManager] '{}' stop() start", name);
    loaded.instance.stop();
    log::info!("[PluginManager] '{}' stop() done", name);
    if let Some(library) = loaded.library.take() {
        unsafe {
            match library.get::<PluginDestroyFn>(b"plugin_destroy") {
                Ok(destroy) => destroy(loaded.instance),
                Err(e) => log::warn!(
                    "[PluginManager] plugin_destroy lookup failed for '{}': {}",
                    name,
                    e
                ),
            }
        }
        log::info!("[PluginManager] '{}' destroy done, keeping library loaded", name);
        // 不 FreeLibrary：该实例只会在 reload 校验失败时走到这里，
        // 立即卸载可能留下未完成的插件线程/析构，Windows 上会偶发访问冲突。
        // 失败路径极少见，泄漏句柄换取进程稳定。
        std::mem::forget(library);
        log::info!("[PluginManager] '{}' library intentionally leaked", name);
    }
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
    /// 最近一次成功加载的路径与工具集合。reload 失败/插件被卸载后，
    /// 配置回滚需要据此重新拉起旧实例并校验工具。
    last_paths: HashMap<String, String>,
    last_tool_names: HashMap<String, Vec<String>>,
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
            last_paths: HashMap::new(),
            last_tool_names: HashMap::new(),
        };
        pm
    }

    /// 记录一次成功加载，供后续 reload/回滚使用。
    fn remember_loaded(&mut self, loaded: &LoadedPlugin) {
        if !loaded.path.is_empty() {
            self.last_paths
                .insert(loaded.info.name.clone(), loaded.path.clone());
        }
        self.last_tool_names
            .insert(loaded.info.name.clone(), loaded.tool_names.clone());
    }

    /// 解析插件路径：优先当前实例，其次最近一次成功加载的路径。
    fn resolve_plugin_path(&self, name: &str) -> Option<String> {
        self.plugins
            .get(name)
            .map(|loaded| loaded.path.clone())
            .filter(|path| !path.is_empty())
            .or_else(|| self.last_paths.get(name).cloned())
    }

    /// reload 后应存在的工具集合。
    fn expected_tools(&self, name: &str) -> Vec<String> {
        self.plugins
            .get(name)
            .map(|loaded| loaded.tool_names.clone())
            .or_else(|| self.last_tool_names.get(name).cloned())
            .unwrap_or_default()
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
                    self.remember_loaded(&loaded);
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
            load_plugin_file(
                Path::new(&path),
                event_bus,
                llm_recipient,
                tool_registry,
                chat_store,
            )
        }
        .into_actor(self)
        .map(
            |result: Result<LoadedPlugin, String>, this: &mut Self, _ctx| match result {
                Ok(loaded) => {
                    let info = loaded.info.clone();
                    let name = info.name.clone();
                    this.remember_loaded(&loaded);
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
                // 显式卸载：不再保留路径/工具记忆，避免后续 reload 意外拉回。
                self.last_paths.remove(&msg.name);
                self.last_tool_names.remove(&msg.name);
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
                if let Some(library) = loaded.library.take() {
                    unsafe {
                        let destroy: libloading::Symbol<PluginDestroyFn> = library
                            .get(b"plugin_destroy")
                            .map_err(|e| format!("symbol 'plugin_destroy' not found: {}", e))?;
                        destroy(loaded.instance);
                    }
                    drop(library);
                }
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
        let path = match self.resolve_plugin_path(&msg.name) {
            Some(path) => {
                log::info!("[PluginManager] reloading plugin '{}'", msg.name);
                path
            }
            None => {
                let fut = async move {
                    Err(format!(
                        "plugin '{}' is not loaded and has no known path",
                        msg.name
                    ))
                }
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
            if let Some(library) = loaded.library.take() {
                unsafe {
                    let destroy: libloading::Symbol<PluginDestroyFn> =
                        library.get(b"plugin_destroy").unwrap();
                    destroy(loaded.instance);
                }
                drop(library);
            }
        }

        let event_bus = self.event_bus.clone();
        let llm_recipient = self.llm_recipient.clone();
        let tool_registry = self.tool_registry.clone();
        let chat_store = self.chat_store.clone();
        let fut = async move {
            match load_plugin_file(
                Path::new(&path),
                event_bus.clone(),
                llm_recipient.clone(),
                tool_registry.clone(),
                chat_store.clone(),
            ) {
                Ok(loaded) => Ok(loaded),
                Err(e) => {
                    // 尽力恢复旧实例，避免插件处于"半加载"状态。
                    // 失败原因若是 DLL 本身损坏，恢复也会失败——此时明确报告插件已卸载。
                    match load_plugin_file(
                        Path::new(&path),
                        event_bus,
                        llm_recipient,
                        tool_registry,
                        chat_store,
                    ) {
                        Ok(old) => Err((e, Some(old))),
                        Err(restore_err) => Err((
                            format!("{}; restore attempt failed: {}", e, restore_err),
                            None,
                        )),
                    }
                }
            }
        }
        .into_actor(self)
        .map(
            |result: Result<LoadedPlugin, (String, Option<LoadedPlugin>)>,
             this: &mut Self,
             _ctx| match result {
                Ok(loaded) => {
                    let info = loaded.info.clone();
                    let name = info.name.clone();
                    this.remember_loaded(&loaded);
                    this.plugins.insert(name, loaded);
                    Ok(info)
                }
                Err((e, Some(old))) => {
                    let info = old.info.clone();
                    let name = info.name.clone();
                    this.remember_loaded(&old);
                    this.plugins.insert(name, old);
                    Err(format!("{}; previous instance restored", e))
                }
                Err((e, None)) => Err(format!("{}; plugin is now unloaded", e)),
            },
        );

        Box::pin(fut)
    }
}

// ── ReloadPluginVerified ─────────────────────────────────────────────────────
//
// 配置保存专用 reload：加载后不仅要求 start() 成功，还要求插件声明的工具
// 仍在 registry 中（旧实例注册过的工具在新实例中必须重新出现）。用于捕获
// "start() 返回 Ok 但内部初始化失败、工具未注册" 的静默失败。

#[derive(Message)]
#[rtype(result = "Result<PluginInfo, VerifiedReloadError>")]
pub struct ReloadPluginVerified {
    pub name: String,
}

/// verified reload 的结构化错误。
#[derive(Debug, Clone)]
pub struct VerifiedReloadError {
    /// 主错误（插件启动失败 / 工具校验失败）。
    pub error: String,
    /// 首次失败后尝试用当前（尚未回滚的）配置恢复实例时的错误。
    /// 调用方通常随后回滚配置并重试；重试成功时此项仅为诊断信息。
    pub restore_attempt_error: Option<String>,
    /// 本次返回时插件是否处于卸载状态（false = 旧实例已恢复）。
    pub plugin_unloaded: bool,
}

impl Handler<ReloadPluginVerified> for PluginManager {
    type Result = ResponseActFuture<Self, Result<PluginInfo, VerifiedReloadError>>;

    fn handle(&mut self, msg: ReloadPluginVerified, _ctx: &mut Self::Context) -> Self::Result {
        let name = msg.name;
        let path = match self.resolve_plugin_path(&name) {
            Some(path) => path,
            None => {
                let fut = async move {
                    Err(VerifiedReloadError {
                        error: format!(
                            "plugin '{}' is not loaded and has no known path",
                            name
                        ),
                        restore_attempt_error: None,
                        plugin_unloaded: true,
                    })
                }
                .into_actor(self);
                return Box::pin(fut);
            }
        };
        let expected_tools = self.expected_tools(&name);

        if let Some(mut loaded) = self.plugins.remove(&name) {
            {
                let mut reg = self.tool_registry.lock();
                for tool in &loaded.tool_names {
                    reg.unregister(tool);
                }
            }
            loaded.instance.stop();
            if let Some(library) = loaded.library.take() {
                unsafe {
                    let destroy: libloading::Symbol<PluginDestroyFn> =
                        library.get(b"plugin_destroy").unwrap();
                    destroy(loaded.instance);
                }
                drop(library);
            }
        }

        log::info!(
            "[PluginManager] verified reload of '{}' (expected {} tool(s))",
            name,
            expected_tools.len()
        );

        let event_bus = self.event_bus.clone();
        let llm_recipient = self.llm_recipient.clone();
        let tool_registry = self.tool_registry.clone();
        let chat_store = self.chat_store.clone();
        let fut = async move {
            let first = load_plugin_file(
                Path::new(&path),
                event_bus.clone(),
                llm_recipient.clone(),
                tool_registry.clone(),
                chat_store.clone(),
            );

            match first {
                Ok(loaded) if loaded.info.name != name => {
                    let err = format!(
                        "reload verification failed: loaded plugin '{}' instead of '{}'",
                        loaded.info.name, name
                    );
                    unload_loaded(loaded, &tool_registry);
                    Err((err, None, None))
                }
                Ok(loaded) => {
                    let missing = missing_tools(&expected_tools, &loaded.tool_names);
                    if missing.is_empty() {
                        return Ok(loaded);
                    }
                    let err = format!(
                        "reload verification failed: tools missing after start: {}",
                        missing.join(", ")
                    );
                    log::error!("[PluginManager] {} (plugin '{}')", err, name);
                    unload_loaded(loaded, &tool_registry);

                    // 用当前磁盘配置再恢复一次；若调用方随后回滚配置并再次
                    // ReloadPluginVerified，会以旧配置成功恢复。
                    match load_plugin_file(
                        Path::new(&path),
                        event_bus,
                        llm_recipient,
                        tool_registry.clone(),
                        chat_store,
                    ) {
                        Ok(old) if old.info.name == name => {
                            let missing_restore =
                                missing_tools(&expected_tools, &old.tool_names);
                            if missing_restore.is_empty() {
                                Err((err, None, Some(old)))
                            } else {
                                let restore_error = format!(
                                    "restored instance also missing tools: {}",
                                    missing_restore.join(", ")
                                );
                                log::error!("[PluginManager] {}", restore_error);
                                unload_loaded(old, &tool_registry);
                                Err((err, Some(restore_error), None))
                            }
                        }
                        Ok(other) => {
                            unload_loaded(other, &tool_registry);
                            Err((
                                err,
                                Some("restore loaded an unexpected plugin".to_string()),
                                None,
                            ))
                        }
                        Err(restore_err) => Err((err, Some(restore_err), None)),
                    }
                }
                Err(e) => match load_plugin_file(
                    Path::new(&path),
                    event_bus,
                    llm_recipient,
                    tool_registry.clone(),
                    chat_store,
                ) {
                    Ok(old) if old.info.name == name => {
                        let missing_restore = missing_tools(&expected_tools, &old.tool_names);
                        if missing_restore.is_empty() {
                            Err((e, None, Some(old)))
                        } else {
                            let restore_error = format!(
                                "restored instance also missing tools: {}",
                                missing_restore.join(", ")
                            );
                            log::error!("[PluginManager] {}", restore_error);
                            unload_loaded(old, &tool_registry);
                            Err((e, Some(restore_error), None))
                        }
                    }
                    Ok(other) => {
                        unload_loaded(other, &tool_registry);
                        Err((
                            e,
                            Some("restore loaded an unexpected plugin".to_string()),
                            None,
                        ))
                    }
                    Err(restore_err) => Err((e, Some(restore_err), None)),
                },
            }
        }
        .into_actor(self)
        .map(
            |result: Result<LoadedPlugin, (String, Option<String>, Option<LoadedPlugin>)>,
             this: &mut Self,
             _ctx| match result {
                Ok(loaded) => {
                    let info = loaded.info.clone();
                    let name = info.name.clone();
                    this.remember_loaded(&loaded);
                    this.plugins.insert(name.clone(), loaded);
                    log::info!("[PluginManager] verified reload OK for '{}'", name);
                    Ok(info)
                }
                Err((error, restore_attempt_error, Some(old))) => {
                    let info = old.info.clone();
                    let name = info.name.clone();
                    this.remember_loaded(&old);
                    this.plugins.insert(name.clone(), old);
                    log::warn!(
                        "[PluginManager] verified reload failed for '{}' but previous instance restored: {}",
                        name,
                        error
                    );
                    Err(VerifiedReloadError {
                        error,
                        restore_attempt_error,
                        plugin_unloaded: false,
                    })
                }
                Err((error, restore_attempt_error, None)) => {
                    log::error!(
                        "[PluginManager] verified reload failed and plugin unloaded: {} (restore: {:?})",
                        error,
                        restore_attempt_error
                    );
                    Err(VerifiedReloadError {
                        error,
                        restore_attempt_error,
                        plugin_unloaded: true,
                    })
                }
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
                    self.remember_loaded(&loaded);
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
            if let Some(library) = loaded.library {
                libs.push(library);
            }
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
        let schema = plugin.config_schema();

        log::info!(
            "[PluginManager] loaded plugin '{}' v{} — {}",
            info.name,
            info.version,
            info.description
        );

        let ctx = build_plugin_context(
            event_bus,
            llm_recipient,
            tool_registry.clone(),
            chat_store,
            &plugin_name,
            schema.as_ref(),
        );

        let before = snapshot_tool_names(&tool_registry);
        if let Err(e) = plugin.start(ctx) {
            // start 失败：清理它失败前已注册的工具，避免 registry 残留
            // 指向已销毁插件实例的工具，影响后续恢复校验。
            let partial = diff_tool_names(&tool_registry, &before);
            if !partial.is_empty() {
                let mut reg = tool_registry.lock();
                for name in &partial {
                    reg.unregister(name);
                }
                log::warn!(
                    "[PluginManager] plugin '{}' failed to start; unregistered partial tools: {}",
                    plugin_name,
                    partial.join(", ")
                );
            }
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.stop()));
            // Windows 下卸载一个 start() 失败的 cdylib 可能触发访问冲突：
            // 实例与 Library 句柄故意泄漏，模块保持加载，后续 reload 复用。
            // 失败路径极少发生，泄漏一个句柄可接受；成功实例仍正常卸载。
            std::mem::forget(plugin);
            std::mem::forget(library);
            log::warn!(
                "[PluginManager] plugin '{}' failed to start; instance/library kept loaded (leaked) to avoid unsafe unload",
                plugin_name
            );
            return Err(format!("plugin.start() failed: {}", e));
        }
        let tool_names = diff_tool_names(&tool_registry, &before);

        let file_path = path.to_string_lossy().to_string();
        Ok(LoadedPlugin {
            info,
            path: file_path,
            instance: plugin,
            library: Some(library),
            tool_names,
        })
    }
}

// ── Plugin config schema queries ────────────────────────────────────────────

/// 列出所有已加载插件及其配置 schema（无 schema 的插件也会出现，`editable=false`）。
#[derive(Message)]
#[rtype(result = "Vec<PluginConfigEntry>")]
pub struct ListPluginConfigSchemas;

/// 查询单个插件的配置 schema。
#[derive(Message)]
#[rtype(result = "Option<PluginConfigEntry>")]
pub struct GetPluginConfigSchema {
    pub name: String,
}

impl Handler<ListPluginConfigSchemas> for PluginManager {
    type Result = Vec<PluginConfigEntry>;

    fn handle(&mut self, _: ListPluginConfigSchemas, _: &mut Self::Context) -> Self::Result {
        let mut entries: Vec<PluginConfigEntry> =
            self.plugins.values().map(config_entry).collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }
}

impl Handler<GetPluginConfigSchema> for PluginManager {
    type Result = Option<PluginConfigEntry>;

    fn handle(&mut self, msg: GetPluginConfigSchema, _: &mut Self::Context) -> Self::Result {
        self.plugins.get(&msg.name).map(config_entry)
    }
}

// ── Test helpers ────────────────────────────────────────────────────────────

/// 测试专用：注入一个不依赖 cdylib 的桩插件，用于端到端验证配置 API。
#[cfg(test)]
#[derive(Message)]
#[rtype(result = "()")]
pub struct InsertTestPlugin(pub Box<dyn Plugin>);

#[cfg(test)]
impl Handler<InsertTestPlugin> for PluginManager {
    type Result = ();

    fn handle(&mut self, msg: InsertTestPlugin, _: &mut Self::Context) {
        let info = msg.0.info();
        let name = info.name.clone();
        self.plugins.insert(
            name,
            LoadedPlugin {
                info,
                path: String::new(),
                instance: msg.0,
                library: None,
                tool_names: Vec::new(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::missing_tools;

    #[test]
    fn missing_tools_empty_when_all_present() {
        let expected = vec!["a".to_string(), "b".to_string()];
        let actual = vec!["b".to_string(), "a".to_string(), "c".to_string()];
        assert!(missing_tools(&expected, &actual).is_empty());
    }

    #[test]
    fn missing_tools_lists_absent_names() {
        let expected = vec![
            "comfyui_generate_image".to_string(),
            "comfyui_edit_image".to_string(),
        ];
        let actual = vec!["generate_image".to_string()];
        let missing = missing_tools(&expected, &actual);
        assert_eq!(
            missing,
            vec![
                "comfyui_generate_image".to_string(),
                "comfyui_edit_image".to_string()
            ]
        );
    }

    #[test]
    fn missing_tools_with_empty_expected_is_empty() {
        assert!(missing_tools(&[], &["x".to_string()]).is_empty());
    }
}
