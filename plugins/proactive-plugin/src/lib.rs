//! proactive-plugin — LLM-driven proactive messaging plugin.
//!
//! Reads `PROACTIVE_MODE` from env:
//!   - "auto"      → LLM 完全自主决策并自动发送追问消息
//!   - "semi-auto" → 某个时间段到某个时间段内由 LLM 决定是否发送追问消息，其他时间段不发送
//!
//! `PROACTIVE_CHAT_ID` and `PROACTIVE_SOURCE` are optional:
//!   - If set via env → used as override
//!   - If empty → auto-detected from the first incoming user.message event
//!     (chat_id and source are extracted from the event payload)
//!
//! Architecture:
//!   - `on_event()` collects user/assistant messages into a shared history buffer.
//!   - A background thread loops every 15s, sends history to LLM for decision,
//!     and schedules/sends proactive follow-up messages.
//!   - Uses `Arc<Mutex<>>` for thread-safe state sharing (DLLs can't use actix actors).

use plugin_interface::*;
use chrono::Timelike;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ── Constants ────────────────────────────────────────────────────────────────

/// Interval between background loop checks.
const LOOP_INTERVAL_SECS: u64 = 15;
/// Minimum interval between two LLM decision calls.
const DECISION_COOLDOWN_SECS: u64 = 15;
/// Maximum number of history entries to keep per chat.
const MAX_HISTORY: usize = 100;

// ── Proactive mode ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProactiveMode {
    Auto,
    SemiAuto,
}

impl ProactiveMode {
    fn from_env() -> Self {
        match std::env::var("PROACTIVE_MODE").unwrap_or_default().to_lowercase().as_str() {
            "semi-auto" | "semi_auto" | "semiauto" => ProactiveMode::SemiAuto,
            _ => ProactiveMode::Auto,
        }
    }
}

// ── Time window for semi-auto mode ───────────────────────────────────────────

#[derive(Debug, Clone)]
struct TimeWindow {
    start_hour: u32,
    start_min: u32,
    end_hour: u32,
    end_min: u32,
}

impl TimeWindow {
    fn from_env() -> Vec<Self> {
        let raw = std::env::var("PROACTIVE_TIME_WINDOWS").unwrap_or_default();
        if raw.is_empty() {
            // Default: 09:00-22:00
            return vec![TimeWindow {
                start_hour: 9,
                start_min: 0,
                end_hour: 22,
                end_min: 0,
            }];
        }
        raw.split(',')
            .filter_map(|w| {
                let parts: Vec<&str> = w.trim().split('-').collect();
                if parts.len() != 2 {
                    return None;
                }
                let start: Vec<&str> = parts[0].split(':').collect();
                let end: Vec<&str> = parts[1].split(':').collect();
                Some(TimeWindow {
                    start_hour: start[0].parse().ok()?,
                    start_min: start.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
                    end_hour: end[0].parse().ok()?,
                    end_min: end.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
                })
            })
            .collect()
    }

    fn contains_now(&self) -> bool {
        let now = chrono::Local::now();
        let now_minutes = now.hour() * 60 + now.minute();
        let start_minutes = self.start_hour * 60 + self.start_min;
        let end_minutes = self.end_hour * 60 + self.end_min;
        now_minutes >= start_minutes && now_minutes <= end_minutes
    }
}

// ── History entry ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct HistoryEntry {
    role: String,
    text: String,
    timestamp: Instant,
}

// ── Scheduled proactive action ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ScheduledAction {
    message: String,
    send_at: Instant,
}

// ── LLM decision output ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct LlmDecision {
    paused: bool,
    #[serde(default)]
    message: String,
    #[serde(default)]
    wait_seconds: u64,
    #[serde(default)]
    continue_: bool,
    #[serde(default)]
    reason: String,
}

// ── Shared state (thread-safe) ───────────────────────────────────────────────

