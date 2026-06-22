//! MessageRouter — 统一消息路由层。
//!
//! 接收 `"route.message"` / `"proactive.message"` 事件，校验/补全字段后转发为 `"assistant.message"`。
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
use std::sync::Arc;
use parking_lot::Mutex;
use std::time::Instant;

// ── Channel state ─────────────────────────────────────────────────────────────

/// 通道会话状态记录。
#[derive(Clone, Debug)]
pub struct ChannelState {
    pub source: String,
    pub chat_id: Option<String>,
    pub peer_id: Option<String>,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
            "route.message" | "proactive.message" => self.on_route_message(&event.data),
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
        let peer_id = Self::peer_id_from_data(data, &source);

        let mut channels = self.channels.lock();
        channels.insert(
            source.clone(),
            ChannelState {
                source,
                chat_id,
                peer_id,
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
        let requested_peer_id = data
            .get("peer_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // 4. 确定目标列表
        let channels = self.channels.lock();
        let targets = resolve_targets(
            &requested_source,
            &requested_chat_id,
            &requested_peer_id,
            &channels,
        );
        drop(channels);

        // 5. 转发
        for target in &targets {
            let chat_id = requested_chat_id.clone().or_else(|| target.chat_id.clone());
            let peer_id = requested_peer_id.clone().or_else(|| target.peer_id.clone());

            let payload = build_route_payload(&text, &target.source, &chat_id, &peer_id);

            self.event_bus
                .do_send(Event::new("assistant.message", payload, "message-router"));

            let preview: String = text.clone();
            log::debug!(
                "[MessageRouter] routed '{}…' to {} (chat_id={:?})",
                preview,
                target.source,
                chat_id.as_deref().unwrap_or("(auto)"),
            );
        }

        if targets.is_empty() {
            log::warn!(
                "[MessageRouter] no targets for route.message (source='{}', no known channels)",
                requested_source
            );
        }
    }

    fn peer_id_from_data(data: &serde_json::Value, source: &str) -> Option<String> {
        if let Some(peer_id) = data.get("peer_id").and_then(|v| v.as_str()) {
            let peer_id = peer_id.trim();
            if !peer_id.is_empty() {
                return Some(peer_id.to_string());
            }
        }

        let raw_id = data
            .get("chat_id")
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| {
                data.get("chat_id")
                    .and_then(|v| v.as_i64().map(|n| n.to_string()))
            })
            .or_else(|| {
                data.get("from_user_id")
                    .and_then(|v| v.as_str().map(String::from))
            })
            .or_else(|| {
                data.get("user_id")
                    .and_then(|v| v.as_str().map(String::from))
            });

        raw_id.and_then(|id| {
            let id = id.trim();
            if source.is_empty() || id.is_empty() {
                None
            } else {
                Some(format!("{}:{}", source, id))
            }
        })
    }
}

/// 纯逻辑：根据 requested_source 与已知通道注册表，确定 route.message 的转发目标。
/// - source 空或 "unknown" → 广播到所有已知通道
/// - source 命中注册表 → 定向该通道
/// - source 未命中但有其他通道 → 回退广播
/// - 无任何通道 → 用请求里的 chat_id/peer_id 发一个临时目标
fn resolve_targets(
    requested_source: &str,
    requested_chat_id: &Option<String>,
    requested_peer_id: &Option<String>,
    channels: &HashMap<String, ChannelState>,
) -> Vec<ChannelState> {
    if requested_source.is_empty() || requested_source == "unknown" {
        let all: Vec<ChannelState> = channels.values().cloned().collect();
        if all.is_empty() {
            log::warn!("[MessageRouter] broadcast requested but no channels registered");
        }
        all
    } else if let Some(state) = channels.get(requested_source) {
        vec![state.clone()]
    } else if !channels.is_empty() {
        log::warn!(
            "[MessageRouter] source '{}' not in registry, falling back to broadcast",
            requested_source
        );
        channels.values().cloned().collect()
    } else {
        vec![ChannelState {
            source: requested_source.to_string(),
            chat_id: requested_chat_id.clone(),
            peer_id: requested_peer_id.clone(),
            updated_at: Instant::now(),
        }]
    }
}

/// 纯逻辑：构建 assistant.message 的 payload。chat_id 为纯数字时按 i64 写入（Telegram）。
fn build_route_payload(
    text: &str,
    target_source: &str,
    chat_id: &Option<String>,
    peer_id: &Option<String>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "text": text,
        "source": target_source,
    });
    if let Some(pid) = peer_id {
        payload["peer_id"] = serde_json::json!(pid);
    }
    if let Some(cid) = chat_id {
        if let Ok(n) = cid.parse::<i64>() {
            payload["chat_id"] = serde_json::json!(n);
        } else {
            payload["chat_id"] = serde_json::json!(cid);
        }
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(source: &str, chat_id: Option<&str>) -> ChannelState {
        ChannelState {
            source: source.to_string(),
            chat_id: chat_id.map(String::from),
            peer_id: chat_id.map(|c| format!("{}:{}", source, c)),
            updated_at: Instant::now(),
        }
    }

    fn registry(items: &[ChannelState]) -> HashMap<String, ChannelState> {
        items
            .iter()
            .map(|c| (c.source.clone(), c.clone()))
            .collect()
    }

    #[test]
    fn empty_source_broadcasts_to_all() {
        let reg = registry(&[ch("telegram", Some("1")), ch("feishu", Some("oc_x"))]);
        assert_eq!(resolve_targets("", &None, &None, &reg).len(), 2);
    }

    #[test]
    fn unknown_source_broadcasts() {
        let reg = registry(&[ch("telegram", Some("1"))]);
        assert_eq!(resolve_targets("unknown", &None, &None, &reg).len(), 1);
    }

    #[test]
    fn known_source_targets_single() {
        let reg = registry(&[ch("telegram", Some("1")), ch("feishu", Some("oc_x"))]);
        let targets = resolve_targets("feishu", &None, &None, &reg);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].source, "feishu");
    }

    #[test]
    fn unknown_named_source_falls_back_to_broadcast() {
        let reg = registry(&[ch("telegram", Some("1")), ch("feishu", Some("oc_x"))]);
        // 指定的 wechat 不在注册表 → 回退广播到其他两个
        assert_eq!(resolve_targets("wechat", &None, &None, &reg).len(), 2);
    }

    #[test]
    fn no_channels_uses_requested_as_temp() {
        let reg = HashMap::new();
        let targets = resolve_targets(
            "telegram",
            &Some("123".to_string()),
            &Some("telegram:123".to_string()),
            &reg,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].source, "telegram");
        assert_eq!(targets[0].chat_id.as_deref(), Some("123"));
    }

    #[test]
    fn payload_numeric_chat_id_is_i64() {
        let p = build_route_payload("hi", "telegram", &Some("123".to_string()), &None);
        assert_eq!(p["chat_id"], serde_json::json!(123));
        assert_eq!(p["text"], "hi");
        assert_eq!(p["source"], "telegram");
    }

    #[test]
    fn payload_non_numeric_chat_id_is_string() {
        let p = build_route_payload("hi", "wechat", &Some("wxid_abc".to_string()), &None);
        assert_eq!(p["chat_id"], serde_json::json!("wxid_abc"));
    }

    #[test]
    fn payload_includes_peer_id_and_omits_missing_chat_id() {
        let p = build_route_payload("hi", "telegram", &None, &Some("telegram:1".to_string()));
        assert_eq!(p["peer_id"], serde_json::json!("telegram:1"));
        assert!(p.get("chat_id").is_none());
    }
}
