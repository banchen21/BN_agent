//! AgentLoopActor — long-running goal loop for observe/decide/act cycles.
//!
//! This is intentionally separate from PipelineActor: chat replies stay reactive,
//! while agent loops run as explicit goal-driven jobs with budgets and status APIs.

use actix::prelude::*;
use plugin_interface::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::metrics_actor::MetricsActor;
use crate::retry_actor::RetryChatRequest;
use crate::token_usage_actor::RecordTokenUsage;

const DEFAULT_MAX_STEPS: usize = 8;
const DEFAULT_MAX_TOOL_ROUNDS: usize = 5;
const MAX_STEPS_CAP: usize = 50;
const MAX_TOOL_ROUNDS_CAP: usize = 20;
/// 终态 loop 的默认保留上限（超出按 updated_at 清理最旧的）。
const DEFAULT_MAX_KEEP: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentLoopStatus {
    Running,
    Paused,
    Completed,
    WaitingForUser,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLoopStep {
    pub step: usize,
    pub status: AgentLoopStatus,
    pub llm_message: String,
    pub reason: Option<String>,
    pub tool_calls: Vec<String>,
    pub tool_results: Vec<String>,
    pub elapsed_ms: u64,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLoopSnapshot {
    pub id: String,
    pub goal: String,
    pub peer_id: String,
    pub status: AgentLoopStatus,
    pub max_steps: usize,
    pub max_tool_rounds: usize,
    pub steps_taken: usize,
    pub observations: Vec<AgentLoopStep>,
    pub error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

struct AgentLoopState {
    snapshot: AgentLoopSnapshot,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
}

pub struct AgentLoopActor {
    // 外部依赖以 Recipient<Msg> 注入（而非具体 Addr<Actor>），便于测试注入 mock。
    retry_addr: Recipient<RetryChatRequest>,
    plugin_manager: Recipient<RefreshSnapshotsForPeer>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
    token_usage_addr: Recipient<RecordTokenUsage>,
    metrics_addr: Addr<MetricsActor>,
    loops: HashMap<String, AgentLoopState>,
    persist: Option<AgentLoopPersist>,
}

impl AgentLoopActor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        retry_addr: Recipient<RetryChatRequest>,
        plugin_manager: Recipient<RefreshSnapshotsForPeer>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        snapshots: Arc<Mutex<Vec<String>>>,
        event_bus: Addr<EventBus>,
        token_usage_addr: Recipient<RecordTokenUsage>,
        metrics_addr: Addr<MetricsActor>,
    ) -> Self {
        Self::from_parts(
            retry_addr,
            plugin_manager,
            tool_registry,
            snapshots,
            event_bus,
            token_usage_addr,
            metrics_addr,
            AgentLoopPersist::open(),
        )
    }

    /// 构造并从给定的持久化（可为 None）恢复历史 loop。便于测试注入。
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        retry_addr: Recipient<RetryChatRequest>,
        plugin_manager: Recipient<RefreshSnapshotsForPeer>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        snapshots: Arc<Mutex<Vec<String>>>,
        event_bus: Addr<EventBus>,
        token_usage_addr: Recipient<RecordTokenUsage>,
        metrics_addr: Addr<MetricsActor>,
        persist: Option<AgentLoopPersist>,
    ) -> Self {
        let mut loops = HashMap::new();
        if let Some(ref p) = persist {
            for mut snap in p.load_all() {
                // runner 线程随进程消失，重启时仍 Running/Stopping 的 loop 视为中断
                if reconcile_restored_status(&mut snap) {
                    snap.updated_at_ms = now_ms();
                    p.save(&snap);
                }
                let stop_flag = Arc::new(AtomicBool::new(false));
                let pause_flag = Arc::new(AtomicBool::new(false));
                loops.insert(
                    snap.id.clone(),
                    AgentLoopState {
                        snapshot: snap,
                        stop_flag,
                        pause_flag,
                    },
                );
            }
            if !loops.is_empty() {
                log::info!(
                    "[AgentLoopActor] restored {} loop(s) from persistence",
                    loops.len()
                );
            }
        }
        Self {
            retry_addr,
            plugin_manager,
            tool_registry,
            snapshots,
            event_bus,
            token_usage_addr,
            metrics_addr,
            loops,
            persist,
        }
    }

    /// 把指定 loop 的当前快照写入持久化存储。
    fn persist_loop(&self, id: &str) {
        if let (Some(ref p), Some(state)) = (&self.persist, self.loops.get(id)) {
            p.save(&state.snapshot);
        }
    }

    /// 清理终态 loop：内存与持久化中只保留最近 `AGENT_LOOP_MAX_KEEP` 个终态记录。
    fn prune_terminal(&mut self) {
        let keep = agent_loop_max_keep();
        let items: Vec<(String, AgentLoopStatus, u64)> = self
            .loops
            .iter()
            .map(|(id, st)| (id.clone(), st.snapshot.status.clone(), st.snapshot.updated_at_ms))
            .collect();
        let to_remove = terminal_ids_to_prune(&items, keep);
        if to_remove.is_empty() {
            return;
        }
        for id in &to_remove {
            self.loops.remove(id);
        }
        if let Some(ref p) = self.persist {
            p.prune(&to_remove);
        }
        log::info!(
            "[AgentLoopActor] pruned {} terminal loop(s) (keep={})",
            to_remove.len(),
            keep
        );
    }
}

impl Actor for AgentLoopActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[AgentLoopActor] started");
    }
}

#[derive(Message)]
#[rtype(result = "Result<AgentLoopSnapshot, String>")]
pub struct StartAgentLoop {
    pub goal: String,
    pub peer_id: Option<String>,
    pub max_steps: Option<usize>,
    pub max_tool_rounds: Option<usize>,
}

#[derive(Message)]
#[rtype(result = "Option<AgentLoopSnapshot>")]
pub struct GetAgentLoop {
    pub id: String,
}

#[derive(Message)]
#[rtype(result = "Vec<AgentLoopSnapshot>")]
pub struct ListAgentLoops;

#[derive(Message)]
#[rtype(result = "bool")]
pub struct StopAgentLoop {
    pub id: String,
}

#[derive(Message)]
#[rtype(result = "()")]
struct AgentLoopProgress {
    id: String,
    step: Option<AgentLoopStep>,
    status: Option<AgentLoopStatus>,
    error: Option<String>,
}

