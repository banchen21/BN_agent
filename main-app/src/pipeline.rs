//! PipelineActor — orchestrates the UserMessage → LLM → tool-calling loop.
//!
//! ## Flow
//!
//! 1. Receive `HandleUserMessage` with user text + chat_id + source.
//! 2. Refresh plugin snapshots (`RefreshSnapshots`).
//! 3. Collect tool definitions from `ToolRegistry`.
//! 4. Send `ChatRequest` to `LlmActor`.
//! 5. If the response contains `tool_calls`, execute each tool.
//! 6. Broadcast the assistant reply via `BroadcastEvent`.

use actix::prelude::*;
use plugin_interface::*;
use std::sync::{Arc, Mutex};

use crate::llm_actor::LlmActor;
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
    llm_addr: Addr<LlmActor>,
    plugin_manager: Addr<PluginManager>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
    event_bus: Addr<EventBus>,
}

impl PipelineActor {
    pub fn new(
        llm_addr: Addr<LlmActor>,
        plugin_manager: Addr<PluginManager>,
        tool_registry: Arc<Mutex<ToolRegistry>>,
        snapshots: Arc<Mutex<Vec<String>>>,
        event_bus: Addr<EventBus>,
    ) -> Self {
        Self { llm_addr, plugin_manager, tool_registry, snapshots, event_bus }
    }
}

impl Actor for PipelineActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[PipelineActor] started");
    }
}

// ── Handler ──────────────────────────────────────────────────────────────────

impl Handler<HandleUserMessage> for PipelineActor {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: HandleUserMessage, _ctx: &mut Self::Context) -> Self::Result {
        let llm_addr = self.llm_addr.clone();
        let plugin_manager = self.plugin_manager.clone();
        let tool_registry = self.tool_registry.clone();
        let snapshots = self.snapshots.clone();
        let event_bus = self.event_bus.clone();

        let text = msg.text;
        let chat_id = msg.chat_id;
        let source = msg.source;
        let user_name = msg.user_name;

        let fut = async move {
            log::info!("[Pipeline] @{}: {}", user_name, text);

            // 1. Refresh snapshots.
            let _ = plugin_manager.send(RefreshSnapshots).await;
            let contexts: Vec<String> = snapshots.lock().unwrap().clone();

            // 2. Collect tool definitions.
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

            // 3. Send to LLM.
            let resp = match llm_addr.send(ChatRequest {
                chat_id,
                message: text.clone(),
                tools: tools.clone(),
                skip_store: false,
                contexts,
                jailbreak_index: None,
                image_base64: None,
                video_base64: None,
                video_mime: None,
                file_base64: None,
                file_name: None,
            }).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[Pipeline] LLM error: {}", e);
                    emit_reply(chat_id, &format!("Sorry, error: {}", e), &source, &event_bus, &plugin_manager).await;
                    return;
                }
                Err(e) => {
                    log::error!("[Pipeline] LLM mailbox error: {}", e);
                    return;
                }
            };

            // 4. Execute tool calls.
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

                    let result = match executor {
                        Some(exec) => exec.execute(&args),
                        None => ToolResult::err(&format!("tool '{}' not found", tc.name)),
                    };

                    if result.success {
                        log::info!("[Pipeline] tool '{}' ok: {}", tc.name, result.content);
                        tool_results.push(format!("【{}】\n{}", tc.name, result.content));
                    } else {
                        let err = result.error.as_deref().unwrap_or("unknown");
                        log::warn!("[Pipeline] tool '{}' failed: {}", tc.name, err);
                        tool_results.push(format!("【{}】错误：{}", tc.name, err));
                    }
                }

                // 工具结果喂回 LLM 进行第二轮推理
                let follow_up = format!(
                    "以下是我调用的工具的执行结果，请根据这些结果组织回复：\n\n{}",
                    tool_results.join("\n\n")
                );
                let final_resp = llm_addr.send(ChatRequest {
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
                }).await;
                match final_resp {
                    Ok(Ok(r)) if !r.content.trim().is_empty() => {
                        emit_reply(chat_id, &r.content, &source, &event_bus, &plugin_manager).await;
                    }
                    Ok(Err(e)) => {
                        log::error!("[Pipeline] tool loop LLM error: {}", e);
                    }
                    Err(e) => {
                        log::error!("[Pipeline] tool loop mailbox error: {}", e);
                    }
                    _ => {}
                }
            } else if !resp.content.trim().is_empty() {
                // 5. Broadcast assistant reply.
                emit_reply(chat_id, &resp.content, &source, &event_bus, &plugin_manager).await;
            }
        }
        .into_actor(self)
        .map(|_, _this: &mut Self, _ctx| ());

        Box::pin(fut)
    }
}

