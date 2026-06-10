//! LLM Actor — 基于 actix 的 OpenAI 兼容 API 客户端
//!
//! 支持：
//! - 多轮对话（SQLite 持久化历史，利用 DeepSeek KV Cache）
//! - JSON Output（response_format: json_object）

use actix::prelude::*;
use serde::{Deserialize, Serialize};

use super::store::ChatStore;

/// 聊天消息
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: &str) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: &str) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

/// LLM 配置
#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub system_prompt: String,
    /// 每个会话最多保留的历史轮数
    pub max_history_turns: usize,
}

impl LlmConfig {
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("LLM_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| "LLM_API_KEY 或 OPENAI_API_KEY 未设置".to_string())?;

        let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
        let base_url = std::env::var("LLM_BASE_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com/v1".into());

        let system_prompt = std::env::var("LLM_SYSTEM_PROMPT")
            .unwrap_or_else(|_| "你是一个有用的 AI 助手。请用简洁的中文回答。".into());

        let max_history_turns = std::env::var("LLM_MAX_HISTORY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);

        Ok(Self { api_key, model, base_url, system_prompt, max_history_turns })
    }
}

// ── Actor 定义 ──

/// LLM Actor — SQLite 持久化多轮对话
pub struct LlmActor {
    config: LlmConfig,
    client: reqwest::Client,
    store: ChatStore,
}

impl LlmActor {
    pub fn new(config: LlmConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

        let db_path = ChatStore::db_path();
        // 确保 data 目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let store = ChatStore::open(db_path.to_str().unwrap_or("data/chat_history.db"))?;

        tracing::info!("聊天记录数据库: {}", db_path.display());
        Ok(Self { config, client, store })
    }

    /// 从 SQLite 加载历史，构建消息列表
    fn build_messages(&self, chat_id: i64, user_msg: &str) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system(&self.config.system_prompt)];

        let limit = self.config.max_history_turns * 2;
        match self.store.recent(chat_id, limit) {
            Ok(records) => {
                for r in records {
                    messages.push(ChatMessage {
                        role: r.role,
                        content: r.content,
                    });
                }
            }
            Err(e) => {
                tracing::warn!("加载聊天记录失败 (chat_id={}): {}", chat_id, e);
            }
        }

        messages.push(ChatMessage::user(user_msg));
        messages
    }

    /// 执行 HTTP 请求
    async fn do_chat(
        client: &reqwest::Client,
        config: &LlmConfig,
        messages: &[ChatMessage],
        json_mode: bool,
    ) -> Result<(String, serde_json::Value), String> {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

        let mut body = serde_json::json!({
            "model": config.model,
            "messages": messages,
            "temperature": 0.7,
            "max_tokens": 2048,
        });

        if json_mode {
            body["response_format"] = serde_json::json!({ "type": "json_object" });
        }

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("读取响应失败: {}", e))?;

        if !status.is_success() {
            return Err(format!("API 错误 ({}): {}", text, status));
        }

        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))?;

        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("响应格式异常: {}", text))?;

        let usage = json.get("usage").cloned().unwrap_or_default();

        Ok((content, usage))
    }
}

impl Actor for LlmActor {
    type Context = Context<Self>;
}

// ── 消息类型 ──

/// 多轮对话请求（带 chat_id，SQLite 持久化历史）
#[derive(Message)]
#[rtype(result = "Result<LlmResponse, String>")]
pub struct ChatRequest {
    pub chat_id: i64,
    pub message: String,
    /// 是否启用 JSON Output
    pub json_mode: bool,
}

/// LLM 响应
#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub content: String,
    /// 缓存命中 tokens
    pub cache_hit_tokens: u64,
    /// 缓存未命中 tokens
    pub cache_miss_tokens: u64,
}

/// 清除指定会话
#[derive(Message)]
#[rtype(result = "()")]
pub struct ClearSession(pub i64);

/// 内部消息：追加历史记录到 SQLite
#[derive(Message)]
#[rtype(result = "()")]
struct AppendHistory {
    chat_id: i64,
    user_msg: String,
    assistant_msg: String,
}

// ── Handler ──

impl Handler<ChatRequest> for LlmActor {
    type Result = ResponseFuture<Result<LlmResponse, String>>;

    fn handle(&mut self, msg: ChatRequest, ctx: &mut Self::Context) -> Self::Result {
        let messages = self.build_messages(msg.chat_id, &msg.message);
        let client = self.client.clone();
        let config = self.config.clone();
        let json_mode = msg.json_mode;
        let chat_id = msg.chat_id;
        let user_msg = msg.message.clone();
        let self_addr = ctx.address();

        Box::pin(async move {
            let (content, usage) = LlmActor::do_chat(&client, &config, &messages, json_mode).await?;

            let cache_hit = usage["prompt_cache_hit_tokens"].as_u64().unwrap_or(0);
            let cache_miss = usage["prompt_cache_miss_tokens"].as_u64().unwrap_or(0);

            // 追加历史记录到 SQLite
            let _ = self_addr.send(AppendHistory {
                chat_id,
                user_msg,
                assistant_msg: content.clone(),
            }).await;

            Ok(LlmResponse { content, cache_hit_tokens: cache_hit, cache_miss_tokens: cache_miss })
        })
    }
}

impl Handler<AppendHistory> for LlmActor {
    type Result = ();
    fn handle(&mut self, msg: AppendHistory, _: &mut Self::Context) {
        if let Err(e) = self.store.append(msg.chat_id, "user", &msg.user_msg) {
            tracing::error!("写入 user 消息失败: {}", e);
        }
        if let Err(e) = self.store.append(msg.chat_id, "assistant", &msg.assistant_msg) {
            tracing::error!("写入 assistant 消息失败: {}", e);
        }
    }
}

impl Handler<ClearSession> for LlmActor {
    type Result = ();
    fn handle(&mut self, msg: ClearSession, _: &mut Self::Context) {
        match self.store.clear(msg.0) {
            Ok(n) => tracing::info!("已清除 chat_id={} 的 {} 条记录", msg.0, n),
            Err(e) => tracing::error!("清除会话失败: {}", e),
        }
    }
}
