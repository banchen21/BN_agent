//! Claude Bridge Plugin — bridge Claude CLI as a tool.
//!
//! 仅工具模式。LLM 后端模式因 Windows DLL TLS 限制无法在 cdylib 内
//! 创建 actix actor，需在主进程中创建。不使用 LLM_BACKEND=claude，
//! 通过 `claude_chat` 工具调用 Claude。

use plugin_interface::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ═══════════════════════════════════════════════════════════════════════════════
//  Session helpers
// ═══════════════════════════════════════════════════════════════════════════════

type Sessions = Arc<Mutex<HashMap<String, Vec<String>>>>;

fn session_add_user_and_build_prompt(
    sessions: &Sessions, session_key: &str, user_msg: &str,
) -> String {
    let mut map = sessions.lock().unwrap();
    let history = map.entry(session_key.to_string()).or_default();
    history.push(format!("User: {}", user_msg));
    history.join("\n\n")
}

fn session_add_assistant(sessions: &Sessions, session_key: &str, response: &str) {
    if let Ok(mut map) = sessions.lock() {
        if let Some(history) = map.get_mut(session_key) {
            history.push(format!("Assistant: {}", response));
        }
    }
}

fn session_clear(sessions: &Sessions, session_key: &str) {
    if let Ok(mut map) = sessions.lock() { map.remove(session_key); }
}

fn session_list(sessions: &Sessions) -> Vec<String> {
    sessions.lock().ok().map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Plugin
// ═══════════════════════════════════════════════════════════════════════════════

pub struct ClaudeBridgePlugin {
    info: PluginInfo,
    claude_path: String,
    sessions: Sessions,
}

impl Plugin for ClaudeBridgePlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, _ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let available = check_claude_available(&self.claude_path);
        if available {
            log::info!("[claude-bridge] Claude CLI available: {}", self.claude_path);
        } else {
            log::warn!("[claude-bridge] Claude CLI NOT available");
        }

        // ── Register tools ──
        if let Some(ref reg) = _ctx.tool_registry {
            let mut r = reg.lock().map_err(|e| format!("lock: {}", e))?;
            r.register(Arc::new(ClaudeChatTool {
                claude_path: self.claude_path.clone(),
                sessions: self.sessions.clone(),
            }));
            r.register(Arc::new(ClaudeSessionsTool { sessions: self.sessions.clone() }));
            r.register(Arc::new(ClaudeClearSessionTool { sessions: self.sessions.clone() }));
        }

        log::info!("[claude-bridge] started (tool mode)");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[claude-bridge] stopped");
    }

    fn snapshot(&self) -> Option<String> {
        let ok = check_claude_available(&self.claude_path);
        let session_count = self.sessions.lock().map(|m| m.len()).unwrap_or(0);
        Some(format!(
            "【claude-bridge】Claude CLI: {} ({}), 会话数: {}",
            if ok { "可用" } else { "不可用" }, self.claude_path, session_count,
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Tools
// ═══════════════════════════════════════════════════════════════════════════════

struct ClaudeChatTool {
    claude_path: String,
    sessions: Sessions,
}

impl ToolExecutor for ClaudeChatTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "claude_chat".into(),
            description: "Send to Claude via CLI. Use session_id to continue conversation.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Message for Claude" },
                    "session_id": { "type": "string", "description": "Optional — reuse to continue" },
                    "model": { "type": "string", "description": "Optional model" }
                },
                "required": ["message"]
            }),
        });
        &DEF
    }
    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(), None => return ToolResult::err("missing: message"),
        };
        let model = args.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
        let sid = args.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string());
        let prompt = match &sid {
            Some(s) => session_add_user_and_build_prompt(&self.sessions, s, &message),
            None => format!("User: {}", message),
        };
        match call_claude_sync(&self.claude_path, &prompt, model.as_deref()) {
            Ok(r) => {
                if let Some(s) = &sid { session_add_assistant(&self.sessions, s, &r); }
                ToolResult::ok(&r)
            }
            Err(e) => ToolResult::err(&e),
        }
    }
}

struct ClaudeSessionsTool { sessions: Sessions }
impl ToolExecutor for ClaudeSessionsTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "claude_sessions".into(), description: "List active Claude sessions.".into(),
            internal: false, parameters: serde_json::json!({}),
        });
        &DEF
    }
    fn execute(&self, _: &serde_json::Value) -> ToolResult {
        let keys = session_list(&self.sessions);
        if keys.is_empty() { ToolResult::ok("当前没有活动的 Claude 会话") }
        else { ToolResult::ok(&format!("活跃会话：\n{}", keys.join("\n"))) }
    }
}

struct ClaudeClearSessionTool { sessions: Sessions }
impl ToolExecutor for ClaudeClearSessionTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "claude_clear_session".into(), description: "Clear a session.".into(),
            internal: false, parameters: serde_json::json!({
                "session_id": { "type": "string", "description": "Session id" }
            }),
        });
        &DEF
    }
    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let sid = match args.get("session_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(), None => return ToolResult::err("missing: session_id"),
        };
        session_clear(&self.sessions, &sid);
        ToolResult::ok(&format!("会话 '{}' 已清除", sid))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Claude CLI call
// ═══════════════════════════════════════════════════════════════════════════════

fn check_claude_available(path: &str) -> bool {
    std::process::Command::new(path).arg("--version")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn call_claude_sync(path: &str, message: &str, model: Option<&str>) -> Result<String, String> {
    let mut cmd = std::process::Command::new(path);
    cmd.arg("-p").arg(message).arg("--output-format").arg("text")
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    if let Some(m) = model { cmd.arg("--model").arg(m); }
    let output = cmd.output().map_err(|e| format!("exec failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exit {}: {}", output.status.code().unwrap_or(-1), stderr.trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { Err("empty response".into()) } else { Ok(text) }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  FFI
// ═══════════════════════════════════════════════════════════════════════════════

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ClaudeBridgePlugin {
        info: PluginInfo {
            name: "claude-bridge-plugin".into(),
            version: "0.3.0".into(),
            description: "Claude CLI tool bridge".into(),
            author: "BN Team".into(),
            min_host_version: "0.1.0".into(),
        },
        claude_path: std::env::var("CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        sessions: Arc::new(Mutex::new(HashMap::new())),
    })
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
