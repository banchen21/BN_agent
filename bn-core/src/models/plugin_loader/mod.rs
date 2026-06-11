//! 插件加载器 Actor — 基于 actix + libloading 的动态插件管理

use actix::prelude::*;
use plugin_core::{AgentEvent, HostContext, Plugin, PluginApi, ToolRegistry};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 用 Arc 包装 Library，确保在 Actor drop 后 Library 仍然存活
type SharedLib = Arc<libloading::Library>;

pub struct PluginManager {
    plugin_dir: String,
    loaded: HashMap<String, (SharedLib, Box<dyn Plugin>)>,
    tool_registry: Option<Arc<Mutex<ToolRegistry>>>,
    /// 共享的 snapshot 容器：每个插件一个条目，逐轮刷新
    snapshots: Arc<Mutex<Vec<String>>>,
}

impl PluginManager {
    pub fn new(plugin_dir: &str) -> Self {
        Self {
            plugin_dir: plugin_dir.to_string(),
            loaded: HashMap::new(),
            tool_registry: None,
            snapshots: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn snapshots_arc(&self) -> Arc<Mutex<Vec<String>>> {
        self.snapshots.clone()
    }
}

impl Actor for PluginManager {
    type Context = Context<Self>;
}

#[derive(Message)]
#[rtype(result = "Result<usize, String>")]
pub struct ScanAndLoad(pub HostContext);

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetToolRegistry(pub Arc<Mutex<ToolRegistry>>);

#[derive(Message)]
#[rtype(result = "()")]
pub struct BroadcastEvent(pub AgentEvent);

#[derive(Message)]
#[rtype(result = "()")]
pub struct StopAll;

impl Handler<SetToolRegistry> for PluginManager {
    type Result = ();
    fn handle(&mut self, msg: SetToolRegistry, _: &mut Self::Context) {
        self.tool_registry = Some(msg.0);
    }
}

impl Handler<ScanAndLoad> for PluginManager {
    type Result = Result<usize, String>;

    fn handle(&mut self, msg: ScanAndLoad, _: &mut Self::Context) -> Self::Result {
        let ctx = msg.0;
        let dir = std::path::Path::new(&self.plugin_dir);
        if !dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "dll" && ext != "so" && ext != "dylib" {
                continue;
            }

            match load_plugin(&path, &ctx) {
                Ok((name, lib, plugin)) => {
                    tracing::info!("已加载插件: {}", name);
                    self.loaded.insert(name, (lib, plugin));
                    count += 1;
                }
                Err(e) => {
                    tracing::error!("加载插件失败 {}: {}", path.display(), e);
                }
            }
        }
        Ok(count)
    }
}

impl Handler<BroadcastEvent> for PluginManager {
    type Result = ();
    fn handle(&mut self, msg: BroadcastEvent, _: &mut Self::Context) {
        for (_, (_, plugin)) in &self.loaded {
            if !plugin.on_event(&msg.0) {
                // 插件返回 false → 拦截，不再广播给后续插件
                break;
            }
        }
    }
}

/// 刷新被动上下文快照（从 Plugin trait 的 snapshot() 收集）
#[derive(Message)]
#[rtype(result = "()")]
pub struct RefreshSnapshots;

impl Handler<RefreshSnapshots> for PluginManager {
    type Result = ();
    fn handle(&mut self, _: RefreshSnapshots, _: &mut Self::Context) {
        let mut snap = self.snapshots.lock().unwrap();
        snap.clear();
        for (_, (_, plugin)) in &self.loaded {
            if let Some(s) = plugin.snapshot() {
                snap.push(s);
            }
        }
    }
}

/// 插件 API 请求（来自 HTTP 路由）
#[derive(Message)]
#[rtype(result = "Option<(u16, String)>")]
pub struct ApiRequest {
    pub plugin: String,
    pub method: String,
    pub path: String,
    pub body: Option<String>,
}

impl Handler<ApiRequest> for PluginManager {
    type Result = Option<(u16, String)>;
    fn handle(&mut self, msg: ApiRequest, _: &mut Self::Context) -> Self::Result {
        let (_, plugin) = self.loaded.get(&msg.plugin)?;
        let api = plugin.api_handler()?;
        api.handle_api(&msg.method, &msg.path, msg.body.as_deref())
    }
}

impl Handler<StopAll> for PluginManager {
    type Result = ();

    fn handle(&mut self, _: StopAll, _: &mut Self::Context) {
        for (name, (_, plugin)) in self.loaded.iter_mut() {
            if let Err(e) = plugin.stop() {
                tracing::warn!("停止插件 {} 失败: {}", name, e);
            }
        }
        // 关键：在 drop 插件和卸载 DLL 之前，先清理 ToolRegistry
        // 因为 ToolRegistry 中的 Arc<dyn ToolExecutor> 的 vtable 在 DLL 中
        if let Some(ref registry) = self.tool_registry {
            if let Ok(mut reg) = registry.lock() {
                reg.clear();
                tracing::info!("已清理工具注册表");
            }
        }
        // 先 drop 插件（Box<dyn Plugin>），再 drop Library
        let mut libs: Vec<SharedLib> = Vec::new();
        for (name, (lib, plugin)) in self.loaded.drain() {
            tracing::info!("卸载插件: {}", name);
            drop(plugin);
            libs.push(lib);
        }
        // 现在安全 drop 所有 Library
        drop(libs);
    }
}

fn load_plugin(
    path: &std::path::Path,
    ctx: &HostContext,
) -> Result<(String, SharedLib, Box<dyn Plugin>), String> {
    unsafe {
        let lib = Arc::new(
            libloading::Library::new(path)
                .map_err(|e| format!("{}: {}", path.display(), e))?,
        );

        let create: libloading::Symbol<unsafe extern "C" fn() -> *mut dyn Plugin> =
            lib.get(b"_plugin_create")
                .map_err(|e| format!("符号 _plugin_create 未找到: {}", e))?;

        let plugin_ptr = create();
        if plugin_ptr.is_null() {
            return Err("_plugin_create 返回 null".into());
        }

        let mut plugin: Box<dyn Plugin> = Box::from_raw(plugin_ptr);
        let name = plugin.meta().name.clone();

        plugin.init(ctx).map_err(|e| format!("初始化失败: {}", e))?;
        plugin.start().map_err(|e| format!("启动失败: {}", e))?;

        Ok((name, lib, plugin))
    }
}
