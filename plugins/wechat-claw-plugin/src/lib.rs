//! WeChat ClawBot Plugin — actor-free port.

mod bot;

use bot::BotHandle;
use plugin_interface::*;
use std::sync::{Arc, Mutex};

pub struct WechatClawPlugin {
    info: PluginInfo,
    bot_handle: Option<Arc<Mutex<Option<BotHandle>>>>,
}

impl WechatClawPlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "wechat-claw-plugin".into(),
                version: "0.1.0".into(),
                description: "微信 ClawBot / iLink API 接入".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            bot_handle: Some(Arc::new(Mutex::new(None))),
        }
    }
}

impl Plugin for WechatClawPlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        // Register tools unconditionally.
        if let Some(ref reg) = ctx.tool_registry {
            let mut r = reg.lock().map_err(|e| format!("lock: {}", e))?;

            let bh1 = self.bot_handle.clone();
            r.register(Arc::new(SendMessageTool { bot_handle: bh1 }));
            log::info!("[wechat-claw] registered: wechat_send_message");

            let bh2 = self.bot_handle.clone();
            let tr = ctx.tool_registry.clone().unwrap();
            r.register(Arc::new(SendVoiceTool { bot_handle: bh2, tool_registry: tr }));
            log::info!("[wechat-claw] registered: wechat_send_voice");
        }

        let api_url = match std::env::var("WECHAT_API_URL") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                log::warn!("[wechat-claw] WECHAT_API_URL not set — polling disabled");
                log::info!("[wechat-claw] started (degraded)");
                return Ok(());
            }
        };
        let api_key = std::env::var("WECHAT_API_KEY").unwrap_or_default();

        let bh = self.bot_handle.clone();
        let eb = ctx.event_bus.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("tokio");
            rt.block_on(async {
                match bot::run_bot(&api_url, &api_key, eb).await {
                    Ok(h) => {
                        if let Some(ref bh) = bh { *bh.lock().unwrap() = Some(h); }
                        std::future::pending::<()>().await;
                    }
                    Err(e) => log::error!("[wechat-claw] bot: {}", e),
                }
            });
        });

        log::info!("[wechat-claw] started");
        Ok(())
    }

    fn stop(&mut self) { log::info!("[wechat-claw] stopped"); }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "assistant.message" {
            let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if !source.is_empty() && source != "wechat" { return true; }

            let chat_id = event.data.get("chat_id").and_then(|v| v.as_str()).map(String::from);
            let text = event.data.get("text").and_then(|v| v.as_str());
            if let (Some(chat_id), Some(text)) = (chat_id, text) {
                if let Some(ref bh) = self.bot_handle {
                    let bh = bh.clone();
                    let t = text.to_string();
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all().build().expect("tokio");
                        rt.block_on(async {
                            if let Ok(guard) = bh.lock() {
                                if let Some(ref h) = *guard {
                                    let _ = h.send_message(&chat_id, &t).await;
                                }
                            }
                        });
                    });
                }
            }
        }
        true
    }
}

// ─── 工具 ─────────────────────────────────────────────────────────

struct SendMessageTool {
    bot_handle: Option<Arc<Mutex<Option<BotHandle>>>>,
}

impl ToolExecutor for SendMessageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "wechat_send_message".into(),
            description: "Send a text message to WeChat.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object", "properties": {
                    "chat_id": {"type": "string", "description": "WeChat chat ID"},
                    "text": {"type": "string", "description": "Message text"}
                }, "required": ["chat_id", "text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(), None => return ToolResult::err("missing: text"),
        };
        let bh = match self.bot_handle.clone() { Some(h) => h, None => return ToolResult::err("bot off"), };

        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
            rt.block_on(async {
                let guard = bh.lock().unwrap();
                match *guard {
                    Some(ref h) => h.send_message(&chat_id, &text).await
                        .map(|_| ToolResult::ok("sent")).unwrap_or_else(|e| ToolResult::err(&e)),
                    None => ToolResult::err("bot off"),
                }
            })
        }).join() {
            Ok(r) => r, Err(_) => ToolResult::err("panic"),
        }
    }
}

struct SendVoiceTool {
    bot_handle: Option<Arc<Mutex<Option<BotHandle>>>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
}

impl ToolExecutor for SendVoiceTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "wechat_send_voice".into(),
            description: "TTS + send voice message to WeChat.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object", "properties": {
                    "chat_id": {"type": "string", "description": "WeChat chat ID"},
                    "text": {"type": "string", "description": "Text to speak"}
                }, "required": ["chat_id", "text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(), None => return ToolResult::err("missing: text"),
        };

        // Call TTS.
        let tts = { let r = self.tool_registry.lock().unwrap(); r.get_executor("tts_synthesize").map(|a| Arc::clone(&a)) };
        let tts = match tts { Some(t) => t, None => return ToolResult::err("tts_synthesize not registered"), };

        let result = tts.execute(&serde_json::json!({"text": text}));
        if !result.success { return ToolResult::err(&format!("TTS: {}", result.error.unwrap_or_default())); }

        let bh = match self.bot_handle.clone() { Some(h) => h, None => return ToolResult::err("bot off"), };
        let audio_b64 = result.content;

        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
            rt.block_on(async {
                let guard = bh.lock().unwrap();
                match *guard {
                    Some(ref h) => h.send_voice(&chat_id, &audio_b64).await
                        .map(|_| ToolResult::ok("voice sent")).unwrap_or_else(|e| ToolResult::err(&e)),
                    None => ToolResult::err("bot off"),
                }
            })
        }).join() {
            Ok(r) => r, Err(_) => ToolResult::err("panic"),
        }
    }
}

// ─── FFI ─────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> { Box::new(WechatClawPlugin::new()) }

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
