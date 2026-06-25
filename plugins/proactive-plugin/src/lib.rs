//! proactive-plugin — LLM 主动消息插件（工具驱动）。
//!
//! LLM 通过两个工具自行安排主动消息（只设定冷却时间，可带到期备注）：
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
//! 自主主动（`PROACTIVE_AUTONOMOUS_ENABLED`）：
//!   - 记录目标会话最后互动时间
//!   - 空闲超过弹性窗口后默认发布 `proactive.trigger`
//!   - `PROACTIVE_AGENT_LOOP_MODE=mirror|replace` 时，可同时或改为发布 `agent.loop.start`
//!   - 默认路径由 PipelineActor 回调 LLM 生成自然主动消息；Agent Loop 路径由目标循环自主判断和行动
//!
//! 用户主动回复 → 取消该会话**全部**已安排任务（等 LLM 重新安排）。
//!
//! DLL 不能使用 actix actor，故用 `Arc<Mutex<>>` + 后台线程共享状态。

use chrono::{Datelike, Timelike};
use plugin_interface::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentLoopTriggerMode {
    Off,
    Mirror,
    Replace,
}

impl AgentLoopTriggerMode {
    fn from_env() -> Self {
        agent_loop_trigger_mode_from_str(
            &std::env::var("PROACTIVE_AGENT_LOOP_MODE").unwrap_or_default(),
        )
    }

    fn as_str(&self) -> &'static str {
        match self {
            AgentLoopTriggerMode::Off => "off",
            AgentLoopTriggerMode::Mirror => "mirror",
            AgentLoopTriggerMode::Replace => "replace",
        }
    }
}

fn agent_loop_trigger_mode_from_str(raw: &str) -> AgentLoopTriggerMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "mirror" | "both" | "also" => AgentLoopTriggerMode::Mirror,
        "replace" | "agent-loop" | "agent_loop" | "loop" => AgentLoopTriggerMode::Replace,
        _ => AgentLoopTriggerMode::Off,
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

#[derive(Debug, Clone)]
struct PeerActivity {
    source: String,
    chat_id: String,
    last_user_at: Option<Instant>,
    last_assistant_at: Option<Instant>,
    last_autonomous_at: Option<Instant>,
    next_autonomous_at: Option<Instant>,
    unanswered_autonomous_count: u32,
    autonomous_day: i32,
    autonomous_count_today: u64,
    user_message_count: u64,
}

impl PeerActivity {
    fn new(source: String, chat_id: String) -> Self {
        Self {
            source,
            chat_id,
            last_user_at: None,
            last_assistant_at: None,
            last_autonomous_at: None,
            next_autonomous_at: None,
            unanswered_autonomous_count: 0,
            autonomous_day: current_day_key(),
            autonomous_count_today: 0,
            user_message_count: 0,
        }
    }

    fn last_interaction_at(&self) -> Option<Instant> {
        match (self.last_user_at, self.last_assistant_at) {
            (Some(user_at), Some(assistant_at)) => Some(user_at.max(assistant_at)),
            (Some(user_at), None) => Some(user_at),
            (None, Some(assistant_at)) => Some(assistant_at),
            (None, None) => None,
        }
    }

    fn reset_daily_count_if_needed(&mut self) {
        let today = current_day_key();
        if self.autonomous_day != today {
            self.autonomous_day = today;
            self.autonomous_count_today = 0;
        }
    }
}

// ── 共享状态 ─────────────────────────────────────────────────────────────────

struct SharedState {
    /// 每会话的已安排任务。
    scheduled: HashMap<String, Vec<ScheduledTask>>,
    /// 每会话最近互动状态，key 为 peer_id。
    peers: HashMap<String, PeerActivity>,
    event_bus: Option<Addr<EventBus>>,
    logger: Option<PluginLogger>,
    mode: ProactiveMode,
    time_windows: Vec<TimeWindow>,
    /// 目标会话 ID（env 覆盖，或从 user.message 自动检测）。
    chat_id: String,
    /// 来源通道（env 覆盖，或自动检测）。
    source: String,
    /// 当前工具调用应作用的会话，通常由最近一条 user.message 设置。
    current_peer_id: String,
    chat_id_from_env: bool,
    source_from_env: bool,
    autonomous_enabled: bool,
    autonomous_idle: Duration,
    autonomous_cooldown: Duration,
    autonomous_min_user_messages: u64,
    autonomous_idle_jitter_pct: u64,
    autonomous_cooldown_jitter_pct: u64,
    autonomous_chance_pct: u64,
    autonomous_daily_limit: u64,
    autonomous_max_backoff_multiplier: u64,
    agent_loop_mode: AgentLoopTriggerMode,
    agent_loop_goal_template: String,
    agent_loop_max_steps: usize,
    agent_loop_max_tool_rounds: usize,
}

