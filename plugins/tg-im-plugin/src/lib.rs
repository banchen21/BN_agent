//! tg-im-plugin — Telegram 即时通讯插件（actor-free 版）。
//!
//! ## 事件流转
//!
//! ```text
//! Telegram ─► bot 线程 ─► EventBus.do_send("user.message")
//!                                       │
//!                                 PipelineActor → LlmActor
//!                                       │
//!                                 EventBus.do_send("assistant.message")
//!                                       │
//!                            PluginManager (订阅 "*")
//!                                       │
//!                            on_event() → bot.send_message → Telegram
//! ```

mod bot;

use bot::{BotHandle, UserMessageCallback};
use plugin_interface::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// ── 工具实现 ─────────────────────────────────────────────────────────────────

/// tg_send_message — LLM 可调用发送文字到 TG。
struct SendMessageTool {
    bot_handle: BotHandle,
    processing_chats: Arc<Mutex<HashSet<i64>>>,
    current_chat_id: Arc<Mutex<Option<i64>>>,
}

impl ToolExecutor for SendMessageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tg_send_message".into(),
            description: "Send a text message to this Telegram chat.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chat_id": {"type": "integer", "description": "Telegram chat ID（可选，不传则发到当前会话）"},
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
            .and_then(|v| v.as_i64())
            .or_else(|| *self.current_chat_id.lock().unwrap());
        let chat_id = match chat_id {
            Some(id) => id,
            None => return ToolResult::err("missing: chat_id"),
        };
        let mut text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("missing: text"),
        };
        // Strip [SCHEDULE:N] line
        if let Some(tag_pos) = text.find("[SCHEDULE:") {
            if let Some(newline_pos) = text[..tag_pos].rfind('\n') {
                text.truncate(newline_pos);
            } else {
                text.truncate(tag_pos);
            }
            text = text.trim().to_string();
        }
        // 停止 typing 循环
        self.processing_chats.lock().unwrap().remove(&chat_id);

        let bot = self.bot_handle.clone();
        let text_clone = text.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                let _ = bot.send_message(chat_id, &text_clone).await;
            });
        });
        ToolResult::ok(&format!("message sent: {}", text))
    }
}

/// tg_send_voice — TTS 后发送语音。
struct SendVoiceTool {
    bot_handle: BotHandle,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    processing_chats: Arc<Mutex<HashSet<i64>>>,
    current_chat_id: Arc<Mutex<Option<i64>>>,
}

impl ToolExecutor for SendVoiceTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "tg_send_voice".into(),
            description: "Convert text to speech and send as voice message to Telegram. Use when the user asks for voice.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chat_id": {"type": "integer", "description": "Telegram chat ID（可选，不传则发到当前会话）"},
                    "text": {"type": "string", "description": "Text to speak"}
                },
                "required": ["text"]
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or_else(|| *self.current_chat_id.lock().unwrap());
        let chat_id = match chat_id {
            Some(id) => id,
            None => return ToolResult::err("missing: chat_id"),
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("missing: text"),
        };

        // 停止 typing 循环，先发"正在录音"状态
        self.processing_chats.lock().unwrap().remove(&chat_id);
        let bot = self.bot_handle.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                let _ = bot.send_record_voice_action(chat_id).await;
            });
        });

        let voice_desc = args
            .get("voice_desc")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);

        let tts_exec = {
            let reg = match self.tool_registry.lock() {
                Ok(r) => r,
                Err(e) => return ToolResult::err(&format!("lock: {}", e)),
            };
            match reg.get_executor("tts_synthesize") {
                Some(e) => e,
                None => return ToolResult::err("tts_synthesize not found"),
            }
        };
        let mut tts_params = serde_json::json!({ "text": text });
        if let Some(ref vd) = voice_desc {
            tts_params["voice_desc"] = serde_json::json!(vd);
        }
        let tts_result = tts_exec.execute(&tts_params);
        if !tts_result.success {
            return ToolResult::err(&format!(
                "TTS failed: {}",
                tts_result.error.unwrap_or_default()
            ));
        }
        let audio_data = match base64_decode(&tts_result.content) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 decode: {}", e)),
        };

        let bot = self.bot_handle.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                match bot.send_voice(chat_id, audio_data).await {
                    Ok(_) => eprintln!("[tg-im] voice sent to {}", chat_id),
                    Err(e) => eprintln!("[tg-im] send_voice FAILED: {}", e),
                }
            });
        });

        ToolResult::ok("voice sent")
    }
}

