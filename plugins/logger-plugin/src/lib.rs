//! logger-plugin — subscribes to all events via `on_event()` and logs them.
//!
//! No internal actor — receives events through the PluginManager's EventBus
//! "global forward" path.

use plugin_interface::*;

struct LoggerPlugin {
    info: PluginInfo,
    event_count: std::cell::Cell<u64>,
}

impl LoggerPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "logger-plugin".into(),
                version: "0.1.0".into(),
                description: "Logs all events it receives via on_event()".into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            event_count: std::cell::Cell::new(0),
        }
    }
}

impl Plugin for LoggerPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, _ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[logger-plugin] started (actor-free mode)");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[logger-plugin] stopped (logged {} events)", self.event_count.get());
    }

    fn on_event(&self, event: &Event) -> bool {
        self.event_count.set(self.event_count.get() + 1);
        log::info!(
            "[logger-plugin] #{} | topic='{}' source='{}' ts={} | {}",
            self.event_count.get(),
            event.topic,
            event.source,
            event.timestamp,
            event.data.to_string().chars().take(200).collect::<String>(),
        );
        true // continue propagation
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(LoggerPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