impl SharedState {
    fn log(&self, msg: impl std::fmt::Display) {
        if let Some(ref l) = self.logger {
            l.info(msg);
        }
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_duration(name: &str, default_secs: u64) -> Duration {
    Duration::from_secs(
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_secs),
    )
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn current_day_key() -> i32 {
    chrono::Local::now().date_naive().num_days_from_ce()
}

fn stable_hash(input: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn random_mod(max_exclusive: u64, salt: &str) -> u64 {
    if max_exclusive == 0 {
        return 0;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut value = now ^ stable_hash(salt);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    value ^= value >> 31;
    value % max_exclusive
}

fn random_percent(salt: &str) -> u64 {
    random_mod(100, salt)
}

fn scale_duration(base: Duration, multiplier: u64) -> Duration {
    Duration::from_secs(base.as_secs().saturating_mul(multiplier.max(1)))
}

fn jitter_duration(base: Duration, jitter_pct: u64, salt: &str) -> Duration {
    let base_secs = base.as_secs();
    if base_secs == 0 {
        return base;
    }
    let jitter_pct = jitter_pct.min(95);
    let radius = base_secs.saturating_mul(jitter_pct) / 100;
    if radius == 0 {
        return base;
    }
    let offset = random_mod(radius.saturating_mul(2).saturating_add(1), salt);
    Duration::from_secs(base_secs.saturating_sub(radius).saturating_add(offset))
}

#[allow(clippy::too_many_arguments)]
fn compute_next_autonomous_at(
    peer_id: &str,
    peer: &PeerActivity,
    idle: Duration,
    cooldown: Duration,
    idle_jitter_pct: u64,
    cooldown_jitter_pct: u64,
    max_backoff_multiplier: u64,
) -> Option<Instant> {
    let last_interaction = peer.last_interaction_at()?;
    let backoff = 2u64
        .saturating_pow(peer.unanswered_autonomous_count.min(8))
        .min(max_backoff_multiplier.max(1));
    let idle_delay = jitter_duration(
        scale_duration(idle, backoff),
        idle_jitter_pct,
        &format!("{}:idle:{}", peer_id, peer.unanswered_autonomous_count),
    );
    let mut next_at = last_interaction + idle_delay;

    if let Some(last_autonomous) = peer.last_autonomous_at {
        let cooldown_delay = jitter_duration(
            scale_duration(cooldown, backoff),
            cooldown_jitter_pct,
            &format!("{}:cooldown:{}", peer_id, peer.unanswered_autonomous_count),
        );
        let cooldown_at = last_autonomous + cooldown_delay;
        if cooldown_at > next_at {
            next_at = cooldown_at;
        }
    }

    Some(next_at)
}

fn chat_id_from_event(data: &serde_json::Value) -> Option<String> {
    data.get("chat_id")
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        .filter(|s| !s.trim().is_empty())
}

fn source_from_event(data: &serde_json::Value) -> Option<String> {
    data.get("source")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("platform").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

fn peer_id_from_parts(source: &str, chat_id: &str) -> String {
    if source.is_empty() {
        chat_id.to_string()
    } else {
        format!("{}:{}", source, chat_id)
    }
}

fn peer_id_from_event(data: &serde_json::Value, fallback_source: &str) -> Option<String> {
    data.get("peer_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let source = source_from_event(data).unwrap_or_else(|| fallback_source.to_string());
            let chat_id = chat_id_from_event(data)?;
            Some(peer_id_from_parts(&source, &chat_id))
        })
}

fn chat_id_from_peer_id(peer_id: &str) -> String {
    peer_id
        .split_once(':')
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| peer_id.to_string())
}

const DEFAULT_AGENT_LOOP_GOAL: &str = "用户已经空闲约 {idle_secs} 秒。请根据该 peer 的最近上下文自主判断是否适合自然地主动联系用户。当前平台 source={source}，chat_id={chat_id}，peer_id={peer_id}。如果适合，请使用当前平台的文本发送工具发一条简短自然的消息：Telegram 用 tg_send_message，微信用 wechat_send_message，飞书用 feishu_send_message；如果不适合，请输出 done 并说明跳过。";

fn render_agent_loop_goal(
    template: &str,
    source: &str,
    chat_id: &str,
    peer_id: &str,
    idle_secs: Option<u64>,
) -> String {
    let idle = idle_secs
        .map(|secs| secs.to_string())
        .unwrap_or_else(|| "未知".to_string());
    let template = if template.trim().is_empty() {
        DEFAULT_AGENT_LOOP_GOAL
    } else {
        template
    };
    template
        .replace("{source}", source)
        .replace("{chat_id}", chat_id)
        .replace("{peer_id}", peer_id)
        .replace("{idle_secs}", &idle)
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
            description: "安排一次性主动消息或定时提醒：经过指定冷却时间后，你会被再次唤起，根据当时的对话上下文主动给用户发一条消息。用于「过一会儿再主动找用户」或「几秒/几分钟后提醒、叫用户」。seconds 与 minutes 可同时给出，累加为总冷却时间。用户一旦回复，所有已安排任务都会被取消。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "description": "冷却秒数（与 minutes 累加）", "minimum": 0},
                    "minutes": {"type": "integer", "description": "冷却分钟数（与 seconds 累加）", "minimum": 0},
                    "note": {"type": "string", "description": "可选：到期时要完成的提醒内容或主动联系目的。用户说“三秒后叫我”时，写“三秒到了，叫用户”这类到期任务，不要写成新的延迟安排。"}
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
        let peer_id = if !s.current_peer_id.is_empty() {
            s.current_peer_id.clone()
        } else if !s.chat_id.is_empty() {
            peer_id_from_parts(&s.source, &s.chat_id)
        } else {
            String::new()
        };
        if peer_id.is_empty() {
            return ToolResult::err("尚未确定目标会话，稍后再试");
        }
        s.scheduled.entry(peer_id).or_default().push(ScheduledTask {
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
                    "note": {"type": "string", "description": "可选：每次到期时要完成的提醒内容或主动联系目的。"}
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
        let peer_id = if !s.current_peer_id.is_empty() {
            s.current_peer_id.clone()
        } else if !s.chat_id.is_empty() {
            peer_id_from_parts(&s.source, &s.chat_id)
        } else {
            String::new()
        };
        if peer_id.is_empty() {
            return ToolResult::err("尚未确定目标会话，稍后再试");
        }
        s.scheduled.entry(peer_id).or_default().push(ScheduledTask {
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
                version: "0.3.0".into(),
                description: "LLM 主动消息：工具驱动定时 + 空闲自主主动，auto/semi-auto 模式"
                    .into(),
                author: "demo".into(),
                min_host_version: "0.1.0".into(),
            },
            state: Arc::new(Mutex::new(SharedState {
                scheduled: HashMap::new(),
                peers: HashMap::new(),
                event_bus: None,
                logger: None,
                mode: ProactiveMode::Auto,
                time_windows: Vec::new(),
                chat_id: String::new(),
                source: String::new(),
                current_peer_id: String::new(),
                chat_id_from_env: false,
                source_from_env: false,
                autonomous_enabled: false,
                autonomous_idle: Duration::from_secs(30 * 60),
                autonomous_cooldown: Duration::from_secs(60 * 60),
                autonomous_min_user_messages: 1,
                autonomous_idle_jitter_pct: 45,
                autonomous_cooldown_jitter_pct: 35,
                autonomous_chance_pct: 65,
                autonomous_daily_limit: 4,
                autonomous_max_backoff_multiplier: 4,
                agent_loop_mode: AgentLoopTriggerMode::Off,
                agent_loop_goal_template: String::new(),
                agent_loop_max_steps: 6,
                agent_loop_max_tool_rounds: 3,
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
        let autonomous_enabled = env_bool("PROACTIVE_AUTONOMOUS_ENABLED", true);
        let autonomous_idle = env_duration("PROACTIVE_AUTONOMOUS_IDLE_SECS", 30 * 60);
        let autonomous_cooldown = env_duration("PROACTIVE_AUTONOMOUS_COOLDOWN_SECS", 60 * 60);
        let autonomous_min_user_messages = env_u64("PROACTIVE_AUTONOMOUS_MIN_USER_MESSAGES", 1);
        let autonomous_idle_jitter_pct = env_u64("PROACTIVE_AUTONOMOUS_IDLE_JITTER_PCT", 45);
        let autonomous_cooldown_jitter_pct =
            env_u64("PROACTIVE_AUTONOMOUS_COOLDOWN_JITTER_PCT", 35);
        let autonomous_chance_pct = env_u64("PROACTIVE_AUTONOMOUS_CHANCE_PCT", 65).min(100);
        let autonomous_daily_limit = env_u64("PROACTIVE_AUTONOMOUS_DAILY_LIMIT", 4);
        let autonomous_max_backoff_multiplier =
            env_u64("PROACTIVE_AUTONOMOUS_MAX_BACKOFF_MULTIPLIER", 4).max(1);
        let agent_loop_mode = AgentLoopTriggerMode::from_env();
        let agent_loop_goal_template =
            std::env::var("PROACTIVE_AGENT_LOOP_GOAL").unwrap_or_default();
        let agent_loop_max_steps = env_usize("PROACTIVE_AGENT_LOOP_MAX_STEPS", 6).clamp(1, 50);
        let agent_loop_max_tool_rounds =
            env_usize("PROACTIVE_AGENT_LOOP_MAX_TOOL_ROUNDS", 3).min(20);

        {
            let mut s = self.state.lock().unwrap();
            s.event_bus = Some(ctx.event_bus.clone());
            s.logger = Some(ctx.logger.clone());
            s.mode = mode;
            s.time_windows = time_windows;
            s.chat_id = env_chat_id.clone();
            s.source = env_source.clone();
            s.current_peer_id = if env_chat_id.is_empty() {
                String::new()
            } else {
                peer_id_from_parts(&env_source, &env_chat_id)
            };
            s.chat_id_from_env = chat_id_from_env;
            s.source_from_env = source_from_env;
            s.autonomous_enabled = autonomous_enabled;
            s.autonomous_idle = autonomous_idle;
            s.autonomous_cooldown = autonomous_cooldown;
            s.autonomous_min_user_messages = autonomous_min_user_messages;
            s.autonomous_idle_jitter_pct = autonomous_idle_jitter_pct;
            s.autonomous_cooldown_jitter_pct = autonomous_cooldown_jitter_pct;
            s.autonomous_chance_pct = autonomous_chance_pct;
            s.autonomous_daily_limit = autonomous_daily_limit;
            s.autonomous_max_backoff_multiplier = autonomous_max_backoff_multiplier;
            s.agent_loop_mode = agent_loop_mode;
            s.agent_loop_goal_template = agent_loop_goal_template.clone();
            s.agent_loop_max_steps = agent_loop_max_steps;
            s.agent_loop_max_tool_rounds = agent_loop_max_tool_rounds;
        }

        // 注册工具给 LLM。
        if let Some(ref registry) = ctx.tool_registry {
            {
                let mut reg = registry.lock();
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
            "started, mode={}, chat_id={}, source={}, autonomous={}, idle={}s±{}%, cooldown={}s±{}%, chance={}%, daily_limit={}, max_backoff={}x, agent_loop_mode={}, agent_loop_steps={}, agent_loop_tool_rounds={}",
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
            autonomous_enabled,
            autonomous_idle.as_secs(),
            autonomous_idle_jitter_pct,
            autonomous_cooldown.as_secs(),
            autonomous_cooldown_jitter_pct,
            autonomous_chance_pct,
            autonomous_daily_limit,
            autonomous_max_backoff_multiplier,
            agent_loop_mode.as_str(),
            agent_loop_max_steps,
            agent_loop_max_tool_rounds,
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
            s.peers.clear();
        }
        log::info!("[proactive-plugin] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        // 关心用户/助手消息：用于自动检测会话、记录活跃时间，以及「用户回复即取消任务」。
        if event.topic != "user.message" && event.topic != "assistant.message" {
            return true;
        }

        if let Ok(mut s) = self.state.lock() {
            let now = Instant::now();
            let event_source = source_from_event(&event.data).unwrap_or_else(|| s.source.clone());
            let event_chat_id = chat_id_from_event(&event.data);
            let event_peer_id = peer_id_from_event(&event.data, &event_source);

            // ── 自动检测 chat_id ──
            if !s.chat_id_from_env && s.chat_id.is_empty() {
                if let Some(cid) = event_chat_id.clone() {
                    s.chat_id = cid.clone();
                    s.log(format!("auto-detected chat_id={}", cid));
                }
            }
            // ── 自动检测 source ──
            if !s.source_from_env && s.source.is_empty() {
                if let Some(src) = source_from_event(&event.data) {
                    s.source = src.clone();
                    s.log(format!("auto-detected source={}", src));
                }
            }

            if let Some(peer_id) = event_peer_id.clone() {
                let chat_id = event_chat_id
                    .clone()
                    .unwrap_or_else(|| chat_id_from_peer_id(&peer_id));
                let source = if event_source.is_empty() {
                    s.source.clone()
                } else {
                    event_source.clone()
                };
                let autonomous_enabled = s.autonomous_enabled;
                let autonomous_idle = s.autonomous_idle;
                let autonomous_cooldown = s.autonomous_cooldown;
                let autonomous_idle_jitter_pct = s.autonomous_idle_jitter_pct;
                let autonomous_cooldown_jitter_pct = s.autonomous_cooldown_jitter_pct;
                let autonomous_max_backoff_multiplier = s.autonomous_max_backoff_multiplier;
                {
                    let peer = s
                        .peers
                        .entry(peer_id.clone())
                        .or_insert_with(|| PeerActivity::new(source.clone(), chat_id.clone()));
                    peer.source = source;
                    peer.chat_id = chat_id;
                    if event.topic == "user.message" {
                        peer.last_user_at = Some(now);
                        peer.user_message_count = peer.user_message_count.saturating_add(1);
                        peer.unanswered_autonomous_count = 0;
                    } else {
                        peer.last_assistant_at = Some(now);
                    }
                    peer.reset_daily_count_if_needed();
                    if autonomous_enabled {
                        peer.next_autonomous_at = compute_next_autonomous_at(
                            &peer_id,
                            peer,
                            autonomous_idle,
                            autonomous_cooldown,
                            autonomous_idle_jitter_pct,
                            autonomous_cooldown_jitter_pct,
                            autonomous_max_backoff_multiplier,
                        );
                    }
                }
                if event.topic == "user.message" {
                    s.current_peer_id = peer_id;
                }
            }

            // ── 用户回复 → 取消该会话全部已安排任务 ──
            if event.topic == "user.message" {
                let scheduled_key = event_peer_id.unwrap_or_else(|| {
                    if !s.chat_id.is_empty() {
                        peer_id_from_parts(&s.source, &s.chat_id)
                    } else {
                        String::new()
                    }
                });
                if !scheduled_key.is_empty() {
                    if let Some(tasks) = s.scheduled.get_mut(&scheduled_key) {
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

struct TriggerEvent {
    peer_id: String,
    chat_id: String,
    source: String,
    note: Option<String>,
    reason: String,
    idle_secs: Option<u64>,
    send_proactive_trigger: bool,
    start_agent_loop: bool,
    agent_loop_goal: Option<String>,
    agent_loop_max_steps: usize,
    agent_loop_max_tool_rounds: usize,
}

/// 一次轮询：检查到期任务和自主主动条件，发布 `proactive.trigger` 事件。
fn tick(state: &Arc<Mutex<SharedState>>) {
    let mut triggers: Vec<TriggerEvent> = Vec::new();

    let event_bus = {
        let mut s = match state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        // 半自动模式且不在时间窗口 → 不触发，任务原样保留（顺延，进入窗口后补发）。
        let in_window = match s.mode {
            ProactiveMode::Auto => true,
            ProactiveMode::SemiAuto => s.time_windows.iter().any(|w| w.contains_now()),
        };
        if !in_window {
            return;
        }

        let now = Instant::now();

        let scheduled_keys: Vec<String> = s.scheduled.keys().cloned().collect();
        for peer_id in scheduled_keys {
            let source = s
                .peers
                .get(&peer_id)
                .map(|p| p.source.clone())
                .unwrap_or_else(|| s.source.clone());
            let chat_id = s
                .peers
                .get(&peer_id)
                .map(|p| p.chat_id.clone())
                .unwrap_or_else(|| chat_id_from_peer_id(&peer_id));

            if let Some(tasks) = s.scheduled.get_mut(&peer_id) {
                let mut keep: Vec<ScheduledTask> = Vec::with_capacity(tasks.len());
                for mut task in tasks.drain(..) {
                    if now >= task.send_at {
                        triggers.push(TriggerEvent {
                            peer_id: peer_id.clone(),
                            chat_id: chat_id.clone(),
                            source: source.clone(),
                            note: task.note.clone(),
                            reason: "scheduled".to_string(),
                            idle_secs: None,
                            send_proactive_trigger: true,
                            start_agent_loop: false,
                            agent_loop_goal: None,
                            agent_loop_max_steps: 0,
                            agent_loop_max_tool_rounds: 0,
                        });
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
        }

        if s.autonomous_enabled {
            let idle_threshold = s.autonomous_idle;
            let cooldown = s.autonomous_cooldown;
            let min_user_messages = s.autonomous_min_user_messages;
            let idle_jitter_pct = s.autonomous_idle_jitter_pct;
            let cooldown_jitter_pct = s.autonomous_cooldown_jitter_pct;
            let chance_pct = s.autonomous_chance_pct;
            let daily_limit = s.autonomous_daily_limit;
            let max_backoff_multiplier = s.autonomous_max_backoff_multiplier;
            let agent_loop_mode = s.agent_loop_mode;
            let agent_loop_goal_template = s.agent_loop_goal_template.clone();
            let agent_loop_max_steps = s.agent_loop_max_steps;
            let agent_loop_max_tool_rounds = s.agent_loop_max_tool_rounds;
            let peers_with_scheduled: Vec<String> = s
                .scheduled
                .iter()
                .filter_map(|(peer_id, tasks)| {
                    if tasks.is_empty() {
                        None
                    } else {
                        Some(peer_id.clone())
                    }
                })
                .collect();
            for (peer_id, peer) in s.peers.iter_mut() {
                peer.reset_daily_count_if_needed();
                if peer.user_message_count < min_user_messages {
                    continue;
                }
                if daily_limit > 0 && peer.autonomous_count_today >= daily_limit {
                    continue;
                }
                if peers_with_scheduled.iter().any(|p| p == peer_id) {
                    continue;
                }
                let Some(last_interaction) = peer.last_interaction_at() else {
                    continue;
                };
                let idle_for = now.saturating_duration_since(last_interaction);
                if peer.next_autonomous_at.is_none() {
                    peer.next_autonomous_at = compute_next_autonomous_at(
                        peer_id,
                        peer,
                        idle_threshold,
                        cooldown,
                        idle_jitter_pct,
                        cooldown_jitter_pct,
                        max_backoff_multiplier,
                    );
                }
                if peer.next_autonomous_at.map(|due| now < due).unwrap_or(true) {
                    continue;
                }

                if chance_pct < 100 && random_percent(&format!("{}:chance", peer_id)) >= chance_pct
                {
                    let snooze_base = Duration::from_secs((idle_threshold.as_secs() / 3).max(60));
                    let snooze = jitter_duration(
                        snooze_base,
                        idle_jitter_pct.max(50),
                        &format!("{}:snooze", peer_id),
                    );
                    peer.next_autonomous_at = Some(now + snooze);
                    continue;
                }

                peer.last_autonomous_at = Some(now);
                peer.autonomous_count_today = peer.autonomous_count_today.saturating_add(1);
                peer.unanswered_autonomous_count =
                    peer.unanswered_autonomous_count.saturating_add(1);
                peer.next_autonomous_at = compute_next_autonomous_at(
                    peer_id,
                    peer,
                    idle_threshold,
                    cooldown,
                    idle_jitter_pct,
                    cooldown_jitter_pct,
                    max_backoff_multiplier,
                );
                let start_agent_loop = agent_loop_mode != AgentLoopTriggerMode::Off;
                let send_proactive_trigger = agent_loop_mode != AgentLoopTriggerMode::Replace;
                let agent_loop_goal = if start_agent_loop {
                    Some(render_agent_loop_goal(
                        &agent_loop_goal_template,
                        &peer.source,
                        &peer.chat_id,
                        peer_id,
                        Some(idle_for.as_secs()),
                    ))
                } else {
                    None
                };
                triggers.push(TriggerEvent {
                    peer_id: peer_id.clone(),
                    chat_id: peer.chat_id.clone(),
                    source: peer.source.clone(),
                    note: None,
                    reason: "autonomous_idle".to_string(),
                    idle_secs: Some(idle_for.as_secs()),
                    send_proactive_trigger,
                    start_agent_loop,
                    agent_loop_goal,
                    agent_loop_max_steps,
                    agent_loop_max_tool_rounds,
                });
            }
        }
        for trigger in &triggers {
            if let Some(secs) = trigger.idle_secs {
                s.log(format!(
                    "triggering {} for peer={} (idle={}s, proactive={}, agent_loop={})",
                    trigger.reason,
                    trigger.peer_id,
                    secs,
                    trigger.send_proactive_trigger,
                    trigger.start_agent_loop,
                ));
            } else {
                s.log(format!(
                    "triggering {} for peer={} (proactive={}, agent_loop={})",
                    trigger.reason,
                    trigger.peer_id,
                    trigger.send_proactive_trigger,
                    trigger.start_agent_loop,
                ));
            }
        }
        s.event_bus.clone()
    };

    if triggers.is_empty() {
        return;
    }
    if let Some(bus) = event_bus {
        for trigger in triggers {
            if trigger.send_proactive_trigger {
                let mut data = serde_json::json!({
                    "chat_id": trigger.chat_id.clone(),
                    "source": trigger.source.clone(),
                    "peer_id": trigger.peer_id.clone(),
                    "reason": trigger.reason.clone(),
                });
                if let Some(n) = trigger.note.clone() {
                    data["note"] = serde_json::json!(n);
                }
                if let Some(secs) = trigger.idle_secs {
                    data["idle_secs"] = serde_json::json!(secs);
                }
                bus.do_send(Event::new("proactive.trigger", data, "proactive-plugin"));
            }

            if trigger.start_agent_loop {
                let goal = trigger.agent_loop_goal.unwrap_or_else(|| {
                    render_agent_loop_goal(
                        "",
                        &trigger.source,
                        &trigger.chat_id,
                        &trigger.peer_id,
                        trigger.idle_secs,
                    )
                });
                bus.do_send(Event::new(
                    "agent.loop.start",
                    serde_json::json!({
                        "goal": goal,
                        "peer_id": trigger.peer_id,
                        "max_steps": trigger.agent_loop_max_steps,
                        "max_tool_rounds": trigger.agent_loop_max_tool_rounds,
                    }),
                    "proactive-plugin",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_duration_stays_within_bounds() {
        let value = jitter_duration(Duration::from_secs(100), 20, "test-jitter");

        assert!(value >= Duration::from_secs(80));
        assert!(value <= Duration::from_secs(120));
    }

    #[test]
    fn unanswered_autonomous_backoff_extends_next_time() {
        let now = Instant::now();
        let mut peer = PeerActivity::new("test".into(), "chat".into());
        peer.last_user_at = Some(now);
        peer.unanswered_autonomous_count = 2;

        let next = compute_next_autonomous_at(
            "test:chat",
            &peer,
            Duration::from_secs(100),
            Duration::from_secs(0),
            0,
            0,
            4,
        )
        .expect("next autonomous time");

        assert!(next >= now + Duration::from_secs(400));
    }

    #[test]
    fn agent_loop_mode_parses_variants() {
        assert_eq!(
            agent_loop_trigger_mode_from_str(""),
            AgentLoopTriggerMode::Off
        );
        assert_eq!(
            agent_loop_trigger_mode_from_str("off"),
            AgentLoopTriggerMode::Off
        );
        assert_eq!(
            agent_loop_trigger_mode_from_str("mirror"),
            AgentLoopTriggerMode::Mirror
        );
        assert_eq!(
            agent_loop_trigger_mode_from_str("both"),
            AgentLoopTriggerMode::Mirror
        );
        assert_eq!(
            agent_loop_trigger_mode_from_str("replace"),
            AgentLoopTriggerMode::Replace
        );
        assert_eq!(
            agent_loop_trigger_mode_from_str("agent_loop"),
            AgentLoopTriggerMode::Replace
        );
    }

    #[test]
    fn render_agent_loop_goal_replaces_placeholders() {
        let goal = render_agent_loop_goal(
            "src={source}; chat={chat_id}; peer={peer_id}; idle={idle_secs}",
            "telegram",
            "123",
            "telegram:123",
            Some(456),
        );
        assert_eq!(goal, "src=telegram; chat=123; peer=telegram:123; idle=456");
    }

    #[test]
    fn render_agent_loop_goal_uses_default_when_empty() {
        let goal = render_agent_loop_goal("", "wechat", "wxid", "wechat:wxid", None);
        assert!(goal.contains("source=wechat"));
        assert!(goal.contains("chat_id=wxid"));
        assert!(goal.contains("空闲约 未知 秒"));
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
