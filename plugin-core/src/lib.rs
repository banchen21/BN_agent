//! BN Agent 动态插件核心库
//!
//! 定义插件 trait、宿主上下文、事件/工具类型。

pub mod context;
pub mod error;
pub mod event;
pub mod plugin;
pub mod tool;

pub use context::{HostContext, LogCallback, LogLevel};
pub use error::PluginError;
pub use event::{AgentEvent, EventEmitter, EventSource, EventType};
pub use plugin::{Plugin, PluginApi, PluginMeta};
pub use tool::{ToolDef, ToolExecutor, ToolRegistry, ToolResult};
