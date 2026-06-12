//! LlmActor — actix actor wrapping an OpenAI-compatible chat-completion API.
//!
//! Two request modes:
//! - `LlmRequest`  — simple stateless call (used by plugins directly).
//! - `ChatRequest` — full-featured: SQLite history, tool-calling, jailbreak prompts,
//!                   passive plugin contexts.
//!
//! ## Lifecycle events published to the EventBus
//!
//! | Topic           | When                          |
//! |-----------------|-------------------------------|
//! | `llm.request`   | A request is received          |
//! | `llm.response`  | A successful completion        |
//! | `llm.error`     | HTTP / API / parse failure     |

use actix::prelude::*;
use crate::chat_store::{ChatStoreActor, FetchRecent, AppendPair, ClearSession as StoreClearSession};
use plugin_interface::*;

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub system_prompt: String,
    pub max_history_turns: usize,
    pub jailbreak_prompts: Vec<String>,
    pub image_model: String,
    pub image_base_url: String,
    pub image_api_key: String,
    pub video_model: String,
    pub video_base_url: String,
    pub video_api_key: String,
}

impl LlmConfig {
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("LLM_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| "LLM_API_KEY or OPENAI_API_KEY not set".to_string())?;

        let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
        let base_url = std::env::var("LLM_BASE_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com/v1".into());

        let system_prompt = Self::load_persona().unwrap_or_else(|| {
            "You are a helpful AI assistant. Reply in the user's language.".into()
        });

        let jailbreak_prompts = Self::load_jailbreak_prompts();

        let max_history_turns = std::env::var("LLM_MAX_HISTORY")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(20);

        let image_model = std::env::var("IMAGE_MODEL")
            .unwrap_or_else(|_| "mimo-v2.5".into());
        let image_base_url = std::env::var("IMAGE_BASE_URL")
            .unwrap_or_else(|_| base_url.clone());
        let image_api_key = std::env::var("IMAGE_API_KEY")
            .unwrap_or_else(|_| api_key.clone());
        let video_model = std::env::var("VIDEO_MODEL")
            .unwrap_or_else(|_| "mimo-v2.5".into());
        let video_base_url = std::env::var("VIDEO_BASE_URL")
            .unwrap_or_else(|_| base_url.clone());
        let video_api_key = std::env::var("VIDEO_API_KEY")
            .unwrap_or_else(|_| api_key.clone());

        Ok(Self {
            api_key, model, base_url, system_prompt, max_history_turns,
            jailbreak_prompts, image_model, image_base_url, image_api_key,
            video_model, video_base_url, video_api_key,
        })
    }

    fn load_persona() -> Option<String> {
        let path = std::path::PathBuf::from("persona.md");
        match std::fs::read_to_string(&path) {
            Ok(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        }
    }

    fn load_jailbreak_prompts() -> Vec<String> {
        let path = std::path::PathBuf::from("data/jailbreak_prompts.csv");
        let mut prompts = Vec::new();
        if let Ok(mut reader) = csv::Reader::from_path(&path) {
            for result in reader.records() {
                if let Ok(record) = result {
                    if let Some(p) = record.get(2) {
                        let t = p.trim();
                        if !t.is_empty() {
                            prompts.push(t.to_string());
                        }
                    }
                }
            }
            if !prompts.is_empty() {
                log::info!("[LlmActor] loaded {} jailbreak prompts", prompts.len());
            }
        }
        prompts
    }

    pub fn jailbreak_at(&self, index: usize) -> Option<&str> {
        self.jailbreak_prompts.get(index).map(|s| s.as_str())
    }

    pub fn jailbreak_count(&self) -> usize {
        self.jailbreak_prompts.len()
    }
}

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct LlmActor {
    config: LlmConfig,
    client: reqwest::Client,
    store_addr: Addr<ChatStoreActor>,
    event_bus: Addr<EventBus>,
}

impl LlmActor {
    pub fn new(config: LlmConfig, event_bus: Addr<EventBus>, store_addr: Addr<ChatStoreActor>) -> Self {
        let api_base = config.base_url.trim_end_matches('/');
        log::info!(
            "[LlmActor] endpoint={}/chat/completions model={} max_history={}",
            api_base, config.model, config.max_history_turns,
        );
        Self {
            client: reqwest::Client::new(),
            config,
            store_addr,
            event_bus,
        }
    }
}

impl Actor for LlmActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[LlmActor] actor started");
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        log::info!("[LlmActor] actor stopping");
        Running::Stop
    }
}

// ── LlmRequest (simple, stateless) ───────────────────────────────────────────

impl Handler<LlmRequest> for LlmActor {
    type Result = ResponseActFuture<Self, Result<LlmResponse, String>>;

