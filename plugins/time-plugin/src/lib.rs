//! Time Plugin — 时间插件（仅被动上下文上报，不注册工具）

use plugin_core::{
    AgentEvent, HostContext, Plugin, PluginError, PluginMeta,
};

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
                description: "时间插件（被动上下文）".into(),
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
        ctx.log_info("time", "TimePlugin 初始化（仅被动上下文）");
        self.ctx = Some(ctx.clone());
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("time", "TimePlugin 已启动");
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

    fn snapshot(&self) -> Option<String> {
        Some(format!("【time_plugin】当前系统时间: {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")))
    }
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(TimePlugin::new()))
}
