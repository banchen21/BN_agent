use std::collections::HashMap;
use std::sync::Arc;

/// 工具定义
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 工具执行结果
#[derive(Clone, Debug)]
pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(content: &str) -> Self {
        Self {
            success: true,
            content: content.to_string(),
            error: None,
        }
    }

    pub fn err(msg: &str) -> Self {
        Self {
            success: false,
            content: String::new(),
            error: Some(msg.to_string()),
        }
    }
}

/// 工具执行器 trait
pub trait ToolExecutor: Send + Sync {
    fn def(&self) -> &ToolDef;
    fn execute(&self, args: &serde_json::Value) -> ToolResult;
}

/// 工具注册表
pub struct ToolRegistry {
    executors: HashMap<String, Arc<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            executors: HashMap::new(),
        }
    }

    pub fn register(&mut self, executor: Arc<dyn ToolExecutor>) {
        let name = executor.def().name.clone();
        self.executors.insert(name, executor);
    }

    pub fn all_defs(&self) -> Vec<ToolDef> {
        self.executors.values().map(|e| e.def().clone()).collect()
    }

    pub fn execute(&self, name: &str, args: &serde_json::Value) -> Option<ToolResult> {
        self.executors.get(name).map(|e| e.execute(args))
    }

    pub fn unregister(&mut self, name: &str) {
        self.executors.remove(name);
    }

    pub fn clear(&mut self) {
        self.executors.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }
}
