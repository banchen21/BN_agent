use std::sync::Arc;

/// 日志级别
#[derive(Debug, Clone)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// 日志回调 trait — 宿主实现，插件调用
pub trait LogCallback: Send + Sync {
    fn log(&self, level: LogLevel, target: &str, message: &str);
}

/// 宿主上下文 — 注入给插件的能力
#[derive(Clone)]
pub struct HostContext {
    pub host_name: String,
    pub host_version: String,
    pub plugin_dir: String,
    pub emitter: Option<Arc<dyn crate::EventEmitter>>,
    pub logger: Option<Arc<dyn LogCallback>>,
    pub tool_registry: Option<Arc<std::sync::Mutex<crate::ToolRegistry>>>,
}

impl HostContext {
    pub fn new(host_name: &str, host_version: &str, plugin_dir: &str) -> Self {
        Self {
            host_name: host_name.to_string(),
            host_version: host_version.to_string(),
            plugin_dir: plugin_dir.to_string(),
            emitter: None,
            logger: None,
            tool_registry: None,
        }
    }

    pub fn with_emitter(mut self, e: Arc<dyn crate::EventEmitter>) -> Self {
        self.emitter = Some(e);
        self
    }

    pub fn with_logger(mut self, l: Arc<dyn LogCallback>) -> Self {
        self.logger = Some(l);
        self
    }

    pub fn with_tool_registry(mut self, r: Arc<std::sync::Mutex<crate::ToolRegistry>>) -> Self {
        self.tool_registry = Some(r);
        self
    }

    pub fn log_info(&self, target: &str, msg: &str) {
        if let Some(ref l) = self.logger {
            l.log(LogLevel::Info, target, msg);
        }
    }

    pub fn log_warn(&self, target: &str, msg: &str) {
        if let Some(ref l) = self.logger {
            l.log(LogLevel::Warn, target, msg);
        }
    }

    pub fn log_debug(&self, target: &str, msg: &str) {
        if let Some(ref l) = self.logger {
            l.log(LogLevel::Debug, target, msg);
        }
    }
}