impl AgentLoopActor {
    /// 共享的 loop 启动逻辑：`StartAgentLoop` 消息与 `agent.loop.start` 事件都复用。
    fn start_loop_internal(
        &mut self,
        goal: String,
        peer_id: Option<String>,
        max_steps: Option<usize>,
        max_tool_rounds: Option<usize>,
        ctx: &mut Context<Self>,
    ) -> Result<AgentLoopSnapshot, String> {
        let goal = goal.trim().to_string();
        if goal.is_empty() {
            return Err("goal is required".into());
        }

        let max_concurrent = agent_loop_max_concurrent();
        if max_concurrent > 0 {
            let statuses: Vec<AgentLoopStatus> = self
                .loops
                .values()
                .map(|s| s.snapshot.status.clone())
                .collect();
            let active = active_loop_count(&statuses);
            if !can_start_new_loop(active, max_concurrent) {
                return Err(format!(
                    "max concurrent agent loops reached ({}/{})",
                    active, max_concurrent
                ));
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        let peer_id = peer_id.unwrap_or_else(|| "agent-loop:default".into());
        let max_steps = max_steps
            .unwrap_or(DEFAULT_MAX_STEPS)
            .clamp(1, MAX_STEPS_CAP);
        let max_tool_rounds = max_tool_rounds
            .unwrap_or(DEFAULT_MAX_TOOL_ROUNDS)
            .clamp(0, MAX_TOOL_ROUNDS_CAP);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));

        let snapshot = AgentLoopSnapshot {
            id: id.clone(),
            goal: goal.clone(),
            peer_id: peer_id.clone(),
            status: AgentLoopStatus::Running,
            max_steps,
            max_tool_rounds,
            steps_taken: 0,
            observations: Vec::new(),
            error: None,
            created_at_ms: now,
            updated_at_ms: now,
        };

        self.loops.insert(
            id.clone(),
            AgentLoopState {
                snapshot: snapshot.clone(),
                stop_flag: Arc::clone(&stop_flag),
                pause_flag: Arc::clone(&pause_flag),
            },
        );
        self.persist_loop(&id);

        let runner = AgentLoopRunner {
            id: id.clone(),
            goal,
            peer_id,
            max_steps,
            max_tool_rounds,
            stop_flag,
            pause_flag,
            retry_addr: self.retry_addr.clone(),
            plugin_manager: self.plugin_manager.clone(),
            tool_registry: self.tool_registry.clone(),
            snapshots: self.snapshots.clone(),
            event_bus: self.event_bus.clone(),
            token_usage_addr: self.token_usage_addr.clone(),
            metrics_addr: self.metrics_addr.clone(),
            addr: ctx.address(),
        };
        actix::spawn(async move { runner.run().await });

        // 新 loop 创建后清理过旧的终态 loop，防止内存/DB 无限增长
        self.prune_terminal();

        Ok(snapshot)
    }
}

impl Handler<StartAgentLoop> for AgentLoopActor {
    type Result = Result<AgentLoopSnapshot, String>;

    fn handle(&mut self, msg: StartAgentLoop, ctx: &mut Self::Context) -> Self::Result {
        self.start_loop_internal(msg.goal, msg.peer_id, msg.max_steps, msg.max_tool_rounds, ctx)
    }
}

impl Handler<Event> for AgentLoopActor {
    type Result = ();

    /// 事件驱动启动：任何插件发 `agent.loop.start`（data: {goal, peer_id?, max_steps?, max_tool_rounds?}）即可启动一个 loop。
    fn handle(&mut self, event: Event, ctx: &mut Self::Context) {
        if event.topic != "agent.loop.start" {
            return;
        }
        let goal = event
            .data
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if goal.trim().is_empty() {
            log::warn!("[AgentLoopActor] agent.loop.start without goal, ignored");
            return;
        }
        let peer_id = event
            .data
            .get("peer_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let max_steps = event
            .data
            .get("max_steps")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let max_tool_rounds = event
            .data
            .get("max_tool_rounds")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        match self.start_loop_internal(goal, peer_id, max_steps, max_tool_rounds, ctx) {
            Ok(snap) => log::info!(
                "[AgentLoopActor] started loop '{}' from event (goal={})",
                snap.id,
                snap.goal
            ),
            Err(e) => log::warn!("[AgentLoopActor] event-driven start failed: {}", e),
        }
    }
}

impl Handler<GetAgentLoop> for AgentLoopActor {
    type Result = MessageResult<GetAgentLoop>;

    fn handle(&mut self, msg: GetAgentLoop, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(self.loops.get(&msg.id).map(|state| state.snapshot.clone()))
    }
}

impl Handler<ListAgentLoops> for AgentLoopActor {
    type Result = MessageResult<ListAgentLoops>;

    fn handle(&mut self, _msg: ListAgentLoops, _ctx: &mut Self::Context) -> Self::Result {
        let mut loops: Vec<AgentLoopSnapshot> = self
            .loops
            .values()
            .map(|state| state.snapshot.clone())
            .collect();
        loops.sort_by_key(|item| item.created_at_ms);
        MessageResult(loops)
    }
}

impl Handler<StopAgentLoop> for AgentLoopActor {
    type Result = bool;

    fn handle(&mut self, msg: StopAgentLoop, _ctx: &mut Self::Context) -> Self::Result {
        let Some(state) = self.loops.get_mut(&msg.id) else {
            return false;
        };
        state.stop_flag.store(true, Ordering::SeqCst);
        if state.snapshot.status == AgentLoopStatus::Running {
            state.snapshot.status = AgentLoopStatus::Stopping;
            state.snapshot.updated_at_ms = now_ms();
        }
        // 结束对 state 的可变借用后再持久化
        self.persist_loop(&msg.id);
        true
    }
}

/// 暂停一个运行中的 agent loop。
#[derive(Message)]
#[rtype(result = "bool")]
pub struct PauseAgentLoop {
    pub id: String,
}

/// 恢复一个已暂停的 agent loop。
#[derive(Message)]
#[rtype(result = "bool")]
pub struct ResumeAgentLoop {
    pub id: String,
}

impl Handler<PauseAgentLoop> for AgentLoopActor {
    type Result = bool;

    fn handle(&mut self, msg: PauseAgentLoop, _ctx: &mut Self::Context) -> Self::Result {
        let Some(state) = self.loops.get_mut(&msg.id) else {
            return false;
        };
        // 仅 Running 可暂停
        if state.snapshot.status != AgentLoopStatus::Running {
            return false;
        }
        state.pause_flag.store(true, Ordering::SeqCst);
        state.snapshot.status = AgentLoopStatus::Paused;
        state.snapshot.updated_at_ms = now_ms();
        self.persist_loop(&msg.id);
        true
    }
}

impl Handler<ResumeAgentLoop> for AgentLoopActor {
    type Result = bool;

    fn handle(&mut self, msg: ResumeAgentLoop, _ctx: &mut Self::Context) -> Self::Result {
        let Some(state) = self.loops.get_mut(&msg.id) else {
            return false;
        };
        // 仅 Paused 可恢复
        if state.snapshot.status != AgentLoopStatus::Paused {
            return false;
        }
        state.pause_flag.store(false, Ordering::SeqCst);
        state.snapshot.status = AgentLoopStatus::Running;
        state.snapshot.updated_at_ms = now_ms();
        self.persist_loop(&msg.id);
        true
    }
}

impl Handler<AgentLoopProgress> for AgentLoopActor {
    type Result = ();

