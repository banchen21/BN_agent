//! wechat-claw-plugin — 微信 iLink Bot API 插件
//!
//! 腾讯官方 iLink Bot API，协议更简洁稳定。
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

mod protocol;

use protocol::*;
use plugin_interface::*;
use reqwest::Client;
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

// ── Session persistence ─────────────────────────────────────────────────────

fn session_path() -> std::path::PathBuf {
    std::path::Path::new("data").join("wechat_session.json")
}

fn save_session(session: &WechatSession) {
    let path = session_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(session) {
        let _ = std::fs::write(&path, &json);
        log::info!("[wechat] session saved to {:?}", path);
    }
}

fn load_session() -> Option<WechatSession> {
    let path = session_path();
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).ok(),
        Err(_) => None,
    }
}

fn clear_session() {
    let path = session_path();
    let _ = std::fs::remove_file(&path);
    log::info!("[wechat] session cleared");
}

// ── Tool: wechat_send_message ────────────────────────────────────────────────

struct SendWechatMessage {
    session: Arc<Mutex<Option<WechatSession>>>,
    context_tokens: Arc<Mutex<HashMap<String, String>>>,
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
        let session = match self.session.lock().unwrap().as_ref() {
            Some(s) => s.clone(),
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
        let ctx_token = self
            .context_tokens
            .lock()
            .unwrap()
            .get(&to_user)
            .cloned()
            .unwrap_or_default();

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(&format!("runtime: {}", e)),
        };
        let client = match build_client() {
            Ok(c) => c,
            Err(e) => return ToolResult::err(&format!("client: {}", e)),
        };

        match rt.block_on(send_message(
            &client,
            &session.token,
            &session.base_url,
            &to_user,
            &text,
            &ctx_token,
        )) {
            Ok(()) => ToolResult::ok(&format!("消息已发送")),
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
            BnStatus::WaitingQr { qr_path, .. } => {
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
    session: Arc<Mutex<Option<WechatSession>>>,
    context_tokens: Arc<Mutex<HashMap<String, String>>>,
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
            session: Arc::new(Mutex::new(None)),
            context_tokens: Arc::new(Mutex::new(HashMap::new())),
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
            version: "0.1.0".into(),
            description: "WeChat IM plugin — iLink Bot API".into(),
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
                session: self.session.clone(),
                context_tokens: self.context_tokens.clone(),
                last_chat_id: self.last_chat_id.clone(),
                processing_users: self.processing_users.clone(),
            }));
            reg.register(Arc::new(GetQrCode {
                status: self.status.clone(),
            }));
            log::info!("[wechat] registered 2 tools");
        }

        // 2. 克隆共享状态给后台线程
        let running = self.running.clone();
        let session = self.session.clone();
        let context_tokens = self.context_tokens.clone();
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
                running, session, context_tokens, last_chat_id,
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
            BnStatus::WaitingQr { qr_path, .. } => {
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

            // 停止输入状态（typing 循环检测到用户移出集合后会发 stop）
            self.processing_users.lock().unwrap().remove(&chat_id);

            let session = match self.session.lock().unwrap().clone() {
                Some(s) => s,
                None => {
                    log::warn!("[wechat] no session for reply");
                    return true;
                }
            };

            let ctx_token = self
                .context_tokens
                .lock()
                .unwrap()
                .get(&chat_id)
                .cloned()
                .unwrap_or_default();

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
            let client = match build_client() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("[wechat] client: {}", e);
                    return true;
                }
            };

            rt.block_on(async {
                if let Err(e) = send_message(
                    &client, &session.token, &session.base_url,
                    &chat_id, &text, &ctx_token,
                )
                .await
                {
                    log::error!("[wechat] reply failed: {}", e);
                    if e.contains("-14") {
                        clear_session();
                    }
                }
            });

            return true;
        }

        // user.message from wechat → 开启 typing 循环
        if event.topic == "user.message" {
            let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if source != "wechat" {
                return true;
            }
            // typing 已在 poll_loop 中启动，这里不需要重复处理
            return true;
        }

        true
    }
}

// ── Main loop (login → poll → reconnect) ─────────────────────────────────────

