//! BN Agent 主程序

mod models {
    pub mod event_bus;
    pub mod plugin_loader;
}

use actix::prelude::*;
use models::event_bus::{BusEmitter, EventBus, EmitEvent, RegisterCallback};
use models::plugin_loader::{BroadcastEvent, PluginManager, ScanAndLoad, SetToolRegistry, StopAll};
use plugin_core::{
    AgentEvent, EventSource, EventType, HostContext, LogCallback, LogLevel, ToolRegistry,
};
use std::sync::{Arc, Mutex};

fn main() -> std::io::Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // 用 System::run() 替代 block_on，确保 run() 返回时所有 Arbiter 已停止
    let system = actix_rt::System::new();

    // 在 System 上下文中 spawn 主逻辑
    let mut started = false;
    system.block_on(async {
        tracing::info!("BN Agent 启动");
        started = true;

        let event_bus = EventBus::new().start();
        let plugin_manager = PluginManager::new("./plugins").start();
        let tool_registry = Arc::new(Mutex::new(ToolRegistry::new()));

        // 将 tool_registry 传给 PluginManager，以便在卸载插件前清理
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
            .with_emitter(emitter)
            .with_logger(Arc::new(TracingLogger))
            .with_tool_registry(tool_registry.clone());

        match plugin_manager.send(ScanAndLoad(ctx)).await {
            Ok(Ok(n)) => tracing::info!("已加载 {} 个插件", n),
            Ok(Err(e)) => tracing::error!("插件加载失败: {}", e),
            Err(e) => tracing::error!("Actor 通信失败: {}", e),
        }

        event_bus
            .send(RegisterCallback(Arc::new(|event: &AgentEvent| {
                tracing::debug!("[EventBus] 收到事件: {:?}", event.event_type);
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

        // 等待 Ctrl+C 信号
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("收到退出信号");

        // 停止并卸载插件
        plugin_manager.send(StopAll).await.ok();

        // 显式 drop Actor Addr，确保 PluginManager 在 System 停止前被清理
        drop(plugin_manager);
        drop(event_bus);
        drop(tool_registry);

        tracing::info!("BN Agent 退出");
        actix_rt::System::current().stop();
    });

    // System::run() 会阻塞直到所有 Arbiter 停止，确保安全退出
    if started {
        let _ = system.run();
    }

    Ok(())
}
