//! Telegram Bot 核心逻辑 — teloxide 消息接收与发送。
//!
//! 不依赖 plugin-interface，保持纯 bot 逻辑可独立测试。

use std::io::Cursor;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::net::Download;
use teloxide::types::{ChatAction, ChatId, InputFile};

// ── BotHandle — 供插件和工具调用 ────────────────────────────────────────────

#[derive(Clone)]
pub struct BotHandle {
    bot: Bot,
}

impl BotHandle {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }

    pub async fn shutdown(&self) {
        log::info!("[tg-im] bot shutting down");
    }

    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), String> {
        self.bot
            .send_message(ChatId(chat_id), text)
            .await
            .map_err(|e| format!("send_message failed: {}", e))?;
        Ok(())
    }

    pub async fn send_typing(&self, chat_id: i64) -> Result<(), String> {
        self.bot
            .send_chat_action(ChatId(chat_id), ChatAction::Typing)
            .await
            .map_err(|e| format!("send_chat_action failed: {}", e))?;
        Ok(())
    }

    pub async fn send_record_voice_action(&self, chat_id: i64) -> Result<(), String> {
        self.bot
            .send_chat_action(ChatId(chat_id), ChatAction::RecordVoice)
            .await
            .map_err(|e| format!("send_chat_action failed: {}", e))?;
        Ok(())
    }

    pub async fn send_voice(&self, chat_id: i64, audio_data: Vec<u8>) -> Result<(), String> {
        let file = InputFile::memory(audio_data).file_name("voice.ogg");
        self.bot
            .send_voice(ChatId(chat_id), file)
            .await
            .map_err(|e| format!("send_voice failed: {}", e))?;
        Ok(())
    }

    pub async fn send_photo(
        &self,
        chat_id: i64,
        photo_data: Vec<u8>,
        caption: Option<&str>,
    ) -> Result<(), String> {
        let file = InputFile::memory(photo_data).file_name("photo.jpg");
        let mut req = self.bot.send_photo(ChatId(chat_id), file);
        if let Some(cap) = caption {
            req = req.caption(cap);
        }
        req.await.map_err(|e| format!("send_photo failed: {}", e))?;
        Ok(())
    }

    pub async fn send_video(
        &self,
        chat_id: i64,
        video_data: Vec<u8>,
        caption: Option<&str>,
    ) -> Result<(), String> {
        let file = InputFile::memory(video_data).file_name("video.mp4");
        let mut req = self.bot.send_video(ChatId(chat_id), file);
        if let Some(cap) = caption {
            req = req.caption(cap);
        }
        req.await.map_err(|e| format!("send_video failed: {}", e))?;
        Ok(())
    }

    pub async fn send_document(
        &self,
        chat_id: i64,
        doc_data: Vec<u8>,
        file_name: String,
        caption: Option<&str>,
    ) -> Result<(), String> {
        let file = InputFile::memory(doc_data).file_name(file_name);
        let mut req = self.bot.send_document(ChatId(chat_id), file);
        if let Some(cap) = caption {
            req = req.caption(cap);
        }
        req.await.map_err(|e| format!("send_document failed: {}", e))?;
        Ok(())
    }
}

// ── 事件发射回调 ────────────────────────────────────────────────────────────

/// Bot 收到用户文本消息时调用此回调发布事件到 EventBus。
pub type UserMessageCallback = Arc<dyn Fn(i64, &str, &str) + Send + Sync>;
/// Bot 收到语音消息时调用：chat_id, base64_audio, mime_type, user_name
pub type VoiceMessageCallback = Arc<dyn Fn(i64, String, &str, &str) + Send + Sync>;
/// Bot 收到图片时调用：chat_id, base64_jpeg, user_name
pub type PhotoMessageCallback = Arc<dyn Fn(i64, String, &str) + Send + Sync>;
/// Bot 收到文件时调用：chat_id, base64_data, file_name, user_name
pub type FileMessageCallback = Arc<dyn Fn(i64, String, String, &str) + Send + Sync>;
/// Bot 收到视频时调用：chat_id, base64_video, mime_type, user_name
pub type VideoMessageCallback = Arc<dyn Fn(i64, String, String, &str) + Send + Sync>;