/// tg_send_photo — 发送图片。
struct SendPhotoTool {
    bot_handle: BotHandle,
    processing_chats: Arc<Mutex<HashSet<i64>>>,
    current_chat_id: Arc<Mutex<Option<i64>>>,
}

impl ToolExecutor for SendPhotoTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tg_send_photo".into(),
            description: "Send a photo to this Telegram chat.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chat_id": {"type": "integer", "description": "Telegram chat ID（可选，不传则发到当前会话）"},
                    "photo_base64": {"type": "string", "description": "Base64 JPEG/PNG（与 file_path 二选一）"},
                    "file_path": {"type": "string", "description": "本地图片文件路径，如 D:\\banch\\ai\\image.png（与 photo_base64 二选一）"},
                    "caption": {"type": "string", "description": "Optional caption"}
                }
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or_else(|| *self.current_chat_id.lock().unwrap());
        let chat_id = match chat_id {
            Some(id) => id,
            None => return ToolResult::err("missing: chat_id"),
        };
        let caption = args
            .get("caption")
            .and_then(|v| v.as_str())
            .map(String::from);

        // 支持 photo_base64 或 file_path 两种输入
        let data = if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
            match std::fs::read(path) {
                Ok(d) => d,
                Err(e) => return ToolResult::err(&format!("文件读取失败 [{}]: {}", path, e)),
            }
        } else if let Some(b64) = args.get("photo_base64").and_then(|v| v.as_str()) {
            match base64_decode(b64) {
                Ok(d) => d,
                Err(e) => return ToolResult::err(&format!("base64: {}", e)),
            }
        } else {
            return ToolResult::err("需要提供 photo_base64 或 file_path");
        };

        // 停止 typing 循环
        self.processing_chats.lock().unwrap().remove(&chat_id);

        let bot = self.bot_handle.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                let _ = bot.send_photo(chat_id, data, caption.as_deref()).await;
            });
        });

        ToolResult::ok("photo sent")
    }
}

/// tg_send_video — 发送视频。
struct SendVideoTool {
    bot_handle: BotHandle,
    processing_chats: Arc<Mutex<HashSet<i64>>>,
    current_chat_id: Arc<Mutex<Option<i64>>>,
}

impl ToolExecutor for SendVideoTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tg_send_video".into(),
            description: "Send a video to this Telegram chat.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chat_id": {"type": "integer", "description": "Telegram chat ID（可选，不传则发到当前会话）"},
                    "video_base64": {"type": "string", "description": "Base64 encoded video data (MP4)"},
                    "caption": {"type": "string", "description": "Optional caption"}
                },
                "required": ["video_base64"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or_else(|| *self.current_chat_id.lock().unwrap());
        let chat_id = match chat_id {
            Some(id) => id,
            None => return ToolResult::err("missing: chat_id"),
        };
        let b64 = match args.get("video_base64").and_then(|v| v.as_str()) {
            Some(d) => d.to_string(),
            None => return ToolResult::err("missing: video_base64"),
        };
        let caption = args
            .get("caption")
            .and_then(|v| v.as_str())
            .map(String::from);
        let data = match base64_decode(&b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64: {}", e)),
        };

        self.processing_chats.lock().unwrap().remove(&chat_id);

        let bot = self.bot_handle.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                let _ = bot.send_video(chat_id, data, caption.as_deref()).await;
            });
        });

        ToolResult::ok("video sent")
    }
}

/// tg_send_file — 发送文件。
struct SendDocumentTool {
    bot_handle: BotHandle,
    processing_chats: Arc<Mutex<HashSet<i64>>>,
    current_chat_id: Arc<Mutex<Option<i64>>>,
}