    fn handle(&mut self, msg: LlmRequest, _ctx: &mut Self::Context) -> Self::Result {
        let client = self.client.clone();
        let api_url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let api_key = self.config.api_key.clone();
        let model = msg.model.unwrap_or_else(|| self.config.model.clone());
        let event_bus = self.event_bus.clone();

        event_bus.do_send(Event::new("llm.request", serde_json::json!({
            "model": model, "mode": "simple"
        }), "llm-actor"));

        let messages: Vec<serde_json::Value> = msg.messages.iter().map(|m| {
            serde_json::json!({ "role": m.role, "content": m.content })
        }).collect();

        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": msg.temperature.unwrap_or(0.7),
            "max_tokens": msg.max_tokens.unwrap_or(1024),
        });

        let fut = async move {
            call_llm(&client, &api_url, &api_key, &body, &event_bus).await
        }
        .into_actor(self)
        .map(|result, _this: &mut Self, _ctx| result);

        Box::pin(fut)
    }
}

// ── ChatRequest (full-featured: history + tools + contexts + jailbreak) ──────

impl Handler<ChatRequest> for LlmActor {
    type Result = ResponseActFuture<Self, Result<LlmResponse, String>>;

    fn handle(&mut self, msg: ChatRequest, _ctx: &mut Self::Context) -> Self::Result {
        let client = self.client.clone();
        let model = self.config.model.clone();
        let event_bus = self.event_bus.clone();
        let store_addr = self.store_addr.clone();

        let jailbreak = msg.jailbreak_index.and_then(|i| self.config.jailbreak_at(i));
        let system_content = if let Some(jb) = jailbreak {
            format!("{}\n\n{}", jb, self.config.system_prompt)
        } else {
            self.config.system_prompt.clone()
        };
        let limit = self.config.max_history_turns * 2;

        let chat_id = msg.chat_id;
        let user_msg = msg.message.clone();
        let skip_store = msg.skip_store;
        let contexts = msg.contexts.clone();
        let tools = msg.tools.clone();
        let image_base64 = msg.image_base64.clone();
        let video_base64 = msg.video_base64.clone();
        let video_mime = msg.video_mime.clone();
        let stream = msg.stream;
        let file_base64 = msg.file_base64.clone();
        let file_name = msg.file_name.clone();
        let source = msg.source.clone();
        let user_name = msg.user_name.clone();

        // Capture config values needed for the async future.
        let image_model = self.config.image_model.clone();
        let image_base_url = self.config.image_base_url.clone();
        let image_api_key = self.config.image_api_key.clone();
        let video_model = self.config.video_model.clone();
        let video_base_url = self.config.video_base_url.clone();
        let video_api_key = self.config.video_api_key.clone();
        let base_url = self.config.base_url.clone();
        let api_key = self.config.api_key.clone();

        event_bus.do_send(Event::new("llm.request", serde_json::json!({
            "model": model, "chat_id": chat_id, "mode": "chat",
            "tool_count": tools.len(), "context_count": contexts.len(),
            "stream": stream,
        }), "llm-actor"));

        let fut = async move {
            // ── 1. Separate contexts into memory vs. other ──
            let mut memory_ctx: Vec<String> = Vec::new();
            let mut other_ctx: Vec<String> = Vec::new();
            for ctx in &contexts {
                if ctx.starts_with("[记忆]") {
                    memory_ctx.push(ctx.clone());
                } else {
                    other_ctx.push(ctx.clone());
                }
            }

            // ── 2. System + memory fragments ──
            let mut messages: Vec<serde_json::Value> = vec![
                serde_json::json!({ "role": "system", "content": system_content }),
            ];
            for ctx in &memory_ctx {
                messages.push(serde_json::json!({ "role": "assistant", "content": ctx }));
            }

            // ── 3. Recent history ──
            let records = store_addr.send(FetchRecent { chat_id, limit }).await.unwrap_or_default();
            for r in &records {
                messages.push(serde_json::json!({
                    "role": r.role, "content": r.content
                }));
            }

            // ── 4. Other plugin contexts ──
            for ctx in &other_ctx {
                messages.push(serde_json::json!({ "role": "assistant", "content": ctx }));
            }

            // ── 5. Current user message (source info merged in) ──
            let channel_prefix = if !source.is_empty() {
                format!("[{}] ", source)
            } else {
                String::new()
            };

            let user_content = if let Some(ref img_b64) = image_base64 {
                serde_json::json!([
                    {"type": "text", "text": format!("{}{}", channel_prefix, user_msg)},
                    {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", img_b64)}}
                ])
            } else if let (Some(ref vid_b64), Some(ref vid_mime)) = (video_base64.as_ref(), video_mime.as_ref()) {
                serde_json::json!([
                    {"type": "video_url", "video_url": {"url": format!("data:{};base64,{}", vid_mime, vid_b64)}, "fps": 2, "media_resolution": "default"},
                    {"type": "text", "text": format!("{}{}", channel_prefix, user_msg)}
                ])
            } else if let (Some(ref fb64), Some(ref fname)) = (file_base64.as_ref(), file_name.as_ref()) {
                serde_json::json!([
                    {"type": "file", "file": {"filename": fname, "file_data": fb64}},
                    {"type": "text", "text": format!("{}{}", channel_prefix, user_msg)}
                ])
            } else {
                serde_json::json!(format!("{}{}", channel_prefix, user_msg))
            };
            messages.push(serde_json::json!({ "role": "user", "content": user_content }));

            // ── 4. Route to correct model/endpoint ──
            let actual_model = if video_base64.is_some() { video_model }
                else if image_base64.is_some() { image_model }
                else { model.clone() };
            let actual_base_url = if video_base64.is_some() { video_base_url }
                else if image_base64.is_some() { image_base_url }
                else { base_url };
            let actual_api_key = if video_base64.is_some() { video_api_key }
                else if image_base64.is_some() { image_api_key }
                else { api_key };

            let api_url = format!("{}/chat/completions", actual_base_url.trim_end_matches('/'));

            let mut body = serde_json::json!({
                "model": actual_model,
                "messages": messages,
                "temperature": 0.7,
                "max_tokens": 2048u32,
            });
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
            }

            // ── 5. Call LLM ──
            let result = if stream {
                stream_llm(&client, &api_url, &actual_api_key, &body, &event_bus, chat_id).await
            } else {
                call_llm(&client, &api_url, &actual_api_key, &body, &event_bus).await
            };

            // ── 6. Persist to history ──
            if !skip_store {
                if let Ok(ref resp) = result {
                    // 始终保存用户消息，助手消息空也保存（如 tool_call 场景）
                    store_addr.do_send(AppendPair {
                        chat_id,
                        user_msg: user_msg.clone(),
                        assistant_msg: if resp.content.trim().is_empty() && !resp.tool_calls.is_empty() { serde_json::to_string(&resp.tool_calls).unwrap_or_default() } else { resp.content.clone() },
                    });
                }
            }

            result
        }
        .into_actor(self)
        .map(|result, _this: &mut Self, _ctx| result);

        Box::pin(fut)
    }
}

