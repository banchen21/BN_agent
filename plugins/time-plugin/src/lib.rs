//! Time Plugin — 时间工具插件

use plugin_core::{
    AgentEvent, HostContext, Plugin, PluginError, PluginMeta, ToolDef, ToolExecutor, ToolResult,
};
use std::sync::Arc;

pub struct TimePlugin {
    meta: PluginMeta,
    ctx: Option<HostContext>,
}

impl TimePlugin {
    pub fn new() -> Self {
        Self {
            meta: PluginMeta {
                name: "time-plugin".into(),
                version: "0.1.0".into(),
                description: "时间工具插件".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            ctx: None,
        }
    }
}

impl Plugin for TimePlugin {
    fn meta(&self) -> &PluginMeta { &self.meta }

    fn init(&mut self, ctx: &HostContext) -> Result<(), PluginError> {
        ctx.log_info("time", "TimePlugin 初始化完成");
        self.ctx = Some(ctx.clone());
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("time", "TimePlugin 已启动");
            if let Some(ref registry) = ctx.tool_registry {
                registry.lock().map_err(|e| PluginError::InitError(format!("{}", e)))?
                    .register(Arc::new(GetTimeTool));
                ctx.log_info("time", "已注册工具: get_time");
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx { ctx.log_info("time", "TimePlugin 已停止"); }
        Ok(())
    }

    fn on_event(&self, event: &AgentEvent) -> bool {
        if let Some(ref ctx) = self.ctx {
            ctx.log_debug("time", &format!("收到事件: {:?}", event.event_type));
        }
        true
    }

    fn ctx(&self) -> Option<&HostContext> {
        self.ctx.as_ref()
    }
}

struct GetTimeTool;

impl ToolExecutor for GetTimeTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "get_time".into(),
            description: "获取当前系统时间".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        });
        &DEF
    }

    fn execute(&self, _args: &serde_json::Value) -> ToolResult {
        let now = chrono::Local::now();
        ToolResult::ok(&format!("当前时间: {}", now.format("%Y-%m-%d %H:%M:%S")))
    }
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(TimePlugin::new()))
}