impl ToolExecutor for SendDocumentTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tg_send_file".into(),
            description: "Send a file to a Telegram chat".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chat_id": {"type": "integer", "description": "Telegram chat ID（可选，不传则发到当前会话）"},
                    "file_base64": {"type": "string", "description": "Base64 encoded file data"},
                    "file_name": {"type": "string", "description": "File name with extension, e.g. report.pdf"},
                    "caption": {"type": "string", "description": "Optional caption"}
                },
                "required": ["file_base64", "file_name"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_i64())
            .or_else(|| *self.current_chat_id.lock().unwrap());
        let chat_id = match chat_id {
            Some(id) => id,
            None => return ToolResult::err("missing: chat_id"),
        };
        let b64 = match args.get("file_base64").and_then(|v| v.as_str()) {
            Some(d) => d.to_string(),
            None => return ToolResult::err("missing: file_base64"),
        };
        let fname = match args.get("file_name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return ToolResult::err("missing: file_name"),
        };
        let caption = args
            .get("caption")
            .and_then(|v| v.as_str())
            .map(String::from);
        let data = match base64_decode(&b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64: {}", e)),
        };

        self.processing_chats.lock().unwrap().remove(&chat_id);

        let bot = self.bot_handle.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                let _ = bot
                    .send_document(chat_id, data, fname, caption.as_deref())
                    .await;
            });
        });

        ToolResult::ok("file sent")
    }
}

// ── Plugin trait ─────────────────────────────────────────────────────────────

pub struct TgImPlugin {
    info: PluginInfo,
    bot_handle: Option<BotHandle>,
    bot_thread: Option<std::thread::JoinHandle<()>>,
    /// 正在等待 LLM 回复的 TG chat_id，用于持续发送 typing
    processing_chats: Arc<Mutex<HashSet<i64>>>,
    /// 当前平台会话 ID（pipeline 不再注入，插件自行维护）
    current_chat_id: Arc<Mutex<Option<i64>>>,
}

impl TgImPlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "tg-im-plugin".into(),
                version: "0.1.0".into(),
                description: "Telegram IM plugin — bridges TG messages into the event system"
                    .into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            bot_handle: None,
            bot_thread: None,
            processing_chats: Arc::new(Mutex::new(HashSet::new())),
            current_chat_id: Arc::new(Mutex::new(None)),
        }
    }
}

