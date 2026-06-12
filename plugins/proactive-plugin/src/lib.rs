//! # Proactive Plugin v3 — 纯 LLM 决策
//!
//! 没有任何硬编码秒数/轮数。插件只做两件事：
//!
//! 1. 收集对话历史（ChatHistory actor）→ 发给 LLM
//! 2. LLM 决定：对话是否暂停？多久后追问？追问什么？是否继续？

use actix::prelude::*;
use chrono::Timelike;
use plugin_interface::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════════
//  ChatHistory Actor
// ═══════════════════════════════════════════════════════════════════════════════

struct ChatHistoryActor {
    history: HashMap<i64, Vec<(String, String, Instant)>>,
    max_per_chat: usize,
}

impl ChatHistoryActor {
    fn new(max_per_chat: usize) -> Self {
        Self { history: HashMap::new(), max_per_chat }
    }
}

impl Actor for ChatHistoryActor {
    type Context = Context<Self>;
}

// ── Messages ──

struct RecordMessage {
    chat_id: i64,
    role: String,
    text: String,
}

impl Message for RecordMessage {
    type Result = ();
}

impl Handler<RecordMessage> for ChatHistoryActor {
    type Result = ();

    fn handle(&mut self, msg: RecordMessage, _ctx: &mut Self::Context) {
        let msgs = self.history.entry(msg.chat_id).or_default();
        msgs.push((msg.role, msg.text, Instant::now()));
        while msgs.len() > self.max_per_chat {
            msgs.remove(0);
        }
    }
}

struct GetHistory {
    chat_id: i64,
}

impl Message for GetHistory {
    type Result = Vec<(String, String, Instant)>;
}

impl Handler<GetHistory> for ChatHistoryActor {
    type Result = Vec<(String, String, Instant)>;

    fn handle(&mut self, msg: GetHistory, _ctx: &mut Self::Context) -> Self::Result {
        self.history.get(&msg.chat_id).cloned().unwrap_or_default()
    }
}

struct ClearHistory {
    chat_id: i64,
}

impl Message for ClearHistory {
    type Result = ();
}

impl Handler<ClearHistory> for ChatHistoryActor {
    type Result = ();

    fn handle(&mut self, msg: ClearHistory, _ctx: &mut Self::Context) {
        self.history.remove(&msg.chat_id);
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
    chat_id: i64,
    source: String,
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
    #[serde(default)]
    reason: String,
}

fn default_wait() -> u64 { 120 }

// ═══════════════════════════════════════════════════════════════════════════════
//  Scheduled action (shared state for the async loop)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
struct ScheduledAction {
    message: String,
    send_at: Instant,
    decided_at: Instant,
    should_continue: bool,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ProactivePlugin
// ═══════════════════════════════════════════════════════════════════════════════

pub struct ProactivePlugin {
    info: PluginInfo,
    stop_flag: Arc<AtomicBool>,
    /// ChatHistory actor address — set during start().
    history_addr: Option<Addr<ChatHistoryActor>>,
    /// Scheduled actions (shared with async loop).
    scheduled: Arc<Mutex<HashMap<i64, ScheduledAction>>>,
    /// Last LLM call timestamps (shared with async loop).
    last_llm_call: Arc<Mutex<HashMap<i64, Instant>>>,
}

impl ProactivePlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "proactive-plugin".into(),
                version: "0.4.0".into(),
                description: "纯 LLM 决策 + ChatHistory actor".into(),
                author: "bn-agent".into(),
                min_host_version: "0.1.0".into(),
            },
            stop_flag: Arc::new(AtomicBool::new(false)),
            history_addr: None,
            scheduled: Arc::new(Mutex::new(HashMap::new())),
            last_llm_call: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn load_config() -> Option<Config> {
        let chat_id = std::env::var("PROACTIVE_CHAT_ID")
            .ok()?.parse::<i64>().ok().filter(|&id| id > 0)?;
        let mode = match std::env::var("PROACTIVE_MODE").as_deref() {
            Ok("semi") => ProactiveMode::Semi,
            _ => ProactiveMode::Full,
        };
        let time_windows = std::env::var("PROACTIVE_TIME_WINDOWS")
            .ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        Some(Config {
            chat_id,
            source: std::env::var("PROACTIVE_SOURCE").unwrap_or_else(|_| "telegram".into()),
            mode,
            time_windows,
        })
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

    /// Extract JSON from LLM text — handles ```json ... ``` wrapping.
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

    fn build_prompt(history: &[(String, String, Instant)], now: Instant) -> String {
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
            "【对话记录】\n{}\n\n\
             【状态】\n最后一条来自: {}\n沉默: {} 前\n当前: {}\n\n\
             【任务】输出 JSON（不要 markdown 代码块）：\n\
             {{\"paused\":true/false,\"message\":\"...\",\"wait_seconds\":120,\"continue_\":true/false,\"reason\":\"...\"}}",
            lines.join("\n"),
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
            Ok(Err(e)) => { log::warn!("[Proactive] LLM error: {}", e); return None; }
            Err(e) => { log::warn!("[Proactive] LLM mailbox: {:?}", e); return None; }
        };

        let json_str = Self::extract_json(&text)?;
        match serde_json::from_str::<ProactiveDecision>(&json_str) {
            Ok(d) => Some(d),
            Err(e) => {
                log::warn!("[Proactive] parse JSON fail: {} — raw: {}", e, text);
                None
            }
        }
    }
}

