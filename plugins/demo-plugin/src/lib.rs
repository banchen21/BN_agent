//! demo-plugin — 示例插件，演示 actix-actor 插件系统的基本用法。
//!
//! 使用 `on_event()` 而非内部 actor（DLL 中无法调用 `Actor::start()`）。

use plugin_interface::*;

struct DemoPlugin {
    info: PluginInfo,
    event_bus: Option<Addr<EventBus>>,
}

impl DemoPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "demo-plugin".into(),
                version: "0.1.0".into(),
                description: "示例插件：订阅 'greeting' 事件并发布 'greeted' 响应".into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            event_bus: None,
        }
    }
}

impl Plugin for DemoPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[demo-plugin] started (actor-free mode)");
        self.event_bus = Some(ctx.event_bus.clone());
        // Subscribe via on_event — no internal actor needed.
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[demo-plugin] stopped");
        self.event_bus = None;
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "greeting" {
            log::info!("[demo-plugin] received greeting: {}", event.data);

            if let Some(ref eb) = self.event_bus {
                let response = Event::new(
                    "greeted",
                    serde_json::json!({
                        "message": format!("Hello back! I received: {}", event.data),
                        "in_response_to": event.timestamp,
                    }),
                    "demo-plugin",
                );
                eb.do_send(response);
            }
        }
        true // continue propagation
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(DemoPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
