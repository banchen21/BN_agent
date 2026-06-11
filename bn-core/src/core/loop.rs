//! 核心循环：事件回调 → LLM 对话 + 工具调用
//! 从 main.rs 抽离，保持主文件简洁。

use actix::prelude::*;
use std::sync::{Arc, Mutex};

use crate::models::llm::client::{ChatRequest, LlmActor};
use crate::models::event_bus::BusEmitter;
use crate::models::plugin_loader::{BroadcastEvent, PluginManager};
use plugin_core::{
    AgentEvent, EventEmitter, EventSource, EventType, ToolRegistry,
};

/// 处理一次 UserMessage：发 LLM 请求 → 执行工具调用 → 回复
pub async fn handle_user_message(
    text: &str,
    chat_id: Option<i64>,
    source: &str,
    llm: &Addr<LlmActor>,
    emitter: &Arc<BusEmitter>,
    pm: &Addr<PluginManager>,
    tool_registry: &Arc<Mutex<ToolRegistry>>,
    snapshots: &Arc<Mutex<Vec<String>>>,
    contexts: Vec<String>,
) {
    // 从 ToolRegistry 获取工具定义（只暴露非 internal 工具给 LLM）
    let tools: Vec<serde_json::Value> = {
        match tool_registry.lock() {
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
        }
    };

    let cid = chat_id.unwrap_or(0);

    // 第一次 LLM 调用
    let req = ChatRequest {
        chat_id: cid,
        message: text.to_string(),
        json_mode: false,
        tools: tools.clone(),
        skip_store: false,
        contexts,
    };

    let resp = match llm.send(req).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::error!("[LLM] 调用失败: {}", e);
            emit_reply(cid, &format!("抱歉，出错了: {}", e), source, emitter, pm).await;
            return;
        }
        Err(e) => {
            tracing::error!("[LLM] Actor 通信失败: {}", e);
            return;
        }
    };

    // 有工具调用 → 执行工具 → 不二次 LLM 追问（工具本身已完成回复）
    if !resp.tool_calls.is_empty() {
        tracing::info!("[LLM] 工具调用: {}",
            resp.tool_calls.iter().map(|tc| tc.name.clone()).collect::<Vec<_>>().join(", "));

        let real_chat_id = cid;
        let executors: Vec<(String, Arc<dyn plugin_core::ToolExecutor>, serde_json::Value)> = {
            match tool_registry.lock() {
                Ok(reg) => resp.tool_calls.iter().filter_map(|tc| {
                    reg.get_executor(&tc.name).map(|e| {
                        let mut args = tc.arguments.clone();
                        if let serde_json::Value::Object(ref mut map) = args {
                            map.entry("chat_id").or_insert(serde_json::json!(real_chat_id));
                        }
                        (tc.id.clone(), e, args)
                    })
                }).collect(),
                Err(_) => vec![],
            }
        };

        for (id, executor, args) in executors {
            let name = executor.def().name.clone();
            tracing::info!("[LLM] 执行工具: {} (id={})", name, id);
            let result = executor.execute(&args);
            if result.success {
                tracing::info!("[LLM] 工具完成: {} → {}", name, result.content);
            } else {
                tracing::warn!("[LLM] 工具失败: {} → {}", name,
                    result.error.as_deref().unwrap_or("未知错误"));
            }
        }
        // 工具调用后不再请求 LLM 追问，工具本身已负责回复
        // 如需文字回复，LLM 应显式调用 tg_send_message 等工具
    } else {
        // 无工具调用，直接回复
        let preview: String = resp.content.chars().take(80).collect();
        tracing::info!("[LLM] 回复: {} | 缓存命中: {} tokens", preview, resp.cache_hit_tokens);

        let reply_text = if resp.content.trim().is_empty() && resp.cache_hit_tokens > 0 {
            // DeepSeek 缓存命中导致空回复，重试一次（不带工具、不存DB）
            tracing::warn!("[LLM] 空回复（缓存命中 {} tokens），重试中...", resp.cache_hit_tokens);
            let retry_req = ChatRequest {
                chat_id: cid,
                message: text.to_string(),
                json_mode: false,
                tools: vec![],
                skip_store: true,
                contexts: vec![],
            };
            match llm.send(retry_req).await {
                Ok(Ok(r)) => {
                    tracing::info!("[LLM] 重试回复: {}", r.content.chars().take(80).collect::<String>());
                    r.content
                }
                _ => String::new(),
            }
        } else {
            resp.content
        };

        if !reply_text.trim().is_empty() {
            emit_reply(cid, &reply_text, source, emitter, pm).await;
        }
    }
}

/// 广播 AssistantMessage
async fn emit_reply(
    chat_id: i64,
    text: &str,
    source: &str,
    emitter: &Arc<BusEmitter>,
    pm: &Addr<PluginManager>,
) {
    let reply = AgentEvent::new(
        EventType::AssistantMessage,
        EventSource::System,
        serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "source": source,
        }),
    );
    emitter.emit(reply.clone());
    let _ = pm.send(BroadcastEvent(reply)).await;
}