    fn handle(&mut self, msg: AgentLoopProgress, _ctx: &mut Self::Context) {
        let Some(state) = self.loops.get_mut(&msg.id) else {
            return;
        };
        if let Some(step) = msg.step {
            state.snapshot.steps_taken = step.step;
            state.snapshot.observations.push(step);
        }
        if let Some(status) = msg.status {
            // 外部控制的 Paused/Stopping 不被 runner 的 Running 心跳覆盖
            let externally_held = matches!(
                state.snapshot.status,
                AgentLoopStatus::Paused | AgentLoopStatus::Stopping
            );
            if !(externally_held && status == AgentLoopStatus::Running) {
                state.snapshot.status = status;
            }
        }
        if let Some(error) = msg.error {
            state.snapshot.error = Some(error);
        }
        state.snapshot.updated_at_ms = now_ms();
        self.persist_loop(&msg.id);
    }
}

struct AgentLoopRunner {
    id: String,
    goal: String,
    peer_id: String,
    max_steps: usize,
    max_tool_rounds: usize,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    retry_addr: Recipient<RetryChatRequest>,
    plugin_manager: Recipient<RefreshSnapshotsForPeer>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
    token_usage_addr: Recipient<RecordTokenUsage>,
    metrics_addr: Addr<MetricsActor>,
    addr: Addr<AgentLoopActor>,
}

impl AgentLoopRunner {
    async fn run(self) {
        let mut observations: Vec<String> = Vec::new();
        let mut final_status = AgentLoopStatus::Completed;
        let mut final_error: Option<String> = None;
        let loop_start = Instant::now();
        let max_duration = agent_loop_max_duration_secs();

        for step_index in 1..=self.max_steps {
            if self.stop_flag.load(Ordering::SeqCst) {
                final_status = AgentLoopStatus::Stopped;
                break;
            }

            if loop_duration_exceeded(loop_start.elapsed().as_secs(), max_duration) {
                final_status = AgentLoopStatus::Failed;
                final_error = Some(format!("exceeded max duration {}s", max_duration));
                break;
            }

            // 暂停：pause_flag 置位且未请求停止时，循环等待恢复
            while self.pause_flag.load(Ordering::SeqCst)
                && !self.stop_flag.load(Ordering::SeqCst)
            {
                actix::clock::sleep(Duration::from_millis(200)).await;
            }
            if self.stop_flag.load(Ordering::SeqCst) {
                final_status = AgentLoopStatus::Stopped;
                break;
            }

            let step_start = Instant::now();
            let result = self.run_step(step_index, &observations).await;
            match result {
                Ok(step_outcome) => {
                    let step = AgentLoopStep {
                        step: step_index,
                        status: step_outcome.status.clone(),
                        llm_message: step_outcome.message.clone(),
                        reason: step_outcome.reason.clone(),
                        tool_calls: step_outcome.tool_calls.clone(),
                        tool_results: step_outcome.tool_results.clone(),
                        elapsed_ms: step_start.elapsed().as_millis() as u64,
                        created_at_ms: now_ms(),
                    };
                    self.addr.do_send(AgentLoopProgress {
                        id: self.id.clone(),
                        step: Some(step.clone()),
                        status: Some(AgentLoopStatus::Running),
                        error: None,
                    });
                    self.event_bus.do_send(Event::new(
                        "agent.loop.step",
                        serde_json::json!({
                            "id": self.id,
                            "step": step_index,
                            "status": step.status,
                            "message": step.llm_message,
                            "tool_calls": step.tool_calls,
                        }),
                        "agent-loop",
                    ));

                    observations.push(format_observation(&step));

                    match step_outcome.status {
                        AgentLoopStatus::Completed => {
                            final_status = AgentLoopStatus::Completed;
                            break;
                        }
                        AgentLoopStatus::WaitingForUser => {
                            final_status = AgentLoopStatus::WaitingForUser;
                            break;
                        }
                        AgentLoopStatus::Failed => {
                            final_status = AgentLoopStatus::Failed;
                            final_error = step_outcome.reason.clone();
                            break;
                        }
                        _ => {}
                    }

                    if let Some(sleep_secs) = step_outcome.sleep_seconds {
                        let sleep_cap = std::env::var("AGENT_LOOP_MAX_SLEEP_SECS")
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(60);
                        let capped = sleep_secs.min(sleep_cap);
                        if capped > 0 {
                            actix::clock::sleep(Duration::from_secs(capped)).await;
                        }
                    }
                }
                Err(error) => {
                    final_status = AgentLoopStatus::Failed;
                    final_error = Some(error.clone());
                    self.addr.do_send(AgentLoopProgress {
                        id: self.id.clone(),
                        step: None,
                        status: Some(AgentLoopStatus::Failed),
                        error: Some(error),
                    });
                    break;
                }
            }
        }

        if self.stop_flag.load(Ordering::SeqCst) {
            final_status = AgentLoopStatus::Stopped;
        }

        self.addr.do_send(AgentLoopProgress {
            id: self.id.clone(),
            step: None,
            status: Some(final_status.clone()),
            error: final_error.clone(),
        });
        self.event_bus.do_send(Event::new(
            "agent.loop.done",
            serde_json::json!({
                "id": self.id,
                "status": final_status,
                "error": final_error,
            }),
            "agent-loop",
        ));
    }

