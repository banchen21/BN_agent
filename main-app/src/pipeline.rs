//! PipelineActor — orchestrates the UserMessage → LLM → tool-calling loop.
//!
//! ## Flow
//!
//! 1. Receive `HandleUserMessage` with user text + source.
//! 2. Check rate limit (via `RateLimitActor`).
//! 3. Generate request_id, register with `CancellationActor`.
//! 4. Refresh plugin snapshots.
//! 5. Collect tool definitions from `ToolRegistry`.
//! 6. Send `ChatRequest` to `RetryActor` (wraps `LlmActor` with retry+circuit-breaker).
//! 7. Record token usage (via `TokenUsageActor`) and metrics (via `MetricsActor`).
//! 8. If the response contains `tool_calls`, execute each tool.
//! 9. Broadcast the assistant reply via `BroadcastEvent`.

use actix::prelude::*;
use plugin_interface::*;
use std::sync::{Arc, Mutex};

use crate::chat_store::{AppendRecord, ChatStoreActor};
use crate::metrics_actor::MetricsActor;
use crate::rate_limit_actor::RateLimitActor;
use crate::retry_actor::{RetryActor, RetryChatRequest};
use crate::token_usage_actor::TokenUsageActor;
use crate::plugin_manager::PluginManager;

// ── Messages ─────────────────────────────────────────────────────────────────

/// Incoming user message from any source (IM plugin, HTTP, etc.).
#[derive(Message)]
#[rtype(result = "()")]
pub struct HandleUserMessage {
    pub text: String,
    pub source: String,
    pub user_name: String,
}

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct PipelineActor {
    retry_addr: Addr<RetryActor>,
    plugin_manager: Addr<PluginManager>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
    rate_limit_addr: Addr<RateLimitActor>,
    token_usage_addr: Addr<TokenUsageActor>,
    metrics_addr: Addr<MetricsActor>,
    store_addr: Addr<ChatStoreActor>,
}

impl PipelineActor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        retry_addr: Addr<RetryActor>,
        plugin_manager: Addr<PluginManager>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        snapshots: Arc<Mutex<Vec<String>>>,
        event_bus: Addr<EventBus>,
        rate_limit_addr: Addr<RateLimitActor>,
        token_usage_addr: Addr<TokenUsageActor>,
        metrics_addr: Addr<MetricsActor>,
        store_addr: Addr<ChatStoreActor>,
    ) -> Self {
        Self {
            retry_addr,
            plugin_manager,
            tool_registry,
            snapshots,
            event_bus,
            rate_limit_addr,
            token_usage_addr,
            metrics_addr,
            store_addr,
        }
    }
}

impl Actor for PipelineActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[PipelineActor] started");
    }
}

