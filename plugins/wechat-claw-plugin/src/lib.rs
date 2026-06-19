//! wechat-claw-plugin — 微信 iLink Bot API 插件
//!
//! Powered by weixin-ilink-sdk for full protocol support (QR login, long-poll,
//! text + media messaging, CDN upload/download, voice decoding).
//!
//! ## 事件流转
//!
//! ```text
//! 微信 ─► get_updates 轮询 ─► EventBus("user.message", {source:"wechat", ...})
//!                                     │
//!                               PipelineActor → LLM → emit_reply()
//!                                     │
//!                               EventBus("route.message", {source:"wechat"})
//!                                     │
//!                               MessageRouter → "assistant.message"
//!                                     │
//!                               PluginManager.on_event()
//!                                     │
//!                               wechat-claw-plugin → sendmessage → 微信
//! ```

mod client;

use client::*;
use plugin_interface::*;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

// ── Status ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum BnStatus {
    Uninitialized,
    WaitingQr { qr_path: String },
    Scanned,
    LoggingIn,
    Online { nick: String },
    Error(String),
}

// ── Tool: wechat_send_message ────────────────────────────────────────────────

struct SendWechatMessage {
    client: Arc<Mutex<Option<WeChatClient>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
    processing_users: Arc<Mutex<HashSet<String>>>,
}

impl ToolExecutor for SendWechatMessage {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "wechat_send_message".into(),
            description: "通过微信发送一条消息".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to_user": {
                        "type": "string",
                        "description": "Contact user_id (optional, defaults to last sender)"
                    },
                    "text": {
                        "type": "string",
                        "description": "Message text content"
                    }
                },
                "required": ["text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let wechat = match self.client.lock().unwrap().clone() {
            Some(c) => c,
            None => return ToolResult::err("微信未登录，请先扫码"),
        };

        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("missing: text"),
        };

        let to_user = args
            .get("to_user")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| self.last_chat_id.lock().unwrap().clone());

        let to_user = match to_user {
            Some(u) => u,
            None => return ToolResult::err("没有指定 to_user 且没有最近联系人"),
        };

        // 停止该用户的输入状态
        self.processing_users.lock().unwrap().remove(&to_user);

        // 查找 context_token
        let ctx_token = wechat.get_context_token(&to_user).unwrap_or_default();

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(&format!("runtime: {}", e)),
        };

        match rt.block_on(wechat.send_text(&to_user, &text, &ctx_token)) {
            Ok(()) => ToolResult::ok("消息已发送"),
            Err(e) => ToolResult::err(&format!("发送失败: {}", e)),
        }
    }
}

// ── Tool: wechat_send_image ──────────────────────────────────────────────────

struct SendWechatImage {
    client: Arc<Mutex<Option<WeChatClient>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
}

impl ToolExecutor for SendWechatImage {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "wechat_send_image".into(),
            description: "通过微信发送一张图片（自动上传 CDN）".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to_user": {
                        "type": "string",
                        "description": "Contact user_id (optional, defaults to last sender)"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Image file path on disk"
                    },
                    "text": {
                        "type": "string",
                        "description": "Optional caption text"
                    }
                },
                "required": ["file_path"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let wechat = match self.client.lock().unwrap().clone() {
            Some(c) => c,
            None => return ToolResult::err("微信未登录，请先扫码"),
        };

        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return ToolResult::err("missing: file_path"),
        };

        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let to_user = args
            .get("to_user")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| self.last_chat_id.lock().unwrap().clone());

        let to_user = match to_user {
            Some(u) => u,
            None => return ToolResult::err("没有指定 to_user 且没有最近联系人"),
        };

        let ctx_token = wechat.get_context_token(&to_user).unwrap_or_default();

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(&format!("runtime: {}", e)),
        };

        match rt.block_on(wechat.send_image(
            &to_user,
            std::path::Path::new(&file_path),
            &text,
            &ctx_token,
        )) {
            Ok(()) => ToolResult::ok(&format!("图片已发送")),
            Err(e) => ToolResult::err(&format!("发送失败: {}", e)),
        }
    }
}

// ── Tool: wechat_send_file ───────────────────────────────────────────────────

struct SendWechatFile {
    client: Arc<Mutex<Option<WeChatClient>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
}