async fn main_loop(
    running: Arc<AtomicBool>,
    session: Arc<Mutex<Option<WechatSession>>>,
    context_tokens: Arc<Mutex<HashMap<String, String>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
    processing_users: Arc<Mutex<HashSet<String>>>,
    typing_tickets: Arc<Mutex<HashMap<String, String>>>,
    status: Arc<Mutex<BnStatus>>,
    poll_thread_holder: Arc<Mutex<Option<JoinHandle<()>>>>,
    event_bus: Addr<EventBus>,
) {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            log::error!("[wechat] client: {}", e);
            *status.lock().unwrap() = BnStatus::Error(format!("http client: {}", e));
            return;
        }
    };

    while running.load(Ordering::SeqCst) {
        // 1. 尝试加载已保存的 session
        if let Some(saved) = load_session() {
            log::info!("[wechat] loaded saved session: {}", saved.account_id);
            *session.lock().unwrap() = Some(saved.clone());
            *status.lock().unwrap() = BnStatus::Online { nick: saved.account_id.clone() };

            start_poll_thread(
                &poll_thread_holder, &running, &session, &context_tokens,
                &last_chat_id, &processing_users, &typing_tickets,
                &status, &event_bus,
            );

            wait_loop(&running, &status).await;
            if !running.load(Ordering::SeqCst) { return; }
            log::warn!("[wechat] session expired, re-login required");
            clear_session();
            *session.lock().unwrap() = None;
            stop_poll_thread(&poll_thread_holder);
        }

        if !running.load(Ordering::SeqCst) { return; }

        // 2. 扫码登录
        log::info!("[wechat] starting QR code login...");
        if let Some(new_session) = qr_login_flow(&client, &status, &running).await {
            save_session(&new_session);
            *session.lock().unwrap() = Some(new_session.clone());
            *status.lock().unwrap() = BnStatus::Online { nick: new_session.account_id.clone() };

            start_poll_thread(
                &poll_thread_holder, &running, &session, &context_tokens,
                &last_chat_id, &processing_users, &typing_tickets,
                &status, &event_bus,
            );

            wait_loop(&running, &status).await;
            if !running.load(Ordering::SeqCst) { return; }
            log::warn!("[wechat] session expired, re-login required");
            clear_session();
            *session.lock().unwrap() = None;
            stop_poll_thread(&poll_thread_holder);
        }
    }
}

// ── QR 登录流程 ──────────────────────────────────────────────────────────────

async fn qr_login_flow(
    client: &Client,
    status: &Arc<Mutex<BnStatus>>,
    running: &Arc<AtomicBool>,
) -> Option<WechatSession> {
    loop {
        if !running.load(Ordering::SeqCst) { return None; }

        let qr = match fetch_qrcode(client).await {
            Ok(q) => q,
            Err(e) => {
                log::error!("[wechat] fetch_qrcode: {}", e);
                *status.lock().unwrap() = BnStatus::Error(format!("获取二维码失败: {}", e));
                sleep_ms(5000).await;
                continue;
            }
        };

        let qr_path = match save_qr_png(&qr.img_content) {
            Some(p) => p,
            None => { sleep_ms(3000).await; continue; }
        };

        *status.lock().unwrap() = BnStatus::WaitingQr {
            qr_path: qr_path.clone(),
        };

        let deadline = now_ms() + 5 * 60_000;
        let current_qrcode = qr.qrcode;
        let mut refresh_count = 0u32;

        while now_ms() < deadline && running.load(Ordering::SeqCst) {
            match poll_qrcode(client, &current_qrcode).await {
                Ok(QrStatus::Wait) => {}
                Ok(QrStatus::Scanned) => {
                    log::info!("[wechat] 二维码已被扫描！请在手机上确认登录。");
                    *status.lock().unwrap() = BnStatus::Scanned;
                }
                Ok(QrStatus::Confirmed { bot_token, base_url, account_id, user_id }) => {
                    log::info!("[wechat] ✅ 登录成功！Bot: {}", account_id);
                    *status.lock().unwrap() = BnStatus::LoggingIn;
                    return Some(WechatSession { token: bot_token, base_url, account_id, user_id });
                }
                Ok(QrStatus::Expired) => {
                    refresh_count += 1;
                    if refresh_count > 3 {
                        log::error!("[wechat] 二维码多次过期，停止登录");
                        *status.lock().unwrap() = BnStatus::Error("二维码多次过期".into());
                        return None;
                    }
                    log::warn!("[wechat] 二维码过期，刷新 ({}/3)", refresh_count);
                    break;
                }
                Err(e) => log::error!("[wechat] poll_qrcode: {}", e),
            }
            sleep_ms(1200).await;
        }
    }
}

// ── QR 码保存 ────────────────────────────────────────────────────────────────

fn save_qr_png(img_content: &str) -> Option<String> {
    let png = match gen_qrcode(img_content) {
        Ok(p) => p,
        Err(e) => { log::error!("[wechat] gen_qrcode: {}", e); return None; }
    };
    let qr_path = std::path::Path::new("data").join("wechat_qrcode.png");
    if let Some(parent) = qr_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&qr_path, &png) {
        Ok(()) => {
            let abs_path = std::fs::canonicalize(&qr_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| qr_path.to_string_lossy().to_string());
            log::info!("[wechat] 二维码已生成: {}", abs_path);
            Some(abs_path)
        }
        Err(e) => { log::error!("[wechat] save qrcode: {}", e); None }
    }
}

// ── Poll thread management ───────────────────────────────────────────────────