// ── Bot 启动 ─────────────────────────────────────────────────────────────────

/// 在后台线程启动 Telegram bot，返回 BotHandle 和 JoinHandle。
///
/// `on_user_message(chat_id, text, user_name)` — 当收到用户文本消息时调用。
/// `bot` 是预先构建好的 teloxide Bot（含代理配置）。
pub fn run_bot(
    bot: teloxide::Bot,
    on_user_message: UserMessageCallback,
    on_voice_message: Option<VoiceMessageCallback>,
    on_photo_message: Option<PhotoMessageCallback>,
    on_file_message: Option<FileMessageCallback>,
    on_video_message: Option<VideoMessageCallback>,
) -> Result<(BotHandle, std::thread::JoinHandle<()>), String> {
    let handle = BotHandle::new(bot.clone());

    // 在独立线程中运行 tokio + teloxide。
    let join_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("[tg-im] failed to build tokio runtime");

        rt.block_on(async move {
            // 验证连接。
            match bot.get_me().await {
                Ok(me) => {
                    eprintln!(
                        "[tg-im] bot @{} connected",
                        me.username.as_deref().unwrap_or("unknown")
                    );
                }
                Err(e) => {
                    eprintln!("[tg-im] get_me failed: {}", e);
                    return;
                }
            }

            let cb = on_user_message.clone();
            let voice_cb = on_voice_message.clone();
            let photo_cb = on_photo_message.clone();
            let file_cb = on_file_message.clone();
            let video_cb = on_video_message.clone();

            let handler = move |msg: Message, bot: Bot| {
                let cb = cb.clone();
                let voice_cb = voice_cb.clone();
                let photo_cb = photo_cb.clone();
                let file_cb = file_cb.clone();
                let video_cb = video_cb.clone();
                async move {
                    let chat_id = msg.chat.id.0;
                    let user_name = msg
                        .from
                        .as_ref()
                        .map(|u| u.first_name.clone())
                        .unwrap_or_else(|| "unknown".to_string());

                    // ── 命令处理 ──
                    if let Some(text) = msg.text() {
                        if text.starts_with('/') {
                            let cmd = text.split_whitespace().next().unwrap_or("");
                            let response = match cmd {
                                "/start" => "👋 Hello! I'm an AI assistant. Send me a message!".into(),
                                "/help" => "Just send me a message and I'll respond.\nCommands: /start /help /status".into(),
                                "/status" => "✅ Bot is running.".into(),
                                _ => format!("Unknown command: {}. Try /help", cmd),
                            };
                            let _ = bot.send_message(ChatId(chat_id), response).await;
                            return respond(());
                        }

                        // ── 普通文本消息 → 发布事件 ──
                        eprintln!("[tg-im] text from @{}: {}", user_name, text);
                        cb(chat_id, text, &user_name);

                        // 发送"正在输入..."状态
                        let _ = bot
                            .send_chat_action(ChatId(chat_id), ChatAction::Typing)
                            .await;
                    }

                    // ── 图片消息 → 下载最大的照片 ──
                    if let Some(photos) = msg.photo() {
                        if let Some(ref pcb) = photo_cb {
                            // 发送"正在输入..."状态
                            let _ = bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await;
                            // 选最大尺寸的照片
                            let best = photos.iter().max_by_key(|p| p.width * p.height).unwrap();
                            eprintln!("[tg-im] photo from @{} ({}x{})", user_name, best.width, best.height);
                            match bot.get_file(&best.file.id).await {
                                Ok(file) => {
                                    let mut buf = Cursor::new(Vec::new());
                                    match bot.download_file(&file.path, &mut buf).await {
                                        Ok(_) => {
                                            let b64 = base64_encode(buf.get_ref());
                                            pcb(chat_id, b64, &user_name);
                                        }
                                        Err(e) => eprintln!("[tg-im] photo download: {}", e),
                                    }
                                }
                                Err(e) => eprintln!("[tg-im] photo get_file: {}", e),
                            }
                        }
                    }

                    // ── 文件消息 → 下载 ──
                    if let Some(doc) = msg.document() {
                        if let Some(ref fcb) = file_cb {
                            let _ = bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await;
                            let fname = doc.file_name.as_deref().unwrap_or("file");
                            eprintln!("[tg-im] file from @{}: {}", user_name, fname);
                            match bot.get_file(&doc.file.id).await {
                                Ok(file) => {
                                    let mut buf = Cursor::new(Vec::new());
                                    match bot.download_file(&file.path, &mut buf).await {
                                        Ok(_) => {
                                            let b64 = base64_encode(buf.get_ref());
                                            fcb(chat_id, b64, fname.to_string(), &user_name);
                                        }
                                        Err(e) => eprintln!("[tg-im] file download: {}", e),
                                    }
                                }
                                Err(e) => eprintln!("[tg-im] file get_file: {}", e),
                            }
                        }
                    }

                    // ── 视频消息 → 下载 ──
                    if let Some(video) = msg.video() {
                        if let Some(ref vcb) = video_cb {
                            let _ = bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await;
                            let mime = video.mime_type.as_ref()
                                .map(|m| m.to_string())
                                .unwrap_or_else(|| "video/mp4".into());
                            eprintln!("[tg-im] video from @{} ({}x{})", user_name, video.width, video.height);
                            match bot.get_file(&video.file.id).await {
                                Ok(file) => {
                                    let mut buf = Cursor::new(Vec::new());
                                    match bot.download_file(&file.path, &mut buf).await {
                                        Ok(_) => {
                                            let b64 = base64_encode(buf.get_ref());
                                            vcb(chat_id, b64, mime, &user_name);
                                        }
                                        Err(e) => eprintln!("[tg-im] video download: {}", e),
                                    }
                                }
                                Err(e) => eprintln!("[tg-im] video get_file: {}", e),
                            }
                        }
                    }

                    // ── 语音消息 → 下载 + ASR ──
                    if let Some(voice) = msg.voice() {
                        if let Some(ref vcb) = voice_cb {
                            let _ = bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await;
                            eprintln!("[tg-im] voice from @{} ({}s)", user_name, voice.duration);
                            let mime = voice.mime_type.as_ref()
                                .map(|m| m.to_string())
                                .unwrap_or_else(|| "audio/ogg".into());
                            match bot.get_file(&voice.file.id).await {
                                Ok(file) => {
                                    let mut buf = Cursor::new(Vec::new());
                                    match bot.download_file(&file.path, &mut buf).await {
                                        Ok(_) => {
                                            let b64 = base64_encode(buf.get_ref());
                                            vcb(chat_id, b64, &mime, &user_name);
                                        }
                                        Err(e) => eprintln!("[tg-im] voice download: {}", e),
                                    }
                                }
                                Err(e) => eprintln!("[tg-im] voice get_file: {}", e),
                            }
                        }
                    }

                    respond(())
                }
            };

            log::info!("[tg-im] dispatcher starting...");
            Dispatcher::builder(
                bot,
                Update::filter_message().branch(dptree::endpoint(handler)),
            )
            .build()
            .dispatch()
            .await;
            log::info!("[tg-im] dispatcher exited");
        });
    });

    Ok((handle, join_handle))
}

// ── reqwest client 构建（支持代理） ──────────────────────────────────────────

// ── base64 ────────────────────────────────────────────────────────

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn build_reqwest_client() -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30));

    if let Ok(proxy_url) = std::env::var("TG_PROXY_URL") {
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(|e| format!("proxy config failed: {}", e))?;
        builder = builder.proxy(proxy);
        log::info!("[tg-im] using proxy: {}", proxy_url);
    }

    builder.build().map_err(|e| format!("reqwest build failed: {}", e))
}