impl ToolExecutor for SendWechatFile {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "wechat_send_file".into(),
            description: "通过微信发送一个文件附件（自动上传 CDN）".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to_user": {
                        "type": "string",
                        "description": "Contact user_id (optional, defaults to last sender)"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "File path on disk"
                    },
                    "text": {
                        "type": "string",
                        "description": "Optional caption text"
                    }
                },
                "required": ["file_path"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let wechat = match self.client.lock().unwrap().clone() {
            Some(c) => c,
            None => return ToolResult::err("微信未登录，请先扫码"),
        };

        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return ToolResult::err("missing: file_path"),
        };

        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let to_user = args
            .get("to_user")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| self.last_chat_id.lock().unwrap().clone());

        let to_user = match to_user {
            Some(u) => u,
            None => return ToolResult::err("没有指定 to_user 且没有最近联系人"),
        };

        let ctx_token = wechat.get_context_token(&to_user).unwrap_or_default();

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(&format!("runtime: {}", e)),
        };

        match rt.block_on(wechat.send_media(
            &to_user,
            std::path::Path::new(&file_path),
            &text,
            &ctx_token,
        )) {
            Ok(()) => ToolResult::ok("文件已发送"),
            Err(e) => ToolResult::err(&format!("发送失败: {}", e)),
        }
    }
}

// ── Tool: wechat_qrcode ──────────────────────────────────────────────────────

struct GetQrCode {
    status: Arc<Mutex<BnStatus>>,
}

impl ToolExecutor for GetQrCode {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "wechat_qrcode".into(),
            description: "Get current WeChat QR code info and login status.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        });
        &DEF
    }

    fn execute(&self, _args: &serde_json::Value) -> ToolResult {
        let st = self.status.lock().unwrap().clone();
        match st {
            BnStatus::WaitingQr { qr_path } => {
                ToolResult::ok(&format!("等待扫码。二维码图片：{}", qr_path))
            }
            BnStatus::Scanned => ToolResult::ok("二维码已被扫描，请在手机上确认登录。"),
            BnStatus::LoggingIn => ToolResult::ok("正在登录中..."),
            BnStatus::Online { nick } => {
                ToolResult::ok(&format!("微信已登录（{}），无需扫码。", nick))
            }
            BnStatus::Error(ref e) => ToolResult::err(&format!("状态异常：{}", e)),
            BnStatus::Uninitialized => ToolResult::err("插件尚未初始化。"),
        }
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct WechatClawPlugin {
    client: Arc<Mutex<Option<WeChatClient>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
    /// 正在等待 LLM 回复的用户集合，typing 循环根据它决定是否继续
    processing_users: Arc<Mutex<HashSet<String>>>,
    /// 每个用户的 typing_ticket（缓存 ≈24h）
    typing_tickets: Arc<Mutex<HashMap<String, String>>>,
    status: Arc<Mutex<BnStatus>>,
    running: Arc<AtomicBool>,
    login_thread: Option<JoinHandle<()>>,
    poll_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl WechatClawPlugin {
    pub fn new() -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
            last_chat_id: Arc::new(Mutex::new(None)),
            processing_users: Arc::new(Mutex::new(HashSet::new())),
            typing_tickets: Arc::new(Mutex::new(HashMap::new())),
            status: Arc::new(Mutex::new(BnStatus::Uninitialized)),
            running: Arc::new(AtomicBool::new(true)),
            login_thread: None,
            poll_thread: Arc::new(Mutex::new(None)),
        }
    }
}

