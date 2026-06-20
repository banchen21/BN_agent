//! proactive-plugin — LLM 主动消息插件（工具驱动）。
//!
//! LLM 通过两个工具自行安排主动消息（**只设定冷却时间，不预写内容**）：
//!   - `proactive_schedule_once`      一次性：经过 N 秒/分钟后触发一次
//!   - `proactive_schedule_recurring` 循环：每隔 N 秒/分钟触发一次
//!
//! 到期后插件发布 `proactive.trigger` 事件，PipelineActor 回调 LLM
//! 按**当前对话上下文实时生成**并发送主动消息。
//!
//! 模式（`PROACTIVE_MODE`）：
//!   - `auto`      全时段触发
//!   - `semi-auto` 仅在 `PROACTIVE_TIME_WINDOWS` 时间窗口内触发；
//!                 窗口外到期的任务**顺延**，进入窗口后立即补发
//!
//! 用户主动回复 → 取消该会话**全部**已安排任务（等 LLM 重新安排）。
//!
//! DLL 不能使用 actix actor，故用 `Arc<Mutex<>>` + 后台线程共享状态。

use chrono::Timelike;
use plugin_interface::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ── 模式 ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProactiveMode {
    Auto,
    SemiAuto,
}

impl ProactiveMode {
    fn from_env() -> Self {
        match std::env::var("PROACTIVE_MODE")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "semi-auto" | "semi_auto" | "semiauto" => ProactiveMode::SemiAuto,
            _ => ProactiveMode::Auto,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            ProactiveMode::Auto => "auto",
            ProactiveMode::SemiAuto => "semi-auto",
        }
    }
}

// ── 时间窗口（semi-auto 模式）────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct TimeWindow {
    start_min: u32,
    end_min: u32,
}

impl TimeWindow {
    /// 解析 `PROACTIVE_TIME_WINDOWS`，如 `09:00-12:00,14:00-22:00`。默认 09:00-22:00。
    fn from_env() -> Vec<Self> {
        let raw = std::env::var("PROACTIVE_TIME_WINDOWS").unwrap_or_default();
        if raw.is_empty() {
            return vec![TimeWindow {
                start_min: 9 * 60,
                end_min: 22 * 60,
            }];
        }
        raw.split(',')
            .filter_map(|w| {
                let parts: Vec<&str> = w.trim().split('-').collect();
                if parts.len() != 2 {
                    return None;
                }
                let parse_hm = |s: &str| -> Option<u32> {
                    let p: Vec<&str> = s.split(':').collect();
                    let h: u32 = p.first()?.trim().parse().ok()?;
                    let m: u32 = p.get(1).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                    Some(h * 60 + m)
                };
                Some(TimeWindow {
                    start_min: parse_hm(parts[0])?,
                    end_min: parse_hm(parts[1])?,
                })
            })
            .collect()
    }

    fn contains_now(&self) -> bool {
        let now = chrono::Local::now();
        let m = now.hour() * 60 + now.minute();
        m >= self.start_min && m <= self.end_min
    }
}

// ── 定时任务 ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum ScheduleKind {
    /// 触发一次后丢弃。
    Once,
    /// 每隔 `Duration` 循环触发。
    Recurring(Duration),
}

#[derive(Debug, Clone)]
struct ScheduledTask {
    kind: ScheduleKind,
    send_at: Instant,
    /// LLM 给未来的自己留的备注（可选）。
    note: Option<String>,
}

// ── 共享状态 ─────────────────────────────────────────────────────────────────

struct SharedState {
    /// 每会话的已安排任务。
    scheduled: HashMap<String, Vec<ScheduledTask>>,
    event_bus: Option<Addr<EventBus>>,
    logger: Option<PluginLogger>,
    mode: ProactiveMode,
    time_windows: Vec<TimeWindow>,
    /// 目标会话 ID（env 覆盖，或从 user.message 自动检测）。
    chat_id: String,
    /// 来源通道（env 覆盖，或自动检测）。
    source: String,
    chat_id_from_env: bool,
    source_from_env: bool,
}

impl SharedState {
    fn log(&self, msg: impl std::fmt::Display) {
        if let Some(ref l) = self.logger {
            l.info(msg);
        }
    }
}

/// 解析 `seconds` + `minutes` 参数，累加为总秒数。
fn parse_delay_secs(args: &serde_json::Value) -> u64 {
    let secs = args.get("seconds").and_then(|v| v.as_u64()).unwrap_or(0);
    let mins = args.get("minutes").and_then(|v| v.as_u64()).unwrap_or(0);
    secs + mins * 60
}

