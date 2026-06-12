//! Claude Bridge Plugin — actor-free port of BN_agent's claude-bridge-plugin.
//!
//! Calls the `claude` CLI tool to send messages and receive replies.
//! Registers `claude_chat` tool for LLM function calling.

use plugin_interface::*;

pub struct ClaudeBridgePlugin {
    info: PluginInfo,
    claude_path: String,
    event_bus: Option<Addr<EventBus>>,
}

impl ClaudeBridgePlugin {
    pub fn new() -> Self {
        let claude_path =
            std::env::var("CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
        Self {
            info: PluginInfo {
                name: "claude-bridge-plugin".into(),
                version: "0.1.0".into(),
                description: "Claude CLI bridge — call claude tool from LLM".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            claude_path,
            event_bus: None,
        }
    }
}

impl Plugin for ClaudeBridgePlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        self.event_bus = Some(ctx.event_bus.clone());
        let available = check_claude_available(&self.claude_path);
        if available {
            log::info!("[claude-bridge] Claude CLI available: {}", self.claude_path);
        } else {
            log::warn!("[claude-bridge] Claude CLI NOT available: {} (tool calls will fail)", self.claude_path);
        }

        if let Some(ref reg) = ctx.tool_registry {
            let path = self.claude_path.clone();
            reg.lock().map_err(|e| format!("lock: {}", e))?
                .register(std::sync::Arc::new(ClaudeChatTool { claude_path: path }));
            log::info!("[claude-bridge] registered tool: claude_chat");
        }

        log::info!("[claude-bridge] started");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[claude-bridge] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "claude_request" {
            let message = event.data.get("message")
                .and_then(|v| v.as_str()).unwrap_or("").to_string();
            let chat_id = event.data.get("chat_id").and_then(|v| v.as_i64());
            if message.is_empty() { return true; }

            let eb = match self.event_bus.clone() {
                Some(e) => e,
                None => { log::warn!("[claude-bridge] no event_bus"); return true; }
            };
            let path = self.claude_path.clone();
            std::thread::spawn(move || {
                match call_claude(&path, &message, None) {
                    Ok(response) => {
                        log::info!("[claude-bridge] response: {}...", &response[..response.len().min(100)]);
                        eb.do_send(Event::new(
                            "claude_response",
                            serde_json::json!({ "chat_id": chat_id, "text": response, "source": "claude" }),
                            "claude-bridge-plugin",
                        ));
                    }
                    Err(e) => {
                        log::warn!("[claude-bridge] call failed: {}", e);
                        eb.do_send(Event::new(
                            "claude_response",
                            serde_json::json!({ "chat_id": chat_id, "text": format!("Claude error: {}", e), "error": true }),
                            "claude-bridge-plugin",
                        ));
                    }
                }
            });
        }
        true
    }

    fn snapshot(&self) -> Option<String> {
        let ok = check_claude_available(&self.claude_path);
        Some(format!("【claude-bridge】Claude CLI: {} ({})",
            if ok { "可用" } else { "不可用" }, self.claude_path))
    }
}

// ─── 工具 ───────────────────────────────────────────────────────

struct ClaudeChatTool {
    claude_path: String,
}

impl ToolExecutor for ClaudeChatTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "claude_chat".into(),
            description: "Send a message to Claude via CLI and get a reply.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Message for Claude" },
                    "model": { "type": "string", "description": "Optional model (e.g. claude-sonnet-4)" }
                },
                "required": ["message"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => return ToolResult::err("missing: message"),
        };
        let model = args.get("model").and_then(|v| v.as_str()).map(String::from);
        if message.is_empty() { return ToolResult::err("message empty"); }

        let path = self.claude_path.clone();
        match std::thread::spawn(move || call_claude(&path, &message, model.as_deref())).join() {
            Ok(Ok(r)) => ToolResult::ok(&r),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("thread panic"),
        }
    }
}

// ─── Claude CLI 调用 ────────────────────────────────────────────

fn check_claude_available(path: &str) -> bool {
    std::process::Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn call_claude(path: &str, message: &str, model: Option<&str>) -> Result<String, String> {
    let mut cmd = std::process::Command::new(path);
    cmd.arg("-p").arg(message)
        .arg("--output-format").arg("text")
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

// ─── FFI ─────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ClaudeBridgePlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
