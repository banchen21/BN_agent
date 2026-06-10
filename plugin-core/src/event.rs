use serde::{Deserialize, Serialize};

/// 事件类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    UserMessage,
    AssistantMessage,
    PluginNotification,
    PluginRequest,
    SystemEvent,
    Custom(String),
}

/// 事件来源
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventSource {
    User,
    System,
    Plugin(String),
}

/// Agent 事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub event_type: EventType,
    pub source: EventSource,
    pub data: serde_json::Value,
}

impl AgentEvent {
    pub fn new(event_type: EventType, source: EventSource, data: serde_json::Value) -> Self {
        Self {
            event_type,
            source,
            data,
        }
    }
}

/// 事件发射器 trait — 插件通过此接口向宿主发送事件
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: AgentEvent);
}