fn parse_note(args: &serde_json::Value) -> Option<String> {
    args.get("note")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

// ── 工具：一次性定时 ─────────────────────────────────────────────────────────

struct ScheduleOnceTool {
    state: Arc<Mutex<SharedState>>,
}

impl ToolExecutor for ScheduleOnceTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "proactive_schedule_once".into(),
            description: "安排一次性主动消息：经过指定冷却时间后，你会被再次唤起，根据当时的对话上下文主动给用户发一条消息。用于「过一会儿再主动找用户」。seconds 与 minutes 可同时给出，累加为总冷却时间。用户一旦回复，所有已安排任务都会被取消。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "description": "冷却秒数（与 minutes 累加）", "minimum": 0},
                    "minutes": {"type": "integer", "description": "冷却分钟数（与 seconds 累加）", "minimum": 0},
                    "note": {"type": "string", "description": "可选：给未来的自己留一句备注，提示到时想聊什么"}
                }
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let delay = parse_delay_secs(args);
        if delay == 0 {
            return ToolResult::err("seconds/minutes 至少给一个且大于 0");
        }
        let note = parse_note(args);
        let mut s = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return ToolResult::err("state lock poisoned"),
        };
        let chat_id = s.chat_id.clone();
        if chat_id.is_empty() {
            return ToolResult::err("尚未确定目标会话，稍后再试");
        }
        s.scheduled.entry(chat_id).or_default().push(ScheduledTask {
            kind: ScheduleKind::Once,
            send_at: Instant::now() + Duration::from_secs(delay),
            note: note.clone(),
        });
        s.log(format!("scheduled once in {}s (note={:?})", delay, note));
        ToolResult::ok(&format!("已安排：{} 秒后主动找用户一次", delay))
    }
}

// ── 工具：循环定时 ───────────────────────────────────────────────────────────

struct ScheduleRecurringTool {
    state: Arc<Mutex<SharedState>>,
}

impl ToolExecutor for ScheduleRecurringTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "proactive_schedule_recurring".into(),
            description: "安排循环主动消息：每隔指定冷却时间就把你唤起一次，根据当时上下文主动给用户发消息，循环往复，直到用户回复（用户一回复即全部取消）。seconds 与 minutes 累加为间隔时间。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "description": "间隔秒数（与 minutes 累加）", "minimum": 0},
                    "minutes": {"type": "integer", "description": "间隔分钟数（与 seconds 累加）", "minimum": 0},
                    "note": {"type": "string", "description": "可选：给未来的自己留一句备注"}
                }
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let delay = parse_delay_secs(args);
        if delay == 0 {
            return ToolResult::err("seconds/minutes 至少给一个且大于 0");
        }
        let note = parse_note(args);
        let mut s = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return ToolResult::err("state lock poisoned"),
        };
        let chat_id = s.chat_id.clone();
        if chat_id.is_empty() {
            return ToolResult::err("尚未确定目标会话，稍后再试");
        }
        s.scheduled.entry(chat_id).or_default().push(ScheduledTask {
            kind: ScheduleKind::Recurring(Duration::from_secs(delay)),
            send_at: Instant::now() + Duration::from_secs(delay),
            note: note.clone(),
        });
        s.log(format!(
            "scheduled recurring every {}s (note={:?})",
            delay, note
        ));
        ToolResult::ok(&format!(
            "已安排：每 {} 秒主动找用户一次（用户回复即停止）",
            delay
        ))
    }
}

// ── 插件 ─────────────────────────────────────────────────────────────────────