struct SharedState {
    /// Chat history per chat_id: Vec<(role, text, timestamp)>
    history: HashMap<String, Vec<HistoryEntry>>,
    /// Scheduled proactive messages per chat_id.
    scheduled: HashMap<String, ScheduledAction>,
    /// Last LLM decision time per chat_id (for cooldown).
    last_decision: HashMap<String, Instant>,
    /// EventBus address for publishing messages.
    event_bus: Option<Addr<EventBus>>,
    /// LLM backend recipient for decision calls.
    llm: Option<Recipient<LlmRequest>>,
    /// Chat store — unified read/write (Append + FetchRecent).
    chat_store: Option<Recipient<ChatStoreMsg>>,
    /// Plugin logger.
    logger: Option<PluginLogger>,
    /// Proactive mode.
    mode: ProactiveMode,
    /// Time windows for semi-auto mode.
    time_windows: Vec<TimeWindow>,
    /// Target chat ID (from env override, or auto-detected from user.message).
    chat_id: String,
    /// Source channel name (from env override, or auto-detected from user.message).
    source: String,
    /// Whether chat_id was set via env (if true, auto-detect is skipped).
    chat_id_from_env: bool,
    /// Whether source was set via env (if true, auto-detect is skipped).
    source_from_env: bool,
    /// Pending LLM decision request (set by background thread, consumed by on_event).
    pending_llm_request: Option<LlmRequest>,
    /// LLM decision response (set by on_event, consumed by background thread).
    pending_llm_response: Option<Result<LlmResponse, String>>,
    /// Pending DB fetch flag (set by background thread, consumed by on_event).
    pending_fetch_history: bool,
    /// DB fetch result (set by on_event, consumed by background thread).
    fetch_history_result: Option<Vec<ChatHistoryRecord>>,
}

// ── Plugin struct ────────────────────────────────────────────────────────────

struct ProactivePlugin {
    info: PluginInfo,
    state: Arc<Mutex<SharedState>>,
    running: Arc<AtomicBool>,
    /// Handle to the background thread.
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl ProactivePlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "proactive-plugin".into(),
                version: "0.1.0".into(),
                description: "LLM-driven proactive messaging: auto or semi-auto mode".into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            state: Arc::new(Mutex::new(SharedState {
                history: HashMap::new(),
                scheduled: HashMap::new(),
                last_decision: HashMap::new(),
                event_bus: None,
                llm: None,
                chat_store: None,
                logger: None,
                mode: ProactiveMode::Auto,
                time_windows: Vec::new(),
                chat_id: String::new(),
                source: String::new(),
                chat_id_from_env: false,
                source_from_env: false,
                pending_llm_request: None,
                pending_llm_response: None,
                pending_fetch_history: false,
                fetch_history_result: None,
            })),
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }
}

