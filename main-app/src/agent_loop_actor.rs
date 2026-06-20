//! AgentLoopActor — long-running goal loop for observe/decide/act cycles.
//!
//! This is intentionally separate from PipelineActor: chat replies stay reactive,
//! while agent loops run as explicit goal-driven jobs with budgets and status APIs.

use actix::prelude::*;
use plugin_interface::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::metrics_actor::MetricsActor;
use crate::plugin_manager::PluginManager;
use crate::retry_actor::{RetryActor, RetryChatRequest};
use crate::token_usage_actor::TokenUsageActor;

const DEFAULT_MAX_STEPS: usize = 8;
const DEFAULT_MAX_TOOL_ROUNDS: usize = 5;
const MAX_STEPS_CAP: usize = 50;
const MAX_TOOL_ROUNDS_CAP: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentLoopStatus {
    Running,
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
}

pub struct AgentLoopActor {
    retry_addr: Addr<RetryActor>,
    plugin_manager: Addr<PluginManager>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
    token_usage_addr: Addr<TokenUsageActor>,
    metrics_addr: Addr<MetricsActor>,
    loops: HashMap<String, AgentLoopState>,
}

impl AgentLoopActor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        retry_addr: Addr<RetryActor>,
        plugin_manager: Addr<PluginManager>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        snapshots: Arc<Mutex<Vec<String>>>,
        event_bus: Addr<EventBus>,
        token_usage_addr: Addr<TokenUsageActor>,
        metrics_addr: Addr<MetricsActor>,
    ) -> Self {
        Self {
            retry_addr,
            plugin_manager,
            tool_registry,
            snapshots,
            event_bus,
            token_usage_addr,
            metrics_addr,
            loops: HashMap::new(),
        }
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

impl Handler<StartAgentLoop> for AgentLoopActor {
    type Result = Result<AgentLoopSnapshot, String>;

    fn handle(&mut self, msg: StartAgentLoop, ctx: &mut Self::Context) -> Self::Result {
        let goal = msg.goal.trim().to_string();
        if goal.is_empty() {
            return Err("goal is required".into());
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        let peer_id = msg.peer_id.unwrap_or_else(|| "agent-loop:default".into());
        let max_steps = msg
            .max_steps
            .unwrap_or(DEFAULT_MAX_STEPS)
            .clamp(1, MAX_STEPS_CAP);
        let max_tool_rounds = msg
            .max_tool_rounds
            .unwrap_or(DEFAULT_MAX_TOOL_ROUNDS)
            .clamp(0, MAX_TOOL_ROUNDS_CAP);
        let stop_flag = Arc::new(AtomicBool::new(false));

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
            },
        );

        let runner = AgentLoopRunner {
            id: id.clone(),
            goal,
            peer_id,
            max_steps,
            max_tool_rounds,
            stop_flag,
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

        Ok(snapshot)
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
            state.snapshot.status = status;
        }
        if let Some(error) = msg.error {
            state.snapshot.error = Some(error);
        }
        state.snapshot.updated_at_ms = now_ms();
    }
}

struct AgentLoopRunner {
    id: String,
    goal: String,
    peer_id: String,
    max_steps: usize,
    max_tool_rounds: usize,
    stop_flag: Arc<AtomicBool>,
    retry_addr: Addr<RetryActor>,
    plugin_manager: Addr<PluginManager>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
    token_usage_addr: Addr<TokenUsageActor>,
    metrics_addr: Addr<MetricsActor>,
    addr: Addr<AgentLoopActor>,
}

impl AgentLoopRunner {
    async fn run(self) {
        let mut observations: Vec<String> = Vec::new();
        let mut final_status = AgentLoopStatus::Completed;
        let mut final_error: Option<String> = None;

        for step_index in 1..=self.max_steps {
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
        let contexts = self.snapshots.lock().unwrap().clone();
        let tools = collect_tool_defs(&self.tool_registry);
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
            let round_results =
                execute_tool_calls(&self.tool_registry, &self.metrics_addr, &round_tool_calls);
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

fn collect_tool_defs(tool_registry: &Arc<Mutex<ToolRegistry>>) -> Vec<serde_json::Value> {
    match tool_registry.lock() {
        Ok(reg) => reg
            .all_defs()
            .iter()
            .filter(|d| !d.internal)
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
            .collect(),
        Err(_) => vec![],
    }
}

fn execute_tool_calls(
    tool_registry: &Arc<Mutex<ToolRegistry>>,
    metrics_addr: &Addr<MetricsActor>,
    tool_calls: &[ToolCall],
) -> Vec<String> {
    let mut results = Vec::new();
    for call in tool_calls {
        let executor = match tool_registry.lock() {
            Ok(reg) => reg.get_executor(&call.name),
            Err(_) => None,
        };
        let started = Instant::now();
        let result = match executor {
            Some(exec) => exec.execute(&call.arguments),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_decision() {
        let decision = parse_loop_decision(
            r#"{"status":"done","message":"完成","reason":"ok","sleep_seconds":0}"#,
        );

        assert_eq!(decision.status.as_deref(), Some("done"));
        assert_eq!(decision.message.as_deref(), Some("完成"));
    }

    #[test]
    fn strips_json_fence() {
        let cleaned = strip_json_fence("```json\n{\"status\":\"continue\"}\n```");

        assert_eq!(cleaned, "{\"status\":\"continue\"}");
    }
}