impl Plugin for ProactivePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let config = match Self::load_config() {
            Some(c) => {
                log::info!("[Proactive] config: chat_id={}, mode={:?}", c.chat_id, c.mode);
                c
            }
            None => {
                log::info!("[Proactive] no PROACTIVE_CHAT_ID, idle");
                return Ok(());
            }
        };

        let llm = match &ctx.llm {
            Some(l) => l.clone(),
            None => { log::warn!("[Proactive] no LLM, idle"); return Ok(()); }
        };

        // ── Start ChatHistory actor ──
        let history_addr = ChatHistoryActor::new(100).start();
        self.history_addr = Some(history_addr.clone());

        let config = Arc::new(config);
        let stop_flag = self.stop_flag.clone();
        let scheduled = self.scheduled.clone();
        let last_llm_call = self.last_llm_call.clone();
        let plugin_name = ctx.plugin_name.clone();
        let eb = ctx.event_bus.clone();

        // ── Main proactive loop ──
        actix::spawn(async move {
            log::info!("[Proactive] v4 loop started — ChatHistory actor + pure LLM");

            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;

                if stop_flag.load(Ordering::Relaxed) { break; }

                let chat_id = config.chat_id;
                let now = Instant::now();

                // ── Semi mode: time window ──
                if matches!(config.mode, ProactiveMode::Semi)
                    && !Self::in_time_window(&config.time_windows)
                {
                    scheduled.lock().unwrap().remove(&chat_id);
                    continue;
                }

                // ── Fetch history from actor ──
                let hist = history_addr.send(GetHistory { chat_id }).await
                    .unwrap_or_default();

                if hist.len() < 2 {
                    continue;
                }

                // ── Check if there's a scheduled action ──
                let action = scheduled.lock().unwrap().get(&chat_id).cloned();

                if let Some(action) = action {
                    // Check if user replied during wait
                    let user_replied = hist.iter()
                        .filter(|(r, _, _)| r == "user")
                        .any(|(_, _, ts)| *ts > action.decided_at);

                    if user_replied {
                        log::info!("[Proactive] user replied — cancelling scheduled action");
                        scheduled.lock().unwrap().remove(&chat_id);
                        continue;
                    }

                    if now < action.send_at {
                        continue; // Still waiting
                    }

                    // ── Send the message ──
                    log::info!("[Proactive] sending: {}", action.message);
                    eb.do_send(Event::new("assistant.message", serde_json::json!({
                        "chat_id": chat_id,
                        "text": &action.message,
                        "source": &config.source,
                    }), &plugin_name));

                    // Record the sent message in history
                    history_addr.do_send(RecordMessage {
                        chat_id,
                        role: "assistant".to_string(),
                        text: action.message.clone(),
                    });

                    scheduled.lock().unwrap().remove(&chat_id);

                    // ── Continue cycle? ──
                    if action.should_continue {
                        let prompt = Self::build_prompt(&hist, now);
                        log::info!("[Proactive] asking LLM for next decision");
                        let decision = ProactivePlugin::call_llm(&llm, &prompt).await;
                        if let Some(d) = decision {
                            log::info!("[Proactive] next decision: paused={}, wait={}s, msg={}, reason={}",
                                d.paused, d.wait_seconds, d.message, d.reason);
                            if d.paused && !d.message.is_empty() {
                                scheduled.lock().unwrap().insert(chat_id, ScheduledAction {
                                    message: d.message,
                                    send_at: Instant::now() + Duration::from_secs(d.wait_seconds),
                                    decided_at: Instant::now(),
                                    should_continue: d.continue_,
                                });
                            }
                        }
                    }
                    continue;
                }

                // ── No scheduled action — ask LLM if we should start one ──

                let too_soon = last_llm_call.lock().unwrap().get(&chat_id)
                    .map(|last| now.duration_since(*last).as_secs() < 15)
                    .unwrap_or(false);
                if too_soon { continue; }

                let prompt = Self::build_prompt(&hist, now);
                log::info!("[Proactive] calling LLM (history: {} msgs)", hist.len());

                last_llm_call.lock().unwrap().insert(chat_id, now);

                let decision = ProactivePlugin::call_llm(&llm, &prompt).await;

                if let Some(d) = decision {
                    log::info!("[Proactive] LLM: paused={}, wait={}s, msg={}, cont={}, reason={}",
                        d.paused, d.wait_seconds, d.message, d.continue_, d.reason);

                    if d.paused && !d.message.is_empty() {
                        let msg = d.message.clone();
                        let send_at = Instant::now() + Duration::from_secs(d.wait_seconds);
                        scheduled.lock().unwrap().insert(chat_id, ScheduledAction {
                            message: d.message,
                            send_at,
                            decided_at: Instant::now(),
                            should_continue: d.continue_,
                        });
                        log::info!("[Proactive] scheduled in {}s: {}", d.wait_seconds, msg);
                    }
                }
            }

            log::info!("[Proactive] loop stopped");
        });

        log::info!("[Proactive] v4 started — ChatHistory actor");
        Ok(())
    }

    fn on_event(&self, event: &Event) -> bool {
        let chat_id = match event.data.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return true,
        };

        let role = match event.topic.as_str() {
            "user.message" => "user",
            "assistant.message" => "assistant",
            _ => return true,
        };

        if let Some(text) = event.data.get("text").and_then(|v| v.as_str()) {
            if let Some(ref addr) = self.history_addr {
                addr.do_send(RecordMessage {
                    chat_id,
                    role: role.to_string(),
                    text: text.to_string(),
                });
            }
        }

        true
    }

    fn stop(&mut self) {
        log::info!("[Proactive] stopping");
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
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {
    // Plugin is dropped here; stop() was already called by PluginManager.
}
