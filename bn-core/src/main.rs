//! BN Agent 主程序

mod models {
    pub mod event_bus;
    pub mod plugin_loader;
}
mod llm;
mod api;

use actix::prelude::*;
use llm::client::{ChatRequest, LlmActor, LlmConfig};
use models::event_bus::{BusEmitter, EventBus, EmitEvent, RegisterCallback};
use models::plugin_loader::{BroadcastEvent, PluginManager, RefreshSnapshots, ScanAndLoad, SetToolRegistry, StopAll};
use plugin_core::{
    AgentEvent, EventEmitter, EventSource, EventType, HostContext, LogCallback, LogLevel, ToolRegistry,
};
use std::sync::{Arc, Mutex};
use tracing_subscriber::prelude::*;

fn main() -> std::io::Result<()> {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if let Err(e) = dotenvy::from_path(&env_path) {
        eprintln!("[main] 警告: .env 加载失败: {}", e);
    }

    // 清除终端可能遗留的全局代理变量（避免所有模块走代理）
    for var in &["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
        std::env::remove_var(var);
    }

    // 日志文件（在 bn-core 目录下）
    let log_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let log_file = std::fs::File::create(log_dir.join("bn-agent.log")).expect("无法创建日志文件");

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // 控制台输出
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(true);

    // 文件输出（纯文本，无颜色）
    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(log_file));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
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
        // 从 cargo build 输出目录加载 DLL，避免手动复制
        let plugin_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug");
        let plugin_dir_str = plugin_dir.to_str().unwrap_or("./plugins");
        let plugin_manager = PluginManager::new(plugin_dir_str);
        let snapshots_for_cb = plugin_manager.snapshots_arc();
        let plugin_manager = plugin_manager.start();
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
        let ctx = HostContext::new("BN Agent", "0.1.0", plugin_dir_str)
            .with_emitter(emitter.clone())
            .with_logger(Arc::new(TracingLogger))
            .with_tool_registry(tool_registry.clone());

        match plugin_manager.send(ScanAndLoad(ctx)).await {
            Ok(Ok(n)) => tracing::info!("已加载 {} 个插件", n),
            Ok(Err(e)) => tracing::error!("插件加载失败: {}", e),
            Err(e) => tracing::error!("Actor 通信失败: {}", e),
        }

        // 注册事件回调：UserMessage → LlmActor（带 tool calling）→ AssistantMessage → 广播给插件
        let emitter_for_cb = emitter.clone();
        let llm_for_cb = llm_addr.clone();
        let pm_for_cb = plugin_manager.clone();
        let tool_registry_for_cb = tool_registry.clone();
        event_bus
            .send(RegisterCallback(Arc::new(move |event: &AgentEvent| -> bool {
                match event.event_type {
                    EventType::UserMessage => {
                        let text = event.data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let chat_id = event.data.get("chat_id").and_then(|v| v.as_i64());
                        let user_name = event.data.get("user_name").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");

                        tracing::info!("[MSG] @{}: {}", user_name, text);

                        if let Some(ref llm) = llm_for_cb {
                            let llm = llm.clone();
                            let emitter = emitter_for_cb.clone();
                            let pm = pm_for_cb.clone();
                            let text = text.to_string();
                            let source = source.to_string();
                            let tool_registry = tool_registry_for_cb.clone();
                            let snapshots = snapshots_for_cb.clone();

                            actix::spawn(async move {
                                // 刷新被动上下文快照
                                let _ = pm.send(RefreshSnapshots).await;

                                // 读取最新快照
                                let contexts: Vec<String> = snapshots.lock().unwrap().clone();

                                // 从 ToolRegistry 获取工具定义（只暴露给 LLM 非 internal 工具）
                                let tools: Vec<serde_json::Value> = {
                                    match tool_registry.lock() {
                                        Ok(reg) => reg.all_defs().iter()
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
                                        }).collect(),
                                        Err(_) => vec![],
                                    }
                                };

                                // 第一次 LLM 调用（原生 tool calling，不需要 json_mode）
                                let req = ChatRequest {
                                    chat_id: chat_id.unwrap_or(0),
                                    message: text.clone(),
                                    json_mode: false,
                                    tools: tools.clone(),
                                    skip_store: false,
                                    contexts,
                                };

                                let resp = match llm.send(req).await {
                                    Ok(Ok(r)) => r,
                                    Ok(Err(e)) => {
                                        tracing::error!("[LLM] 调用失败: {}", e);
                                        let reply = AgentEvent::new(
                                            EventType::AssistantMessage,
                                            EventSource::System,
                                            serde_json::json!({
                                                "chat_id": chat_id,
                                                "text": format!("抱歉，出错了: {}", e),
                                                "source": source,
                                            }),
                                        );
                                        emitter.emit(reply.clone());
                                        let _ = pm.send(BroadcastEvent(reply)).await;
                                        return;
                                    }
                                    Err(e) => {
                                        tracing::error!("[LLM] Actor 通信失败: {}", e);
                                        return;
                                    }
                                };

                                // 检查是否有 tool_calls
                                if !resp.tool_calls.is_empty() {
                                    tracing::info!(
                                        "[LLM] 工具调用: {}",
                                        resp.tool_calls.iter()
                                            .map(|tc| tc.name.clone())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    );

                                    // 先收集工具执行器（克隆 Arc 后释放锁），再执行。
                                    // 避免跨插件工具调用（如 send_voice → tts）时发生 Mutex 死锁。
                                    // 同时把真实的 chat_id 注入工具参数，LLM 不需要知道 chat_id
                                    let real_chat_id = chat_id.unwrap_or(0);
                                    let executors: Vec<(String, Arc<dyn plugin_core::ToolExecutor>, serde_json::Value)> = {
                                        match tool_registry.lock() {
                                            Ok(reg) => resp.tool_calls.iter().filter_map(|tc| {
                                                reg.get_executor(&tc.name).map(|e| {
                                                    // 注入真实 chat_id
                                                    let mut args = tc.arguments.clone();
                                                    if let serde_json::Value::Object(ref mut map) = args {
                                                        map.entry("chat_id")
                                                            .or_insert(serde_json::json!(real_chat_id));
                                                    }
                                                    (tc.id.clone(), e, args)
                                                })
                                            }).collect(),
                                            Err(_) => vec![],
                                        }
                                    };

                                    let tool_results: Vec<(String, String)> = executors.into_iter().map(
                                        |(id, executor, args)| {
                                            let name = executor.def().name.clone();
                                            tracing::info!("[LLM] 执行工具: {} (id={})", name, id);
                                            let result = executor.execute(&args);
                                            let text = if result.success {
                                                tracing::info!("[LLM] 工具完成: {} → {}", name, result.content);
                                                result.content
                                            } else {
                                                let err = format!("错误: {}", result.error.as_deref().unwrap_or("未知错误"));
                                                tracing::warn!("[LLM] 工具失败: {} → {}", name, err);
                                                err
                                            };
                                            (id, text)
                                        }
                                    ).collect();

                                    // 构建 tool 结果消息，再次调用 LLM
                                    // 注意：这里简化处理，不维护完整的消息历史用于 tool calling
                                    // 直接让 LLM 基于工具结果生成最终回复
                                    let mut tool_result_text = String::from("工具执行结果：\n");
                                    for (id, result) in &tool_results {
                                        tool_result_text.push_str(&format!("[{}] {}\n", id, result));
                                    }

                                    let followup_req = ChatRequest {
                                        chat_id: chat_id.unwrap_or(0),
                                        message: format!(
                                            "用户原始消息: {}\n\n{}",
                                            text, tool_result_text
                                        ),
                                        json_mode: false,
                                        tools: vec![], // 不再传工具，避免循环
                                        skip_store: true,
                                        contexts: vec![],
                                    };

                                    match llm.send(followup_req).await {
                                        Ok(Ok(followup_resp)) => {
                                            let preview: String = followup_resp.content
                                                .chars()
                                                .take(80)
                                                .collect();
                                            tracing::info!("[LLM] 工具后回复: {}", preview);
                                            let reply = AgentEvent::new(
                                                EventType::AssistantMessage,
                                                EventSource::System,
                                                serde_json::json!({
                                                    "chat_id": chat_id,
                                                    "text": followup_resp.content,
                                                    "source": source,
                                                }),
                                            );
                                            emitter.emit(reply.clone());
                                            let _ = pm.send(BroadcastEvent(reply)).await;
                                        }
                                        Ok(Err(e)) => {
                                            tracing::error!("[LLM] 工具后调用失败: {}", e);
                                        }
                                        Err(e) => {
                                            tracing::error!("[LLM] 工具后 Actor 通信失败: {}", e);
                                        }
                                    }
                                } else {
                                    // 无工具调用，直接回复
                                    let preview: String = resp.content
                                        .chars()
                                        .take(80)
                                        .collect();
                                    tracing::info!(
                                        "[LLM] 回复: {} | 缓存命中: {} tokens",
                                        preview,
                                        resp.cache_hit_tokens,
                                    );
                                    let reply = AgentEvent::new(
                                        EventType::AssistantMessage,
                                        EventSource::System,
                                        serde_json::json!({
                                            "chat_id": chat_id,
                                            "text": resp.content,
                                            "source": source,
                                        }),
                                    );
                                    emitter.emit(reply.clone());
                                    let _ = pm.send(BroadcastEvent(reply)).await;
                                }
                            });
                        } else {
                            let reply = AgentEvent::new(
                                EventType::AssistantMessage,
                                EventSource::System,
                                serde_json::json!({
                                    "chat_id": chat_id,
                                    "text": format!("你说: {}", text),
                                    "source": source,
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
                true
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

        // 启动 HTTP API server（后台）
        let api_pm = plugin_manager.clone();
        let api_tr = tool_registry.clone();
        let _server_handle = actix_rt::spawn(async move {
            if let Err(e) = api::start_server(api_pm, api_tr).await {
                tracing::error!("API server 错误: {}", e);
            }
        });

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
