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
use rand::Rng;
use crate::chat_store::{ChatStoreActor, FetchRecent, AppendJsonMessage};
use plugin_interface::*;

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub system_prompt: String,
    pub max_history_turns: usize,
    pub max_tokens: u32,
    pub thinking: bool,
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

        let max_tokens = std::env::var("LLM_MAX_TOKENS")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(384000);

        let thinking = std::env::var("LLM_THINKING")
            .ok().map(|v| v.to_lowercase())
            .map(|v| v == "enabled" || v == "true" || v == "1")
            .unwrap_or(false);

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
            api_key, model, base_url, system_prompt, max_history_turns, max_tokens, thinking,
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
            "temperature": msg.temperature.unwrap_or(rand::thread_rng().gen_range(0.7..=1.2)),
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

        // 破限词：多模态请求或工具调用时自动随机选取
        let jailbreak = msg.jailbreak_index
            .or_else(|| {
                let has_tools = !msg.tools.is_empty();
                let is_multimodal = msg.image_base64.is_some()
                    || msg.video_base64.is_some()
                    || msg.file_base64.is_some();
                if (is_multimodal || has_tools) && !self.config.jailbreak_prompts.is_empty() {
                    use rand::Rng;
                    Some(rand::thread_rng().gen_range(0..self.config.jailbreak_prompts.len()))
                } else {
                    None
                }
            })
            .and_then(|i| self.config.jailbreak_at(i));
        // system_content 构建顺序：persona → jailbreak（如果有）→ tool_hint（在下面追加）
        // 利用 LLM 近因效应让工具提示在后，权重更高
        let system_content = if let Some(jb) = jailbreak {
            format!("{}\n\n{}", self.config.system_prompt, jb)
        } else {
            self.config.system_prompt.clone()
        };
        // Immediate context: recent N messages (env IMMEDIATE_CONTEXT_MSGS, default 200 = 100 rounds).
        let immediate_limit: usize = std::env::var("IMMEDIATE_CONTEXT_MSGS")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(200);

        let tools = msg.tools.clone();

        // 动态生成工具提示（从实际注册的工具列表中提取）
        // 顺序：persona → jailbreak → tool_hint（近因效应让工具提示权重最高）
        let tool_hint = build_tool_hint(&tools);
        let system_content = if !tools.is_empty() {
            format!("{}\n\n{}", system_content, tool_hint)
        } else {
            system_content
        };
        let user_msg = msg.message.clone();
        let original_user_msg = msg.original_user_msg.clone();
        let skip_store = msg.skip_store;
        let contexts = msg.contexts.clone();
        let image_base64 = msg.image_base64.clone();
        let video_base64 = msg.video_base64.clone();
        let video_mime = msg.video_mime.clone();
        let stream = msg.stream;
        let max_tokens = msg.max_tokens.unwrap_or(self.config.max_tokens);
        let file_base64 = msg.file_base64.clone();
        let file_name = msg.file_name.clone();
        // Capture config values needed for the async future.
        let image_model = self.config.image_model.clone();
        let image_base_url = self.config.image_base_url.clone();
        let image_api_key = self.config.image_api_key.clone();
        let video_model = self.config.video_model.clone();
        let video_base_url = self.config.video_base_url.clone();
        let video_api_key = self.config.video_api_key.clone();
        let base_url = self.config.base_url.clone();
        let api_key = self.config.api_key.clone();
        let thinking = self.config.thinking;

        event_bus.do_send(Event::new("llm.request", serde_json::json!({
            "model": model, "mode": "chat",
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

            // ── 3. Recent history (last 4 messages for immediate context) ──
            let records = store_addr.send(FetchRecent { limit: immediate_limit }).await.unwrap_or_default();
            // Filter orphan tools at the head.
            let mut tool_calls_seen = false;
            for r in &records {
                let role_is_tool = r.role == "tool"
                    || r.message_json.as_deref()
                        .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                        .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(|s| s == "tool"))
                        .unwrap_or(false);
                let role_is_tool_calls = r.message_json.as_deref()
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                    .map(|v| v.get("tool_calls").is_some())
                    .unwrap_or(false);
                if role_is_tool && !tool_calls_seen { continue; }
                if role_is_tool_calls { tool_calls_seen = true; }
                if let Some(ref json_str) = r.message_json {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(json_str) {
                        messages.push(msg);
                        continue;
                    }
                }
                messages.push(serde_json::json!({
                    "role": r.role.clone(), "content": r.content.clone()
                }));
            }

            // ── 4. Other plugin contexts ──
            for ctx in &other_ctx {
                messages.push(serde_json::json!({ "role": "assistant", "content": ctx }));
            }

            // ── 4b. Follow-up: assistant tool_calls + tool results (DeepSeek API spec) ──
            let assistant_tool_calls = msg.assistant_tool_calls.clone();
            let tool_results = msg.tool_results.clone();
            if !assistant_tool_calls.is_empty() {
                let mut tc_array = Vec::new();
                for tc in &assistant_tool_calls {
                    tc_array.push(serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()
                        }
                    }));
                }
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tc_array
                }));
                for (i, tc) in assistant_tool_calls.iter().enumerate() {
                    let content = tool_results.get(i).cloned().unwrap_or_default();
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "content": content
                    }));
                }
            }

            // ── 5. Current user message ──

            let user_content: serde_json::Value = if let Some(ref img_b64) = image_base64 {
                serde_json::json!([
                    {"type": "text", "text": user_msg},
                    {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", img_b64)}}
                ])
            } else if let (Some(ref vid_b64), Some(ref vid_mime)) = (video_base64.as_ref(), video_mime.as_ref()) {
                serde_json::json!([
                    {"type": "video_url", "video_url": {"url": format!("data:{};base64,{}", vid_mime, vid_b64)}, "fps": 2, "media_resolution": "default"},
                    {"type": "text", "text": user_msg}
                ])
            } else if let (Some(_fb64), Some(ref fname)) = (file_base64.as_ref(), file_name.as_ref()) {
                let file_hint = if user_msg.is_empty() {
                    format!("用户发送了一个文件：{}", fname)
                } else {
                    format!("{}\n\n[用户发送了文件：{}]", user_msg, fname)
                };
                serde_json::json!(file_hint)
            } else {
                serde_json::json!(user_msg)
            };
            // 工具回调轮可能没有新用户消息，跳过空消息
            if !user_msg.is_empty() || assistant_tool_calls.is_empty() {
                messages.push(serde_json::json!({ "role": "user", "content": user_content }));
            }

            // ── 4. Route to correct model/endpoint ──
            // Clone defaults before the if-else moves them (needed for potential fallback).
            let default_model = model.clone();
            let default_base_url = base_url.clone();
            let default_api_key = api_key.clone();
            let actual_model = if video_base64.is_some() { video_model }
                else if image_base64.is_some() { image_model }
                else { model };
            let actual_base_url = if video_base64.is_some() { video_base_url }
                else if image_base64.is_some() { image_base_url }
                else { base_url };
            let actual_api_key = if video_base64.is_some() { video_api_key }
                else if image_base64.is_some() { image_api_key }
                else { api_key };

            let api_url = format!("{}/chat/completions", actual_base_url.trim_end_matches('/'));

            // Clone messages before they're moved into body (needed for potential fallback).
            let messages_for_fallback = messages.clone();

            let mut body = serde_json::json!({
                "model": actual_model,
                "messages": messages,
                "temperature": rand::thread_rng().gen_range(0.7..=1.2),
                "max_tokens": max_tokens,
            });
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
                body["tool_choice"] = serde_json::json!("auto");
            }
            // DeepSeek 思考模式（thinking）开关
            body["thinking"] = serde_json::json!({ "type": if thinking { "enabled" } else { "disabled" } });

            // ── 5. Call LLM ──
            let mut result = if stream {
                stream_llm(&client, &api_url, &actual_api_key, &body, &event_bus).await
            } else {
                call_llm(&client, &api_url, &actual_api_key, &body, &event_bus).await
            };

            // ── 5b. Fallback: 图片模型失败 → 用文本模型 + 工具重试 ──
            if let Some(ref _img_b64) = image_base64 {
                if should_fallback_to_tools(&result) {
                    log::info!("[LlmActor] image model failed, retrying with text model + tools");
                    let fallback_text = if user_msg.is_empty() {
                        "用户发送了一张图片。你无法直接查看图片，请使用 image_understand 或 image_describe 工具来分析它（image_base64 参数可省略，工具会自动使用最近收到的图片）。".to_string()
                    } else {
                        format!(
                            "用户消息：{}\n\n用户还发送了一张图片。请使用 image_understand 或 image_describe 工具来分析它（image_base64 参数可省略，工具会自动使用最近收到的图片）。",
                            user_msg
                        )
                    };
                    let mut fallback_messages = messages_for_fallback.clone();
                    if let Some(last) = fallback_messages.last_mut() {
                        last["content"] = serde_json::json!(fallback_text);
                    }

                    let fallback_api_url = format!("{}/chat/completions", default_base_url.trim_end_matches('/'));
                    let mut fallback_body = serde_json::json!({
                        "model": default_model,
                        "messages": fallback_messages,
                        "temperature": rand::thread_rng().gen_range(0.7..=1.2),
                        "max_tokens": max_tokens,
                    });
                    if !tools.is_empty() {
                        fallback_body["tools"] = serde_json::json!(tools);
                        fallback_body["tool_choice"] = serde_json::json!("auto");
                    }
                    fallback_body["thinking"] = serde_json::json!({ "type": if thinking { "enabled" } else { "disabled" } });

                    result = if stream {
                        stream_llm(&client, &fallback_api_url, &default_api_key, &fallback_body, &event_bus).await
                    } else {
                        call_llm(&client, &fallback_api_url, &default_api_key, &fallback_body, &event_bus).await
                    };
                }
            }

            // ── 5c. Fallback: 视频模型失败 → 用文本模型 + video_analyze 工具重试 ──
            if let (Some(_vid_b64), Some(_vid_mime)) = (video_base64.as_ref(), video_mime.as_ref()) {
                if should_fallback_to_tools(&result) {
                    log::info!("[LlmActor] video model failed, retrying with text model + video_analyze tool");
                    let fallback_text = if user_msg.is_empty() {
                        "用户发送了一段视频。你无法直接查看视频，请使用 video_analyze 工具来分析它（参数可省略，工具会自动使用最近收到的视频）。".to_string()
                    } else {
                        format!(
                            "用户消息：{}\n\n用户还发送了一段视频。请使用 video_analyze 工具来分析它（参数可省略，工具会自动使用最近收到的视频）。",
                            user_msg
                        )
                    };
                    let mut fallback_messages = messages_for_fallback.clone();
                    if let Some(last) = fallback_messages.last_mut() {
                        last["content"] = serde_json::json!(fallback_text);
                    }

                    let fallback_api_url = format!("{}/chat/completions", default_base_url.trim_end_matches('/'));
                    let mut fallback_body = serde_json::json!({
                        "model": default_model,
                        "messages": fallback_messages,
                        "temperature": rand::thread_rng().gen_range(0.7..=1.2),
                        "max_tokens": max_tokens,
                    });
                    if !tools.is_empty() {
                        fallback_body["tools"] = serde_json::json!(tools);
                        fallback_body["tool_choice"] = serde_json::json!("auto");
                    }
                    fallback_body["thinking"] = serde_json::json!({ "type": if thinking { "enabled" } else { "disabled" } });

                    result = if stream {
                        stream_llm(&client, &fallback_api_url, &default_api_key, &fallback_body, &event_bus).await
                    } else {
                        call_llm(&client, &fallback_api_url, &default_api_key, &fallback_body, &event_bus).await
                    };
                }
            }

            // ── 6. Persist to history ──
            if !skip_store {
                if let Ok(ref resp) = result {
                    let user_text = original_user_msg.clone().unwrap_or_else(|| user_msg.clone());
                    // 始终保存用户消息
                    if !user_text.is_empty() {
                        let user_json = serde_json::json!({
                            "role": "user",
                            "content": user_text
                        });
                        store_addr.do_send(AppendJsonMessage {
                            message_json: user_json.to_string(),
                        });
                    }
                    // 保存助手回复：带有 tool_calls 的都不存（不论是否有文本），
                    // 等 PipelineActor 的工具循环完成后保存完整链（tool_calls + tool role 结果 + 最终文本）。
                    // 只有纯文本回复才保存。
                    if !resp.tool_calls.is_empty() {
                        // 跳过，PipelineActor 会存完整链
                    } else if !resp.content.trim().is_empty() {
                        let mut assistant_msg = serde_json::json!({
                            "role": "assistant",
                            "content": resp.content.clone()
                        });
                        if !resp.tool_calls.is_empty() {
                            let tc_array: Vec<serde_json::Value> = resp.tool_calls.iter().map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()
                                    }
                                })
                            }).collect();
                            assistant_msg["tool_calls"] = serde_json::Value::Array(tc_array);
                        }
                        store_addr.do_send(AppendJsonMessage {
                            message_json: assistant_msg.to_string(),
                        });
                    }
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

