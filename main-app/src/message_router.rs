//! MessageRouter — 统一消息路由层。
//!
//! 接收 `"route.message"` 事件，校验/补全字段后转发为 `"assistant.message"`。
//!
//! 同时订阅 `"user.message"` 事件，自动构建通道注册表（source → chat_id）。
//!
//! ## 路由规则
//!
//! - `source` 为空 → 广播到所有已知通道
//! - `source` 已指定 → 只发对应通道（不在注册表中也尝试发送）
//! - `text` 为空 → 丢弃 + 日志
//! - `chat_id` 缺失 → 从注册表中补全

use actix::prelude::*;
use plugin_interface::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ── Channel state ─────────────────────────────────────────────────────────────

/// 通道会话状态记录。
#[derive(Clone, Debug)]
pub struct ChannelState {
    pub source: String,
    pub chat_id: Option<String>,
    pub updated_at: Instant,
}

// ── Actor ─────────────────────────────────────────────────────────────────────

pub struct MessageRouter {
    /// 通道注册表: source → ChannelState
    channels: Arc<Mutex<HashMap<String, ChannelState>>>,
    event_bus: Addr<EventBus>,
}

impl MessageRouter {
    pub fn new(event_bus: Addr<EventBus>) -> Self {
        Self {
            channels: Arc::new(Mutex::new(HashMap::new())),
            event_bus,
        }
    }

    /// 供外部获取通道注册表（如需要）。
    pub fn channels_arc(&self) -> Arc<Mutex<HashMap<String, ChannelState>>> {
        self.channels.clone()
    }
}

impl Actor for MessageRouter {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[MessageRouter] actor started");
    }
}

impl Handler<Event> for MessageRouter {
    type Result = ();

    fn handle(&mut self, event: Event, _ctx: &mut Self::Context) {
        match event.topic.as_str() {
            "user.message" => self.on_user_message(&event.data),
            "route.message" => self.on_route_message(&event.data),
            _ => {}
        }
    }
}

// ── Event handlers ───────────────────────────────────────────────────────────

impl MessageRouter {
    /// 从 user.message 事件中提取通道信息，更新注册表。
    fn on_user_message(&self, data: &serde_json::Value) {
        // source: 优先用 "source"，其次尝试 "platform"（飞书兼容）
        let source = data
            .get("source")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("platform").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        let source = match source {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };

        // chat_id: 支持字符串或数字
        let chat_id = data
            .get("chat_id")
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| {
                data.get("chat_id")
                    .and_then(|v| v.as_i64().map(|n| n.to_string()))
            });

        let mut channels = self.channels.lock().unwrap();
        channels.insert(
            source.clone(),
            ChannelState {
                source,
                chat_id,
                updated_at: Instant::now(),
            },
        );
    }

    /// 处理 route.message 事件：校验 → 路由 → 转发为 assistant.message。
    fn on_route_message(&self, data: &serde_json::Value) {
        // 1. 校验 text
        let text = match data.get("text").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t.to_string(),
            _ => {
                log::warn!("[MessageRouter] dropping route.message with empty text");
                return;
            }
        };

        // 2. 解析目标 source
        let requested_source = data
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // 3. 解析 chat_id（事件中的优先）
        let requested_chat_id = data
            .get("chat_id")
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| {
                data.get("chat_id")
                    .and_then(|v| v.as_i64().map(|n| n.to_string()))
            });

        // 4. 确定目标列表
        let channels = self.channels.lock().unwrap();
        let targets: Vec<ChannelState> = if requested_source.is_empty()
            || requested_source == "unknown"
        {
            // source 为空或 unknown → 广播到所有已知通道
            let all: Vec<ChannelState> = channels.values().cloned().collect();
            if all.is_empty() {
                log::warn!("[MessageRouter] broadcast requested but no channels registered");
            }
            all
        } else if let Some(state) = channels.get(&requested_source) {
            vec![state.clone()]
        } else if !channels.is_empty() {
            // 指定的 source 不在注册表中 → 回退到广播
            log::warn!(
                "[MessageRouter] source '{}' not in registry, falling back to broadcast",
                requested_source
            );
            channels.values().cloned().collect()
        } else {
            // 无任何通道 — 发个临时的，听天由命
            vec![ChannelState {
                source: requested_source.clone(),
                chat_id: requested_chat_id.clone(),
                updated_at: Instant::now(),
            }]
        };
        drop(channels);

        // 5. 转发
        for target in &targets {
            let chat_id = requested_chat_id
                .clone()
                .or_else(|| target.chat_id.clone());

            let mut payload = serde_json::json!({
                "text": text,
                "source": target.source,
            });

            if let Some(ref cid) = chat_id {
                // 尝试数值解析（Telegram 用 i64）
                if let Ok(n) = cid.parse::<i64>() {
                    payload["chat_id"] = serde_json::json!(n);
                } else {
                    payload["chat_id"] = serde_json::json!(cid);
                }
            }

            self.event_bus
                .do_send(Event::new("assistant.message", payload, "message-router"));

            let preview: String = text.chars().take(40).collect();
            log::info!(
                "[MessageRouter] routed '{}…' to {} (chat_id={:?})",
                preview,
                target.source,
                chat_id.as_deref().unwrap_or("(auto)"),
            );
        }

        if targets.is_empty() {
            log::warn!("[MessageRouter] no targets for route.message (source='{}', no known channels)", requested_source);
        }
    }
}
