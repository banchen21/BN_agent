//! # Proactive Plugin v6 — 自动覆盖所有活跃对话
//!
//! 无需手动配置 chat_id / source。
//! 通过共享 ChatHistory 自动追踪所有活跃对话，
//! 对每个对话独立判断是否需要主动追问。
//!
//! 可选配置：
//!   PROACTIVE_MODE=full|semi        (默认 full)
//!   PROACTIVE_TIME_WINDOWS=[...]   (semi 模式下的时间窗口)

use chrono::Timelike;
use plugin_interface::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════════
//  Shared ChatHistory (no actix actor — safe to use from plugin start())
// ═══════════════════════════════════════════════════════════════════════════════

struct ChatHistory {
    history: HashMap<i64, Vec<(String, String, Instant)>>,
    sources: HashMap<i64, String>,
    max_per_chat: usize,
}

impl ChatHistory {
    fn new(max_per_chat: usize) -> Self {
        Self { history: HashMap::new(), sources: HashMap::new(), max_per_chat }
    }

    fn record(&mut self, chat_id: i64, role: &str, text: &str, source: &str) {
        let msgs = self.history.entry(chat_id).or_default();
        msgs.push((role.to_string(), text.to_string(), Instant::now()));
        while msgs.len() > self.max_per_chat {
            msgs.remove(0);
        }
        if !source.is_empty() {
            self.sources.insert(chat_id, source.to_string());
        }
    }

    fn get(&self, chat_id: i64) -> (Vec<(String, String, Instant)>, String) {
        let hist = self.history.get(&chat_id).cloned().unwrap_or_default();
        let source = self.sources.get(&chat_id).cloned().unwrap_or_default();
        (hist, source)
    }

