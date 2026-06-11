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

    /// 被动上下文：每次 LLM 请求前调用，返回临时注入到 messages 的内容。
    /// 格式应为 `【plugin_name】详情`，不存入聊天记录。
    fn snapshot(&self) -> Option<String> { None }

    /// 返回 HTTP API 处理器，None 表示该插件不暴露 API
    fn api_handler(&self) -> Option<&dyn PluginApi> { None }
}

/// 插件暴露的 HTTP API 端点（通过 /v1/{plugin_name}/... 路由）
pub trait PluginApi: Send + Sync {
    /// 处理 HTTP 请求
    /// method: GET/POST/PUT/DELETE
    /// path: 插件名之后的路径部分
    /// body: 请求体（POST/PUT）
    /// 返回 (http_status_code, response_body)
    fn handle_api(&self, method: &str, path: &str, body: Option<&str>)
        -> Option<(u16, String)> { None }
}

/// 插件构造函数签名（FFI 导出）
pub type PluginCreateFn = unsafe extern "C" fn() -> *mut dyn Plugin;