// ── Internal messages ────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "usize")]
pub struct JailbreakCount;

impl Handler<JailbreakCount> for LlmActor {
    type Result = usize;
    fn handle(&mut self, _: JailbreakCount, _: &mut Self::Context) -> Self::Result {
        self.config.jailbreak_count()
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct ClearSession(pub i64);

impl Handler<ClearSession> for LlmActor {
    type Result = ();
    fn handle(&mut self, msg: ClearSession, _: &mut Self::Context) {
        self.store_addr.do_send(StoreClearSession(msg.0));
    }
}

// ── Streaming LLM call ──────────────────────────────────────────────────────

/// Call the LLM with SSE streaming, emitting chunks on the EventBus.
async fn stream_llm(
    client: &reqwest::Client,
    api_url: &str,
    api_key: &str,
    body: &serde_json::Value,
    event_bus: &Addr<EventBus>,
    chat_id: i64,
) -> Result<LlmResponse, String> {
    use futures_util::StreamExt;

    let mut body = body.clone();
    body["stream"] = serde_json::json!(true);

    let response = client
        .post(api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            let err = format!("LLM stream HTTP error: {}", e);
            event_bus.do_send(Event::new("llm.error", serde_json::json!({ "error": err }), "llm-actor"));
            err
        })?;

    let status = response.status();
    if !status.is_success() {
        let json: serde_json::Value = response.json().await.unwrap_or(serde_json::json!({}));
        let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
        let err = format!("LLM API error ({}): {}", status.as_u16(), err_msg);
        event_bus.do_send(Event::new("llm.error", serde_json::json!({ "error": err }), "llm-actor"));
        return Err(err);
    }

    let mut full_content = String::new();
    let mut model_name = String::new();
    let mut collected_tool_calls: Vec<ToolCall> = Vec::new();
    let mut prompt_tokens: u32 = 0;
    let mut completion_tokens: u32 = 0;
    let mut chunk_count = 0u64;

    struct PartialToolCall {
        id: String, name: String, arguments: String,
    }
    let mut partial_tool: Option<PartialToolCall> = None;

    let mut stream = response.bytes_stream();
    let mut buf = String::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| {
            let err = format!("LLM stream read error: {}", e);
            event_bus.do_send(Event::new("llm.error", serde_json::json!({ "error": err }), "llm-actor"));
            err
        })?;

        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        while let Some(line_end) = buf.find('\n') {
            let line = buf[..line_end].trim().to_string();
            buf = buf[line_end + 1..].to_string();

            if line.is_empty() || line == "data: [DONE]" { continue; }

            let json_str = match line.strip_prefix("data: ") {
                Some(s) => s,
                None => continue,
            };

            let data: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let choices = data["choices"].as_array();
            if choices.is_none() || choices.unwrap().is_empty() {
                if let Some(usage) = data.get("usage") {
                    prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                    completion_tokens = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
                }
                if model_name.is_empty() {
                    if let Some(m) = data["model"].as_str() {
                        model_name = m.to_string();
                    }
                }
                continue;
            }

            let delta = &data["choices"][0]["delta"];

            if let Some(content) = delta["content"].as_str() {
                full_content.push_str(content);
                chunk_count += 1;
                event_bus.do_send(Event::new("llm.chunk", serde_json::json!({
                    "chat_id": chat_id,
                    "content": content,
                    "accumulated_length": full_content.len(),
                }), "llm-actor"));
            }

            if let Some(tc) = delta.get("tool_calls") {
                if let Some(tc_arr) = tc.as_array() {
                    for tc_item in tc_arr {
                        if let Some(id) = tc_item["id"].as_str() {
                            if !id.is_empty() {
                                partial_tool = Some(PartialToolCall {
                                    id: id.to_string(),
                                    name: String::new(),
                                    arguments: String::new(),
                                });
                            }
                        }
                        if let Some(name) = tc_item["function"]["name"].as_str() {
                            if !name.is_empty() {
                                if let Some(ref mut pt) = partial_tool {
                                    pt.name.push_str(name);
                                }
                            }
                        }
                        if let Some(args) = tc_item["function"]["arguments"].as_str() {
                            if let Some(ref mut pt) = partial_tool {
                                pt.arguments.push_str(args);
                            }
                        }
                    }
                }
            }

            if model_name.is_empty() {
                if let Some(m) = data["model"].as_str() {
                    model_name = m.to_string();
                }
            }
        }
    }

    if let Some(pt) = partial_tool.take() {
        let arguments = serde_json::from_str(&pt.arguments).unwrap_or(serde_json::Value::Null);
        collected_tool_calls.push(ToolCall { id: pt.id, name: pt.name, arguments });
    }

    let result = LlmResponse {
        content: full_content.clone(),
        model: model_name,
        prompt_tokens,
        completion_tokens,
        tool_calls: collected_tool_calls,
    };

    let preview = truncate_preview(&full_content, 200);
    event_bus.do_send(Event::new("llm.response", serde_json::json!({
        "content_preview": preview,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "chunk_count": chunk_count,
        "streamed": true,
    }), "llm-actor"));
    Ok(result)
}

