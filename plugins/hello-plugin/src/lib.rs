//! hello-plugin — example plugin for the actix-actor plugin system.
//!
//! Uses `on_event()` instead of an internal actor (DLLs can't call `Actor::start()`).

use plugin_interface::*;

struct HelloPlugin {
    info: PluginInfo,
    event_bus: Option<Addr<EventBus>>,
}

impl HelloPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "hello-plugin".into(),
                version: "0.1.0".into(),
                description: "Subscribes to 'greeting' events and publishes 'greeted' responses".into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            event_bus: None,
        }
    }
}

impl Plugin for HelloPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[hello-plugin] started (actor-free mode)");
        self.event_bus = Some(ctx.event_bus.clone());
        // Subscribe via on_event — no internal actor needed.
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[hello-plugin] stopped");
        self.event_bus = None;
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "greeting" {
            log::info!("[hello-plugin] received greeting: {}", event.data);

            if let Some(ref eb) = self.event_bus {
                let response = Event::new(
                    "greeted",
                    serde_json::json!({
                        "message": format!("Hello back! I received: {}", event.data),
                        "in_response_to": event.timestamp,
                    }),
                    "hello-plugin",
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
    Box::new(HelloPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
