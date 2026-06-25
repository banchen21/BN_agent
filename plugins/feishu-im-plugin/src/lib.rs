//! Feishu IM Plugin — actor-free port of BN_agent's feishu-im-plugin.

mod bot;

use bot::BotHandle;
use plugin_interface::*;
use std::sync::{Arc, Mutex};

pub struct FeishuImPlugin {
    info: PluginInfo,
    bot_handle: Option<Arc<Mutex<Option<BotHandle>>>>,
    event_bus: Option<Addr<EventBus>>,
    current_chat_id: Arc<Mutex<Option<String>>>,
    streaming_text: Arc<Mutex<TextStreamState>>,
}

impl FeishuImPlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "feishu-im-plugin".into(),
                version: "0.1.0".into(),
                description: "飞书即时通讯插件 — 接收/发送飞书消息".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            bot_handle: Some(Arc::new(Mutex::new(None))),
            event_bus: None,
            current_chat_id: Arc::new(Mutex::new(None)),
            streaming_text: Arc::new(Mutex::new(TextStreamState::default())),
        }
    }
}

fn im_stream_chunks_enabled() -> bool {
    std::env::var("IM_STREAM_CHUNKS")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "disabled")
        })
        .unwrap_or(true)
}

fn stream_request_id(event: &Event) -> Option<String> {
    event
        .data
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn feishu_chat_id_from_event(event: &Event, fallback: Option<String>) -> Option<String> {
    event
        .data
        .get("chat_id")
        .and_then(|v| v.as_str().map(String::from))
        .or_else(|| {
            event
                .data
                .get("peer_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.strip_prefix("feishu:"))
                .map(String::from)
        })
        .or(fallback)
}

fn send_feishu_stream_segments(
    bot_handle: Option<Arc<Mutex<Option<BotHandle>>>>,
    chat_id: String,
    segments: Vec<String>,
) {
    if segments.is_empty() {
        return;
    }
    let Some(bot_handle) = bot_handle else {
        return;
    };
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        rt.block_on(async {
            for segment in segments {
                if let Ok(guard) = bot_handle.lock() {
                    if let Some(ref h) = *guard {
                        let _ = h.send_message(&chat_id, &segment).await;
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            }
        });
    });
}

impl Plugin for FeishuImPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        self.event_bus = Some(ctx.event_bus.clone());

        // Register tool unconditionally.
        if let Some(ref reg) = ctx.tool_registry {
            let bh = self.bot_handle.clone();
            let cc = self.current_chat_id.clone();
            reg.lock().register(Arc::new(SendMessageTool {
                bot_handle: bh,
                current_chat_id: cc,
            }));
            log::info!("[feishu-im] registered tool: feishu_send_message");
        }

        let app_id = match std::env::var("FEISHU_APP_ID") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                log::warn!("[feishu-im] FEISHU_APP_ID not set — polling disabled (tool only)");
                log::info!("[feishu-im] started (degraded)");
                return Ok(());
            }
        };
        let app_secret = std::env::var("FEISHU_APP_SECRET").unwrap_or_default();

        let bh = self.bot_handle.clone();
        let eb = ctx.event_bus.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                match bot::run_bot(&app_id, &app_secret, eb).await {
                    Ok(h) => {
                        if let Some(ref bh) = bh {
                            *bh.lock().unwrap() = Some(h);
                        }
                        // Keep thread alive — bot runs its own poll loop in tokio::spawn.
                        std::future::pending::<()>().await;
                    }
                    Err(e) => log::error!("[feishu-im] bot failed: {}", e),
                }
            });
        });

        log::info!("[feishu-im] started");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[feishu-im] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        if im_stream_chunks_enabled() && event.topic == "llm.chunk" {
            let source = event
                .data
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if source != "feishu" {
                return true;
            }
            let Some(request_id) = stream_request_id(event) else {
                return true;
            };
            let Some(content) = event.data.get("content").and_then(|v| v.as_str()) else {
                return true;
            };
            let chat_id =
                feishu_chat_id_from_event(event, self.current_chat_id.lock().unwrap().clone());
            let Some(chat_id) = chat_id else {
                return true;
            };
            let segments = self
                .streaming_text
                .lock()
                .unwrap()
                .push_chunk(&request_id, content);
            send_feishu_stream_segments(self.bot_handle.clone(), chat_id, segments);
            return true;
        }

        if im_stream_chunks_enabled() && event.topic == "llm.response" {
            let source = event
                .data
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if source != "feishu" {
                return true;
            }
            let Some(request_id) = stream_request_id(event) else {
                return true;
            };
            let chat_id =
                feishu_chat_id_from_event(event, self.current_chat_id.lock().unwrap().clone());
            let Some(chat_id) = chat_id else {
                return true;
            };
            let segments = self.streaming_text.lock().unwrap().flush(&request_id);
            send_feishu_stream_segments(self.bot_handle.clone(), chat_id, segments);
            return true;
        }

        if event.topic == "assistant.message" {
            let source = event
                .data
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !source.is_empty() && source != "feishu" {
                return true;
            }

            if let Some(request_id) = stream_request_id(event) {
                if self
                    .streaming_text
                    .lock()
                    .unwrap()
                    .take_streamed_request(&request_id)
                {
                    return true;
                }
            }

            // 从事件中提取并存储当前平台会话 ID
            if let Some(cid) = event.data.get("chat_id").and_then(|v| v.as_str()) {
                *self.current_chat_id.lock().unwrap() = Some(cid.to_string());
            }
            let chat_id = self.current_chat_id.lock().unwrap().clone();
            let text = event.data.get("text").and_then(|v| v.as_str());
            if let (Some(chat_id), Some(text)) = (chat_id, text) {
                if let Some(ref bh) = self.bot_handle {
                    let bh = bh.clone();
                    let t = text.to_string();
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("tokio");
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
    current_chat_id: Arc<Mutex<Option<String>>>,
}

impl ToolExecutor for SendMessageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "feishu_send_message".into(),
            description: "Send a text message to a Feishu chat.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chat_id": {"type": "string", "description": "Feishu chat ID（可选，不传则发到当前会话）"},
                    "text": {"type": "string", "description": "Message text"}
                },
                "required": ["text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.current_chat_id.lock().unwrap().clone())
            .unwrap_or_default();
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("missing: text"),
        };
        let bh = match self.bot_handle.clone() {
            Some(h) => h,
            None => return ToolResult::err("bot not started"),
        };

        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                let guard = bh.lock().unwrap();
                match *guard {
                    Some(ref h) => h
                        .send_message(&chat_id, &text)
                        .await
                        .map(|_| ToolResult::ok("sent"))
                        .unwrap_or_else(|e| ToolResult::err(&e)),
                    None => ToolResult::err("bot not started"),
                }
            })
        })
        .join()
        {
            Ok(r) => r,
            Err(_) => ToolResult::err("thread panic"),
        }
    }
}

// ─── FFI ─────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(FeishuImPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