// ── Shared HTTP call logic ───────────────────────────────────────────────────

async fn call_llm(
    client: &reqwest::Client,
    api_url: &str,
    api_key: &str,
    body: &serde_json::Value,
    event_bus: &Addr<EventBus>,
) -> Result<LlmResponse, String> {
    let response = client
        .post(api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| {
            let err = format!("LLM HTTP error: {}", e);
            event_bus.do_send(Event::new("llm.error", serde_json::json!({ "error": err }), "llm-actor"));
            err
        })?;

    let status = response.status();
    let json: serde_json::Value = response.json().await.map_err(|e| {
        let err = format!("LLM JSON parse error: {}", e);
        event_bus.do_send(Event::new("llm.error", serde_json::json!({ "error": err }), "llm-actor"));
        err
    })?;

    if !status.is_success() {
        let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
        let err = format!("LLM API error ({}): {}", status.as_u16(), err_msg);
        event_bus.do_send(Event::new("llm.error", serde_json::json!({ "error": err }), "llm-actor"));
        return Err(err);
    }

    let choice = &json["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();
    let prompt_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
    let completion_tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;

    let tool_calls: Vec<ToolCall> = choice["message"]["tool_calls"]
        .as_array()
        .map(|arr| {
            arr.iter().filter_map(|tc| {
                let id = tc["id"].as_str()?.to_string();
                let name = tc["function"]["name"].as_str()?.to_string();
                let arguments = serde_json::from_str(tc["function"]["arguments"].as_str()?)
                    .unwrap_or(serde_json::Value::Null);
                Some(ToolCall { id, name, arguments })
            }).collect()
        })
        .unwrap_or_default();

    let tc_count = tool_calls.len();
    let llm_response = LlmResponse {
        content: content.clone(),
        model: json["model"].as_str().unwrap_or("").to_string(),
        prompt_tokens,
        completion_tokens,
        tool_calls,
    };

    let preview = truncate_preview(&content, 200);
    event_bus.do_send(Event::new("llm.response", serde_json::json!({
        "content_preview": preview,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "tool_calls_count": tc_count,
    }), "llm-actor"));
    Ok(llm_response)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn truncate_preview(s: &str, max: usize) -> &str {
    let max = max.min(s.len());
    let cut = s.char_indices()
        .take_while(|(i, _)| *i < max)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &s[..cut]
}
