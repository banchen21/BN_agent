//! # Proactive Plugin v7 — 单会话主动追问
//!
//! 针对单会话场景，自动判断是否需要主动追问。
//!
//! 可选配置：
//!   PROACTIVE_MODE=full|semi        (默认 full)
//!   PROACTIVE_TIME_WINDOWS=[...]   (semi 模式下的时间窗口)

use chrono::Timelike;
use plugin_interface::*;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
    /// 对话历史: Vec<(role, text, timestamp)>
    history: Arc<Mutex<Vec<(String, String, Instant)>>>,
    /// 消息来源
    source: Arc<Mutex<String>>,
    /// 待执行的追问
    scheduled: Arc<Mutex<Option<ScheduledAction>>>,
    /// 上次 LLM 调用时间
    last_llm_call: Arc<Mutex<Option<Instant>>>,
    llm_recipient: Option<actix::Recipient<LlmRequest>>,
    event_bus: Option<Addr<plugin_interface::EventBus>>,
    plugin_name: Option<String>,
    loop_spawned: Arc<AtomicBool>,
    max_history: usize,
}

impl ProactivePlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "proactive-plugin".into(),
                version: "0.7.0".into(),
                description: "单会话自动主动追问".into(),
                author: "bn-agent".into(),
                min_host_version: "0.1.0".into(),
            },
            stop_flag: Arc::new(AtomicBool::new(false)),
            history: Arc::new(Mutex::new(Vec::new())),
            source: Arc::new(Mutex::new(String::new())),
            scheduled: Arc::new(Mutex::new(None)),
            last_llm_call: Arc::new(Mutex::new(None)),
            llm_recipient: None,
            event_bus: None,
            plugin_name: None,
            loop_spawned: Arc::new(AtomicBool::new(false)),
            max_history: 100,
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
        let history = self.history.clone();
        let source = self.source.clone();
        let stop_flag = self.stop_flag.clone();
        let scheduled = self.scheduled.clone();
        let last_llm_call = self.last_llm_call.clone();
        let max_history = self.max_history;

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("proactive tokio runtime");
            rt.block_on(async {
                eprintln!("[proactive] v7 loop started — single-session mode");
                let _ = main_loop(config, history, source, stop_flag, scheduled, last_llm_call, llm, eb, plugin_name, max_history).await;
                eprintln!("[proactive] loop stopped");
            });
        });
    }
}

// ── Main proactive loop ──────────────────────────────────────────────────────

async fn main_loop(
    config: Arc<Config>,
    history: Arc<Mutex<Vec<(String, String, Instant)>>>,
    source: Arc<Mutex<String>>,
    stop_flag: Arc<AtomicBool>,
    scheduled: Arc<Mutex<Option<ScheduledAction>>>,
    last_llm_call: Arc<Mutex<Option<Instant>>>,
    llm: actix::Recipient<LlmRequest>,
    eb: Addr<plugin_interface::EventBus>,
    plugin_name: String,
    max_history: usize,
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
        let hist = history.lock().unwrap().clone();
        if hist.len() < 2 { continue; }
        let cur_source = source.lock().unwrap().clone();
        let cur_source = if cur_source.is_empty() { "unknown".to_string() } else { cur_source };

        let action = scheduled.lock().unwrap().clone();

        if let Some(action) = action {
            let user_replied = hist.iter()
                .filter(|(r, _, _)| r == "user")
                .any(|(_, _, ts)| *ts > action.decided_at);
            if user_replied {
                *scheduled.lock().unwrap() = None;
                continue;
            }
            if now < action.send_at { continue; }

            eprintln!("[proactive] sending: {}", action.message);
            eb.do_send(Event::new("assistant.message", serde_json::json!({
                "text": &action.message,
                "source": &action.source,
            }), &plugin_name));

            // Record in history
            {
                let mut h = history.lock().unwrap();
                h.push(("assistant".to_string(), action.message.clone(), Instant::now()));
                while h.len() > max_history { h.remove(0); }
            }
            *scheduled.lock().unwrap() = None;

            if action.should_continue {
                let prompt = ProactivePlugin::build_prompt(&hist, now, &action.source);
                let decision = ProactivePlugin::call_llm(&llm, &prompt).await;
                if let Some(d) = decision {
                    if d.paused && !d.message.is_empty() {
                        *scheduled.lock().unwrap() = Some(ScheduledAction {
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

        let too_soon = last_llm_call.lock().unwrap()
            .map(|last| now.duration_since(last).as_secs() < 30)
            .unwrap_or(false);
        if too_soon { continue; }

        let prompt = ProactivePlugin::build_prompt(&hist, now, &cur_source);
        *last_llm_call.lock().unwrap() = Some(now);

        let decision = ProactivePlugin::call_llm(&llm, &prompt).await;

        if let Some(d) = decision {
            let msg_preview = if d.message.len() > 30 { &d.message[..30] } else { &d.message };
            eprintln!("[proactive] paused={} wait={}s msg={} cont={}",
                d.paused, d.wait_seconds, msg_preview, d.continue_);

            if d.paused && !d.message.is_empty() {
                *scheduled.lock().unwrap() = Some(ScheduledAction {
                    message: d.message,
                    send_at: Instant::now() + Duration::from_secs(d.wait_seconds),
                    decided_at: Instant::now(),
                    should_continue: d.continue_,
                    source: cur_source.clone(),
                });
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

        self.llm_recipient = ctx.llm.clone();
        self.event_bus = Some(ctx.event_bus.clone());
        self.plugin_name = Some(ctx.plugin_name.clone());
        self.loop_spawned.store(false, Ordering::Relaxed);

        eprintln!("[proactive] v7 started — lazy spawn on first event");
        Ok(())
    }

    fn on_event(&self, event: &Event) -> bool {
        self.maybe_spawn_loop();

        let role = match event.topic.as_str() {
            "user.message" => "user",
            "assistant.message" => "assistant",
            _ => return true,
        };
        let source = event.data.get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(text) = event.data.get("text").and_then(|v| v.as_str()) {
            let mut h = self.history.lock().unwrap();
            h.push((role.to_string(), text.to_string(), Instant::now()));
            while h.len() > self.max_history { h.remove(0); }
            if !source.is_empty() {
                *self.source.lock().unwrap() = source.to_string();
            }
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
