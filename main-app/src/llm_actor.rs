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
use crate::chat_store::ChatStore;
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
    /// 图片理解专用模型（如 mimo-v2.5）
    pub image_model: String,
    /// 图片理解专用接口地址
    pub image_base_url: String,
    /// 图片理解专用 API Key
    pub image_api_key: String,
    /// 视频理解专用模型（如 mimo-v2.5）
    pub video_model: String,
    /// 视频理解专用接口地址
    pub video_base_url: String,
    /// 视频理解专用 API Key
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
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);

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
        match csv::Reader::from_path(&path) {
            Ok(mut reader) => {
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
            Err(_) => { /* optional — no prompts is fine */ }
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
    store: ChatStore,
    event_bus: Addr<EventBus>,
}

impl LlmActor {
    pub fn from_env(event_bus: Addr<EventBus>) -> Option<Self> {
        let config = match LlmConfig::from_env() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[LlmActor] config failed: {}", e);
                return None;
            }
        };

        let db_path = ChatStore::db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let store = match ChatStore::open(db_path.to_str().unwrap_or("data/chat_history.db")) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[LlmActor] failed to open chat store: {}", e);
                return None;
            }
        };

        // Clear history on startup (development convenience).
        if let Err(e) = store.clear_all() {
            log::warn!("[LlmActor] clear_all failed: {}", e);
        } else {
            log::info!("[LlmActor] chat history cleared");
        }

        let api_base = config.base_url.trim_end_matches('/');

        log::info!(
            "[LlmActor] endpoint={}/chat/completions model={} max_history={}",
            api_base,
            config.model,
            config.max_history_turns,
        );

        Some(Self {
            client: reqwest::Client::new(),
            config,
            store,
            event_bus,
        })
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

        // Build messages from history + contexts + current message.
        let jailbreak = msg.jailbreak_index.and_then(|i| self.config.jailbreak_at(i));
        let system_content = if let Some(jb) = jailbreak {
            format!("{}\n\n{}", jb, self.config.system_prompt)
        } else {
            self.config.system_prompt.clone()
        };

        let mut messages: Vec<serde_json::Value> = vec![
            serde_json::json!({ "role": "system", "content": system_content }),
        ];

        // Load history from SQLite.
        let limit = self.config.max_history_turns * 2;
        match self.store.recent(msg.chat_id, limit) {
            Ok(records) => {
                for r in records {
                    messages.push(serde_json::json!({
                        "role": r.role, "content": r.content
                    }));
                }
            }
            Err(e) => log::warn!("[LlmActor] load history failed (chat_id={}): {}", msg.chat_id, e),
        }

        // Inject plugin passive contexts.
        for ctx in &msg.contexts {
            messages.push(serde_json::json!({
                "role": "assistant", "content": ctx
            }));
        }

        // Current user message (支持多模态图片/视频).
        let user_content = if let Some(ref img_b64) = msg.image_base64 {
            serde_json::json!([
                {"type": "text", "text": msg.message},
                {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", img_b64)}}
            ])
        } else if let (Some(ref vid_b64), Some(ref vid_mime)) = (msg.video_base64.as_ref(), msg.video_mime.as_ref()) {
            serde_json::json!([
                {
                    "type": "video_url",
                    "video_url": {"url": format!("data:{};base64,{}", vid_mime, vid_b64)},
                    "fps": 2,
                    "media_resolution": "default"
                },
                {"type": "text", "text": msg.message}
            ])
        } else {
            serde_json::json!(msg.message)
        };
        messages.push(serde_json::json!({
            "role": "user", "content": user_content
        }));

        // 图片/视频分别用专用模型 + 接口地址（mimo-v2.5-pro 不支持多模态）
        let actual_model = if msg.video_base64.is_some() {
            self.config.video_model.clone()
        } else if msg.image_base64.is_some() {
            self.config.image_model.clone()
        } else {
            model.clone()
        };
        let actual_base_url = if msg.video_base64.is_some() {
            self.config.video_base_url.clone()
        } else if msg.image_base64.is_some() {
            self.config.image_base_url.clone()
        } else {
            self.config.base_url.clone()
        };
        let actual_api_key = if msg.video_base64.is_some() {
            self.config.video_api_key.clone()
        } else if msg.image_base64.is_some() {
            self.config.image_api_key.clone()
        } else {
            self.config.api_key.clone()
        };

        // 用实际 URL/key 覆盖原值
        let api_url = format!("{}/chat/completions", actual_base_url.trim_end_matches('/'));
        let api_key = actual_api_key;

        let mut body = serde_json::json!({
            "model": actual_model,
            "messages": messages,
            "temperature": 0.7,
            "max_tokens": 2048u32,
        });

        // Attach tools if provided.
        if !msg.tools.is_empty() {
            body["tools"] = serde_json::json!(msg.tools);
        }

        let chat_id = msg.chat_id;
        let user_msg = msg.message.clone();
        let skip_store = msg.skip_store;
        let self_addr = _ctx.address();

        event_bus.do_send(Event::new("llm.request", serde_json::json!({
            "model": model, "chat_id": chat_id, "mode": "chat",
            "tool_count": msg.tools.len(), "context_count": msg.contexts.len(),
        }), "llm-actor"));

        let fut = async move {
            let result = call_llm(&client, &api_url, &api_key, &body, &event_bus).await;

            // Persist to history (unless skip_store).
            if !skip_store {
                if let Ok(ref resp) = result {
                    if !resp.content.trim().is_empty() {
                        let _ = self_addr.send(AppendHistory {
                            chat_id,
                            user_msg,
                            assistant_msg: resp.content.clone(),
                        }).await;
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
#[rtype(result = "()")]
struct AppendHistory {
    chat_id: i64,
    user_msg: String,
    assistant_msg: String,
}

impl Handler<AppendHistory> for LlmActor {
    type Result = ();
    fn handle(&mut self, msg: AppendHistory, _: &mut Self::Context) {
        if let Err(e) = self.store.append(msg.chat_id, "user", &msg.user_msg) {
            log::error!("[LlmActor] append user failed: {}", e);
        }
        if let Err(e) = self.store.append(msg.chat_id, "assistant", &msg.assistant_msg) {
            log::error!("[LlmActor] append assistant failed: {}", e);
        }
    }
}

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
        match self.store.clear(msg.0) {
            Ok(n) => log::info!("[LlmActor] cleared {} records for chat_id={}", n, msg.0),
            Err(e) => log::error!("[LlmActor] clear session failed: {}", e),
        }
    }
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

    let choice = json["choices"][0].clone();
    let content = choice["message"]["content"].as_str().unwrap_or("").to_string();

    let usage = &json["usage"];
    let prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
    let completion_tokens = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;

    // Parse tool calls.
    let tool_calls: Vec<ToolCall> = choice["message"]["tool_calls"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    let id = tc["id"].as_str()?.to_string();
                    let name = tc["function"]["name"].as_str()?.to_string();
                    let arguments: serde_json::Value =
                        serde_json::from_str(tc["function"]["arguments"].as_str()?)
                            .unwrap_or(serde_json::Value::Null);
                    Some(ToolCall { id, name, arguments })
                })
                .collect()
        })
        .unwrap_or_default();

    let tool_calls_count = tool_calls.len();

    let llm_response = LlmResponse {
        content: content.clone(),
        model: json["model"].as_str().unwrap_or("").to_string(),
        prompt_tokens,
        completion_tokens,
        tool_calls,
    };

    let preview = {
        let max = content.len().min(200);
        let cut = content.char_indices()
            .take_while(|(i, _)| *i < max)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        &content[..cut]
    };
    event_bus.do_send(Event::new("llm.response", serde_json::json!({
        "content_preview": preview,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "tool_calls_count": tool_calls_count,
    }), "llm-actor"));

    Ok(llm_response)
}
