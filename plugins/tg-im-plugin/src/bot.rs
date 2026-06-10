//! Telegram Bot 核心逻辑
//!
//! 使用 teloxide 库与 Telegram API 交互：
//! - 接收用户私聊消息 → 发射 UserMessage 事件到宿主
//! - 提供 send_message 方法供宿主回复

use plugin_core::{AgentEvent, EventSource, EventType, LogCallback};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::Recipient;

/// Bot 句柄，用于外部控制（如关闭）
pub struct BotHandle {
    bot: Bot,
    chat_id: Option<i64>,
}

impl BotHandle {
    pub async fn shutdown(&self) {
        // teloxide 的 Dispatcher 通过 drop 来停止
        // 这里可以发送一条消息通知用户
        if let Some(chat_id) = self.chat_id {
            let _ = self
                .bot
                .send_message(
                    Recipient::Id(teloxide::types::ChatId(chat_id)),
                    "BN Agent 正在关闭...",
                )
                .await;
        }
    }

    /// 发送消息到指定聊天
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), String> {
        self.bot
            .send_message(
                Recipient::Id(teloxide::types::ChatId(chat_id)),
                text,
            )
            .await
            .map_err(|e| format!("发送消息失败: {}", e))?;
        Ok(())
    }
}

/// 启动 Telegram Bot
pub async fn run_bot(
    token: &str,
    emitter: Arc<dyn plugin_core::EventEmitter>,
    logger: Option<Arc<dyn LogCallback>>,
) -> Result<BotHandle, String> {
    let bot = Bot::new(token);

    // 获取 bot 信息并打印
    let me = bot.get_me().await.map_err(|e| format!("获取 Bot 信息失败: {}", e))?;
    if let Some(ref log) = logger {
        log.log(
            plugin_core::LogLevel::Info,
            "tg-im",
            &format!("Bot @{} 已连接", me.username.as_deref().unwrap_or("unknown")),
        );
    }

    let bot_clone = bot.clone();
    let emitter_clone = emitter.clone();
    let logger_clone = logger.clone();

    // 消息处理闭包
    let handler = move |msg: Message, bot: Bot| {
        let emitter = emitter_clone.clone();
        let logger = logger_clone.clone();

        async move {
            // 只处理文本消息
            let text = match msg.text() {
                Some(t) => t.to_string(),
                None => return Ok::<(), teloxide::RequestError>(()),
            };

            let chat_id = msg.chat.id.0;
            let user_name = msg
                .from
                .as_ref()
                .map(|u| u.username.as_deref().unwrap_or("unknown"))
                .unwrap_or("unknown");

            if let Some(ref log) = logger {
                log.log(
                    plugin_core::LogLevel::Info,
                    "tg-im",
                    &format!("收到来自 @{} 的消息: {}", user_name, text),
                );
            }

            // 发射 UserMessage 事件到宿主
            emitter.emit(AgentEvent::new(
                EventType::UserMessage,
                EventSource::Plugin("tg-im".into()),
                serde_json::json!({
                    "platform": "telegram",
                    "chat_id": chat_id,
                    "user_name": user_name,
                    "text": text,
                }),
            ));

            // 发送确认回执
            let _ = bot
                .send_message(
                    Recipient::Id(teloxide::types::ChatId(chat_id)),
                    "收到，正在处理...",
                )
                .await;

            Ok(())
        }
    };

    // 创建 Dispatcher
    let mut dispatcher = Dispatcher::builder(bot_clone, Update::filter_message().branch(dptree::endpoint(handler)))
        .build();

    // 在后台 spawn dispatcher
    let handle = BotHandle {
        bot: bot.clone(),
        chat_id: None,
    };

    // dispatcher 需要被持续 poll，这里用 tokio::spawn
    tokio::spawn(async move {
        dispatcher.dispatch().await;
    });

    Ok(handle)
}
