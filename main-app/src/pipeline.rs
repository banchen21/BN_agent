//! PipelineActor — orchestrates the UserMessage → LLM → tool-calling loop.
//!
//! ## Flow
//!
//! 1. Receive `HandleUserMessage` with user text + chat_id + source.
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

use crate::cancellation_actor::CancellationActor;
use crate::metrics_actor::MetricsActor;
use crate::chat_store::{ChatStoreActor};
use crate::rate_limit_actor::RateLimitActor;
use crate::retry_actor::{RetryActor, RetryChatRequest};
use crate::token_usage_actor::TokenUsageActor;
use crate::plugin_manager::PluginManager;

// ── Messages ─────────────────────────────────────────────────────────────────

/// Incoming user message from any source (IM plugin, HTTP, etc.).
#[derive(Message)]
#[rtype(result = "()")]
pub struct HandleUserMessage {
    pub chat_id: i64,
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
    cancellation_addr: Addr<CancellationActor>,
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
        cancellation_addr: Addr<CancellationActor>,
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
            cancellation_addr,
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
    chat_id: i64,
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
    cancellation_addr: Addr<CancellationActor>,
    store_addr: Addr<ChatStoreActor>,
) {
    // 1. Rate limit check.
    let allowed = rate_limit_addr.send(crate::rate_limit_actor::CheckRateLimit { chat_id }).await
        .unwrap_or(true);
    if !allowed {
        log::warn!("[Pipeline] chat_id={} rate limited", chat_id);
        emit_reply(chat_id, "⏳ 请求过于频繁，请稍后再试。", &source, &event_bus, &plugin_manager).await;
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

    let tool_count = tools.len();

    // 5. Send to RetryActor (streaming enabled).
    let req_start = std::time::Instant::now();
    let retry_msg = RetryChatRequest {
        request: ChatRequest {
            chat_id,
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
        },
        max_retries: 3,
    };

    let resp = match retry_addr.send(retry_msg).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log::error!("[Pipeline] LLM error: {}", e);
            // Record error metric.
            metrics_addr.do_send(crate::metrics_actor::RecordError { category: "llm_api".into() });
            emit_reply(chat_id, &format!("抱歉，出错了：{}", e), &source, &event_bus, &plugin_manager).await;
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
        chat_id,
        model: resp.model.clone(),
        prompt_tokens: resp.prompt_tokens,
        completion_tokens: resp.completion_tokens,
    });

    // Record metrics.
    metrics_addr.do_send(crate::metrics_actor::RecordLlmLatency {
        seconds: llm_elapsed.as_secs_f64(),
        model: resp.model.clone(),
        success: true,
    });

    // 7. Execute tool calls.
    if !resp.tool_calls.is_empty() {
        log::info!("[Pipeline] {} tool call(s): {:?}",
            resp.tool_calls.len(),
            resp.tool_calls.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
        );

        let mut tool_results: Vec<String> = Vec::new();
        for tc in &resp.tool_calls {
            let executor = {
                match tool_registry.lock() {
                    Ok(reg) => reg.get_executor(&tc.name),
                    Err(_) => None,
                }
            };

            let mut args = tc.arguments.clone();
            if let serde_json::Value::Object(ref mut map) = args {
                map.entry("chat_id").or_insert(serde_json::json!(chat_id));
            }

            let tool_start = std::time::Instant::now();
            let result = match executor {
                Some(exec) => exec.execute(&args),
                None => ToolResult::err(&format!("tool '{}' not found", tc.name)),
            };
            let tool_ms = tool_start.elapsed().as_millis() as u64;

            // Record tool metrics.
            metrics_addr.do_send(crate::metrics_actor::RecordToolCall {
                tool_name: tc.name.clone(),
                success: result.success,
                duration_ms: tool_ms,
            });

            if result.success {
                log::info!("[Pipeline] tool '{}' ok ({}ms): {}", tc.name, tool_ms, result.content);
                tool_results.push(format!("【{}】\n{}", tc.name, result.content));
            } else {
                let err = result.error.as_deref().unwrap_or("unknown");
                log::warn!("[Pipeline] tool '{}' failed ({}ms): {}", tc.name, tool_ms, err);
                tool_results.push(format!("【{}】错误：{}", tc.name, err));
            }
        }

        // 工具结果喂回 LLM 进行第二轮推理.
        let follow_up = format!(
            "以下是我调用的工具的执行结果，请根据这些结果组织回复：\n\n{}",
            tool_results.join("\n\n")
        );

        let req_start2 = std::time::Instant::now();
        let final_resp = retry_addr.send(RetryChatRequest {
            request: ChatRequest {
                chat_id,
                message: follow_up,
                tools: vec![],
                skip_store: false,
                contexts: vec![],
                jailbreak_index: None,
                image_base64: None,
                video_base64: None,
                video_mime: None,
                file_base64: None,
                file_name: None,
                stream: true,
                request_id: format!("{}-followup", request_id),
                source: String::new(),
                user_name: String::new(),
            },
            max_retries: 2,
        }).await;

        match final_resp {
            Ok(Ok(r)) if !r.content.trim().is_empty() => {
                // Record token usage for the follow-up.
                token_usage_addr.do_send(crate::token_usage_actor::RecordTokenUsage {
                    chat_id,
                    model: r.model.clone(),
                    prompt_tokens: r.prompt_tokens,
                    completion_tokens: r.completion_tokens,
                });
                metrics_addr.do_send(crate::metrics_actor::RecordLlmLatency {
                    seconds: req_start2.elapsed().as_secs_f64(),
                    model: r.model,
                    success: true,
                });


                emit_reply(chat_id, &r.content, &source, &event_bus, &plugin_manager).await;
            }
            Ok(Err(e)) => {
                log::error!("[Pipeline] tool loop LLM error: {}", e);
                metrics_addr.do_send(crate::metrics_actor::RecordError { category: "llm_api".into() });
            }
            Err(e) => {
                log::error!("[Pipeline] tool loop mailbox error: {}", e);
            }
            _ => {}
        }
    } else if !resp.content.trim().is_empty() {
        // 8. Broadcast assistant reply.
        emit_reply(chat_id, &resp.content, &source, &event_bus, &plugin_manager).await;
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
        let cancellation_addr = self.cancellation_addr.clone();
        let store_addr = self.store_addr.clone();

        let text = msg.text;
        let chat_id = msg.chat_id;
        let source = msg.source;
        let user_name = msg.user_name;

        let fut = async move {
            log::info!("[Pipeline] @{}: {}", user_name, text);
            process_message(
                chat_id, text, source, user_name,
                None, None, None, None, None,
                retry_addr, plugin_manager, tool_registry, snapshots,
                event_bus, rate_limit_addr, token_usage_addr,
                metrics_addr, cancellation_addr, store_addr,
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

        let chat_id = event.data.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0);
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
        let cancellation_addr = self.cancellation_addr.clone();
        let store_addr = self.store_addr.clone();

        actix::spawn(async move {
            if !text.is_empty() {
                log::info!("[Pipeline] @{}: {}", user_name, text);
            }
            if image_base64.is_some() {
                log::info!("[Pipeline] @{} sent a photo", user_name);
            }

            process_message(
                chat_id, text, source, user_name,
                image_base64, video_base64, video_mime, file_base64, file_name,
                retry_addr, plugin_manager, tool_registry, snapshots,
                event_bus, rate_limit_addr, token_usage_addr,
                metrics_addr, cancellation_addr, store_addr,
            ).await;
        });
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

async fn emit_reply(
    chat_id: i64,
    text: &str,
    source: &str,
    event_bus: &Addr<EventBus>,
    _plugin_manager: &Addr<PluginManager>,
) {
    let reply = Event::new(
        "assistant.message",
        serde_json::json!({ "chat_id": chat_id, "text": text, "source": source }),
        "pipeline",
    );
    event_bus.do_send(reply);
}
