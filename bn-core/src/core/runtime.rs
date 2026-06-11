//! 运行时初始化：LLM、EventBus、PluginManager、API server 启动

use actix::prelude::*;
use std::sync::{Arc, Mutex};

use crate::api_server;
use super::r#loop as core_loop;
use crate::models::llm::client::{LlmActor, LlmConfig};
use crate::models::event_bus::{BusEmitter, EventBus, EmitEvent, RegisterCallback};
use crate::models::plugin_loader::{
    BroadcastEvent, PluginManager, RefreshSnapshots, ScanAndLoad, SetToolRegistry, StopAll,
};
use plugin_core::{
    AgentEvent, EventEmitter, EventSource, EventType, HostContext, LogCallback, LogLevel, ToolRegistry,
};

pub fn run() -> std::io::Result<()> {
    let system = actix_rt::System::new();
    let mut started = false;

    system.block_on(async {
        tracing::info!("BN Agent 启动");
        started = true;

        let llm_addr = init_llm();
        let event_bus = EventBus::new().start();

        let plugin_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug");
        let plugin_dir_str = plugin_dir.to_str().unwrap_or("./plugins");
        let plugin_manager = PluginManager::new(plugin_dir_str);
        let snapshots_for_cb = plugin_manager.snapshots_arc();
        let plugin_manager = plugin_manager.start();
        let tool_registry = Arc::new(Mutex::new(ToolRegistry::new()));

        plugin_manager.send(SetToolRegistry(tool_registry.clone())).await.ok();

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

        register_user_message_handler(
            &event_bus,
            llm_addr.clone(),
            plugin_manager.clone(),
            tool_registry.clone(),
            emitter,
            snapshots_for_cb.clone(),
        )
        .await;

        broadcast_startup(&event_bus, &plugin_manager).await;

        tracing::info!("BN Agent 运行中，按 Ctrl+C 退出...");

        start_api_server(
            plugin_manager.clone(),
            llm_addr,
            tool_registry.clone(),
            snapshots_for_cb,
        );

        tokio::signal::ctrl_c().await.ok();
        tracing::info!("收到退出信号");

        plugin_manager.send(StopAll).await.ok();
        drop(plugin_manager);
        drop(event_bus);
        drop(tool_registry);

        tracing::info!("BN Agent 退出");
        actix_rt::System::current().stop();
    });

    if started {
        let _ = system.run();
    }
    Ok(())
}

// ─── helpers ───

fn init_llm() -> Option<Addr<LlmActor>> {
    match LlmConfig::from_env() {
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
    }
}

async fn register_user_message_handler(
    event_bus: &Addr<EventBus>,
    llm_addr: Option<Addr<LlmActor>>,
    pm: Addr<PluginManager>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    emitter: Arc<BusEmitter>,
    snapshots_for_cb: Arc<Mutex<Vec<String>>>,
) {
    let llm_for_cb = llm_addr.clone();
    let pm_for_cb = pm.clone();
    let tool_for_cb = tool_registry.clone();
    let emitter_for_cb = emitter.clone();

    event_bus
        .send(RegisterCallback(Arc::new(move |event: &AgentEvent| -> bool {
            match event.event_type {
                EventType::UserMessage => {
                    let text = event.data.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let chat_id = event.data.get("chat_id").and_then(|v| v.as_i64());
                    let user_name = event.data.get("user_name").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string();

                    tracing::info!("[MSG] @{}: {}", user_name, text);

                    if let Some(ref llm) = llm_for_cb {
                        let llm = llm.clone();
                        let emitter = emitter_for_cb.clone();
                        let pm = pm_for_cb.clone();
                        let tool_registry = tool_for_cb.clone();
                        let snapshots = snapshots_for_cb.clone();

                        actix::spawn(async move {
                            let _ = pm.send(RefreshSnapshots).await;
                            let contexts: Vec<String> = snapshots.lock().unwrap().clone();
                            core_loop::handle_user_message(
                                &text, chat_id, &source, &llm, &emitter, &pm,
                                &tool_registry, &snapshots, contexts,
                            ).await;
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
                _ => tracing::debug!("[EventBus] 收到事件: {:?}", event.event_type),
            }
            true
        })))
        .await
        .ok();
}

async fn broadcast_startup(event_bus: &Addr<EventBus>, pm: &Addr<PluginManager>) {
    event_bus
        .send(EmitEvent(AgentEvent::new(
            EventType::SystemEvent,
            EventSource::System,
            serde_json::json!({"message": "系统启动完成"}),
        )))
        .await
        .ok();

    pm.send(BroadcastEvent(AgentEvent::new(
        EventType::PluginNotification,
        EventSource::System,
        serde_json::json!({"message": "宿主已就绪"}),
    )))
    .await
    .ok();
}

fn start_api_server(
    pm: Addr<PluginManager>,
    llm_addr: Option<Addr<LlmActor>>,
    tool_registry: Arc<Mutex<ToolRegistry>>,
    snapshots: Arc<Mutex<Vec<String>>>,
) {
    let api_llm = llm_addr.unwrap_or_else(|| panic!("LLM 未初始化"));
    let api_pm = pm;
    actix_rt::spawn(async move {
        if let Err(e) = api_server::start_server(api_pm, api_llm, tool_registry, snapshots).await {
            tracing::error!("API server 错误: {}", e);
        }
    });
}

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