impl Plugin for ProactivePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let mode = ProactiveMode::from_env();
        let time_windows = TimeWindow::from_env();
        let env_chat_id = std::env::var("PROACTIVE_CHAT_ID").unwrap_or_default();
        let env_source = std::env::var("PROACTIVE_SOURCE").unwrap_or_default();
        let chat_id_from_env = !env_chat_id.is_empty();
        let source_from_env = !env_source.is_empty();

        {
            let mut s = self.state.lock().unwrap();
            s.event_bus = Some(ctx.event_bus.clone());
            s.llm = ctx.llm.clone();
            s.chat_store = ctx.chat_store.clone();
            s.logger = Some(ctx.logger.clone());
            s.mode = mode;
            s.time_windows = time_windows;
            s.chat_id = env_chat_id.clone();
            s.source = env_source.clone();
            s.chat_id_from_env = chat_id_from_env;
            s.source_from_env = source_from_env;
        }

        let logger = ctx.logger.clone();
        logger.info(format!(
            "started, mode={}, chat_id={}, source={}",
            match mode {
                ProactiveMode::Auto => "auto",
                ProactiveMode::SemiAuto => "semi-auto",
            },
            if chat_id_from_env { env_chat_id.as_str() } else { "(auto-detect)" },
            if source_from_env { env_source.as_str() } else { "(auto-detect)" },
        ));

        // ── Load history from DB on startup ──────────────────────────────
        // Set a flag; on_event() will pick it up and do the actual fetch
        // inside the actix runtime, then store the result.
        {
            let mut s = self.state.lock().unwrap();
            if s.chat_store.is_some() {
                s.pending_fetch_history = true;
            }
        }

        // ── Spawn background loop thread ─────────────────────────────────
        self.running.store(true, Ordering::SeqCst);
        let state = Arc::clone(&self.state);
        let running = Arc::clone(&self.running);

        let handle = thread::spawn(move || {
            background_loop(state, running);
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        // Signal the background thread to stop.
        self.running.store(false, Ordering::SeqCst);

        // Wait for the thread to finish (with timeout).
        if let Some(handle) = self.thread_handle.take() {
            // Give the thread up to 5 seconds to clean up.
            let _ = handle.join();
        }

        // Clear state.
        if let Ok(mut s) = self.state.lock() {
            s.event_bus = None;
            s.llm = None;
            s.chat_store = None;
            s.logger = None;
        }

        log::info!("[proactive-plugin] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        match event.topic.as_str() {
            "user.message" | "assistant.message" => {
                if let Ok(mut s) = self.state.lock() {
                    // ── Auto-detect chat_id from user.message ───────────
                    if event.topic == "user.message" {
                        if !s.chat_id_from_env {
                            let event_cid = event.data.get("chat_id").and_then(|v| {
                                v.as_str().map(String::from)
                                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                            });
                            if let Some(ref cid) = event_cid {
                                if s.chat_id.is_empty() {
                                    s.chat_id = cid.clone();
                                    if let Some(ref logger) = s.logger {
                                        logger.info(format!("auto-detected chat_id={}", cid));
                                    }
                                }
                            }
                        }
                        if !s.source_from_env {
                            if let Some(src) = event.data.get("source").and_then(|v| v.as_str()) {
                                if s.source.is_empty() {
                                    s.source = src.to_string();
                                    if let Some(ref logger) = s.logger {
                                        logger.info(format!("auto-detected source={}", src));
                                    }
                                }
                            }
                        }
                    }

                    let chat_id = s.chat_id.clone();
                    if chat_id.is_empty() {
                        return true;
                    }

                    // ── Filter: only record messages for our tracked chat_id ──
                    let event_cid = event.data.get("chat_id").and_then(|v| {
                        v.as_str().map(String::from)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    });
                    if let Some(ref ecid) = event_cid {
                        if ecid != &chat_id {
                            return true; // different chat, skip
                        }
                    }

                    // Extract text from event data.
                    let text = event
                        .data
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if text.is_empty() {
                        return true;
                    }

                    let role = if event.topic == "user.message" {
                        "user"
                    } else {
                        "assistant"
                    };

                    let entry = HistoryEntry {
                        role: role.to_string(),
                        text,
                        timestamp: Instant::now(),
                    };

                    let history = s.history.entry(chat_id.clone()).or_default();
                    history.push(entry);
                    if history.len() > MAX_HISTORY {
                        history.remove(0);
                    }

                    // If user replied, cancel any scheduled action.
                    if role == "user" {
                        s.scheduled.remove(&chat_id);
                    }

                    if let Some(ref logger) = s.logger {
                        logger.info(format!(
                            "recorded {} message (history len={})",
                            role,
                            s.history.get(&chat_id).map(|h| h.len()).unwrap_or(0),
                        ));
                    }
                }
            }
            // ── Internal: fetch history from DB ──────────────────────────
            "proactive.internal.fetch_history" => {
                let (chat_store, _logger) = {
                    let s = match self.state.lock() {
                        Ok(s) => s,
                        Err(_) => return true,
                    };
                    (s.chat_store.clone(), s.logger.clone())
                };
                if let Some(store) = chat_store {
                    let state = Arc::clone(&self.state);
                    // Use std::thread::spawn + own runtime to avoid spawn_local requirement.
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("tokio runtime for fetch_history");
                        rt.block_on(async {
                            match store.send(ChatStoreMsg::FetchRecent { limit: MAX_HISTORY }).await {
                                Ok(ChatStoreResponse::FetchRecent(records)) => {
                                    if let Ok(mut s) = state.lock() {
                                        s.fetch_history_result = Some(records.clone());
                                        if let Some(ref l) = s.logger {
                                            l.info(format!("fetched {} history records from DB", records.len()));
                                        }
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    if let Ok(mut s) = state.lock() {
                                        s.fetch_history_result = Some(vec![]);
                                        if let Some(ref l) = s.logger {
                                            l.error(format!("failed to fetch history: {}", e));
                                        }
                                    }
                                }
                            }
                        });
                    });
                }
            }
            // ── Internal: LLM decision request ───────────────────────────
            "proactive.internal.llm_decision" => {
                let (llm, request, _logger) = {
                    let mut s = match self.state.lock() {
                        Ok(s) => s,
                        Err(_) => return true,
                    };
                    let llm = s.llm.clone();
                    let request = s.pending_llm_request.take();
                    let logger = s.logger.clone();
                    (llm, request, logger)
                };
                if let (Some(llm), Some(request)) = (llm, request) {
                    let state = Arc::clone(&self.state);
                    // Use std::thread::spawn + own runtime to avoid spawn_local requirement.
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("tokio runtime for llm_decision");
                        rt.block_on(async {
                            let result = llm.send(request).await;
                            let response = match result {
                                Ok(Ok(resp)) => Ok(resp),
                                Ok(Err(e)) => Err(format!("LLM error: {}", e)),
                                Err(e) => Err(format!("LLM mailbox error: {}", e)),
                            };
                            if let Ok(mut s) = state.lock() {
                                s.pending_llm_response = Some(response);
                            }
                        });
                    });
                }
            }
            _ => {}
        }
        true
    }
}