impl Plugin for TgImPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let token = std::env::var("TG_BOT_TOKEN").map_err(|_| "TG_BOT_TOKEN env var not set")?;

        log::info!(
            "[tg-im] starting with token ***{}",
            &token[token.len().saturating_sub(4)..]
        );

        // 1. 注册工具。
        let client = bot::build_reqwest_client()?;
        let bot = teloxide::Bot::with_client(&token, client);
        let bot_handle = BotHandle::new(bot.clone());

        let pc = self.processing_chats.clone();
        let cc = self.current_chat_id.clone();
        if let Some(ref reg) = ctx.tool_registry {
            let mut reg = reg.lock().map_err(|e| format!("lock: {}", e))?;
            reg.register(Arc::new(SendMessageTool {
                bot_handle: bot_handle.clone(),
                processing_chats: pc.clone(),
                current_chat_id: cc.clone(),
            }));
            reg.register(Arc::new(SendVoiceTool {
                bot_handle: bot_handle.clone(),
                tool_registry: ctx.tool_registry.clone().unwrap(),
                processing_chats: pc.clone(),
                current_chat_id: cc.clone(),
            }));
            reg.register(Arc::new(SendPhotoTool {
                bot_handle: bot_handle.clone(),
                processing_chats: pc.clone(),
                current_chat_id: cc.clone(),
            }));
            reg.register(Arc::new(SendVideoTool {
                bot_handle: bot_handle.clone(),
                processing_chats: pc.clone(),
                current_chat_id: cc.clone(),
            }));
            reg.register(Arc::new(SendDocumentTool {
                bot_handle: bot_handle.clone(),
                processing_chats: pc.clone(),
                current_chat_id: cc.clone(),
            }));
            log::info!("[tg-im] registered 5 tools");
        }

        // 2. 启动 bot 线程（独立 tokio 运行时）。
        let event_bus = ctx.event_bus.clone();
        let cc2 = self.current_chat_id.clone();
        let on_user_message: UserMessageCallback =
            Arc::new(move |chat_id: i64, text: &str, user_name: &str| {
                *cc2.lock().unwrap() = Some(chat_id);
                event_bus.do_send(Event::new(
                    "user.message",
                    serde_json::json!({
                        "text": text,
                        "source": "telegram",
                        "peer_id": format!("telegram:{}", chat_id),
                        "user_name": user_name,
                        "chat_id": chat_id,
                    }),
                    "tg-im-plugin",
                ));
            });

        // 语音消息回调：下载后调用 ASR 工具，将识别结果发布为 user.message
        let tr = ctx.tool_registry.clone().unwrap();
        let eb_vc = ctx.event_bus.clone();
        let cc3 = self.current_chat_id.clone();
        let on_voice_message: bot::VoiceMessageCallback = Arc::new(
            move |chat_id: i64, audio_b64: String, mime: &str, user_name: &str| {
                *cc3.lock().unwrap() = Some(chat_id);
                let tr = tr.clone();
                let eb = eb_vc.clone();
                let m = mime.to_string();
                let un = user_name.to_string();
                std::thread::spawn(move || {
                    eprintln!("[tg-im:voice] calling asr_transcribe...");
                    let asr_result = match tr.lock() {
                        Ok(reg) => match reg.get_executor("asr_transcribe") {
                            Some(exec) => exec.execute(&serde_json::json!({
                                "audio_base64": audio_b64,
                                "mime_type": m,
                            })),
                            None => {
                                eprintln!("[tg-im:voice] asr_transcribe tool not found");
                                return;
                            }
                        },
                        Err(e) => {
                            eprintln!("[tg-im:voice] lock failed: {:?}", e);
                            return;
                        }
                    };
                    eprintln!(
                        "[tg-im:voice] asr done: success={} content_len={} error={:?}",
                        asr_result.success,
                        asr_result.content.len(),
                        asr_result.error
                    );
                    let text = if asr_result.success && !asr_result.content.trim().is_empty() {
                        asr_result.content
                    } else {
                        eprintln!("[tg-im:voice] ASR failed, sending fallback message");
                        "（语音消息）".to_string()
                    };
                    eb.do_send(Event::new(
                        "user.message",
                        serde_json::json!({
                            "text": text,
                            "source": "telegram",
                            "peer_id": format!("telegram:{}", chat_id),
                            "user_name": format!("{} (语音)", un),
                            "chat_id": chat_id,
                        }),
                        "tg-im-plugin",
                    ));
                });
            },
        );

        // 图片消息回调：下载后发布为带 image_base64 的 user.message
        // 如有 caption 文本则作为用户问题一同发送（避免 bot.rs 中 caption 和图片双发）
        let eb_photo = ctx.event_bus.clone();
        let cc4 = self.current_chat_id.clone();
        let on_photo_message: bot::PhotoMessageCallback = Arc::new(
            move |chat_id: i64, img_b64: String, user_name: &str, caption: &str| {
                *cc4.lock().unwrap() = Some(chat_id);
                let eb = eb_photo.clone();
                let un = user_name.to_string();
                let cap = caption.to_string();
                std::thread::spawn(move || {
                    let text = if cap.is_empty() {
                        format!("@{} 发送了一张图片", un)
                    } else {
                        format!("@{} (图片): {}", un, cap)
                    };
                    eprintln!("[tg-im:photo] received from @{} caption='{}'", un, cap);
                    eb.do_send(Event::new(
                        "user.message",
                        serde_json::json!({
                            "text": text,
                            "image_base64": img_b64,
                            "source": "telegram",
                            "peer_id": format!("telegram:{}", chat_id),
                            "user_name": format!("{} (图片)", un),
                            "chat_id": chat_id,
                        }),
                        "tg-im-plugin",
                    ));
                });
            },
        );

        // 文件消息回调：下载后发布为带 file 信息的 user.message
        let eb_file = ctx.event_bus.clone();
        let cc5 = self.current_chat_id.clone();
        let on_file_message: bot::FileMessageCallback = Arc::new(
            move |chat_id: i64, file_b64: String, file_name: String, user_name: &str| {
                *cc5.lock().unwrap() = Some(chat_id);
                let eb = eb_file.clone();
                let un = user_name.to_string();
                let fn2 = file_name.clone();
                std::thread::spawn(move || {
                    eprintln!(
                        "[tg-im:file] received '{}' from @{} ({}b)",
                        fn2,
                        un,
                        file_b64.len()
                    );
                    eb.do_send(Event::new(
                        "user.message",
                        serde_json::json!({
                            "text": format!("@{} 发送了文件：{}", un, fn2),
                            "source": "telegram",
                            "peer_id": format!("telegram:{}", chat_id),
                            "user_name": format!("{} (文件)", un),
                            "file_base64": file_b64,
                            "file_name": fn2,
                            "chat_id": chat_id,
                        }),
                        "tg-im-plugin",
                    ));
                });
            },
        );

        // 视频消息回调：下载后发布为带 video_base64 的 user.message
        let eb_video = ctx.event_bus.clone();
        let cc6 = self.current_chat_id.clone();
        let on_video_message: bot::VideoMessageCallback = Arc::new(
            move |chat_id: i64, video_b64: String, mime: String, user_name: &str| {
                *cc6.lock().unwrap() = Some(chat_id);
                let eb = eb_video.clone();
                let un = user_name.to_string();
                let m = mime;
                std::thread::spawn(move || {
                    eprintln!(
                        "[tg-im:video] received from @{} ({}b, {})",
                        un,
                        video_b64.len(),
                        m
                    );
                    eb.do_send(Event::new(
                        "user.message",
                        serde_json::json!({
                            "text": format!("@{} 发送了一个视频", un),
                            "video_base64": video_b64,
                            "video_mime": m,
                            "source": "telegram",
                            "peer_id": format!("telegram:{}", chat_id),
                            "user_name": format!("{} (视频)", un),
                            "chat_id": chat_id,
                        }),
                        "tg-im-plugin",
                    ));
                });
            },
        );

        let (handle, join_handle) = bot::run_bot(
            bot,
            on_user_message,
            Some(on_voice_message),
            Some(on_photo_message),
            Some(on_file_message),
            Some(on_video_message),
        )
        .map_err(|e| format!("bot: {}", e))?;

        self.bot_handle = Some(handle);
        self.bot_thread = Some(join_handle);

        log::info!("[tg-im] plugin started — bot is running");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[tg-im] stopping...");
        self.bot_handle = None;
        log::info!("[tg-im] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        // NOTE: log::info! 在 cdylib 中不生效（log crate 被静态链接，独立于主进程的 env_logger）。
        // 使用 eprintln! 替代。
        let source = event
            .data
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let chat_id = self.current_chat_id.lock().unwrap().unwrap_or(0);

        // ── user.message from Telegram → 持续发 Typing 直到回复 ──
        if event.topic == "user.message" && source == "telegram" && chat_id != 0 {
            if let Some(ref handle) = self.bot_handle {
                let h = handle.clone();
                let chats = self.processing_chats.clone();
                chats.lock().unwrap().insert(chat_id);
                log::info!("start typing loop for chat {}", chat_id);

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio");
                    rt.block_on(async {
                        // 每 4 秒发一次 typing，直到 chat_id 移除或超时 30 秒
                        let mut elapsed = 0u64;
                        loop {
                            if !chats.lock().unwrap().contains(&chat_id) {
                                break;
                            }
                            if elapsed >= 30 {
                                chats.lock().unwrap().remove(&chat_id);
                                break;
                            }
                            if let Err(e) = h.send_typing(chat_id).await {
                                eprintln!("[tg-im] typing err: {}", e);
                            }
                            tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                            elapsed += 4;
                        }
                    });
                });
            }
            return true;
        }

        // ── assistant.message from Telegram → 停止 typing + 逐条发送回复 ──
        if event.topic == "assistant.message" && source == "telegram" && chat_id != 0 {
            // 静默事件：仅供插件（proactive 等）感知回复，不实际发送
            if event
                .data
                .get("silent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                self.processing_chats.lock().unwrap().remove(&chat_id);
                return true;
            }
            // 停止 typing 循环
            self.processing_chats.lock().unwrap().remove(&chat_id);

            let full_text = match event.data.get("text").and_then(|v| v.as_str()) {
                Some(t) => {
                    let mut s = t.to_string();
                    s = s.replace("\n---\n", "\n\n");
                    s = s.replace("---", "———");
                    s = s.replace("\\n", "\n");
                    // Strip [SCHEDULE:N] line from end
                    if let Some(tag_pos) = s.find("[SCHEDULE:") {
                        if let Some(newline_pos) = s[..tag_pos].rfind('\n') {
                            s.truncate(newline_pos);
                        } else {
                            s.truncate(tag_pos);
                        }
                        s = s.trim().to_string();
                    }
                    s
                }
                None => return true,
            };

            if let Some(ref handle) = self.bot_handle {
                let h = handle.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio");
                    rt.block_on(async {
                        // Step 1: split by newline
                        let lines: Vec<String> = if full_text.contains('\n') {
                            full_text
                                .split('\n')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        } else {
                            vec![full_text.clone()]
                        };
                        // Step 2: split each line by sentence punctuation
                        let mut segments = Vec::new();
                        for line in &lines {
                            let mut current = String::new();
                            let mut has_punct = false;
                            for ch in line.chars() {
                                current.push(ch);
                                let is_comma = ch == '，';
                                let is_split = is_comma
                                    || ch == '。'
                                    || ch == '？'
                                    || ch == '！'
                                    || ch == '…'
                                    || ch == '?'
                                    || ch == '!';
                                if is_split {
                                    if is_comma {
                                        current.pop(); // remove the comma from output
                                    }
                                    if !current.trim().is_empty() {
                                        segments.push(current.trim().to_string());
                                    }
                                    current = String::new();
                                    has_punct = true;
                                }
                            }
                            if !current.trim().is_empty() {
                                segments.push(current.trim().to_string());
                            } else if !has_punct && !line.trim().is_empty() {
                                segments.push(line.trim().to_string());
                            }
                        }

                        let raw_segments = segments;

                        let segments: Vec<String> = raw_segments
                            .into_iter()
                            .filter(|s| s.trim().chars().count() > 1) // skip single-char fragments (e.g. standalone "…")
                            .collect();

                        for (i, seg) in segments.iter().enumerate() {
                            // 段间发 typing 保持存在感
                            let _ = h.send_typing(chat_id).await;
                            // 尝试 Markdown 发送，失败则回退纯文本
                            if let Err(_) = h.send_markdown(chat_id, seg).await {
                                if let Err(e) = h.send_message(chat_id, seg).await {
                                    eprintln!(
                                        "[tg-im] send failed (segment {}/{}): {}",
                                        i + 1,
                                        segments.len(),
                                        e
                                    );
                                }
                            }
                            // 段间短暂延时，避免 TG 限频
                            if i + 1 < segments.len() {
                                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                            }
                        }
                    });
                });
            }
            return true;
        }

        // ── image.gen.complete 不再由 TG 自动发送 ──
        // 生图后由 LLM 根据当前会话平台主动调用 tg_send_photo / wechat_send_image
        // （携生图工具返回的 file_path）发送，避免跨平台误发到 TG。

        true
    }
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(TgImPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}