// ── Streaming LLM call ──────────────────────────────────────────────────────

/// Call the LLM with SSE streaming, emitting chunks on the EventBus.
async fn stream_llm(
    client: &reqwest::Client,
    api_url: &str,
    api_key: &str,
    body: &serde_json::Value,
    event_bus: &Addr<EventBus>,
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
    let mut prompt_cache_hit_tokens: u32 = 0;
    let mut prompt_cache_miss_tokens: u32 = 0;

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

            // Capture usage from ANY chunk (some APIs send it with choices, others separately)
            if let Some(usage) = data.get("usage") {
                if usage["prompt_tokens"].as_u64().unwrap_or(0) > 0 {
                    prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                    completion_tokens = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
                    prompt_cache_hit_tokens = usage["prompt_cache_hit_tokens"].as_u64().unwrap_or(0) as u32;
                    prompt_cache_miss_tokens = usage["prompt_cache_miss_tokens"].as_u64().unwrap_or(0) as u32;
                    log::info!("[stream_llm] usage captured: prompt={}, completion={}, cache_hit={}, cache_miss={}",
                        prompt_tokens, completion_tokens, prompt_cache_hit_tokens, prompt_cache_miss_tokens);
                }
            }

            if choices.is_none() || choices.unwrap().is_empty() {
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
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
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
    let prompt_cache_hit_tokens = json["usage"]["prompt_cache_hit_tokens"].as_u64().unwrap_or(0) as u32;
    let prompt_cache_miss_tokens = json["usage"]["prompt_cache_miss_tokens"].as_u64().unwrap_or(0) as u32;

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
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
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

/// 判断是否需要从多模态模型回退到文本模型 + 工具调用。
/// 条件：LLM 调用出错，或返回内容太短（说明模型无法处理图片）。
fn should_fallback_to_tools(result: &Result<LlmResponse, String>) -> bool {
    match result {
        Err(_) => true,
        Ok(resp) => {
            // 有 tool_calls 说明模型正在使用工具 → 不需要回退
            if !resp.tool_calls.is_empty() {
                return false;
            }
            // 内容 ≤1 字符 → 模型可能无法处理图片/视频
            resp.content.trim().chars().count() <= 1
        }
    }
}

/// 从 tools JSON 数组中动态生成工具提示文本。
/// 建立 Agent 身份意识 + 分类陈列可用工具。
fn build_tool_hint(tools: &[serde_json::Value]) -> String {
    let mut send_tools = Vec::new();
    let mut other_tools = Vec::new();
    for tool in tools {
        let func = match tool.get("function") {
            Some(f) => f,
            None => continue,
        };
        let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let desc = func.get("description").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() { continue; }
        let line = format!("- {}：{}", name, desc);
        if name.contains("send") || name.contains("message") || name.contains("voice") {
            send_tools.push(line);
        } else {
            other_tools.push(line);
        }
    }
    let mut lines = vec![
        "【Core Rules】".to_string(),
        "You have tools. User requests (send msg, generate image, query info, process files, etc.) MUST use tools. Never describe operations in text.".to_string(),
        "Do NOT send confirmation text after calling tools — the system handles delivery automatically.".to_string(),
        String::new(),
    ];
    if !send_tools.is_empty() {
        lines.push("【Send Tools (direct to user chat)】".to_string());
        lines.extend(send_tools);
        lines.push(String::new());
    }
    if !other_tools.is_empty() {
        lines.push("【Other Tools (query / generate / process)】".to_string());
        lines.extend(other_tools);
    }
    lines.join("\n")
}

fn truncate_preview(s: &str, max: usize) -> &str {
    let max = max.min(s.len());
    let cut = s.char_indices()
        .take_while(|(i, _)| *i < max)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &s[..cut]
}
