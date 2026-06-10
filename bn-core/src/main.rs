//! BN Agent 主程序

mod models {
    pub mod event_bus;
    pub mod plugin_loader;
}
mod llm;

use actix::prelude::*;
use llm::client::{ChatRequest, LlmActor, LlmConfig};
use models::event_bus::{BusEmitter, EventBus, EmitEvent, RegisterCallback};
use models::plugin_loader::{BroadcastEvent, PluginManager, ScanAndLoad, SetToolRegistry, StopAll};
use plugin_core::{
    AgentEvent, EventEmitter, EventSource, EventType, HostContext, LogCallback, LogLevel, ToolRegistry,
};
use std::sync::{Arc, Mutex};

fn main() -> std::io::Result<()> {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(&env_path);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let system = actix_rt::System::new();
    let mut started = false;

    system.block_on(async {
        tracing::info!("BN Agent 启动");
        started = true;

        // 初始化 LLM Actor
        let llm_addr: Option<Addr<LlmActor>> = match LlmConfig::from_env() {
            Ok(config) => {
                tracing::info!("LLM 模型: {} @ {}", config.model, config.base_url);
                match LlmActor::new(config) {
                    Ok(actor) => Some(actor.start()),
                    Err(e) => {
                        tracing::error!("LLM Actor 创建失败: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!("LLM 未配置: {}", e);
                None
            }
        };

        let event_bus = EventBus::new().start();
        let plugin_manager = PluginManager::new("./plugins").start();
        let tool_registry = Arc::new(Mutex::new(ToolRegistry::new()));

        plugin_manager.send(SetToolRegistry(tool_registry.clone())).await.ok();

        struct TracingLogger;
        impl LogCallback for TracingLogger {
            fn log(&self, level: LogLevel, target: &str, message: &str) {
                match level {
                    LogLevel::Error => tracing::error!(target = target, "{}", message),
                    LogLevel::Warn => tracing::warn!(target = target, "{}", message),
                    LogLevel::Info => tracing::info!(target = target, "{}", message),
                    LogLevel::Debug => tracing::debug!(target = target, "{}", message),
                    LogLevel::Trace => tracing::trace!(target = target, "{}", message),
                }
            }
        }

        let emitter = Arc::new(BusEmitter::new(event_bus.clone()));
        let ctx = HostContext::new("BN Agent", "0.1.0", "./plugins")
            .with_emitter(emitter.clone())
            .with_logger(Arc::new(TracingLogger))
            .with_tool_registry(tool_registry.clone());

        match plugin_manager.send(ScanAndLoad(ctx)).await {
            Ok(Ok(n)) => tracing::info!("已加载 {} 个插件", n),
            Ok(Err(e)) => tracing::error!("插件加载失败: {}", e),
            Err(e) => tracing::error!("Actor 通信失败: {}", e),
        }

        // 注册事件回调：UserMessage → LlmActor → AssistantMessage → 广播给插件
        let emitter_for_cb = emitter.clone();
        let llm_for_cb = llm_addr.clone();
        let pm_for_cb = plugin_manager.clone();
        event_bus
            .send(RegisterCallback(Arc::new(move |event: &AgentEvent| {
                match event.event_type {
                    EventType::UserMessage => {
                        let text = event.data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let chat_id = event.data.get("chat_id").and_then(|v| v.as_i64());
                        let user_name = event.data.get("user_name").and_then(|v| v.as_str()).unwrap_or("unknown");

                        tracing::info!("[MSG] @{}: {}", user_name, text);

                        if let Some(ref llm) = llm_for_cb {
                            let llm = llm.clone();
                            let emitter = emitter_for_cb.clone();
                            let pm = pm_for_cb.clone();
                            let text = text.to_string();

                            actix::spawn(async move {
                                let req = ChatRequest {
                                    chat_id: chat_id.unwrap_or(0),
                                    message: text.clone(),
                                    json_mode: false,
                                };
                                let reply_event = match llm.send(req).await {
                                    Ok(Ok(resp)) => {
                                        let preview: String = resp.content
                                            .chars()
                                            .take(80)
                                            .collect();
                                        tracing::info!(
                                            "[LLM] 回复: {} | 缓存命中: {} tokens",
                                            preview,
                                            resp.cache_hit_tokens,
                                        );
                                        AgentEvent::new(
                                            EventType::AssistantMessage,
                                            EventSource::System,
                                            serde_json::json!({
                                                "chat_id": chat_id,
                                                "text": resp.content,
                                            }),
                                        )
                                    }
                                    Ok(Err(e)) => {
                                        tracing::error!("[LLM] 调用失败: {}", e);
                                        AgentEvent::new(
                                            EventType::AssistantMessage,
                                            EventSource::System,
                                            serde_json::json!({
                                                "chat_id": chat_id,
                                                "text": format!("抱歉，出错了: {}", e),
                                            }),
                                        )
                                    }
                                    Err(e) => {
                                        tracing::error!("[LLM] Actor 通信失败: {}", e);
                                        return;
                                    }
                                };
                                // 发射到 EventBus + 广播给插件
                                emitter.emit(reply_event.clone());
                                let _ = pm.send(BroadcastEvent(reply_event)).await;
                            });
                        } else {
                            let reply = AgentEvent::new(
                                EventType::AssistantMessage,
                                EventSource::System,
                                serde_json::json!({
                                    "chat_id": chat_id,
                                    "text": format!("你说: {}", text),
                                }),
                            );
                            emitter_for_cb.emit(reply.clone());
                            let _ = pm_for_cb.send(BroadcastEvent(reply));
                        }
                    }
                    _ => {
                        tracing::debug!("[EventBus] 收到事件: {:?}", event.event_type);
                    }
                }
            })))
            .await
            .ok();

        event_bus
            .send(EmitEvent(AgentEvent::new(
                EventType::SystemEvent,
                EventSource::System,
                serde_json::json!({"message": "系统启动完成"}),
            )))
            .await
            .ok();

        plugin_manager
            .send(BroadcastEvent(AgentEvent::new(
                EventType::PluginNotification,
                EventSource::System,
                serde_json::json!({"message": "宿主已就绪"}),
            )))
            .await
            .ok();

        tracing::info!("BN Agent 运行中，按 Ctrl+C 退出...");

        tokio::signal::ctrl_c().await.ok();
        tracing::info!("收到退出信号");

        plugin_manager.send(StopAll).await.ok();
        drop(plugin_manager);
        drop(event_bus);
        drop(tool_registry);
        // llm_addr 也会被 drop，Actor 自动停止

        tracing::info!("BN Agent 退出");
        actix_rt::System::current().stop();
    });

    if started {
        let _ = system.run();
    }

    Ok(())
}