impl Plugin for WechatClawPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "wechat-claw-plugin".into(),
            version: "0.2.0".into(),
            description: "WeChat IM plugin — iLink Bot API (powered by weixin-ilink-sdk)".into(),
            author: "BN Team".into(),
            min_host_version: "0.1.0".into(),
        }
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[wechat] plugin starting...");

        // 1. 注册工具
        if let Some(ref reg) = ctx.tool_registry {
            let mut reg = reg.lock().map_err(|e| format!("lock: {}", e))?;
            reg.register(Arc::new(SendWechatMessage {
                client: self.client.clone(),
                last_chat_id: self.last_chat_id.clone(),
                processing_users: self.processing_users.clone(),
            }));
            reg.register(Arc::new(SendWechatImage {
                client: self.client.clone(),
                last_chat_id: self.last_chat_id.clone(),
            }));
            reg.register(Arc::new(SendWechatFile {
                client: self.client.clone(),
                last_chat_id: self.last_chat_id.clone(),
            }));
            reg.register(Arc::new(GetQrCode {
                status: self.status.clone(),
            }));
            log::info!("[wechat] registered 4 tools");
        }

        // 2. 克隆共享状态给后台线程
        let running = self.running.clone();
        let client = self.client.clone();
        let last_chat_id = self.last_chat_id.clone();
        let processing_users = self.processing_users.clone();
        let typing_tickets = self.typing_tickets.clone();
        let status = self.status.clone();
        let poll_thread_holder = self.poll_thread.clone();
        let event_bus = ctx.event_bus.clone();

        // 3. 启动主循环线程
        let handle = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    log::error!("[wechat] runtime: {}", e);
                    *status.lock().unwrap() = BnStatus::Error(format!("runtime: {}", e));
                    return;
                }
            };
            rt.block_on(main_loop(
                running, client, last_chat_id,
                processing_users, typing_tickets, status,
                poll_thread_holder, event_bus,
            ));
        });

        self.login_thread = Some(handle);
        log::info!("[wechat] plugin started");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[wechat] stopping...");
        self.running.store(false, Ordering::SeqCst);

        if let Some(h) = self.login_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.poll_thread.lock().unwrap().take() {
            let _ = h.join();
        }
        log::info!("[wechat] stopped");
    }

    fn snapshot(&self) -> Option<String> {
        let st = self.status.lock().unwrap().clone();
        match st {
            BnStatus::Online { nick } => {
                Some(format!("[微信插件] 已登录 | 账号：{}", nick))
            }
            BnStatus::WaitingQr { qr_path } => {
                Some(format!("[微信插件] 等待扫码 ┃ 二维码：{}", qr_path))
            }
            BnStatus::Scanned => Some("[微信插件] 二维码已扫描，请在手机上确认登录".into()),
            BnStatus::LoggingIn => Some("[微信插件] 正在登录中...".into()),
            BnStatus::Error(ref e) => Some(format!("[微信插件] 异常：{}", e)),
            BnStatus::Uninitialized => None,
        }
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "assistant.message" {
            let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if source != "wechat" {
                return true;
            }

            let text = match event.data.get("text").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => return true,
            };

            let chat_id = event
                .data
                .get("chat_id")
                .and_then(|v| v.as_str().map(String::from))
                .or_else(|| event.data.get("chat_id").and_then(|v| v.as_i64().map(|n| n.to_string())))
                .or_else(|| self.last_chat_id.lock().unwrap().clone());

            let chat_id = match chat_id {
                Some(id) if !id.is_empty() => id,
                _ => {
                    log::warn!("[wechat] no chat_id for reply");
                    return true;
                }
            };

            // 停止输入状态
            self.processing_users.lock().unwrap().remove(&chat_id);

            let wechat = match self.client.lock().unwrap().clone() {
                Some(c) => c,
                None => {
                    log::warn!("[wechat] no client for reply");
                    return true;
                }
            };

            let ctx_token = wechat.get_context_token(&chat_id).unwrap_or_default();

            // Check for media in the event data
            let image_path = event.data.get("image_path").and_then(|v| v.as_str());
            let file_path = event.data.get("file_path").and_then(|v| v.as_str());

            log::info!("[wechat] replying to {}: {}", chat_id, text.chars().take(40).collect::<String>());

            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    log::error!("[wechat] runtime: {}", e);
                    return true;
                }
            };

            if let Some(path) = image_path {
                rt.block_on(async {
                    if let Err(e) = wechat.send_image(
                        &chat_id, std::path::Path::new(path), &text, &ctx_token,
                    ).await {
                        log::error!("[wechat] send_image failed: {}", e);
                        if e.contains("-14") {
                            WeChatClient::clear_saved();
                        }
                    }
                });
            } else if let Some(path) = file_path {
                rt.block_on(async {
                    if let Err(e) = wechat.send_media(
                        &chat_id, std::path::Path::new(path), &text, &ctx_token,
                    ).await {
                        log::error!("[wechat] send_media failed: {}", e);
                        if e.contains("-14") {
                            WeChatClient::clear_saved();
                        }
                    }
                });
            } else {
                rt.block_on(async {
                    if let Err(e) = wechat.send_text(&chat_id, &text, &ctx_token).await {
                        log::error!("[wechat] reply failed: {}", e);
                        if e.contains("-14") {
                            WeChatClient::clear_saved();
                        }
                    }
                });
            }

            return true;
        }

        // user.message from wechat → typing already started in poll_loop
        if event.topic == "user.message" {
            let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if source != "wechat" {
                return true;
            }
            return true;
        }

        true
    }
}

