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
use std::sync::Arc;
use parking_lot::Mutex;

use crate::chat_store::{AppendJsonMessage, ChatStoreActor};
use crate::metrics_actor::MetricsActor;
use crate::plugin_manager::PluginManager;
use crate::rate_limit_actor::RateLimitActor;
use crate::retry_actor::{RetryActor, RetryChatRequest};
use crate::token_usage_actor::TokenUsageActor;

const NO_PROACTIVE_MARKER: &str = "[NO_PROACTIVE]";

// ── Messages ─────────────────────────────────────────────────────────────────

/// Incoming user message from any source (IM plugin, HTTP, etc.).
#[derive(Message)]
#[rtype(result = "()")]
pub struct HandleUserMessage {
    pub text: String,
    pub source: String,
    pub user_name: String,
    pub peer_id: String,
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
    peer_id: String,
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
    // 存库时用的真实 user 文本：传 Some(String::new()) 可让本次 user 消息不入库（主动触发用）
    original_user_msg: Option<String>,
    allow_tools: bool,
    defer_store: bool,
    drop_reply_marker: Option<String>,
) {
    // 1. Rate limit check.
    let allowed = rate_limit_addr
        .send(crate::rate_limit_actor::CheckRateLimit)
        .await
        .unwrap_or(true);
    if !allowed {
        log::warn!("[Pipeline] rate limited");
        emit_reply(
            "⏳ 请求过于频繁，请稍后再试。",
            &source,
            &peer_id,
            &event_bus,
        )
        .await;
        return;
    }

    // 1b. Token budget check.
    if let Ok(check) = token_usage_addr
        .send(crate::token_usage_actor::CheckTokenBudget)
        .await
    {
        if !check.allowed {
            let period = check.period.as_deref().unwrap_or("?");
            log::warn!(
                "[Pipeline] token budget exceeded ({}): {}/{}",
                period,
                check.used,
                check.limit
            );
            emit_reply(
                &format!(
                    "🪙 Token 额度（{}）已用尽（{}/{}），请稍后再试。",
                    period, check.used, check.limit
                ),
                &source,
                &peer_id,
                &event_bus,
            )
            .await;
            return;
        }
    }

    // 2. Generate request_id and register with cancellation.
    let request_id = uuid::Uuid::new_v4().to_string();

    // 3. Refresh peer-scoped snapshots.
    let _ = plugin_manager
        .send(RefreshSnapshotsForPeer {
            peer_id: peer_id.clone(),
        })
        .await;
    let contexts: Vec<String> = snapshots.lock().clone();

    // 4. Collect tool definitions.
    let tools: Vec<serde_json::Value> = if allow_tools {
        let reg = tool_registry.lock();
        reg
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
            .collect()
    } else {
        vec![]
    };

    // 5. Send to RetryActor (streaming enabled).
    let req_start = std::time::Instant::now();
    let retry_msg = RetryChatRequest {
        request: ChatRequest {
            message: text.clone(),
            peer_id: peer_id.clone(),
            tools: tools.clone(),
            skip_store: defer_store,
            contexts: contexts.clone(),
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
            original_user_msg,
            assistant_tool_calls: vec![],
            tool_results: vec![],
        },
        max_retries: 3,
    };

    let resp = match retry_addr.send(retry_msg).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log::error!("[Pipeline] LLM error: {}", e);
            metrics_addr.do_send(crate::metrics_actor::RecordLlmLatency {
                seconds: req_start.elapsed().as_secs_f64(),
                model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "unknown".into()),
                success: false,
            });
            metrics_addr.do_send(crate::metrics_actor::RecordError {
                category: "llm_api".into(),
            });
            emit_reply(
                &format!("抱歉，出错了：{}", e),
                &source,
                &peer_id,
                &event_bus,
            )
            .await;
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
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let mut current_resp = resp;
    let mut tool_round: usize = 0;
    // 累积全部轮次的 tool_calls 和 tool_results，每轮一起传给 LLM。
    let mut all_tool_calls: Vec<ToolCall> = Vec::new();
    let mut all_tool_results: Vec<String> = Vec::new();
    // 是否已通过 IM 插件工具直接发送了消息（避免 emit_reply 重复发送）
    let mut already_sent_via_im = false;

    loop {
        if current_resp.tool_calls.is_empty() || tool_round >= max_tool_rounds {
            let final_text = current_resp.content.trim();
            let drop_final_reply = drop_reply_marker
                .as_deref()
                .map(|marker| final_text == marker)
                .unwrap_or(false);

            // 持久化：工具调用链（不论最终有无文本回复，都要保存）
            if tool_round > 0 {
                // 保存助手 tool_calls 消息（作为 tool role 结果的前导）
                let tc_array: Vec<serde_json::Value> = all_tool_calls.iter().map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()
                        }
                    })
                }).collect();
                store_addr.do_send(AppendJsonMessage {
                    peer_id: peer_id.clone(),
                    message_json: serde_json::json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": tc_array
                    })
                    .to_string(),
                });
                // 保存每轮工具调用的结果（tool role 消息）
                for (i, tc) in all_tool_calls.iter().enumerate() {
                    let result_text = all_tool_results.get(i).cloned().unwrap_or_default();
                    let tool_msg = serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "content": result_text,
                    });
                    store_addr.do_send(AppendJsonMessage {
                        peer_id: peer_id.clone(),
                        message_json: tool_msg.to_string(),
                    });
                }
                // 保存最终助理文本回复（可能为空，但确保链完整）
                store_addr.do_send(AppendJsonMessage {
                    peer_id: peer_id.clone(),
                    message_json: serde_json::json!({
                        "role": "assistant",
                        "content": if current_resp.content.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(current_resp.content.clone())
                        }
                    })
                    .to_string(),
                });
            }
            if defer_store
                && tool_round == 0
                && !current_resp.content.trim().is_empty()
                && !drop_final_reply
            {
                store_addr.do_send(AppendJsonMessage {
                    peer_id: peer_id.clone(),
                    message_json: serde_json::json!({
                        "role": "assistant",
                        "content": current_resp.content.clone()
                    })
                    .to_string(),
                });
            }
            // 广播最终回复（仅当有文本且未通过 IM 工具直接发送时）
            if !current_resp.content.trim().is_empty() && !already_sent_via_im && !drop_final_reply
            {
                emit_reply(&current_resp.content, &source, &peer_id, &event_bus).await;
            }
            // 如果已通过 IM 工具发送，发静默事件通知插件对话已完成
            if already_sent_via_im && !drop_final_reply {
                let chat_id = chat_id_from_peer_id(&peer_id, &source);
                event_bus.do_send(Event::new(
                    "assistant.message",
                    serde_json::json!({
                        "text": current_resp.content,
                        "source": source,
                        "peer_id": peer_id,
                        "chat_id": chat_id,
                        "silent": true
                    }),
                    "pipeline",
                ));
            }
            break;
        }

        log::info!(
            "[Pipeline] tool round {}/{} — {} tool call(s): {:?}",
            tool_round + 1,
            max_tool_rounds,
            current_resp.tool_calls.len(),
            current_resp
                .tool_calls
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        );

        // 执行本轮工具调用。
        let mut round_results: Vec<String> = Vec::new();
        for tc in &current_resp.tool_calls {
            let executor = {
                let reg = tool_registry.lock();
                reg.get_executor(&tc.name)
            };

            let args = tc.arguments.clone();

            let tool_start = std::time::Instant::now();
            let result = match executor {
                Some(exec) => {
                    crate::tool_exec::execute_with_timeout(
                        exec,
                        args,
                        &tc.name,
                        crate::tool_exec::tool_timeout_secs(),
                    )
                    .await
                }
                None => ToolResult::err(&format!("tool '{}' not found", tc.name)),
            };
            let tool_ms = tool_start.elapsed().as_millis() as u64;

            metrics_addr.do_send(crate::metrics_actor::RecordToolCall {
                tool_name: tc.name.clone(),
                success: result.success,
                duration_ms: tool_ms,
            });

            if result.success {
                log::info!(
                    "[Pipeline] tool '{}' ok ({}ms): {}",
                    tc.name,
                    tool_ms,
                    result.content
                );
                round_results.push(format!("【{}】\n{}", tc.name, result.content));

                // 标记：IM 插件工具已直接发送消息到用户，避免 emit_reply 重复
                if tc.name.starts_with("tg_send_")
                    || tc.name == "feishu_send_message"
                    || tc.name == "wechat_send_message"
                {
                    already_sent_via_im = true;
                }

                if tc.name == "generate_image" {
                    let desc_args = result.metadata.as_ref().and_then(|m| {
                        let b64 = m.get("image_base64")?.as_str()?;
                        if b64.is_empty() {
                            return None;
                        }
                        Some(serde_json::json!({
                            "image_base64": b64,
                            "mime_type": m.get("mime_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("image/png"),
                        }))
                    });
                    if let Some(desc_args) = desc_args {
                        {
                            let reg = tool_registry.lock();
                            if let Some(desc_exec) = reg.get_executor("image_describe") {
                                let desc_result = desc_exec.execute(&desc_args);
                                if desc_result.success {
                                    log::info!(
                                        "[Pipeline] auto image_describe ok: {}",
                                        desc_result.content
                                    );
                                    round_results
                                        .push(format!("【自动图片理解】\n{}", desc_result.content));
                                } else if let Some(err) = &desc_result.error {
                                    log::warn!("[Pipeline] auto image_describe failed: {}", err);
                                }
                            }
                        }
                    }
                }
            } else {
                let err = result.error.as_deref().unwrap_or("unknown");
                log::warn!(
                    "[Pipeline] tool '{}' failed ({}ms): {}",
                    tc.name,
                    tool_ms,
                    err
                );
                round_results.push(format!("【{}】错误：{}", tc.name, err));
            }
        }

        // 累积到历史。
        all_tool_calls.extend(current_resp.tool_calls.clone());
        all_tool_results.extend(round_results);

        // 喂回 LLM（带上所有历史 tool_calls + results）。
        let req_start = std::time::Instant::now();
        let next = retry_addr
            .send(RetryChatRequest {
                request: ChatRequest {
                    message: String::new(),
                    peer_id: peer_id.clone(),
                    tools: tools.clone(),
                    skip_store: true,
                    original_user_msg: None,
                    contexts: contexts.clone(),
                    jailbreak_index: None,
                    image_base64: None,
                    video_base64: None,
                    video_mime: None,
                    file_base64: None,
                    file_name: None,
                    stream: true,
                    request_id: format!("{}-t{}", request_id, tool_round + 1),
                    source: source.clone(),
                    user_name: user_name.clone(),
                    max_tokens: None,
                    assistant_tool_calls: all_tool_calls.clone(),
                    tool_results: all_tool_results.clone(),
                },
                max_retries: 2,
            })
            .await;

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
                metrics_addr.do_send(crate::metrics_actor::RecordLlmLatency {
                    seconds: req_start.elapsed().as_secs_f64(),
                    model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "unknown".into()),
                    success: false,
                });
                metrics_addr.do_send(crate::metrics_actor::RecordError {
                    category: "llm_api".into(),
                });
                break;
            }
            Err(e) => {
                log::error!(
                    "[Pipeline] tool round {} mailbox error: {:?}",
                    tool_round + 1,
                    e
                );
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
        let peer_id = msg.peer_id;

        let fut = async move {
            log::info!("[Pipeline] @{}: {}", user_name, text);
            process_message(
                text,
                source,
                peer_id,
                user_name,
                None,
                None,
                None,
                None,
                None,
                retry_addr,
                plugin_manager,
                tool_registry,
                snapshots,
                event_bus,
                rate_limit_addr,
                token_usage_addr,
                metrics_addr,
                store_addr,
                None,
                true,
                false,
                None,
            )
            .await;
        }
        .into_actor(self)
        .map(|_, _ctx: &mut Self, _| ());

        Box::pin(fut)
    }
}

