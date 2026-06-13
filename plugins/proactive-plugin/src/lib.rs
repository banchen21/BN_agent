//! proactive-plugin — example plugin skeleton for the actix-actor plugin system.
//!
//! Uses `on_event()` instead of an internal actor (DLLs can't call `Actor::start()`).

use plugin_interface::*;

struct ProactivePlugin {
    info: PluginInfo,
    event_bus: Option<Addr<EventBus>>,
}

impl ProactivePlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "proactive-plugin".into(),
                version: "0.1.0".into(),
                description: "Subscribes to 'proactive.ping' events and publishes 'proactive.pong' responses".into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            event_bus: None,
        }
    }
}

impl Plugin for ProactivePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[proactive-plugin] started (actor-free mode)");
        self.event_bus = Some(ctx.event_bus.clone());
        // Subscribe via on_event — no internal actor needed.
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[proactive-plugin] stopped");
        self.event_bus = None;
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "proactive.ping" {
            log::info!("[proactive-plugin] received ping: {}", event.data);

            if let Some(ref eb) = self.event_bus {
                let response = Event::new(
                    "proactive.pong",
                    serde_json::json!({
                        "message": format!("Pong! I received: {}", event.data),
                        "in_response_to": event.timestamp,
                    }),
                    "proactive-plugin",
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
    Box::new(ProactivePlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}

