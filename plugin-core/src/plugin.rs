/// 插件元数据
#[derive(Clone, Debug)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub min_host_version: String,
}

/// 插件 trait — 所有插件必须实现
pub trait Plugin: Send + Sync {
    fn meta(&self) -> &PluginMeta;

    /// 初始化（接收宿主上下文）
    fn init(&mut self, ctx: &crate::HostContext) -> Result<(), crate::PluginError>;

    /// 启动
    fn start(&mut self) -> Result<(), crate::PluginError>;

    /// 停止
    fn stop(&mut self) -> Result<(), crate::PluginError>;

    /// 接收事件。返回 `true` 继续传播，`false` 拦截（后续回调不再收到此事件）。
    fn on_event(&self, event: &crate::AgentEvent) -> bool;

    /// 获取宿主上下文（用于清理资源）
    fn ctx(&self) -> Option<&crate::HostContext> { None }
}

/// 插件构造函数签名（FFI 导出）
pub type PluginCreateFn = unsafe extern "C" fn() -> *mut dyn Plugin;