    async fn run_step(
        &self,
        step_index: usize,
        observations: &[String],
    ) -> Result<StepOutcome, String> {
        let _ = self
            .plugin_manager
            .send(RefreshSnapshotsForPeer {
                peer_id: self.peer_id.clone(),
            })
            .await;
        let contexts = self.snapshots.lock().clone();
        let tool_deny = agent_loop_tool_deny();
        let tools = collect_tool_defs(&self.tool_registry, &tool_deny);
        let prompt = build_loop_prompt(&self.goal, observations, step_index, self.max_steps);
        let request_id = format!("agent-loop-{}-s{}", self.id, step_index);

        let mut response = self
            .send_chat(ChatRequest {
                message: prompt,
                peer_id: self.peer_id.clone(),
                tools: tools.clone(),
                skip_store: true,
                contexts,
                jailbreak_index: None,
                image_base64: None,
                video_base64: None,
                video_mime: None,
                file_base64: None,
                file_name: None,
                stream: false,
                request_id: request_id.clone(),
                source: "agent-loop".into(),
                user_name: "agent-loop".into(),
                max_tokens: None,
                original_user_msg: None,
                assistant_tool_calls: vec![],
                tool_results: vec![],
            })
            .await?;

        let mut all_tool_calls: Vec<ToolCall> = Vec::new();
        let mut all_tool_results = Vec::new();
        let mut tool_round = 0;

        while !response.tool_calls.is_empty() && tool_round < self.max_tool_rounds {
            if self.stop_flag.load(Ordering::SeqCst) {
                return Ok(StepOutcome {
                    status: AgentLoopStatus::Stopped,
                    message: "stopped".into(),
                    reason: None,
                    sleep_seconds: None,
                    tool_calls: tool_call_names(&all_tool_calls),
                    tool_results: all_tool_results,
                });
            }

            let round_tool_calls = response.tool_calls.clone();
            let round_results = execute_tool_calls(
                &self.tool_registry,
                &self.metrics_addr,
                &round_tool_calls,
                &tool_deny,
            )
            .await;
            all_tool_calls.extend(round_tool_calls.clone());
            all_tool_results.extend(round_results.clone());

            response = self
                .send_chat(ChatRequest {
                    message: String::new(),
                    peer_id: self.peer_id.clone(),
                    tools: tools.clone(),
                    skip_store: true,
                    contexts: vec![],
                    jailbreak_index: None,
                    image_base64: None,
                    video_base64: None,
                    video_mime: None,
                    file_base64: None,
                    file_name: None,
                    stream: false,
                    request_id: format!("{}-t{}", request_id, tool_round + 1),
                    source: "agent-loop".into(),
                    user_name: "agent-loop".into(),
                    max_tokens: None,
                    original_user_msg: None,
                    assistant_tool_calls: all_tool_calls.clone(),
                    tool_results: all_tool_results.clone(),
                })
                .await?;
            tool_round += 1;
        }

        if !response.tool_calls.is_empty() {
            return Ok(StepOutcome {
                status: AgentLoopStatus::Failed,
                message: "工具调用轮次已达到上限，Agent Loop 已停止。".into(),
                reason: Some("tool round budget exhausted".into()),
                sleep_seconds: None,
                tool_calls: tool_call_names(&all_tool_calls),
                tool_results: all_tool_results,
            });
        }

        let decision = parse_loop_decision(&response.content);
        let status = decision
            .status
            .as_deref()
            .map(status_from_decision)
            .unwrap_or(AgentLoopStatus::Running);
        let message = decision
            .message
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| response.content.clone());

        Ok(StepOutcome {
            status,
            message,
            reason: decision.reason,
            sleep_seconds: decision.sleep_seconds,
            tool_calls: tool_call_names(&all_tool_calls),
            tool_results: all_tool_results,
        })
    }

    async fn send_chat(&self, request: ChatRequest) -> Result<LlmResponse, String> {
        let started = Instant::now();
        match self
            .retry_addr
            .send(RetryChatRequest {
                request,
                max_retries: 2,
            })
            .await
        {
            Ok(Ok(resp)) => {
                self.token_usage_addr
                    .do_send(crate::token_usage_actor::RecordTokenUsage {
                        model: resp.model.clone(),
                        prompt_tokens: resp.prompt_tokens,
                        completion_tokens: resp.completion_tokens,
                        prompt_cache_hit_tokens: resp.prompt_cache_hit_tokens,
                        prompt_cache_miss_tokens: resp.prompt_cache_miss_tokens,
                    });
                self.metrics_addr
                    .do_send(crate::metrics_actor::RecordLlmLatency {
                        seconds: started.elapsed().as_secs_f64(),
                        model: resp.model.clone(),
                        success: true,
                    });
                Ok(resp)
            }
            Ok(Err(error)) => {
                self.metrics_addr
                    .do_send(crate::metrics_actor::RecordLlmLatency {
                        seconds: started.elapsed().as_secs_f64(),
                        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "unknown".into()),
                        success: false,
                    });
                self.metrics_addr
                    .do_send(crate::metrics_actor::RecordError {
                        category: "agent_loop_llm".into(),
                    });
                Err(error)
            }
            Err(error) => Err(format!("agent loop LLM mailbox error: {}", error)),
        }
    }
}

#[derive(Debug)]
struct StepOutcome {
    status: AgentLoopStatus,
    message: String,
    reason: Option<String>,
    sleep_seconds: Option<u64>,
    tool_calls: Vec<String>,
    tool_results: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LoopDecision {
    status: Option<String>,
    message: Option<String>,
    reason: Option<String>,
    sleep_seconds: Option<u64>,
}

/// 读取 Agent Loop 最大并发数（env `AGENT_LOOP_MAX_CONCURRENT`，0=不限）。
fn agent_loop_max_concurrent() -> usize {
    std::env::var("AGENT_LOOP_MAX_CONCURRENT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// 统计活跃（未终态）loop 数：Running / Paused / WaitingForUser / Stopping。
fn active_loop_count(statuses: &[AgentLoopStatus]) -> usize {
    statuses
        .iter()
        .filter(|s| {
            matches!(
                s,
                AgentLoopStatus::Running
                    | AgentLoopStatus::Paused
                    | AgentLoopStatus::WaitingForUser
                    | AgentLoopStatus::Stopping
            )
        })
        .count()
}

/// 是否允许启动新 loop（max=0 表示不限）。
fn can_start_new_loop(active: usize, max_concurrent: usize) -> bool {
    max_concurrent == 0 || active < max_concurrent
}

/// 读取 Agent Loop 墙钟时长上限（env `AGENT_LOOP_MAX_DURATION_SECS`，0=不限）。
fn agent_loop_max_duration_secs() -> u64 {
    std::env::var("AGENT_LOOP_MAX_DURATION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// 墙钟时长是否超限（max_secs=0 表示不限）。
fn loop_duration_exceeded(elapsed_secs: u64, max_secs: u64) -> bool {
    max_secs > 0 && elapsed_secs >= max_secs
}

/// 读取 Agent Loop 工具 denylist（env `AGENT_LOOP_TOOL_DENY`，逗号分隔）。
fn agent_loop_tool_deny() -> Vec<String> {
    std::env::var("AGENT_LOOP_TOOL_DENY")
        .map(|s| parse_tool_deny_list(&s))
        .unwrap_or_default()
}

/// 解析 denylist：逗号分隔、去空白、去空项。
fn parse_tool_deny_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 工具是否被 Agent Loop 策略禁用（精确名匹配）。
fn is_tool_denied(name: &str, deny: &[String]) -> bool {
    deny.iter().any(|d| d == name)
}

fn collect_tool_defs(
    tool_registry: &Arc<Mutex<ToolRegistry>>,
    deny: &[String],
) -> Vec<serde_json::Value> {
    let reg = tool_registry.lock();
    reg
        .all_defs()
        .iter()
        .filter(|d| !d.internal && !is_tool_denied(&d.name, deny))
        .map(|d| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                }
            })
        })
        .collect()
}

async fn execute_tool_calls(
    tool_registry: &Arc<Mutex<ToolRegistry>>,
    metrics_addr: &Addr<MetricsActor>,
    tool_calls: &[ToolCall],
    deny: &[String],
) -> Vec<String> {
    let timeout_secs = crate::tool_exec::tool_timeout_secs();
    let mut results = Vec::new();
    for call in tool_calls {
        if is_tool_denied(&call.name, deny) {
            results.push(format!(
                "【{}】错误：该工具已被 Agent Loop 策略禁用（AGENT_LOOP_TOOL_DENY）",
                call.name
            ));
            continue;
        }
        let executor = {
            let reg = tool_registry.lock();
            reg.get_executor(&call.name)
        };
        let started = Instant::now();
        let result = match executor {
            Some(exec) => {
                crate::tool_exec::execute_with_timeout(
                    exec,
                    call.arguments.clone(),
                    &call.name,
                    timeout_secs,
                )
                .await
            }
            None => ToolResult::err(&format!("tool '{}' not found", call.name)),
        };
        metrics_addr.do_send(crate::metrics_actor::RecordToolCall {
            tool_name: call.name.clone(),
            success: result.success,
            duration_ms: started.elapsed().as_millis() as u64,
        });
        if result.success {
            results.push(format!("【{}】\n{}", call.name, result.content));
        } else {
            results.push(format!(
                "【{}】错误：{}",
                call.name,
                result.error.unwrap_or_else(|| "unknown".into())
            ));
        }
    }
    results
}

fn tool_call_names(tool_calls: &[ToolCall]) -> Vec<String> {
    tool_calls.iter().map(|call| call.name.clone()).collect()
}

fn build_loop_prompt(
    goal: &str,
    observations: &[String],
    step_index: usize,
    max_steps: usize,
) -> String {
    let observation_text = if observations.is_empty() {
        "（暂无）".to_string()
    } else {
        observations.join("\n\n")
    };
    format!(
        "[系统·Agent Loop]\n目标：{}\n当前步数：{}/{}\n已有观察：\n{}\n\n请执行一次 observe -> decide -> act。你可以调用可用工具完成下一步行动。\n如果需要行动，请直接调用工具。工具执行结果会作为 observation 进入下一轮。\n如果不需要工具，请严格输出 JSON，不要包裹 Markdown：\n{{\"status\":\"continue|done|ask_user\",\"message\":\"你对本步的简短观察或最终结论\",\"reason\":\"为什么这样判断\",\"sleep_seconds\":0}}\n状态含义：continue=还要继续；done=目标完成；ask_user=需要用户补充信息。",
        goal, step_index, max_steps, observation_text
    )
}

fn parse_loop_decision(content: &str) -> LoopDecision {
    let cleaned = strip_json_fence(content.trim());
    serde_json::from_str::<LoopDecision>(&cleaned).unwrap_or_else(|_| LoopDecision {
        status: None,
        message: Some(content.trim().to_string()),
        reason: None,
        sleep_seconds: None,
    })
}

fn strip_json_fence(input: &str) -> String {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        return rest.trim().trim_end_matches("```").trim().to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.trim().trim_end_matches("```").trim().to_string();
    }
    trimmed.to_string()
}