// ── Background loop ──────────────────────────────────────────────────────────

fn background_loop(state: Arc<Mutex<SharedState>>, running: Arc<AtomicBool>) {
    log::info!("[proactive-plugin] background loop started");

    // ── Wait for DB history fetch to complete ────────────────────────────
    // on_event() handles the actual fetch inside actix runtime.
    // We poll here until the result is ready.
    {
        let mut fired = false;
        loop {
            if !running.load(Ordering::SeqCst) {
                return;
            }
            {
                let mut s = state.lock().unwrap();
                if s.pending_fetch_history && !fired {
                    // Fire the internal event to trigger fetch in on_event().
                    if let Some(ref eb) = s.event_bus {
                        eb.do_send(Event::new(
                            "proactive.internal.fetch_history",
                            serde_json::json!({}),
                            "proactive-plugin",
                        ));
                        fired = true;
                    }
                    s.pending_fetch_history = false;
                }
                if let Some(ref records) = s.fetch_history_result {
                    let chat_id = s.chat_id.clone();
                    let records = records.clone(); // release immutable borrow
                    if !chat_id.is_empty() && !records.is_empty() {
                        let history = s.history.entry(chat_id).or_default();
                        for r in &records {
                            history.push(HistoryEntry {
                                role: r.role.clone(),
                                text: r.content.clone(),
                                timestamp: Instant::now()
                                    .checked_sub(Duration::from_secs(3600))
                                    .unwrap_or(Instant::now()),
                            });
                        }
                        if let Some(ref l) = s.logger {
                            l.info(format!("loaded {} history records from DB", records.len()));
                        }
                    }
                    s.fetch_history_result = None;
                    break; // done
                }
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    while running.load(Ordering::SeqCst) {
        // Sleep in small chunks so we can respond to stop quickly.
        for _ in 0..LOOP_INTERVAL_SECS {
            if !running.load(Ordering::SeqCst) {
                log::info!("[proactive-plugin] background loop stopped");
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }

        // ── Tick ─────────────────────────────────────────────────────────
        if let Err(e) = tick(&state) {
            log::error!("[proactive-plugin] tick error: {}", e);
        }
    }

    log::info!("[proactive-plugin] background loop stopped");
}

fn tick(state: &Arc<Mutex<SharedState>>) -> Result<(), String> {
    let (chat_id, has_scheduled, send_at, scheduled_msg) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let chat_id = s.chat_id.clone();
        if chat_id.is_empty() {
            return Ok(());
        }

        // Check time window for semi-auto mode.
        if s.mode == ProactiveMode::SemiAuto {
            let in_window = s.time_windows.iter().any(|w| w.contains_now());
            if !in_window {
                return Ok(());
            }
        }

        let scheduled = s.scheduled.get(&chat_id).cloned();
        let has_scheduled = scheduled.is_some();
        let (send_at, scheduled_msg) = scheduled
            .map(|a| (a.send_at, a.message.clone()))
            .unwrap_or((Instant::now(), String::new()));

        (chat_id, has_scheduled, send_at, scheduled_msg)
    };

    // ── Case 1: There's a scheduled action, check if it's time to send ────
    if has_scheduled {
        if Instant::now() >= send_at {
            // Time to send!
            send_proactive_message(state, &chat_id, &scheduled_msg)?;

            // After sending, ask LLM for next decision.
            let decision = ask_llm_for_decision(state, &chat_id)?;
            apply_decision(state, &chat_id, &decision)?;
        }
        // else: not yet time, wait for next tick.
        return Ok(());
    }

    // ── Case 2: No scheduled action, check cooldown ───────────────────────
    {
        let s = state.lock().map_err(|e| e.to_string())?;
        if let Some(&last) = s.last_decision.get(&chat_id) {
            if last.elapsed() < Duration::from_secs(DECISION_COOLDOWN_SECS) {
                return Ok(()); // still in cooldown
            }
        }
    }

    // ── Case 3: Ask LLM for decision ─────────────────────────────────────
    let decision = ask_llm_for_decision(state, &chat_id)?;
    apply_decision(state, &chat_id, &decision)?;

    Ok(())
}

// ── LLM decision ─────────────────────────────────────────────────────────────

fn ask_llm_for_decision(
    state: &Arc<Mutex<SharedState>>,
    chat_id: &str,
) -> Result<LlmDecision, String> {
    let (_llm, history, logger) = {
        let s = state.lock().map_err(|e| e.to_string())?;
        let llm = s.llm.clone().ok_or("no LLM backend")?;
        let history = s
            .history
            .get(chat_id)
            .cloned()
            .unwrap_or_default();
        let logger = s.logger.clone();
        (llm, history, logger)
    };

    if history.is_empty() {
        return Ok(LlmDecision {
            paused: true,
            message: String::new(),
            wait_seconds: 0,
            continue_: false,
            reason: "no history".into(),
        });
    }

    // Determine who spoke last and how long ago.
    let last_entry = history.last().unwrap();
    let last_role = &last_entry.role;
    let elapsed = last_entry.timestamp.elapsed();
    let elapsed_str = if elapsed.as_secs() < 60 {
        format!("{} 秒前", elapsed.as_secs())
    } else {
        format!("{} 分钟前", elapsed.as_secs() / 60)
    };

    let now = chrono::Local::now();
    let now_str = now.format("%H:%M").to_string();

    // Build conversation transcript.
    let mut transcript = String::new();
    for entry in &history {
        transcript.push_str(&format!("{}: {}\n", entry.role, entry.text));
    }

    let prompt = format!(
        "【对话记录】\n\
         {transcript}\n\
         \n\
         【状态】\n\
         - 最后一条消息来自: {last_role}\n\
         - 距离最后一条消息: {elapsed_str}\n\
         - 当前时间: {now_str}\n\
         \n\
         【任务】\n\
         分析以上对话。请以 JSON 格式输出以下决策：\n\
         {{\n\
           \"paused\": true,\n\
           \"message\": \"追问内容\",\n\
           \"wait_seconds\": 300,\n\
           \"continue_\": true,\n\
           \"reason\": \"决策理由\"\n\
         }}\n\
         \n\
         规则：\n\
         - 如果对话已经自然结束（用户已得到满意答复），paused=true，不需要追问。\n\
         - 如果对话还在进行中（用户在等待回复），paused=false，但也不要追问（等用户回复）。\n\
         - 只有当 assistant 已经回复、用户沉默了一段时间，才适合追问。\n\
         - wait_seconds 建议 120-600 秒。\n\
         - 只输出 JSON，不要输出其他内容。",
        transcript = transcript,
        last_role = last_role,
        elapsed_str = elapsed_str,
        now_str = now_str,
    );

    if let Some(ref l) = logger {
        l.info("asking LLM for proactive decision...");
    }

    // Build LLM request.
    let request = LlmRequest {
        messages: vec![ChatMessage::user(prompt)],
        model: None,
        temperature: Some(0.7),
        max_tokens: Some(512),
    };

    // Bridge from raw thread into actix runtime via event_bus internal event.
    // Set pending_llm_request, fire event, then poll for pending_llm_response.
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.pending_llm_request = Some(request);
        s.pending_llm_response = None;
        let event_bus = s.event_bus.clone().ok_or("no event bus")?;
        drop(s);
        event_bus.do_send(Event::new(
            "proactive.internal.llm_decision",
            serde_json::json!({}),
            "proactive-plugin",
        ));
    }

    // Poll for response with timeout.
    let start = Instant::now();
    let timeout = Duration::from_secs(30);
    loop {
        {
            let s = state.lock().map_err(|e| e.to_string())?;
            if let Some(ref resp) = s.pending_llm_response {
                let llm_response = match resp {
                    Ok(r) => r.clone(),
                    Err(e) => return Err(e.clone()),
                };

                if let Some(ref l) = logger {
                    l.info(format!("LLM decision response: {}", &llm_response.content));
                }

                // Parse the JSON decision from the LLM response.
                let content = llm_response.content.trim();
                let json_str = if let Some(start) = content.find('{') {
                    if let Some(end) = content.rfind('}') {
                        &content[start..=end]
                    } else {
                        content
                    }
                } else {
                    content
                };

                let decision: LlmDecision = serde_json::from_str(json_str)
                    .map_err(|e| format!("failed to parse LLM decision: {} — raw: {}", e, content))?;

                return Ok(decision);
            }
        }
        if start.elapsed() > timeout {
            return Err("LLM decision timed out".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

// ── Apply decision ───────────────────────────────────────────────────────────

fn apply_decision(
    state: &Arc<Mutex<SharedState>>,
    chat_id: &str,
    decision: &LlmDecision,
) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;

    // Update last decision time.
    s.last_decision
        .insert(chat_id.to_string(), Instant::now());

    if decision.paused {
        // Conversation is paused, do nothing.
        if let Some(ref logger) = s.logger {
            logger.info(format!(
                "LLM decision: paused (reason: {})",
                decision.reason,
            ));
        }
        return Ok(());
    }

    if decision.message.is_empty() {
        if let Some(ref logger) = s.logger {
            logger.info("LLM decision: no message to send, skipping");
        }
        return Ok(());
    }

    // Schedule the message.
    let send_at = Instant::now() + Duration::from_secs(decision.wait_seconds.max(30));
    s.scheduled.insert(
        chat_id.to_string(),
        ScheduledAction {
            message: decision.message.clone(),
            send_at,
        },
    );

    if let Some(ref logger) = s.logger {
        logger.info(format!(
            "LLM decision: scheduled message in {}s (reason: {}, continue: {})",
            decision.wait_seconds,
            decision.reason,
            decision.continue_,
        ));
    }

    Ok(())
}

// ── Send proactive message ───────────────────────────────────────────────────

fn send_proactive_message(
    state: &Arc<Mutex<SharedState>>,
    chat_id: &str,
    message: &str,
) -> Result<(), String> {
    let (event_bus, source, chat_store, logger) = {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        // Remove the scheduled action.
        s.scheduled.remove(chat_id);

        let event_bus = s.event_bus.clone().ok_or("no event bus")?;
        let source = s.source.clone();
        let chat_store = s.chat_store.clone();
        let logger = s.logger.clone();
        (event_bus, source, chat_store, logger)
    };

    if let Some(ref l) = logger {
        l.info(format!("sending proactive message: {}", message));
    }

    // Publish the proactive message via EventBus.
    let event = Event::new(
        "proactive.message",
        serde_json::json!({
            "text": message,
            "chat_id": chat_id,
            "source": source,
        }),
        "proactive-plugin",
    );
    event_bus.do_send(event);

    // Persist the assistant message to chat history.
    if let Some(ref store) = chat_store {
        store.do_send(ChatStoreMsg::Append {
            role: "assistant".into(),
            content: message.to_string(),
        });
    }

    // Also record in our own history.
    if let Ok(mut s) = state.lock() {
        let history = s.history.entry(chat_id.to_string()).or_default();
        history.push(HistoryEntry {
            role: "assistant".into(),
            text: message.to_string(),
            timestamp: Instant::now(),
        });
        if history.len() > MAX_HISTORY {
            history.remove(0);
        }
    }

    Ok(())
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ProactivePlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}