// ── Main loop (login → poll → reconnect) ─────────────────────────────────────

async fn main_loop(
    running: Arc<AtomicBool>,
    client: Arc<Mutex<Option<WeChatClient>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
    processing_users: Arc<Mutex<HashSet<String>>>,
    typing_tickets: Arc<Mutex<HashMap<String, String>>>,
    status: Arc<Mutex<BnStatus>>,
    poll_thread_holder: Arc<Mutex<Option<JoinHandle<()>>>>,
    event_bus: Addr<EventBus>,
) {
    while running.load(Ordering::SeqCst) {
        // 1. 尝试加载已保存的 session
        if let Some(session_info) = WeChatClient::load_from_file() {
            let wechat = WeChatClient::restore(session_info.clone());
            log::info!("[wechat] loaded saved session: {}", session_info.account_id);
            *client.lock().unwrap() = Some(wechat.clone());
            *status.lock().unwrap() = BnStatus::Online { nick: session_info.account_id.clone() };

            start_poll_thread(
                &poll_thread_holder, &running, &wechat, &last_chat_id,
                &processing_users, &typing_tickets, &status, &event_bus,
            );

            wait_loop(&running, &status).await;
            if !running.load(Ordering::SeqCst) { return; }
            log::warn!("[wechat] session expired, re-login required");
            WeChatClient::clear_saved();
            *client.lock().unwrap() = None;
            stop_poll_thread(&poll_thread_holder);
        }

        if !running.load(Ordering::SeqCst) { return; }

        // 2. 扫码登录 (via SDK)
        log::info!("[wechat] starting QR code login...");
        match qr_login_via_sdk(&status, &running).await {
            Some(wechat) => {
                let _ = wechat.save_to_file();
                let nick = wechat.session.account_id.clone();
                *client.lock().unwrap() = Some(wechat.clone());
                *status.lock().unwrap() = BnStatus::Online { nick: nick.clone() };

                start_poll_thread(
                    &poll_thread_holder, &running, &wechat, &last_chat_id,
                    &processing_users, &typing_tickets, &status, &event_bus,
                );

                wait_loop(&running, &status).await;
                if !running.load(Ordering::SeqCst) { return; }
                log::warn!("[wechat] session expired, re-login required");
                WeChatClient::clear_saved();
                *client.lock().unwrap() = None;
                stop_poll_thread(&poll_thread_holder);
            }
            None => {
                log::error!("[wechat] QR login returned None");
                sleep_ms(5000).await;
            }
        }
    }
}

// ── QR login via SDK ─────────────────────────────────────────────────────────

async fn qr_login_via_sdk(
    status: &Arc<Mutex<BnStatus>>,
    running: &Arc<AtomicBool>,
) -> Option<WeChatClient> {
    let max_attempts = 3u32;

    for attempt in 1..=max_attempts {
        if !running.load(Ordering::SeqCst) {
            return None;
        }

        let handler = Arc::new(PluginLoginHandler::new());
        let handler_clone = handler.clone();

        // Spawn a status polling task that updates BnStatus
        let status_task = {
            let status = status.clone();
            let running = running.clone();
            tokio::spawn(async move {
                loop {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    let status_text = handler_clone.status_text.lock().unwrap().clone();
                    if status_text == "qr_ready" {
                        let qr_path = handler_clone.qr_path.lock().unwrap().clone();
                        if let Some(ref p) = qr_path {
                            *status.lock().unwrap() = BnStatus::WaitingQr { qr_path: p.clone() };
                        }
                    }
                    if *handler_clone.scanned.lock().unwrap() {
                        *status.lock().unwrap() = BnStatus::Scanned;
                    }
                    // Check if login completed
                    if handler_clone.result.lock().unwrap().is_some() {
                        break;
                    }
                    sleep_ms(500).await;
                }
            })
        };

        // Run login directly (awaited, not spawned — handler lives on stack)
        match WeChatClient::login_with_result(handler.as_ref()).await {
            Ok(wechat) => {
                status_task.abort();
                log::info!("[wechat] ✅ 登录成功！Bot: {}", wechat.session.account_id);
                *status.lock().unwrap() = BnStatus::LoggingIn;
                return Some(wechat);
            }
            Err(e) => {
                status_task.abort();
                log::error!("[wechat] login error: {}", e);
                if attempt < max_attempts {
                    log::warn!("[wechat] retrying login ({}/{})", attempt + 1, max_attempts);
                    sleep_ms(2000).await;
                }
            }
        }
    }

    log::error!("[wechat] all login attempts failed");
    *status.lock().unwrap() = BnStatus::Error("登录失败（多次尝试）".into());
    None
}