fn status_from_decision(status: &str) -> AgentLoopStatus {
    match status.trim().to_lowercase().as_str() {
        "done" | "completed" | "complete" => AgentLoopStatus::Completed,
        "ask_user" | "waiting_for_user" | "need_user" => AgentLoopStatus::WaitingForUser,
        "failed" | "error" => AgentLoopStatus::Failed,
        "stopped" => AgentLoopStatus::Stopped,
        _ => AgentLoopStatus::Running,
    }
}

fn format_observation(step: &AgentLoopStep) -> String {
    let tools = if step.tool_calls.is_empty() {
        "无".to_string()
    } else {
        step.tool_calls.join(", ")
    };
    format!(
        "Step {} [{:?}]\n消息：{}\n工具：{}\n结果：{}",
        step.step,
        step.status,
        step.llm_message,
        tools,
        step.tool_results.join("\n")
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 终态 loop 保留上限（env `AGENT_LOOP_MAX_KEEP`，默认 200）。
fn agent_loop_max_keep() -> usize {
    std::env::var("AGENT_LOOP_MAX_KEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_KEEP)
}

/// 纯逻辑：从 (id, status, updated_at_ms) 列表中挑出超出保留上限、需清理的终态 loop id。
/// 仅清理终态（Completed/Stopped/Failed）；活跃（Running/Paused/Stopping/WaitingForUser）始终保留。
/// 终态中按 updated_at 降序保留最近 `keep` 个，其余返回待删除。
fn terminal_ids_to_prune(items: &[(String, AgentLoopStatus, u64)], keep: usize) -> Vec<String> {
    let mut terminal: Vec<&(String, AgentLoopStatus, u64)> = items
        .iter()
        .filter(|(_, s, _)| {
            matches!(
                s,
                AgentLoopStatus::Completed | AgentLoopStatus::Stopped | AgentLoopStatus::Failed
            )
        })
        .collect();
    if terminal.len() <= keep {
        return Vec::new();
    }
    terminal.sort_by(|a, b| b.2.cmp(&a.2));
    terminal
        .into_iter()
        .skip(keep)
        .map(|(id, _, _)| id.clone())
        .collect()
}

/// Agent Loop 状态名转持久化字符串（与 serde snake_case 一致）。
fn loop_status_str(s: &AgentLoopStatus) -> &'static str {
    match s {
        AgentLoopStatus::Running => "running",
        AgentLoopStatus::Paused => "paused",
        AgentLoopStatus::Completed => "completed",
        AgentLoopStatus::WaitingForUser => "waiting_for_user",
        AgentLoopStatus::Stopping => "stopping",
        AgentLoopStatus::Stopped => "stopped",
        AgentLoopStatus::Failed => "failed",
    }
}

/// 进程重启后，仍处于 running/stopping 的 loop 因 runner 线程已消失需视为中断。
/// 返回 true 表示状态被改写（需要回写持久化）。
fn reconcile_restored_status(snap: &mut AgentLoopSnapshot) -> bool {
    if matches!(
        snap.status,
        AgentLoopStatus::Running | AgentLoopStatus::Stopping | AgentLoopStatus::Paused
    ) {
        snap.status = AgentLoopStatus::Failed;
        if snap.error.is_none() {
            snap.error = Some("interrupted by process restart".into());
        }
        true
    } else {
        false
    }
}

/// Agent Loop 状态的 SQLite 持久化（每个 loop 一行，存完整快照 JSON），
/// 用于进程重启后恢复历史与按 peer 归档。
struct AgentLoopPersist {
    conn: Connection,
}

impl AgentLoopPersist {
    fn open() -> Option<Self> {
        let path = std::env::var("AGENT_LOOP_DB_PATH")
            .unwrap_or_else(|_| "data/agent_loops.db".to_string());
        Self::open_path(&path)
    }

    fn open_path(path: &str) -> Option<Self> {
        let p = std::path::Path::new(path);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let conn = match Connection::open(p) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[AgentLoopActor] persistence disabled: {}", e);
                return None;
            }
        };
        if let Err(e) = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_loops (
                id            TEXT PRIMARY KEY,
                peer_id       TEXT NOT NULL,
                status        TEXT NOT NULL,
                snapshot_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_agent_loops_peer ON agent_loops(peer_id);",
        ) {
            log::warn!("[AgentLoopActor] table init failed: {}", e);
            return None;
        }
        Some(Self { conn })
    }

    fn save(&self, snap: &AgentLoopSnapshot) {
        let json = match serde_json::to_string(snap) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("[AgentLoopActor] serialize loop {} failed: {}", snap.id, e);
                return;
            }
        };
        if let Err(e) = self.conn.execute(
            "INSERT INTO agent_loops (id, peer_id, status, snapshot_json, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                peer_id = ?2, status = ?3, snapshot_json = ?4, updated_at_ms = ?6",
            rusqlite::params![
                snap.id,
                snap.peer_id,
                loop_status_str(&snap.status),
                json,
                snap.created_at_ms as i64,
                snap.updated_at_ms as i64,
            ],
        ) {
            log::warn!("[AgentLoopActor] save loop {} failed: {}", snap.id, e);
        }
    }

    /// 从持久化删除指定 id 的 loop（清理终态旧记录）。
    fn prune(&self, ids: &[String]) {
        for id in ids {
            if let Err(e) = self.conn.execute("DELETE FROM agent_loops WHERE id = ?1", [id]) {
                log::warn!("[AgentLoopActor] prune loop {} failed: {}", id, e);
            }
        }
    }

    fn load_all(&self) -> Vec<AgentLoopSnapshot> {
        let mut stmt = match self
            .conn
            .prepare("SELECT snapshot_json FROM agent_loops ORDER BY created_at_ms")
        {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[AgentLoopActor] prepare load failed: {}", e);
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[AgentLoopActor] query load failed: {}", e);
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for row in rows.flatten() {
            match serde_json::from_str::<AgentLoopSnapshot>(&row) {
                Ok(snap) => out.push(snap),
                Err(e) => log::warn!("[AgentLoopActor] deserialize loop failed: {}", e),
            }
        }
        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_step() -> AgentLoopStep {
        AgentLoopStep {
            step: 2,
            status: AgentLoopStatus::Running,
            llm_message: "hello".into(),
            reason: None,
            tool_calls: vec!["tool_a".into()],
            tool_results: vec!["result_a".into()],
            elapsed_ms: 10,
            created_at_ms: 0,
        }
    }

    fn make_snapshot(id: &str, status: AgentLoopStatus) -> AgentLoopSnapshot {
        AgentLoopSnapshot {
            id: id.into(),
            goal: "g".into(),
            peer_id: "test:1".into(),
            status,
            max_steps: 8,
            max_tool_rounds: 5,
            steps_taken: 0,
            observations: Vec::new(),
            error: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn temp_db_path(tag: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("bn_agent_loop_test_{}_{}.db", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p.to_string_lossy().to_string()
    }

    #[test]
    fn deny_list_parse_trims_and_drops_empty() {
        assert_eq!(parse_tool_deny_list(""), Vec::<String>::new());
        assert_eq!(
            parse_tool_deny_list(" shutdown , , unload_plugin "),
            vec!["shutdown".to_string(), "unload_plugin".to_string()]
        );
    }

    #[test]
    fn deny_list_matches_exact_name() {
        let deny = parse_tool_deny_list("shutdown,danger_tool");
        assert!(is_tool_denied("shutdown", &deny));
        assert!(is_tool_denied("danger_tool", &deny));
        assert!(!is_tool_denied("safe_tool", &deny));
        assert!(!is_tool_denied("shutdown_x", &deny));
    }

    #[test]
    fn deny_list_empty_denies_nothing() {
        let deny: Vec<String> = Vec::new();
        assert!(!is_tool_denied("anything", &deny));
    }

    #[test]
    fn duration_unlimited_when_zero() {
        assert!(!loop_duration_exceeded(0, 0));
        assert!(!loop_duration_exceeded(999_999, 0));
    }

    #[test]
    fn duration_exceeded_at_or_past_limit() {
        assert!(!loop_duration_exceeded(9, 10));
        assert!(loop_duration_exceeded(10, 10));
        assert!(loop_duration_exceeded(11, 10));
    }

    #[test]
    fn concurrent_unlimited_when_zero() {
        assert!(can_start_new_loop(0, 0));
        assert!(can_start_new_loop(9999, 0));
    }

    #[test]
    fn concurrent_blocks_at_limit() {
        assert!(can_start_new_loop(0, 2));
        assert!(can_start_new_loop(1, 2));
        assert!(!can_start_new_loop(2, 2));
        assert!(!can_start_new_loop(3, 2));
    }

    #[test]
    fn active_count_excludes_terminal() {
        let statuses = vec![
            AgentLoopStatus::Running,
            AgentLoopStatus::Paused,
            AgentLoopStatus::Completed,
            AgentLoopStatus::Stopped,
            AgentLoopStatus::Failed,
            AgentLoopStatus::WaitingForUser,
            AgentLoopStatus::Stopping,
        ];
        assert_eq!(active_loop_count(&statuses), 4);
    }

    #[test]
    fn strip_json_fence_plain() {
        assert_eq!(strip_json_fence("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn strip_json_fence_markers() {
        assert_eq!(strip_json_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_json_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
    }

    #[test]
    fn parse_loop_decision_valid() {
        let d = parse_loop_decision("{\"status\":\"done\",\"message\":\"ok\",\"sleep_seconds\":3}");
        assert_eq!(d.status.as_deref(), Some("done"));
        assert_eq!(d.message.as_deref(), Some("ok"));
        assert_eq!(d.sleep_seconds, Some(3));
    }

    #[test]
    fn parse_loop_decision_fenced() {
        let d = parse_loop_decision("```json\n{\"status\":\"continue\",\"message\":\"m\"}\n```");
        assert_eq!(d.status.as_deref(), Some("continue"));
        assert_eq!(d.message.as_deref(), Some("m"));
    }

    #[test]
    fn parse_loop_decision_invalid_falls_back_to_message() {
        let d = parse_loop_decision("just some text, not json");
        assert!(d.status.is_none());
        assert_eq!(d.message.as_deref(), Some("just some text, not json"));
    }

    #[test]
    fn status_from_decision_maps_variants() {
        assert_eq!(status_from_decision("done"), AgentLoopStatus::Completed);
        assert_eq!(status_from_decision("complete"), AgentLoopStatus::Completed);
        assert_eq!(status_from_decision("ask_user"), AgentLoopStatus::WaitingForUser);
        assert_eq!(status_from_decision("need_user"), AgentLoopStatus::WaitingForUser);
        assert_eq!(status_from_decision("failed"), AgentLoopStatus::Failed);
        assert_eq!(status_from_decision("error"), AgentLoopStatus::Failed);
        assert_eq!(status_from_decision("stopped"), AgentLoopStatus::Stopped);
        assert_eq!(status_from_decision("continue"), AgentLoopStatus::Running);
        assert_eq!(status_from_decision("whatever"), AgentLoopStatus::Running);
    }

    #[test]
    fn status_from_decision_case_insensitive_and_trimmed() {
        assert_eq!(status_from_decision("  DONE  "), AgentLoopStatus::Completed);
        assert_eq!(status_from_decision("Ask_User"), AgentLoopStatus::WaitingForUser);
    }

    #[test]
    fn loop_status_str_all() {
        assert_eq!(loop_status_str(&AgentLoopStatus::Running), "running");
        assert_eq!(loop_status_str(&AgentLoopStatus::Completed), "completed");
        assert_eq!(
            loop_status_str(&AgentLoopStatus::WaitingForUser),
            "waiting_for_user"
        );
        assert_eq!(loop_status_str(&AgentLoopStatus::Stopping), "stopping");
        assert_eq!(loop_status_str(&AgentLoopStatus::Stopped), "stopped");
        assert_eq!(loop_status_str(&AgentLoopStatus::Failed), "failed");
    }

    #[test]
    fn format_observation_contains_key_fields() {
        let s = format_observation(&sample_step());
        assert!(s.contains("Step 2"));
        assert!(s.contains("hello"));
        assert!(s.contains("tool_a"));
        assert!(s.contains("result_a"));
    }

    #[test]
    fn format_observation_no_tools() {
        let mut step = sample_step();
        step.tool_calls.clear();
        assert!(format_observation(&step).contains("无"));
    }

    #[test]
    fn build_loop_prompt_includes_goal_step_and_empty_obs() {
        let prompt = build_loop_prompt("my goal", &[], 1, 5);
        assert!(prompt.contains("my goal"));
        assert!(prompt.contains("1/5"));
        assert!(prompt.contains("（暂无）"));
    }

    #[test]
    fn build_loop_prompt_includes_observations() {
        let obs = vec!["obs-1".to_string(), "obs-2".to_string()];
        let prompt = build_loop_prompt("g", &obs, 3, 8);
        assert!(prompt.contains("obs-1"));
        assert!(prompt.contains("obs-2"));
        assert!(prompt.contains("3/8"));
    }

    #[test]
    fn reconcile_marks_running_as_interrupted() {
        let mut s = make_snapshot("a", AgentLoopStatus::Running);
        assert!(reconcile_restored_status(&mut s));
        assert_eq!(s.status, AgentLoopStatus::Failed);
        assert_eq!(s.error.as_deref(), Some("interrupted by process restart"));
    }

    #[test]
    fn reconcile_marks_stopping_as_interrupted() {
        let mut s = make_snapshot("a", AgentLoopStatus::Stopping);
        assert!(reconcile_restored_status(&mut s));
        assert_eq!(s.status, AgentLoopStatus::Failed);
    }

    #[test]
    fn reconcile_keeps_terminal_status() {
        let mut s = make_snapshot("a", AgentLoopStatus::Completed);
        assert!(!reconcile_restored_status(&mut s));
        assert_eq!(s.status, AgentLoopStatus::Completed);
    }

    #[test]
    fn persist_save_and_reload_roundtrip() {
        let path = temp_db_path("roundtrip");
        {
            let p = AgentLoopPersist::open_path(&path).expect("open");
            p.save(&make_snapshot("id-1", AgentLoopStatus::Running));
        }
        // 重新打开（模拟重启）
        let p2 = AgentLoopPersist::open_path(&path).expect("reopen");
        let loaded = p2.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "id-1");
        assert_eq!(loaded[0].status, AgentLoopStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persist_upsert_updates_same_id() {
        let path = temp_db_path("upsert");
        let p = AgentLoopPersist::open_path(&path).expect("open");
        p.save(&make_snapshot("id-x", AgentLoopStatus::Running));
        let mut updated = make_snapshot("id-x", AgentLoopStatus::Completed);
        updated.steps_taken = 5;
        p.save(&updated);
        let loaded = p.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].status, AgentLoopStatus::Completed);
        assert_eq!(loaded[0].steps_taken, 5);
        let _ = std::fs::remove_file(&path);
    }

    // ── Actor 级测试（mock 依赖注入）──────────────────────────────────────

    struct MockLlm {
        response: String,
    }
    impl Actor for MockLlm {
        type Context = Context<Self>;
    }
    impl Handler<RetryChatRequest> for MockLlm {
        type Result = Result<LlmResponse, String>;
        fn handle(&mut self, _msg: RetryChatRequest, _ctx: &mut Self::Context) -> Self::Result {
            Ok(LlmResponse {
                content: self.response.clone(),
                model: "mock".into(),
                prompt_tokens: 1,
                completion_tokens: 1,
                prompt_cache_hit_tokens: 0,
                prompt_cache_miss_tokens: 0,
                tool_calls: vec![],
            })
        }
    }

    struct MockRefresher;
    impl Actor for MockRefresher {
        type Context = Context<Self>;
    }
    impl Handler<RefreshSnapshotsForPeer> for MockRefresher {
        type Result = ();
        fn handle(&mut self, _msg: RefreshSnapshotsForPeer, _ctx: &mut Self::Context) {}
    }

    struct MockTokenUsage;
    impl Actor for MockTokenUsage {
        type Context = Context<Self>;
    }
    impl Handler<RecordTokenUsage> for MockTokenUsage {
        type Result = ();
        fn handle(&mut self, _msg: RecordTokenUsage, _ctx: &mut Self::Context) {}
    }

    /// 构造一个注入 mock 依赖、不落盘（persist=None）的 AgentLoopActor。
    fn build_actor(llm: Recipient<RetryChatRequest>) -> AgentLoopActor {
        AgentLoopActor::from_parts(
            llm,
            MockRefresher.start().recipient(),
            Arc::new(Mutex::new(ToolRegistry::new())),
            Arc::new(Mutex::new(Vec::new())),
            EventBus::new().start(),
            MockTokenUsage.start().recipient(),
            MetricsActor::new().start(),
            None,
        )
    }

    async fn wait_terminal(actor: &Addr<AgentLoopActor>, id: &str) -> AgentLoopStatus {
        let mut status = AgentLoopStatus::Running;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if let Some(s) = actor.send(GetAgentLoop { id: id.to_string() }).await.unwrap() {
                status = s.status.clone();
                if status != AgentLoopStatus::Running {
                    break;
                }
            }
        }
        status
    }

    #[actix_rt::test]
    async fn start_rejects_empty_goal() {
        let llm = MockLlm { response: "{}".into() }.start();
        let actor = build_actor(llm.recipient()).start();
        let res = actor
            .send(StartAgentLoop {
                goal: "   ".into(),
                peer_id: None,
                max_steps: Some(1),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap();
        assert!(res.is_err());
    }

    #[actix_rt::test]
    async fn start_clamps_and_defaults() {
        let llm = MockLlm {
            response: r#"{"status":"done","message":"d"}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        let snap = actor
            .send(StartAgentLoop {
                goal: "  trimmed goal  ".into(),
                peer_id: None,
                max_steps: Some(9999),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snap.goal, "trimmed goal");
        assert_eq!(snap.peer_id, "agent-loop:default");
        assert_eq!(snap.max_steps, MAX_STEPS_CAP);
        assert_eq!(snap.status, AgentLoopStatus::Running);
    }

    #[actix_rt::test]
    async fn loop_completes_with_mock_llm() {
        let llm = MockLlm {
            response: r#"{"status":"done","message":"完成"}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        let snap = actor
            .send(StartAgentLoop {
                goal: "g".into(),
                peer_id: Some("test:peer".into()),
                max_steps: Some(5),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap()
            .unwrap();
        let status = wait_terminal(&actor, &snap.id).await;
        assert_eq!(status, AgentLoopStatus::Completed);
    }

    #[actix_rt::test]
    async fn stop_request_terminates_loop() {
        let llm = MockLlm {
            response: r#"{"status":"continue","message":"x","sleep_seconds":0}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        let snap = actor
            .send(StartAgentLoop {
                goal: "g".into(),
                peer_id: None,
                max_steps: Some(50),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap()
            .unwrap();
        let stopped = actor
            .send(StopAgentLoop {
                id: snap.id.clone(),
            })
            .await
            .unwrap();
        assert!(stopped);
        let status = wait_terminal(&actor, &snap.id).await;
        assert_eq!(status, AgentLoopStatus::Stopped);
    }

    #[actix_rt::test]
    async fn get_and_list_return_loops() {
        let llm = MockLlm {
            response: r#"{"status":"done","message":"d"}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        let snap = actor
            .send(StartAgentLoop {
                goal: "g1".into(),
                peer_id: None,
                max_steps: Some(1),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap()
            .unwrap();
        let got = actor
            .send(GetAgentLoop {
                id: snap.id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(got.map(|s| s.goal), Some("g1".to_string()));
        let list = actor.send(ListAgentLoops).await.unwrap();
        assert!(list.iter().any(|s| s.id == snap.id));
        let none = actor
            .send(GetAgentLoop { id: "nope".into() })
            .await
            .unwrap();
        assert!(none.is_none());
    }

    #[actix_rt::test]
    async fn pause_holds_then_resume_completes() {
        let llm = MockLlm {
            response: r#"{"status":"continue","message":"x","sleep_seconds":0}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        let snap = actor
            .send(StartAgentLoop {
                goal: "g".into(),
                peer_id: None,
                max_steps: Some(50),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap()
            .unwrap();
        let id = snap.id;

        let paused = actor.send(PauseAgentLoop { id: id.clone() }).await.unwrap();
        assert!(paused);

        // 等 runner 进入暂停等待，status 应稳定为 Paused
        tokio::time::sleep(Duration::from_millis(300)).await;
        let s = actor
            .send(GetAgentLoop { id: id.clone() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.status, AgentLoopStatus::Paused);
        let steps_when_paused = s.steps_taken;

        // 暂停期间 steps 不再增长
        tokio::time::sleep(Duration::from_millis(300)).await;
        let s2 = actor
            .send(GetAgentLoop { id: id.clone() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s2.status, AgentLoopStatus::Paused);
        assert_eq!(s2.steps_taken, steps_when_paused);

        let resumed = actor.send(ResumeAgentLoop { id: id.clone() }).await.unwrap();
        assert!(resumed);

        // 恢复后继续推进，continue 跑满 max_steps → Completed
        let status = wait_terminal(&actor, &id).await;
        assert_eq!(status, AgentLoopStatus::Completed);
    }

    #[actix_rt::test]
    async fn pause_resume_rejected_in_wrong_state() {
        let llm = MockLlm {
            response: r#"{"status":"done","message":"d"}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        let snap = actor
            .send(StartAgentLoop {
                goal: "g".into(),
                peer_id: None,
                max_steps: Some(1),
                max_tool_rounds: Some(0),
            })
            .await
            .unwrap()
            .unwrap();
        let _ = wait_terminal(&actor, &snap.id).await;
        // 已 Completed：pause 与 resume 均应拒绝
        let paused = actor
            .send(PauseAgentLoop {
                id: snap.id.clone(),
            })
            .await
            .unwrap();
        assert!(!paused);
        let resumed = actor.send(ResumeAgentLoop { id: snap.id }).await.unwrap();
        assert!(!resumed);
    }

    #[test]
    fn prune_keeps_when_under_limit() {
        let items = vec![
            ("a".to_string(), AgentLoopStatus::Completed, 1),
            ("b".to_string(), AgentLoopStatus::Failed, 2),
        ];
        assert!(terminal_ids_to_prune(&items, 5).is_empty());
    }

    #[test]
    fn prune_removes_oldest_terminal_beyond_keep() {
        let items = vec![
            ("old".to_string(), AgentLoopStatus::Completed, 1),
            ("mid".to_string(), AgentLoopStatus::Stopped, 2),
            ("new".to_string(), AgentLoopStatus::Failed, 3),
        ];
        let removed = terminal_ids_to_prune(&items, 1);
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"old".to_string()));
        assert!(removed.contains(&"mid".to_string()));
        assert!(!removed.contains(&"new".to_string()));
    }

    #[test]
    fn prune_never_removes_active() {
        let items = vec![
            ("run".to_string(), AgentLoopStatus::Running, 1),
            ("pause".to_string(), AgentLoopStatus::Paused, 2),
            ("done1".to_string(), AgentLoopStatus::Completed, 3),
            ("done2".to_string(), AgentLoopStatus::Completed, 4),
        ];
        let removed = terminal_ids_to_prune(&items, 0);
        assert!(removed.contains(&"done1".to_string()));
        assert!(removed.contains(&"done2".to_string()));
        assert!(!removed.contains(&"run".to_string()));
        assert!(!removed.contains(&"pause".to_string()));
    }

    #[actix_rt::test]
    async fn event_driven_start_creates_loop() {
        let llm = MockLlm {
            response: r#"{"status":"done","message":"d"}"#.into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        actor
            .send(Event::new(
                "agent.loop.start",
                serde_json::json!({
                    "goal": "event goal",
                    "peer_id": "test:1",
                    "max_steps": 1,
                    "max_tool_rounds": 0,
                }),
                "test",
            ))
            .await
            .unwrap();
        let list = actor.send(ListAgentLoops).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].goal, "event goal");
        assert_eq!(list[0].peer_id, "test:1");
    }

    #[actix_rt::test]
    async fn event_without_goal_ignored() {
        let llm = MockLlm {
            response: "{}".into(),
        }
        .start();
        let actor = build_actor(llm.recipient()).start();
        actor
            .send(Event::new("agent.loop.start", serde_json::json!({}), "test"))
            .await
            .unwrap();
        assert_eq!(actor.send(ListAgentLoops).await.unwrap().len(), 0);
    }
}