fn start_poll_thread(
    holder: &Arc<Mutex<Option<JoinHandle<()>>>>,
    running: &Arc<AtomicBool>,
    session: &Arc<Mutex<Option<WechatSession>>>,
    context_tokens: &Arc<Mutex<HashMap<String, String>>>,
    last_chat_id: &Arc<Mutex<Option<String>>>,
    processing_users: &Arc<Mutex<HashSet<String>>>,
    typing_tickets: &Arc<Mutex<HashMap<String, String>>>,
    status: &Arc<Mutex<BnStatus>>,
    event_bus: &Addr<EventBus>,
) {
    stop_poll_thread(holder);

    let running_c = running.clone();
    let session_c = session.clone();
    let ctx_c = context_tokens.clone();
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
            running_c, session_c, ctx_c, last_c,
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
    session: Arc<Mutex<Option<WechatSession>>>,
    context_tokens: Arc<Mutex<HashMap<String, String>>>,
    last_chat_id: Arc<Mutex<Option<String>>>,
    processing_users: Arc<Mutex<HashSet<String>>>,
    typing_tickets: Arc<Mutex<HashMap<String, String>>>,
    status: Arc<Mutex<BnStatus>>,
    event_bus: Addr<EventBus>,
) {
    let (token, base_url) = {
        let s = session.lock().unwrap();
        match s.as_ref() {
            Some(s) => (s.token.clone(), s.base_url.clone()),
            None => { log::warn!("[wechat] poll: no session"); return; }
        }
    };

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => { log::error!("[wechat] poll: client: {}", e); return; }
    };

    let mut buf = String::new();
    let mut consecutive_errors = 0u32;

    while running.load(Ordering::SeqCst) {
        match get_updates(&client, &token, &base_url, &buf).await {
            Ok(resp) => {
                consecutive_errors = 0;
                buf = resp.next_buf;

                for msg in &resp.messages {
                    // 保存 context_token
                    context_tokens.lock().unwrap().insert(
                        msg.from_user_id.clone(),
                        msg.context_token.clone(),
                    );
                    *last_chat_id.lock().unwrap() = Some(msg.from_user_id.clone());

                    log::info!(
                        "[wechat] msg from {}: {}",
                        msg.from_user_id,
                        msg.text.chars().take(60).collect::<String>(),
                    );

                    // 发布 user.message 事件
                    event_bus.do_send(Event::new(
                        "user.message",
                        serde_json::json!({
                            "text": msg.text,
                            "source": "wechat",
                            "user_name": msg.from_user_id,
                            "chat_id": msg.from_user_id,
                        }),
                        "wechat-claw-plugin",
                    ));

                    // ── 启动输入状态指示器 ──
                    // 查找该用户的 context_token
                    let ctx_token = context_tokens
                        .lock().unwrap()
                        .get(&msg.from_user_id)
                        .cloned()
                        .unwrap_or_default();
                    start_typing(
                        &client, &token, &base_url, &msg.from_user_id, &ctx_token,
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

// ── Typing indicator ─────────────────────────────────────────────────────────

/// 为指定用户启动输入状态指示器。
/// 获取 typing_ticket → 每 4 秒发一次 typing:start → 用户移出集合时发 typing:stop。
async fn start_typing(
    client: &Client,
    token: &str,
    base_url: &str,
    to_user_id: &str,
    context_token: &str,
    processing_users: &Arc<Mutex<HashSet<String>>>,
    typing_tickets: &Arc<Mutex<HashMap<String, String>>>,
    running: &Arc<AtomicBool>,
) {
    // 标记该用户在 processing 中
    processing_users.lock().unwrap().insert(to_user_id.to_string());

    // 获取/复用 typing_ticket
    let ticket = {
        let cached = typing_tickets.lock().unwrap().get(to_user_id).cloned();
        if let Some(t) = cached {
            t
        } else {
            match get_typing_ticket(client, token, base_url, to_user_id, context_token).await {
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
    let client_c = client.clone();
    let token_c = token.to_string();
    let base_url_c = base_url.to_string();
    let uid_c = to_user_id.to_string();

    // 在 current_thread 运行时中 spawn 并发任务
    tokio::spawn(async move {
        let mut elapsed = 0u64;
        let max_duration = 30u64; // 最长 30 秒

        while running_c.load(Ordering::SeqCst) && elapsed < max_duration {
            if !pu_c.lock().unwrap().contains(&uid_c) {
                break; // 回复已发送，停止
            }
            if let Err(e) = send_typing(
                &client_c, &token_c, &base_url_c,
                &uid_c, &ticket, 1, // status: 1 = 开始输入
            ).await {
                log::warn!("[wechat] send_typing: {}", e);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            elapsed += 4;
        }

        // 发送输入状态停止
        let _ = send_typing(
            &client_c, &token_c, &base_url_c,
            &uid_c, &ticket, 2, // status: 2 = 停止输入
        ).await;

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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(WechatClawPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
