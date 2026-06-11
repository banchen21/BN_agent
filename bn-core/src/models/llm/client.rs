//! LLM Actor — 基于 async-openai 的 OpenAI 兼容 API 客户端
//!
//! 支持：
//! - 多轮对话（SQLite 持久化历史，利用 DeepSeek KV Cache）
//! - Function Calling（tool calls）
//! - JSON Output（response_format: json_object）

use actix::prelude::*;
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestAssistantMessageArgs,
        ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs,
        ChatCompletionTool,
        ChatCompletionToolArgs,
        ChatCompletionToolType,
        CreateChatCompletionRequestArgs,
        FunctionObject,
        ResponseFormat,
    },
    Client,
};
use serde::{Deserialize, Serialize};

use super::store::ChatStore;

/// LLM 配置
#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub system_prompt: String,
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

        let system_prompt = Self::load_persona().unwrap_or_else(|| {
            "你是一个有用的 AI 助手。请用简洁的中文回答。".into()
        });

        let max_history_turns = std::env::var("LLM_MAX_HISTORY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);

        Ok(Self { api_key, model, base_url, system_prompt, max_history_turns })
    }

    /// 从 persona.md 读取人格设定，找不到则返回 None
    fn load_persona() -> Option<String> {
        let persona_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("persona.md");
        match std::fs::read_to_string(&persona_path) {
            Ok(content) if !content.trim().is_empty() => Some(content.trim().to_string()),
            _ => None,
        }
    }
}

// ── Actor 定义 ──

pub struct LlmActor {
    config: LlmConfig,
    client: Client<OpenAIConfig>,
    store: ChatStore,
}

impl LlmActor {
    pub fn new(config: LlmConfig) -> Result<Self, String> {
        let openai_config = OpenAIConfig::new()
            .with_api_base(config.base_url.trim_end_matches('/'))
            .with_api_key(&config.api_key);

        let client = Client::with_config(openai_config);

        let db_path = ChatStore::db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let store = ChatStore::open(db_path.to_str().unwrap_or("data/chat_history.db"))?;

        // TODO: 未发布版本前保留,启动时清空历史聊天记录
        if let Err(e) = store.clear_all() {
            tracing::warn!("清空聊天记录失败: {}", e);
        } else {
            tracing::info!("聊天记录已清空");
        }

        tracing::info!("聊天记录数据库: {}", db_path.display());
        Ok(Self { config, client, store })
    }

    fn build_messages(&self, chat_id: i64, user_msg: &str, contexts: &[String]) -> Vec<ChatCompletionRequestMessage> {
        let mut messages: Vec<ChatCompletionRequestMessage> = vec![
            ChatCompletionRequestSystemMessageArgs::default()
                .content(self.config.system_prompt.as_str())
                .build()
                .unwrap()
                .into(),
        ];

        let limit = self.config.max_history_turns * 2;
        match self.store.recent(chat_id, limit) {
            Ok(records) => {
                for r in records {
                    let msg = match r.role.as_str() {
                        "user" => ChatCompletionRequestUserMessageArgs::default()
                            .content(r.content)
                            .build()
                            .unwrap()
                            .into(),
                        "assistant" => ChatCompletionRequestAssistantMessageArgs::default()
                            .content(r.content)
                            .build()
                            .unwrap()
                            .into(),
                        _ => continue,
                    };
                    messages.push(msg);
                }
            }
            Err(e) => {
                tracing::warn!("加载聊天记录失败 (chat_id={}): {}", chat_id, e);
            }
        }

        // 插件实时上下文（不存 DB，临时注入）
        for ctx in contexts {
            messages.push(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .content(ctx.as_str())
                    .build()
                    .unwrap()
                    .into(),
            );
        }

        messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(user_msg)
                .build()
                .unwrap()
                .into(),
        );

        messages
    }
}

impl Actor for LlmActor {
    type Context = Context<Self>;
}

// ── 消息类型 ──

#[derive(Message)]
#[rtype(result = "Result<LlmResponse, String>")]
pub struct ChatRequest {
    pub chat_id: i64,
    pub message: String,
    pub json_mode: bool,
    pub tools: Vec<serde_json::Value>,
    /// 跳过存储：工具调用中间请求不存聊天记录
    pub skip_store: bool,
    /// 插件实时上下文：不存 DB，临时注入到 messages 中
    pub contexts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub content: String,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct ClearSession(pub i64);

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
        let messages = self.build_messages(msg.chat_id, &msg.message, &msg.contexts);
        let client = self.client.clone();
        let model = self.config.model.clone();
        let json_mode = msg.json_mode;
        let tools = msg.tools;
        let chat_id = msg.chat_id;
        let user_msg = msg.message.clone();
        let self_addr = ctx.address();

        Box::pin(async move {
            let mut request_builder = CreateChatCompletionRequestArgs::default();
            request_builder.model(&model);
            request_builder.messages(messages);
            request_builder.temperature(0.7);
            request_builder.max_tokens(2048u32);

            if json_mode {
                request_builder.response_format(ResponseFormat::JsonObject);
            }

            if !tools.is_empty() {
                let openai_tools: Vec<ChatCompletionToolArgs> = tools
                    .iter()
                    .filter_map(|t| {
                        let func = t.get("function")?;
                        let name = func.get("name")?.as_str()?;
                        let desc = func.get("description").and_then(|v| v.as_str()).unwrap_or("");
                        let params = func.get("parameters").cloned();

                        let fo = FunctionObject {
                            name: name.to_string(),
                            description: Some(desc.to_string()),
                            parameters: params,
                            strict: None,
                        };

                        Some(ChatCompletionToolArgs::default()
                            .r#type(ChatCompletionToolType::Function)
                            .function(fo)
                            .clone())
                    })
                    .collect();

                if !openai_tools.is_empty() {
                    let tools: Vec<ChatCompletionTool> = openai_tools
                        .into_iter()
                        .map(|t| t.build().unwrap())
                        .collect();
                    request_builder.tools(tools);
                }
            }

            let request = request_builder.build().map_err(|e| format!("构建请求失败: {}", e))?;

            let response = client.chat().create(request).await
                .map_err(|e| format!("LLM 调用失败: {}", e))?;

            let choice = response.choices.first()
                .ok_or_else(|| "LLM 返回空 choices".to_string())?;

            let content = choice.message.content.clone().unwrap_or_default();

            let cache_hit = response.usage
                .as_ref()
                .and_then(|u| u.prompt_tokens_details.as_ref())
                .and_then(|d| d.cached_tokens)
                .unwrap_or(0) as u64;

            let tool_calls: Vec<ToolCall> = choice.message.tool_calls.as_ref()
                .map(|tc_list| {
                    tc_list.iter().filter_map(|tc| {
                        let id = tc.id.clone();
                        let name = tc.function.name.clone();
                        let arguments: serde_json::Value = serde_json::from_str(
                            &tc.function.arguments
                        ).unwrap_or(serde_json::Value::Null);
                        Some(ToolCall { id, name, arguments })
                    }).collect()
                })
                .unwrap_or_default();

            if !msg.skip_store && !content.trim().is_empty() {
                let _ = self_addr.send(AppendHistory {
                    chat_id,
                    user_msg,
                    assistant_msg: content.clone(),
                }).await;
            }

            Ok(LlmResponse {
                content,
                cache_hit_tokens: cache_hit,
                cache_miss_tokens: 0,
                tool_calls,
            })
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