// ── Poll thread management ───────────────────────────────────────────────────

fn start_poll_thread(
    holder: &Arc<Mutex<Option<JoinHandle<()>>>>,
    running: &Arc<AtomicBool>,
    wechat: &WeChatClient,
    last_chat_id: &Arc<Mutex<Option<String>>>,
    processing_users: &Arc<Mutex<HashSet<String>>>,
    typing_tickets: &Arc<Mutex<HashMap<String, String>>>,
    status: &Arc<Mutex<BnStatus>>,
    event_bus: &Addr<EventBus>,
) {
    stop_poll_thread(holder);

    let running_c = running.clone();
    let wechat_c = wechat.clone();
    let last_c = last_chat_id.clone();
    let pu_c = processing_users.clone();
    let tt_c = typing_tickets.clone();
    let status_c = status.clone();
    let eb_c = event_bus.clone();

    let handle = std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => { log::error!("[wechat] poll runtime: {}", e); return; }
        };
        rt.block_on(poll_loop(
            running_c, wechat_c, last_c,
            pu_c, tt_c, status_c, eb_c,
        ));
    });

    *holder.lock().unwrap() = Some(handle);
}

fn stop_poll_thread(holder: &Arc<Mutex<Option<JoinHandle<()>>>>) {
    if let Some(h) = holder.lock().unwrap().take() {
        let _ = h.join();
    }
}

// ── Message poll loop ────────────────────────────────────────────────────────

async fn poll_loop(
    running: Arc<AtomicBool>,
    wechat: WeChatClient,
    last_chat_id: Arc<Mutex<Option<String>>>,
    processing_users: Arc<Mutex<HashSet<String>>>,
    typing_tickets: Arc<Mutex<HashMap<String, String>>>,
    status: Arc<Mutex<BnStatus>>,
    event_bus: Addr<EventBus>,
) {
    let mut buf = String::new();
    let mut consecutive_errors = 0u32;

    while running.load(Ordering::SeqCst) {
        match wechat.poll_messages(&buf, 35).await {
            Ok(result) => {
                consecutive_errors = 0;
                buf = result.next_buf;

                for msg in &result.messages {
                    // Auto-cache context token via SDK
                    wechat.inner.set_context_token(&msg.from_user_id, &msg.context_token);
                    *last_chat_id.lock().unwrap() = Some(msg.from_user_id.clone());

                    log::info!(
                        "[wechat] msg from {}: text={}, items={}",
                        msg.from_user_id,
                        msg.text.chars().take(60).collect::<String>(),
                        msg.items.len(),
                    );

                    // ── Build event data ──
                    let mut event_data = serde_json::json!({
                        "text": msg.text,
                        "source": "wechat",
                        "user_name": msg.from_user_id,
                        "chat_id": msg.from_user_id,
                    });

                    // Check for media items and download asynchronously
                    for item in &msg.items {
                        let item_type = item.item_type;
                        match item_type {
                            Some(MessageItemType::Image) => {
                                if let Some(img) = &item.image_item {
                                    handle_incoming_image(
                                        &wechat, &msg.from_user_id, img, &mut event_data,
                                    ).await;
                                }
                            }
                            Some(MessageItemType::Voice) => {
                                if let Some(voice) = &item.voice_item {
                                    handle_incoming_voice(
                                        &wechat, &msg.from_user_id, voice, &mut event_data,
                                    ).await;
                                }
                            }
                            Some(MessageItemType::Video) => {
                                if let Some(video) = &item.video_item {
                                    handle_incoming_video(
                                        &wechat, video, &mut event_data,
                                    ).await;
                                }
                            }
                            Some(MessageItemType::File) => {
                                if let Some(file) = &item.file_item {
                                    handle_incoming_file(
                                        &wechat, file, &mut event_data,
                                    ).await;
                                }
                            }
                            _ => {}
                        }
                    }

                    // 发布 user.message 事件
                    event_bus.do_send(Event::new(
                        "user.message",
                        event_data,
                        "wechat-claw-plugin",
                    ));

                    // ── 启动输入状态指示器 ──
                    start_typing(
                        &wechat, &msg.from_user_id,
                        &processing_users, &typing_tickets, &running,
                    ).await;
                }
            }
            Err(e) => {
                if e.contains("-14") {
                    log::warn!("[wechat] poll: session expired");
                    *status.lock().unwrap() = BnStatus::Error("登录过期".into());
                    return;
                }
                consecutive_errors += 1;
                log::error!("[wechat] poll error ({}): {}", consecutive_errors, e);
                if consecutive_errors > 5 {
                    log::error!("[wechat] too many poll errors");
                    *status.lock().unwrap() = BnStatus::Error("轮询异常".into());
                    return;
                }
                sleep_ms(3000).await;
            }
        }
    }
}