// ── base64 decode ────────────────────────────────────────────────────────────

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    let mut buf = Vec::with_capacity(chars.len() * 3 / 4);
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut i = 0;
    while i + 3 < chars.len() {
        let a = cv(chars[i], TABLE)?;
        let b = cv(chars[i + 1], TABLE)?;
        let c = cv(chars[i + 2], TABLE)?;
        let d = cv(chars[i + 3], TABLE)?;
        buf.push((a << 2) | (b >> 4));
        buf.push((b << 4) | (c >> 2));
        buf.push((c << 6) | d);
        i += 4;
    }
    if i < chars.len() {
        let a = cv(chars[i], TABLE)?;
        let b = if i + 1 < chars.len() && chars[i + 1] != '=' {
            cv(chars[i + 1], TABLE)?
        } else {
            0
        };
        let c = if i + 2 < chars.len() && chars[i + 2] != '=' {
            cv(chars[i + 2], TABLE)?
        } else {
            0
        };
        buf.push((a << 2) | (b >> 4));
        if i + 1 < chars.len() && chars[i + 1] != '=' {
            buf.push((b << 4) | (c >> 2));
        }
    }
    Ok(buf)
}

fn cv(c: char, table: &[u8; 64]) -> Result<u8, String> {
    if c == '=' {
        return Ok(0);
    }
    table
        .iter()
        .position(|&x| x == c as u8)
        .map(|p| p as u8)
        .ok_or_else(|| format!("invalid base64 char: '{}'", c))
}