struct ProactivePlugin {
    info: PluginInfo,
    state: Arc<Mutex<SharedState>>,
    running: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl ProactivePlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "proactive-plugin".into(),
                version: "0.2.0".into(),
                description: "LLM 主动消息：工具驱动的一次性/循环定时，auto/semi-auto 模式".into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            state: Arc::new(Mutex::new(SharedState {
                scheduled: HashMap::new(),
                event_bus: None,
                logger: None,
                mode: ProactiveMode::Auto,
                time_windows: Vec::new(),
                chat_id: String::new(),
                source: String::new(),
                chat_id_from_env: false,
                source_from_env: false,
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
            s.logger = Some(ctx.logger.clone());
            s.mode = mode;
            s.time_windows = time_windows;
            s.chat_id = env_chat_id.clone();
            s.source = env_source.clone();
            s.chat_id_from_env = chat_id_from_env;
            s.source_from_env = source_from_env;
        }

        // 注册工具给 LLM。
        if let Some(ref registry) = ctx.tool_registry {
            if let Ok(mut reg) = registry.lock() {
                reg.register(Arc::new(ScheduleOnceTool {
                    state: Arc::clone(&self.state),
                }));
                reg.register(Arc::new(ScheduleRecurringTool {
                    state: Arc::clone(&self.state),
                }));
            }
            ctx.logger
                .info("registered tools: proactive_schedule_once, proactive_schedule_recurring");
        } else {
            ctx.logger
                .error("tool_registry unavailable — proactive tools NOT registered");
        }

        ctx.logger.info(format!(
            "started, mode={}, chat_id={}, source={}",
            mode.as_str(),
            if chat_id_from_env {
                env_chat_id.as_str()
            } else {
                "(auto-detect)"
            },
            if source_from_env {
                env_source.as_str()
            } else {
                "(auto-detect)"
            },
        ));

        // 启动后台轮询线程。
        self.running.store(true, Ordering::SeqCst);
        let state = Arc::clone(&self.state);
        let running = Arc::clone(&self.running);
        self.thread_handle = Some(thread::spawn(move || background_loop(state, running)));
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        if let Ok(mut s) = self.state.lock() {
            s.event_bus = None;
            s.logger = None;
            s.scheduled.clear();
        }
        log::info!("[proactive-plugin] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        // 只关心用户消息：用于自动检测 chat_id/source，以及「用户回复即取消任务」。
        if event.topic != "user.message" {
            return true;
        }

        if let Ok(mut s) = self.state.lock() {
            // ── 自动检测 chat_id ──
            if !s.chat_id_from_env && s.chat_id.is_empty() {
                if let Some(cid) = event.data.get("chat_id").and_then(|v| {
                    v.as_str()
                        .map(String::from)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                }) {
                    s.chat_id = cid.clone();
                    s.log(format!("auto-detected chat_id={}", cid));
                }
            }
            // ── 自动检测 source ──
            if !s.source_from_env && s.source.is_empty() {
                if let Some(src) = event.data.get("source").and_then(|v| v.as_str()) {
                    s.source = src.to_string();
                    s.log(format!("auto-detected source={}", src));
                }
            }

            // ── 用户回复 → 取消该会话全部已安排任务 ──
            let chat_id = s.chat_id.clone();
            if !chat_id.is_empty() {
                let event_cid = event.data.get("chat_id").and_then(|v| {
                    v.as_str()
                        .map(String::from)
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                });
                // 事件无 chat_id 时按当前会话处理；有则需匹配。
                if event_cid.as_deref().map(|c| c == chat_id).unwrap_or(true) {
                    if let Some(tasks) = s.scheduled.get_mut(&chat_id) {
                        if !tasks.is_empty() {
                            tasks.clear();
                            s.log("user replied — cancelled all scheduled tasks");
                        }
                    }
                }
            }
        }
        true
    }
}

// ── 后台循环 ─────────────────────────────────────────────────────────────────

fn background_loop(state: Arc<Mutex<SharedState>>, running: Arc<AtomicBool>) {
    let interval: u64 = std::env::var("PROACTIVE_LOOP_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    log::info!(
        "[proactive-plugin] background loop started (interval={}s)",
        interval
    );

    while running.load(Ordering::SeqCst) {
        // 可中断的分段睡眠。
        for _ in 0..interval {
            if !running.load(Ordering::SeqCst) {
                log::info!("[proactive-plugin] background loop stopped");
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
        tick(&state);
    }
    log::info!("[proactive-plugin] background loop stopped");
}

/// 一次轮询：检查到期任务，发布 `proactive.trigger` 事件。
fn tick(state: &Arc<Mutex<SharedState>>) {
    // (chat_id, source, note)
    let mut triggers: Vec<(String, String, Option<String>)> = Vec::new();

    let event_bus = {
        let mut s = match state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let chat_id = s.chat_id.clone();
        if chat_id.is_empty() {
            return;
        }

        // 半自动模式且不在时间窗口 → 不触发，任务原样保留（顺延，进入窗口后补发）。
        let in_window = match s.mode {
            ProactiveMode::Auto => true,
            ProactiveMode::SemiAuto => s.time_windows.iter().any(|w| w.contains_now()),
        };
        if !in_window {
            return;
        }

        let source = s.source.clone();
        let now = Instant::now();
        if let Some(tasks) = s.scheduled.get_mut(&chat_id) {
            let mut keep: Vec<ScheduledTask> = Vec::with_capacity(tasks.len());
            for mut task in tasks.drain(..) {
                if now >= task.send_at {
                    triggers.push((chat_id.clone(), source.clone(), task.note.clone()));
                    match task.kind {
                        ScheduleKind::Once => { /* 触发后丢弃 */ }
                        ScheduleKind::Recurring(iv) => {
                            task.send_at = now + iv; // 重新计时，继续循环
                            keep.push(task);
                        }
                    }
                } else {
                    keep.push(task);
                }
            }
            *tasks = keep;
        }
        s.event_bus.clone()
    };

    if triggers.is_empty() {
        return;
    }
    if let Some(bus) = event_bus {
        for (chat_id, source, note) in triggers {
            let peer_id = if source.is_empty() {
                String::new()
            } else {
                format!("{}:{}", source, chat_id)
            };
            let mut data = serde_json::json!({
                "chat_id": chat_id,
                "source": source,
                "peer_id": peer_id,
            });
            if let Some(n) = note {
                data["note"] = serde_json::json!(n);
            }
            bus.do_send(Event::new("proactive.trigger", data, "proactive-plugin"));
        }
    }
}

// ── FFI 导出 ─────────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ProactivePlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