// ── Incoming media handlers ──────────────────────────────────────────────────

use weixin_ilink_sdk::types::{
    FileItem, ImageItem, MessageItemType, VideoItem, VoiceItem,
};

async fn handle_incoming_image(
    wechat: &WeChatClient,
    from_user: &str,
    img: &ImageItem,
    event_data: &mut serde_json::Value,
) {
    let param = match img.media.as_ref().and_then(|m| m.encrypt_query_param.as_deref()) {
        Some(p) => p,
        None => {
            log::warn!("[wechat] image has no encrypt_query_param");
            return;
        }
    };

    // Prefer hex aeskey (ImageItem.aeskey), fall back to base64 aes_key
    let data = if let Some(hex_key) = img.aeskey.as_deref() {
        wechat.download_media_hex_key(param, hex_key).await
    } else if let Some(aes_key) = img.media.as_ref().and_then(|m| m.aes_key.as_deref()) {
        wechat.download_media(param, aes_key).await
    } else {
        log::warn!("[wechat] image has no aes key");
        return;
    };

    match data {
        Ok(bytes) => {
            let b64 = base64_encode(&bytes);
            log::info!(
                "[wechat] image downloaded from {}: {} bytes (base64: {} chars)",
                from_user, bytes.len(), b64.len()
            );
            event_data["image_base64"] = serde_json::json!(b64);
        }
        Err(e) => {
            log::error!("[wechat] image download failed: {}", e);
        }
    }
}

async fn handle_incoming_voice(
    wechat: &WeChatClient,
    from_user: &str,
    voice: &VoiceItem,
    event_data: &mut serde_json::Value,
) {
    if voice.media.is_none() {
        log::warn!("[wechat] voice has no media");
        return;
    }

    match wechat.download_voice(voice).await {
        Ok(wav_bytes) => {
            let b64 = base64_encode(&wav_bytes);
            log::info!(
                "[wechat] voice downloaded from {}: {} bytes WAV (base64: {} chars)",
                from_user, wav_bytes.len(), b64.len()
            );
            event_data["voice_base64"] = serde_json::json!(b64);

            // Include server-side voice-to-text if available
            if let Some(vt) = voice.text.as_deref() {
                if !vt.is_empty() {
                    // Prepend voice transcription to text
                    let existing = event_data["text"].as_str().unwrap_or("");
                    if existing.is_empty() {
                        event_data["text"] = serde_json::json!(format!("[语音] {}", vt));
                    }
                }
            }
        }
        Err(e) => {
            log::error!("[wechat] voice download failed: {}", e);
        }
    }
}