    fn active_chats(&self, min_msgs: usize) -> Vec<i64> {
        self.history.iter()
            .filter(|(_, msgs)| msgs.len() >= min_msgs)
            .map(|(id, _)| *id)
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Configuration
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, Clone)]
struct TimeWindow {
    start: String,
    end: String,
    #[serde(default)]
    days: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
enum ProactiveMode {
    Full,
    Semi,
}

#[derive(Clone)]
struct Config {
    mode: ProactiveMode,
    time_windows: Vec<TimeWindow>,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  LLM Decision types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct ProactiveDecision {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    message: String,
    #[serde(default = "default_wait")]
    wait_seconds: u64,
    #[serde(default)]
    continue_: bool,
}

fn default_wait() -> u64 { 120 }

// ═══════════════════════════════════════════════════════════════════════════════
//  Scheduled action
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
struct ScheduledAction {
    message: String,
    send_at: Instant,
    decided_at: Instant,
    should_continue: bool,
    source: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ProactivePlugin
// ═══════════════════════════════════════════════════════════════════════════════

pub struct ProactivePlugin {
    info: PluginInfo,
    stop_flag: Arc<AtomicBool>,
    chat_history: Arc<Mutex<ChatHistory>>,
    scheduled: Arc<Mutex<HashMap<i64, ScheduledAction>>>,
    last_llm_call: Arc<Mutex<HashMap<i64, Instant>>>,
    /// LLM recipient cached for lazy spawn on first event.
    llm_recipient: Option<actix::Recipient<LlmRequest>>,
    /// EventBus cached for lazy spawn.
    event_bus: Option<Addr<plugin_interface::EventBus>>,
    /// Plugin name cached for lazy spawn.
    plugin_name: Option<String>,
    /// Flag: main loop already spawned?
    loop_spawned: Arc<AtomicBool>,
}

impl ProactivePlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "proactive-plugin".into(),
                version: "0.6.0".into(),
                description: "自动覆盖所有活跃对话的主动追问".into(),
                author: "bn-agent".into(),
                min_host_version: "0.1.0".into(),
            },
            stop_flag: Arc::new(AtomicBool::new(false)),
            chat_history: Arc::new(Mutex::new(ChatHistory::new(100))),
            scheduled: Arc::new(Mutex::new(HashMap::new())),
            last_llm_call: Arc::new(Mutex::new(HashMap::new())),
            llm_recipient: None,
            event_bus: None,
            plugin_name: None,
            loop_spawned: Arc::new(AtomicBool::new(false)),
        }
    }

    fn load_config() -> Config {
        let mode = match std::env::var("PROACTIVE_MODE").as_deref() {
            Ok("semi") => ProactiveMode::Semi,
            _ => ProactiveMode::Full,
        };
        let time_windows = std::env::var("PROACTIVE_TIME_WINDOWS")
            .ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        Config { mode, time_windows }
    }

    fn in_time_window(windows: &[TimeWindow]) -> bool {
        if windows.is_empty() { return true; }
        let now = chrono::Local::now();
        let weekday = now.format("%u").to_string().parse::<u8>().unwrap_or(0);
        let secs = now.hour() as u64 * 3600 + now.minute() as u64 * 60;
        for w in windows {
            if !w.days.is_empty() && !w.days.contains(&weekday) { continue; }
            let Ok(start) = parse_time(&w.start) else { continue };
            let Ok(end) = parse_time(&w.end) else { continue };
            if secs >= start && secs < end { return true; }
        }
        false
    }

    fn extract_json(text: &str) -> Option<String> {
        let text = text.trim();
        if let Some(start) = text.find("```json") {
            let from = start + 7;
            if let Some(end) = text[from..].find("```") {
                return Some(text[from..from + end].trim().to_string());
            }
        }
        if let Some(start) = text.find('{') {
            if let Some(end) = text[start..].rfind('}') {
                return Some(text[start..start + end + 1].to_string());
            }
        }
        None
    }

    fn build_prompt(history: &[(String, String, Instant)], now: Instant, source: &str) -> String {
        if history.is_empty() {
            return "对话尚未开始。".to_string();
        }
        let mut lines = Vec::new();
        for (role, text, _ts) in history.iter().rev().take(40) {
            lines.push(format!("{}: {}", role, text));
        }
        lines.reverse();
        let last_msg_ago = if let Some((_, _, ts)) = history.last() {
            let secs = now.duration_since(*ts).as_secs();
            if secs < 60 { format!("{} 秒", secs) }
            else { format!("{} 分钟", secs / 60) }
        } else {
            "未知".to_string()
        };
        let last_role = history.last().map(|(r, _, _)| r.as_str()).unwrap_or("unknown");
        format!(
            "【对话记录 — 来源: {}】\n{}\n\n\
             【状态】\n最后一条来自: {}\n沉默: {} 前\n当前: {}\n\n\
             【任务】输出 JSON（不要 markdown 代码块）：\n\
             {{\"paused\":true/false,\"message\":\"...\",\"wait_seconds\":120,\"continue_\":true/false}}",
            source, lines.join("\n"),
            last_role, last_msg_ago,
            chrono::Local::now().format("%H:%M:%S"),
        )
    }

    async fn call_llm(
        llm: &actix::Recipient<LlmRequest>,
        prompt: &str,
    ) -> Option<ProactiveDecision> {
        let resp = llm.send(LlmRequest {
            messages: vec![
                ChatMessage::system(
                    "你是对话节奏控制器。分析对话，判断：\n\
                     1) 对话是否已暂停（paused）\n\
                     2) 如果暂停了应主动追问什么（message）\n\
                     3) 等多久再追问（wait_seconds，30~600 秒）\n\
                     4) 追问后是否继续主动循环（continue_）"
                ),
                ChatMessage::user(prompt),
            ],
            model: None,
            temperature: Some(0.3),
            max_tokens: Some(400),
        }).await;

        let text = match resp {
            Ok(Ok(r)) => r.content,
            Ok(Err(e)) => { eprintln!("[proactive] LLM error: {}", e); return None; }
            Err(e) => { eprintln!("[proactive] LLM mailbox: {:?}", e); return None; }
        };

        let json_str = Self::extract_json(&text)?;
        match serde_json::from_str::<ProactiveDecision>(&json_str) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("[proactive] parse JSON fail: {} — raw: {}", e, text);
                None
            }
        }
    }

    /// Lazy-spawn the main proactive loop on first event.
    fn maybe_spawn_loop(&self) {
        if self.loop_spawned.swap(true, Ordering::Relaxed) {
            return;
        }

        let llm = match &self.llm_recipient {
            Some(l) => l.clone(),
            None => return,
        };
        let eb = match &self.event_bus {
            Some(b) => b.clone(),
            None => return,
        };
        let plugin_name = match &self.plugin_name {
            Some(n) => n.clone(),
            None => return,
        };

        let config = Arc::new(Self::load_config());
        let chat_history = self.chat_history.clone();
        let stop_flag = self.stop_flag.clone();
        let scheduled = self.scheduled.clone();
        let last_llm_call = self.last_llm_call.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("proactive tokio runtime");
            rt.block_on(async {
                eprintln!("[proactive] v6 loop started — auto all-chat mode");
                let _ = main_loop(config, chat_history, stop_flag, scheduled, last_llm_call, llm, eb, plugin_name).await;
                eprintln!("[proactive] loop stopped");
            });
        });
    }
}

// ── Main proactive loop (spawned lazily on first event) ─────────────────