// ── Shared message processing ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn process_message(
    text: String,
    source: String,
    user_name: String,
    image_base64: Option<String>,
    video_base64: Option<String>,
    video_mime: Option<String>,
    file_base64: Option<String>,
    file_name: Option<String>,
    retry_addr: Addr<RetryActor>,
    plugin_manager: Addr<PluginManager>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
    rate_limit_addr: Addr<RateLimitActor>,
    token_usage_addr: Addr<TokenUsageActor>,
    metrics_addr: Addr<MetricsActor>,
    store_addr: Addr<ChatStoreActor>,
) {
    // 1. Rate limit check.
    let allowed = rate_limit_addr.send(crate::rate_limit_actor::CheckRateLimit).await
        .unwrap_or(true);
    if !allowed {
        log::warn!("[Pipeline] rate limited");
        emit_reply("⏳ 请求过于频繁，请稍后再试。", &source, &event_bus).await;
        return;
    }

    // 2. Generate request_id and register with cancellation.
    let request_id = uuid::Uuid::new_v4().to_string();

    // 3. Refresh snapshots.
    let _ = plugin_manager.send(RefreshSnapshots).await;
    let contexts: Vec<String> = snapshots.lock().unwrap().clone();

    // 4. Collect tool definitions.
    let tools: Vec<serde_json::Value> = match tool_registry.lock() {
        Ok(reg) => reg.all_defs().iter()
            .filter(|d| !d.internal)
            .map(|d| serde_json::json!({
                "type": "function",
                "function": {
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                }
            }))
            .collect(),
        Err(_) => vec![],
    };

    // 5. Send to RetryActor (streaming enabled).
    let req_start = std::time::Instant::now();
    let retry_msg = RetryChatRequest {
        request: ChatRequest {
            message: text.clone(),
            tools: tools.clone(),
            skip_store: false,
            contexts,
            jailbreak_index: None,
            image_base64,
            video_base64,
            video_mime,
            file_base64,
            file_name,
            stream: true,
            request_id: request_id.clone(),
            source: source.clone(),
            user_name: user_name.clone(),
            max_tokens: None,
            original_user_msg: None,
            assistant_tool_calls: vec![],
            tool_results: vec![],
        },
        max_retries: 3,
    };

    let resp = match retry_addr.send(retry_msg).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log::error!("[Pipeline] LLM error: {}", e);
            // Record error metric.
            metrics_addr.do_send(crate::metrics_actor::RecordError { category: "llm_api".into() });
            emit_reply(&format!("抱歉，出错了：{}", e), &source, &event_bus).await;
            return;
        }
        Err(e) => {
            log::error!("[Pipeline] LLM mailbox error: {}", e);
            return;
        }
    };

    let llm_elapsed = req_start.elapsed();

    // 6. Record token usage.
    token_usage_addr.do_send(crate::token_usage_actor::RecordTokenUsage {
        model: resp.model.clone(),
        prompt_tokens: resp.prompt_tokens,
        completion_tokens: resp.completion_tokens,
        prompt_cache_hit_tokens: resp.prompt_cache_hit_tokens,
        prompt_cache_miss_tokens: resp.prompt_cache_miss_tokens,
    });

    // Record metrics.
    metrics_addr.do_send(crate::metrics_actor::RecordLlmLatency {
        seconds: llm_elapsed.as_secs_f64(),
        model: resp.model.clone(),
        success: true,
    });

    // 7. Tool call loop — 最多 LLM_MAX_TOOL_ROUNDS 轮（默认 20）。
    let max_tool_rounds: usize = std::env::var("LLM_MAX_TOOL_ROUNDS")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(20);

    let mut current_resp = resp;
    let mut tool_round: usize = 0;
    // 累积全部轮次的 tool_calls 和 tool_results，每轮一起传给 LLM。
    let mut all_tool_calls: Vec<ToolCall> = Vec::new();
    let mut all_tool_results: Vec<String> = Vec::new();

    loop {
        if current_resp.tool_calls.is_empty() || tool_round >= max_tool_rounds {
            if !current_resp.content.trim().is_empty() {
                // 最终回复：持久化 + 广播。
                if tool_round > 0 {
                    store_addr.do_send(AppendRecord {
                        role: "assistant".into(),
                        content: current_resp.content.clone(),
                    });
                }
                emit_reply(&current_resp.content, &source, &event_bus).await;
            }
            break;
        }

        log::info!("[Pipeline] tool round {}/{} — {} tool call(s): {:?}",
            tool_round + 1, max_tool_rounds,
            current_resp.tool_calls.len(),
            current_resp.tool_calls.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
        );

        // 执行本轮工具调用。
        let mut round_results: Vec<String> = Vec::new();
        for tc in &current_resp.tool_calls {
            let executor = {
                match tool_registry.lock() {
                    Ok(reg) => reg.get_executor(&tc.name),
                    Err(_) => None,
                }
            };

            let args = tc.arguments.clone();

            let tool_start = std::time::Instant::now();
            let result = match executor {
                Some(exec) => exec.execute(&args),
                None => ToolResult::err(&format!("tool '{}' not found", tc.name)),
            };
            let tool_ms = tool_start.elapsed().as_millis() as u64;

            metrics_addr.do_send(crate::metrics_actor::RecordToolCall {
                tool_name: tc.name.clone(),
                success: result.success,
                duration_ms: tool_ms,
            });

            if result.success {
                log::info!("[Pipeline] tool '{}' ok ({}ms): {}", tc.name, tool_ms, result.content);
                round_results.push(format!("【{}】\n{}", tc.name, result.content));
            } else {
                let err = result.error.as_deref().unwrap_or("unknown");
                log::warn!("[Pipeline] tool '{}' failed ({}ms): {}", tc.name, tool_ms, err);
                round_results.push(format!("【{}】错误：{}", tc.name, err));
            }
        }

        // 累积到历史。
        all_tool_calls.extend(current_resp.tool_calls.clone());
        all_tool_results.extend(round_results);

        // 喂回 LLM（带上所有历史 tool_calls + results）。
        let req_start = std::time::Instant::now();
        let next = retry_addr.send(RetryChatRequest {
            request: ChatRequest {
                message: String::new(),
                tools: tools.clone(),
                skip_store: true,
                original_user_msg: None,
                contexts: vec![],
                jailbreak_index: None,
                image_base64: None,
                video_base64: None,
                video_mime: None,
                file_base64: None,
                file_name: None,
                stream: true,
                request_id: format!("{}-t{}", request_id, tool_round + 1),
                source: String::new(),
                user_name: String::new(),
                max_tokens: None,
                assistant_tool_calls: all_tool_calls.clone(),
                tool_results: all_tool_results.clone(),
            },
            max_retries: 2,
        }).await;

        match next {
            Ok(Ok(r)) => {
                let elapsed = req_start.elapsed();
                token_usage_addr.do_send(crate::token_usage_actor::RecordTokenUsage {
                    model: r.model.clone(),
                    prompt_tokens: r.prompt_tokens,
                    completion_tokens: r.completion_tokens,
                    prompt_cache_hit_tokens: r.prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens: r.prompt_cache_miss_tokens,
                });
                metrics_addr.do_send(crate::metrics_actor::RecordLlmLatency {
                    seconds: elapsed.as_secs_f64(),
                    model: r.model.clone(),
                    success: true,
                });
                current_resp = r;
                tool_round += 1;
            }
            Ok(Err(e)) => {
                log::error!("[Pipeline] tool round {} LLM error: {}", tool_round + 1, e);
                metrics_addr.do_send(crate::metrics_actor::RecordError { category: "llm_api".into() });
                break;
            }
            Err(e) => {
                log::error!("[Pipeline] tool round {} mailbox error: {:?}", tool_round + 1, e);
                break;
            }
        }
    }
}

