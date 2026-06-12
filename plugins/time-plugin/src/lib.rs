//! time-plugin — provides current time as passive context (snapshot) to LLM
//! and exposes a GET /api/plugin/time endpoint returning the current time.
//!
//! No internal actor — uses snapshot() and api_handler() exclusively.

use plugin_interface::*;

// ── Plugin API handler ───────────────────────────────────────────────────────

struct TimeApi;

impl PluginApi for TimeApi {
    fn handle_api(&self, method: &str, _path: &str, _body: Option<&str>) -> Option<(u16, String)> {
        if method == "GET" {
            let now: chrono::DateTime<chrono::Local> = chrono::Local::now();
            Some((
                200,
                serde_json::json!({
                    "time": now.format("%Y-%m-%d %H:%M:%S").to_string(),
                    "timestamp": now.timestamp(),
                    "timezone": now.format("%:z").to_string(),
                })
                .to_string(),
            ))
        } else {
            None
        }
    }
}

// ── Plugin trait implementation ──────────────────────────────────────────────

struct TimePlugin {
    info: PluginInfo,
    api: TimeApi,
}

impl TimePlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "time-plugin".into(),
                version: "0.1.0".into(),
                description: "Provides current time as passive context and HTTP API".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            api: TimeApi,
        }
    }
}

impl Plugin for TimePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, _ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[time-plugin] started (actor-free) — API: GET /api/plugin/time");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[time-plugin] stopped");
    }

    fn snapshot(&self) -> Option<String> {
        let now = chrono::Local::now();
        Some(format!(
            "【time_plugin】当前系统时间: {}",
            now.format("%Y-%m-%d %H:%M:%S")
        ))
    }

    fn api_handler(&self) -> Option<&dyn PluginApi> {
        Some(&self.api)
    }
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(TimePlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
