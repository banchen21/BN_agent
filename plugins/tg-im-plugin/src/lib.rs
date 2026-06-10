//! TG-IM Plugin — Telegram 即时通讯插件
//!
//! 通过 Telegram Bot 与 BN Agent 交互：
//! - 接收用户消息 → 发射 UserMessage 事件
//! - 监听 AssistantMessage 事件 → 发送回复到 Telegram

use plugin_core::{
    AgentEvent, HostContext, Plugin, PluginError, PluginMeta,
};
use std::sync::Arc;
use tokio::sync::Mutex;

mod bot;

pub struct TgImPlugin {
    meta: PluginMeta,
    ctx: Option<HostContext>,
    /// Bot 句柄，用于优雅关闭
    bot_handle: Option<Arc<Mutex<Option<bot::BotHandle>>>>,
}

impl TgImPlugin {
    pub fn new() -> Self {
        Self {
            meta: PluginMeta {
                name: "tg-im-plugin".into(),
                version: "0.1.0".into(),
                description: "Telegram 即时通讯插件".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            ctx: None,
            bot_handle: None,
        }
    }
}

impl Plugin for TgImPlugin {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn init(&mut self, ctx: &HostContext) -> Result<(), PluginError> {
        ctx.log_info("tg-im", "TgImPlugin 初始化完成");
        self.ctx = Some(ctx.clone());
        self.bot_handle = Some(Arc::new(Mutex::new(None)));
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| PluginError::InitError("未初始化".into()))?;

        ctx.log_info("tg-im", "TgImPlugin 已启动");

        // 从环境变量读取配置
        let token = std::env::var("TG_BOT_TOKEN").map_err(|_| {
            PluginError::InitError("环境变量 TG_BOT_TOKEN 未设置".into())
        })?;

        let handle = self.bot_handle.clone();
        let emitter = ctx
            .emitter
            .clone()
            .ok_or_else(|| PluginError::InitError("EventEmitter 未注入".into()))?;
        let logger = ctx.logger.clone();

        // 在后台线程启动 bot
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                match bot::run_bot(&token, emitter, logger).await {
                    Ok(h) => {
                        if let Some(ref handle) = handle {
                            *handle.lock().await = Some(h);
                        }
                    }
                    Err(e) => {
                        eprintln!("[tg-im] Bot 运行失败: {}", e);
                    }
                }
            });
        });

        Ok(())
    }

    fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("tg-im", "TgImPlugin 正在停止...");
        }

        // 通知 bot 关闭
        if let Some(ref handle) = self.bot_handle {
            let handle = handle.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .expect("无法创建 tokio runtime");
                rt.block_on(async {
                    let mut guard = handle.lock().await;
                    if let Some(ref h) = *guard {
                        h.shutdown().await;
                    }
                    *guard = None;
                });
            });
        }

        if let Some(ref ctx) = self.ctx {
            ctx.log_info("tg-im", "TgImPlugin 已停止");
        }
        Ok(())
    }

    fn on_event(&self, event: &AgentEvent) {
        if let Some(ref ctx) = self.ctx {
            ctx.log_debug("tg-im", &format!("收到事件: {:?}", event.event_type));
        }
    }

    fn ctx(&self) -> Option<&HostContext> {
        self.ctx.as_ref()
    }
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(TgImPlugin::new()))
}