async fn handle_incoming_video(
    wechat: &WeChatClient,
    video: &VideoItem,
    event_data: &mut serde_json::Value,
) {
    let param = match video.media.as_ref().and_then(|m| m.encrypt_query_param.as_deref()) {
        Some(p) => p,
        None => {
            log::warn!("[wechat] video has no encrypt_query_param");
            return;
        }
    };

    let aes_key = match video.media.as_ref().and_then(|m| m.aes_key.as_deref()) {
        Some(k) => k,
        None => {
            log::warn!("[wechat] video has no aes key");
            return;
        }
    };

    match wechat.download_media(param, aes_key).await {
        Ok(bytes) => {
            let b64 = base64_encode(&bytes);
            log::info!("[wechat] video downloaded: {} bytes (base64: {} chars)", bytes.len(), b64.len());
            event_data["video_base64"] = serde_json::json!(b64);
        }
        Err(e) => {
            log::error!("[wechat] video download failed: {}", e);
        }
    }
}

async fn handle_incoming_file(
    wechat: &WeChatClient,
    file: &FileItem,
    event_data: &mut serde_json::Value,
) {
    let param = match file.media.as_ref().and_then(|m| m.encrypt_query_param.as_deref()) {
        Some(p) => p,
        None => {
            log::warn!("[wechat] file has no encrypt_query_param");
            return;
        }
    };

    let aes_key = match file.media.as_ref().and_then(|m| m.aes_key.as_deref()) {
        Some(k) => k,
        None => {
            log::warn!("[wechat] file has no aes key");
            return;
        }
    };

    match wechat.download_media(param, aes_key).await {
        Ok(bytes) => {
            let b64 = base64_encode(&bytes);
            let name = file.file_name.as_deref().unwrap_or("file.bin");
            log::info!("[wechat] file downloaded '{}': {} bytes", name, bytes.len());
            event_data["file_base64"] = serde_json::json!(b64);
            event_data["file_name"] = serde_json::json!(name);
        }
        Err(e) => {
            log::error!("[wechat] file download failed: {}", e);
        }
    }
}

// ── Typing indicator ─────────────────────────────────────────────────────────

/// 为指定用户启动输入状态指示器。
/// 获取 typing_ticket → 每 4 秒发一次 typing:start → 用户移出集合时发 typing:stop。
async fn start_typing(
    wechat: &WeChatClient,
    to_user_id: &str,
    processing_users: &Arc<Mutex<HashSet<String>>>,
    typing_tickets: &Arc<Mutex<HashMap<String, String>>>,
    running: &Arc<AtomicBool>,
) {
    // 标记该用户在 processing 中
    processing_users.lock().unwrap().insert(to_user_id.to_string());

    // 获取/复用 typing_ticket
    let context_token = wechat.get_context_token(to_user_id).unwrap_or_default();
    let ticket = {
        let cached = typing_tickets.lock().unwrap().get(to_user_id).cloned();
        if let Some(t) = cached {
            t
        } else {
            match wechat.get_typing_ticket(to_user_id, &context_token).await {
                Ok(t) => {
                    typing_tickets.lock().unwrap().insert(to_user_id.to_string(), t.clone());
                    t
                }
                Err(e) => {
                    log::warn!("[wechat] get_typing_ticket: {}", e);
                    return;
                }
            }
        }
    };

    let running_c = running.clone();
    let pu_c = processing_users.clone();
    let wechat_c = wechat.clone();
    let uid_c = to_user_id.to_string();

    // 在 current_thread 运行时中 spawn 并发任务
    tokio::spawn(async move {
        let mut elapsed = 0u64;
        let max_duration = 30u64; // 最长 30 秒

        while running_c.load(Ordering::SeqCst) && elapsed < max_duration {
            if !pu_c.lock().unwrap().contains(&uid_c) {
                break; // 回复已发送，停止
            }
            if let Err(e) = wechat_c.send_typing(&uid_c, &ticket, 1).await {
                log::warn!("[wechat] send_typing: {}", e);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            elapsed += 4;
        }

        // 发送输入状态停止
        if running_c.load(Ordering::SeqCst) {
            let _ = wechat_c.send_typing(&uid_c, &ticket, 2).await;
        }

        pu_c.lock().unwrap().remove(&uid_c);
    });
}

// ── Wait helper ──────────────────────────────────────────────────────────────

async fn wait_loop(running: &Arc<AtomicBool>, status: &Arc<Mutex<BnStatus>>) {
    while running.load(Ordering::SeqCst) {
        match status.lock().unwrap().clone() {
            BnStatus::Online { .. } => {}
            _ => return,
        }
        sleep_ms(2000).await;
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(WechatClawPlugin::new())
}

#[unsafe(no_mangle)]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