async fn main_loop(
    config: Arc<Config>,
    chat_history: Arc<Mutex<ChatHistory>>,
    stop_flag: Arc<AtomicBool>,
    scheduled: Arc<Mutex<HashMap<i64, ScheduledAction>>>,
    last_llm_call: Arc<Mutex<HashMap<i64, Instant>>>,
    llm: actix::Recipient<LlmRequest>,
    eb: Addr<plugin_interface::EventBus>,
    plugin_name: String,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;

        if stop_flag.load(Ordering::Relaxed) { break; }

        if matches!(config.mode, ProactiveMode::Semi)
            && !ProactivePlugin::in_time_window(&config.time_windows)
        {
            continue;
        }

        let now = Instant::now();
        let active_chats = chat_history.lock().unwrap().active_chats(2);

        for chat_id in active_chats {
            if stop_flag.load(Ordering::Relaxed) { break; }

            let (hist, source) = chat_history.lock().unwrap().get(chat_id);
            if hist.len() < 2 { continue; }
            let source = if source.is_empty() { "unknown".to_string() } else { source };

            let action = scheduled.lock().unwrap().get(&chat_id).cloned();

            if let Some(action) = action {
                let user_replied = hist.iter()
                    .filter(|(r, _, _)| r == "user")
                    .any(|(_, _, ts)| *ts > action.decided_at);
                if user_replied {
                    scheduled.lock().unwrap().remove(&chat_id);
                    continue;
                }
                if now < action.send_at { continue; }

                eprintln!("[proactive] chat={} sending: {}", chat_id, action.message);
                eb.do_send(Event::new("assistant.message", serde_json::json!({
                    "chat_id": chat_id,
                    "text": &action.message,
                    "source": &action.source,
                }), &plugin_name));

                chat_history.lock().unwrap().record(chat_id, "assistant", &action.message, &action.source);
                scheduled.lock().unwrap().remove(&chat_id);

                if action.should_continue {
                    let prompt = ProactivePlugin::build_prompt(&hist, now, &action.source);
                    let decision = ProactivePlugin::call_llm(&llm, &prompt).await;
                    if let Some(d) = decision {
                        if d.paused && !d.message.is_empty() {
                            scheduled.lock().unwrap().insert(chat_id, ScheduledAction {
                                message: d.message,
                                send_at: Instant::now() + Duration::from_secs(d.wait_seconds),
                                decided_at: Instant::now(),
                                should_continue: d.continue_,
                                source: action.source,
                            });
                        }
                    }
                }
                continue;
            }

            let too_soon = last_llm_call.lock().unwrap().get(&chat_id)
                .map(|last| now.duration_since(*last).as_secs() < 30)
                .unwrap_or(false);
            if too_soon { continue; }

            let prompt = ProactivePlugin::build_prompt(&hist, now, &source);
            last_llm_call.lock().unwrap().insert(chat_id, now);

            let decision = ProactivePlugin::call_llm(&llm, &prompt).await;

            if let Some(d) = decision {
                let msg_preview = if d.message.len() > 30 { &d.message[..30] } else { &d.message };
                eprintln!("[proactive] chat={} paused={} wait={}s msg={} cont={}",
                    chat_id, d.paused, d.wait_seconds, msg_preview, d.continue_);

                if d.paused && !d.message.is_empty() {
                    scheduled.lock().unwrap().insert(chat_id, ScheduledAction {
                        message: d.message,
                        send_at: Instant::now() + Duration::from_secs(d.wait_seconds),
                        decided_at: Instant::now(),
                        should_continue: d.continue_,
                        source: source.clone(),
                    });
                }
            }
        }
    }
}

// ── Plugin trait ─────────────────────────────────────────────────────────────

impl Plugin for ProactivePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let config = Self::load_config();
        eprintln!("[proactive] mode={:?}", config.mode);

        // Cache LLM / EventBus / plugin_name for lazy spawn on first event.
        // We can't spawn here because the tokio runtime isn't running yet
        // (PluginManager auto_scan runs before system.block_on).
        self.llm_recipient = ctx.llm.clone();
        self.event_bus = Some(ctx.event_bus.clone());
        self.plugin_name = Some(ctx.plugin_name.clone());
        self.loop_spawned.store(false, Ordering::Relaxed);

        eprintln!("[proactive] v6 started — lazy spawn on first event");
        Ok(())
    }

    fn on_event(&self, event: &Event) -> bool {
        // Lazy-spawn the main loop on first event (tokio runtime is now up).
        self.maybe_spawn_loop();

        let chat_id = match event.data.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return true,
        };
        let role = match event.topic.as_str() {
            "user.message" => "user",
            "assistant.message" => "assistant",
            _ => return true,
        };
        let source = event.data.get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(text) = event.data.get("text").and_then(|v| v.as_str()) {
            self.chat_history.lock().unwrap().record(chat_id, role, text, source);
        }
        true
    }

    fn stop(&mut self) {
        eprintln!("[proactive] stopping");
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn parse_time(s: &str) -> Result<u64, ()> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 { return Err(()); }
    let h = parts[0].parse::<u64>().map_err(|_| ())?;
    let m = parts[1].parse::<u64>().map_err(|_| ())?;
    Ok(h * 3600 + m * 60)
}

// ── FFI Exports ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ProactivePlugin::new())
}

#[no_mangle]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