// ── Handler: Event (bridges EventBus → process_message) ──────────────────────

impl Handler<Event> for PipelineActor {
    type Result = ();

    fn handle(&mut self, event: Event, _ctx: &mut Self::Context) {
        // ── 主动触发：到期后回调 LLM，按当前上下文实时生成主动消息 ──
        if event.topic == "proactive.trigger" {
            let source = event
                .data
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let peer_id = event
                .data
                .get("peer_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| derive_peer_id(&event.data, &source))
                .unwrap_or_else(|| {
                    if source.is_empty() {
                        "system:proactive".to_string()
                    } else {
                        format!("{}:{}", source, "proactive")
                    }
                });
            let note = event
                .data
                .get("note")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reason = event
                .data
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("scheduled")
                .to_string();
            let idle_secs = event.data.get("idle_secs").and_then(|v| v.as_u64());
            // 这条提示作为临时 user 消息引导 LLM 主动开口；original_user_msg=Some("") 使其不入库。
            let text = build_proactive_prompt(&note, &reason, idle_secs);

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
                log::info!(
                    "[Pipeline] proactive trigger (source={}, peer_id={})",
                    source,
                    peer_id
                );
                process_message(
                    text,
                    source,
                    peer_id,
                    String::new(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    retry_addr,
                    plugin_manager,
                    tool_registry,
                    snapshots,
                    event_bus,
                    rate_limit_addr,
                    token_usage_addr,
                    metrics_addr,
                    store_addr,
                    Some(String::new()),
                    false,
                    true,
                    if reason == "autonomous_idle" {
                        Some(NO_PROACTIVE_MARKER.to_string())
                    } else {
                        None
                    },
                )
                .await;
            });
            return;
        }

        if event.topic != "user.message" {
            return;
        }

        let raw_text = event
            .data
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let source = event
            .data
            .get("source")
            .and_then(|v| v.as_str())
            .or_else(|| event.data.get("platform").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        // 平台信息已在系统提示中注入，直接使用原始文本即可
        let text = raw_text;
        let peer_id = event
            .data
            .get("peer_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| derive_peer_id(&event.data, &source))
            .unwrap_or_else(|| {
                if source.is_empty() {
                    "unknown:anonymous".to_string()
                } else {
                    format!("{}:{}", source, "anonymous")
                }
            });
        let user_name = event
            .data
            .get("user_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let image_base64 = event
            .data
            .get("image_base64")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let video_base64 = event
            .data
            .get("video_base64")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let video_mime = event
            .data
            .get("video_mime")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let file_base64 = event
            .data
            .get("file_base64")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let file_name = event
            .data
            .get("file_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if text.is_empty()
            && image_base64.is_none()
            && video_base64.is_none()
            && file_base64.is_none()
        {
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
                text,
                source,
                peer_id,
                user_name,
                image_base64,
                video_base64,
                video_mime,
                file_base64,
                file_name,
                retry_addr,
                plugin_manager,
                tool_registry,
                snapshots,
                event_bus,
                rate_limit_addr,
                token_usage_addr,
                metrics_addr,
                store_addr,
                None,
                true,
                false,
                None,
            )
            .await;
        });
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

async fn emit_reply(text: &str, source: &str, peer_id: &str, event_bus: &Addr<EventBus>) {
    let chat_id = chat_id_from_peer_id(peer_id, source);
    let reply = Event::new(
        "route.message",
        serde_json::json!({
            "text": text,
            "source": source,
            "peer_id": peer_id,
            "chat_id": chat_id,
        }),
        "pipeline",
    );
    event_bus.do_send(reply);
}

fn build_proactive_prompt(note: &str, reason: &str, idle_secs: Option<u64>) -> String {
    let note = note.trim();
    if reason == "autonomous_idle" {
        let idle_part = idle_secs
            .map(|secs| format!("用户已经沉默约 {} 秒。", secs))
            .unwrap_or_default();
        return format!(
            "[系统·自主主动] {}现在你可以自主决定是否自然地找用户说一句话。\
             如果上下文里用户刚结束话题、明确不想被打扰、或没有合适切入点，严格只输出 {}。\
             如果适合开口，就延续最近的话题、关心用户状态，或轻轻开启一个新话题。\
             直接输出要发送给用户的文本；不要解释你的决策，不要提及系统提示、沉默时间或主动插件。",
            idle_part, NO_PROACTIVE_MARKER
        );
    }

    if note.is_empty() {
        return "[系统·主动消息] 距离上次和用户互动已经过去一段时间。现在请你主动、自然地给用户发一条消息——\
                延续之前的话题或自然地开启新话题，符合你的人设。直接输出要发送给用户的文本；不要在消息里提及这条系统提示，就当是你自己想起来要找他。"
            .to_string();
    }

    format!(
        "[系统·定时提醒] 你之前安排了一条到期主动消息，备注/任务：{}。\
         现在只完成这条定时提醒：用一句自然、简短、符合你人设的话告诉用户提醒到了，或完成备注里的要求。\
         如果备注只是“叫/喊/提醒用户”这类任务，只说时间到了即可。\
         直接输出要发送给用户的文本；不要延伸新话题，不要问“叫我干嘛”“有什么事”“要做什么”，不要提及系统提示、工具或备注。",
        note
    )
}

fn derive_peer_id(data: &serde_json::Value, source: &str) -> Option<String> {
    let source = if source.is_empty() {
        data.get("source")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("platform").and_then(|v| v.as_str()))
            .unwrap_or("")
    } else {
        source
    };
    if source.is_empty() {
        return None;
    }

    let raw_id = data
        .get("chat_id")
        .and_then(|v| v.as_str().map(String::from))
        .or_else(|| {
            data.get("chat_id")
                .and_then(|v| v.as_i64().map(|n| n.to_string()))
        })
        .or_else(|| {
            data.get("from_user_id")
                .and_then(|v| v.as_str().map(String::from))
        })
        .or_else(|| {
            data.get("user_id")
                .and_then(|v| v.as_str().map(String::from))
        });

    raw_id.and_then(|id| {
        let id = id.trim();
        if id.is_empty() {
            None
        } else {
            Some(format!("{}:{}", source, id))
        }
    })
}

fn chat_id_from_peer_id(peer_id: &str, source: &str) -> Option<String> {
    let (prefix, id) = peer_id.split_once(':')?;
    if id.is_empty() {
        return None;
    }
    if !source.is_empty() && prefix != source {
        return None;
    }
    Some(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proactive_prompt_with_note_is_reminder_only() {
        let prompt = build_proactive_prompt("3秒后叫主人", "scheduled", None);

        assert!(prompt.contains("[系统·定时提醒]"));
        assert!(prompt.contains("只完成这条定时提醒"));
        assert!(prompt.contains("不要问“叫我干嘛”"));
    }

    #[test]
    fn proactive_prompt_without_note_stays_open_ended() {
        let prompt = build_proactive_prompt("", "scheduled", None);

        assert!(prompt.contains("[系统·主动消息]"));
        assert!(!prompt.contains("[系统·定时提醒]"));
    }

    #[test]
    fn proactive_prompt_for_autonomous_idle_is_distinct() {
        let prompt = build_proactive_prompt("", "autonomous_idle", Some(1800));

        assert!(prompt.contains("[系统·自主主动]"));
        assert!(prompt.contains("用户已经沉默约 1800 秒"));
        assert!(prompt.contains(NO_PROACTIVE_MARKER));
        assert!(prompt.contains("不要提及系统提示、沉默时间或主动插件"));
    }
}