// ── Handler: HandleUserMessage ───────────────────────────────────────────────

impl Handler<HandleUserMessage> for PipelineActor {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: HandleUserMessage, _ctx: &mut Self::Context) -> Self::Result {
        let retry_addr = self.retry_addr.clone();
        let plugin_manager = self.plugin_manager.clone();
        let tool_registry = self.tool_registry.clone();
        let snapshots = self.snapshots.clone();
        let event_bus = self.event_bus.clone();
        let rate_limit_addr = self.rate_limit_addr.clone();
        let token_usage_addr = self.token_usage_addr.clone();
        let metrics_addr = self.metrics_addr.clone();
        let store_addr = self.store_addr.clone();
        let text = msg.text;
        let source = msg.source;
        let user_name = msg.user_name;

        let fut = async move {
            log::info!("[Pipeline] @{}: {}", user_name, text);
            process_message(
                text, source, user_name,
                None, None, None, None, None,
                retry_addr, plugin_manager, tool_registry, snapshots,
                event_bus, rate_limit_addr, token_usage_addr,
                metrics_addr, store_addr,
            ).await;
        }.into_actor(self).map(|_, _ctx: &mut Self, _| ());

        Box::pin(fut)
    }
}

// ── Handler: Event (bridges EventBus → process_message) ──────────────────────

impl Handler<Event> for PipelineActor {
    type Result = ();

    fn handle(&mut self, event: Event, _ctx: &mut Self::Context) {
        if event.topic != "user.message" {
            return;
        }

        let text = event.data.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let user_name = event.data.get("user_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let image_base64 = event.data.get("image_base64").and_then(|v| v.as_str()).map(|s| s.to_string());
        let video_base64 = event.data.get("video_base64").and_then(|v| v.as_str()).map(|s| s.to_string());
        let video_mime = event.data.get("video_mime").and_then(|v| v.as_str()).map(|s| s.to_string());
        let file_base64 = event.data.get("file_base64").and_then(|v| v.as_str()).map(|s| s.to_string());
        let file_name = event.data.get("file_name").and_then(|v| v.as_str()).map(|s| s.to_string());

        if text.is_empty() && image_base64.is_none() && video_base64.is_none() && file_base64.is_none() {
            return;
        }

        let retry_addr = self.retry_addr.clone();
        let plugin_manager = self.plugin_manager.clone();
        let tool_registry = self.tool_registry.clone();
        let snapshots = self.snapshots.clone();
        let event_bus = self.event_bus.clone();
        let rate_limit_addr = self.rate_limit_addr.clone();
        let token_usage_addr = self.token_usage_addr.clone();
        let metrics_addr = self.metrics_addr.clone();
        let store_addr = self.store_addr.clone();
        actix::spawn(async move {
            if !text.is_empty() {
                log::info!("[Pipeline] @{}: {}", user_name, text);
            }
            if image_base64.is_some() {
                log::info!("[Pipeline] @{} sent a photo", user_name);
            }

            process_message(
                text, source, user_name,
                image_base64, video_base64, video_mime, file_base64, file_name,
                retry_addr, plugin_manager, tool_registry, snapshots,
                event_bus, rate_limit_addr, token_usage_addr,
                metrics_addr, store_addr,
            ).await;
        });
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

async fn emit_reply(
    text: &str,
    source: &str,
    event_bus: &Addr<EventBus>,
) {
    let reply = Event::new(
        "route.message",
        serde_json::json!({ "text": text, "source": source }),
        "pipeline",
    );
    event_bus.do_send(reply);
}