// ── Event handler (bridges EventBus → HandleUserMessage logic) ───────────────

impl Handler<Event> for PipelineActor {
    type Result = ();

    fn handle(&mut self, event: Event, _ctx: &mut Self::Context) {
        // Only process user.message events.
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

        // Forward to internal handler — spawn to avoid blocking the EventBus.
        let llm_addr = self.llm_addr.clone();
        let plugin_manager = self.plugin_manager.clone();
        let tool_registry = self.tool_registry.clone();
        let snapshots = self.snapshots.clone();
        let event_bus = self.event_bus.clone();

        actix::spawn(async move {
            if !text.is_empty() {
                log::info!("[Pipeline] @{}: {}", user_name, text);
            }
            if image_base64.is_some() {
                log::info!("[Pipeline] @{} sent a photo", user_name);
            }

            let _ = plugin_manager.send(RefreshSnapshots).await;
            let contexts: Vec<String> = snapshots.lock().unwrap().clone();

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

            let resp = match llm_addr.send(ChatRequest {
                chat_id,
                message: text.clone(),
                tools: tools.clone(),
                skip_store: false,
                contexts,
                jailbreak_index: None,
                image_base64: image_base64.clone(),
                video_base64: video_base64.clone(),
                video_mime: video_mime.clone(),
                file_base64: file_base64.clone(),
                file_name: file_name.clone(),
            }).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[Pipeline] LLM error: {}", e);
                    emit_reply(chat_id, &format!("Sorry, error: {}", e), &source, &event_bus, &plugin_manager).await;
                    return;
                }
                Err(e) => {
                    log::error!("[Pipeline] LLM mailbox error: {}", e);
                    return;
                }
            };

            if !resp.tool_calls.is_empty() {
                log::info!("[Pipeline] {} tool call(s): {:?}",
                    resp.tool_calls.len(),
                    resp.tool_calls.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
                );
                let mut tool_results: Vec<String> = Vec::new();
                for tc in &resp.tool_calls {
                    let executor = match tool_registry.lock() {
                        Ok(reg) => reg.get_executor(&tc.name),
                        Err(_) => None,
                    };
                    let mut args = tc.arguments.clone();
                    if let serde_json::Value::Object(ref mut map) = args {
                        map.entry("chat_id").or_insert(serde_json::json!(chat_id));
                    }
                    let result = match executor {
                        Some(exec) => exec.execute(&args),
                        None => ToolResult::err(&format!("tool '{}' not found", tc.name)),
                    };
                    if result.success {
                        log::info!("[Pipeline] tool '{}' ok: {}", tc.name, result.content);
                        tool_results.push(format!("【{}】\n{}", tc.name, result.content));
                    } else {
                        let err = result.error.as_deref().unwrap_or("unknown");
                        log::warn!("[Pipeline] tool '{}' failed: {}", tc.name, err);
                        tool_results.push(format!("【{}】错误：{}", tc.name, err));
                    }
                }

                // 工具结果喂回 LLM 进行第二轮推理
                let follow_up = format!(
                    "以下是我调用的工具的执行结果，请根据这些结果组织回复：\n\n{}",
                    tool_results.join("\n\n")
                );
                let final_resp = llm_addr.send(ChatRequest {
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
                }).await;
                match final_resp {
                    Ok(Ok(r)) if !r.content.trim().is_empty() => {
                        emit_reply(chat_id, &r.content, &source, &event_bus, &plugin_manager).await;
                    }
                    Ok(Err(e)) => {
                        log::error!("[Pipeline] tool loop LLM error: {}", e);
                    }
                    Err(e) => {
                        log::error!("[Pipeline] tool loop mailbox error: {}", e);
                    }
                    _ => {}
                }
            } else if !resp.content.trim().is_empty() {
                emit_reply(chat_id, &resp.content, &source, &event_bus, &plugin_manager).await;
            }
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
    // 只走 EventBus，PluginManager 已订阅 '*' 会自动广播给所有插件的 on_event
    event_bus.do_send(reply);
}
