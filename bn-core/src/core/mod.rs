//! 核心模块
//!
//! runtime.rs — 运行时初始化（LLM、EventBus、PluginManager、API server）
//! loop.rs   — 核心循环（事件回调 → LLM 对话 + 工具调用）

pub mod runtime;
pub mod r#loop;
