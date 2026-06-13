//! Claude CLI backend actor — runs in the main process where actix TLS works.
//!
//! Uses `claude --resume <session-id>` with stdin piping (not `-p`).
//! Session history is managed natively by Claude and does NOT need to be
//! passed in‑memory.  Tool definitions are included in each message so
//! Claude knows what tools exist and how to call them via `<tool_call>`.

use actix::prelude::*;
use plugin_interface::*;

// ── Tool system prompt ──────────────────────────────────────────────────────

fn build_tools_system_prompt(tools: &[serde_json::Value]) -> String {
    if tools.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n【系统指令——严格遵守】\n\
         你运行在一个工具编排引擎内。完成用户请求时，如果需要调用工具，\
         必须在回复末尾输出以下格式（纯 JSON 对象包裹在标签内）：\n\n\
         <tool_call>{\"name\":\"工具名\",\"arguments\":{\"参数名\":\"值\"}}</tool_call>\n\n\
         系统会解析此标签并执行对应工具。如果不需要调用工具，正常回复即可。\n"
    );
    for tool in tools {
        let func = &tool["function"];
        let name = func["name"].as_str().unwrap_or("unknown");
        let desc = func["description"].as_str().unwrap_or("");
        let params = &func["parameters"];
        out.push_str(&format!("- {}: {} ({})\n", name, desc, params));
    }
    out.push_str("\n【示例】用户问\"查天气\"，回复末尾：<tool_call>{\"name\":\"web_search\",\"arguments\":{\"query\":\"今天天气\"}}</tool_call>\n");
    out
}

fn parse_tool_calls(response: &str) -> (Vec<ToolCall>, String) {
    let mut calls = Vec::new();
    let mut clean = response.to_string();

    loop {
        let start = match clean.find("<tool_call>") {
            Some(s) => s,
            None => break,
        };
        let end = match clean[start..].find("</tool_call>") {
            Some(e) => start + e + "</tool_call>".len(),
            None => break,
        };
        let raw = &clean[start + "<tool_call>".len()..end - "</tool_call>".len()];
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
            let args = parsed.get("arguments").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
            calls.push(ToolCall { id: format!("call_{}", calls.len()), name: name.to_string(), arguments: args });
        }
        clean.replace_range(start..end, "");
    }

    (calls, clean.trim().to_string())
}

// ── Actor ───────────────────────────────────────────────────────────────────

pub struct ClaudeBridgeActor {
    claude_path: String,
}

impl ClaudeBridgeActor {
    pub fn new(claude_path: String) -> Self {
        Self { claude_path }
    }
}

impl Actor for ClaudeBridgeActor {
    type Context = Context<Self>;
}

impl Handler<LlmRequest> for ClaudeBridgeActor {
    type Result = ResponseFuture<Result<LlmResponse, String>>;

    fn handle(&mut self, msg: LlmRequest, _ctx: &mut Self::Context) -> Self::Result {
        let path = self.claude_path.clone();
        let prompt = msg.messages.iter()
            .map(|m| match m.role.as_str() {
                "system" => format!("[System]\n{}", m.content),
                "user" => format!("User: {}", m.content),
                "assistant" => format!("Assistant: {}", m.content),
                _ => format!("{}: {}", m.role, m.content),
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        Box::pin(async move {
            match call_claude(&path, &prompt, None, None).await {
                Ok(text) => { let (tc, c) = parse_tool_calls(&text); Ok(LlmResponse { content: c, model: "claude-cli".into(), prompt_tokens: 0, completion_tokens: 0, prompt_cache_hit_tokens: 0, prompt_cache_miss_tokens: 0, tool_calls: tc }) }
                Err(e) => Err(e),
            }
        })
    }
}

impl Handler<ChatRequest> for ClaudeBridgeActor {
    type Result = ResponseFuture<Result<LlmResponse, String>>;

    fn handle(&mut self, msg: ChatRequest, _ctx: &mut Self::Context) -> Self::Result {
        let path = self.claude_path.clone();
        let user_msg = msg.message.clone();

        let session_id = std::env::var("CLAUDE_SESSION_ID").ok();

        // Inject memory fragments + plugin contexts before user message.
        let mut contexts: Vec<String> = Vec::new();
        for ctx in &msg.contexts {
            contexts.push(ctx.clone());
        }
        let contexts_prompt = if contexts.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", contexts.join("\n"))
        };

        let tools_prompt = build_tools_system_prompt(&msg.tools);
        let prompt = if tools_prompt.is_empty() {
            format!("{}{}", contexts_prompt, user_msg)
        } else {
            format!("{}\n\n{}{}", tools_prompt, contexts_prompt, user_msg)
        };

        Box::pin(async move {
            match call_claude(&path, &prompt, session_id.as_deref(), None).await {
                Ok(text) => {
                    let (tool_calls, content) = parse_tool_calls(&text);
                    Ok(LlmResponse { content, model: "claude-cli".into(), prompt_tokens: 0, completion_tokens: 0, prompt_cache_hit_tokens: 0, prompt_cache_miss_tokens: 0, tool_calls })
                }
                Err(e) => Err(e),
            }
        })
    }
}

// ── Claude CLI call ─────────────────────────────────────────────────────────

fn check_claude_available(path: &str) -> bool {
    std::process::Command::new(path).arg("--version")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

async fn call_claude(path: &str, message: &str, session_id: Option<&str>, model: Option<&str>) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(path);

    // Claude sessions are stored per project (working directory).
    // Use the user's home dir so sessions from interactive `claude` are found.
    if let Ok(home) = std::env::var("USERPROFILE") {
        cmd.current_dir(&home);
    }

    // Resume an existing session by UUID (avoids --session-id session lock).
    if let Some(sid) = session_id {
        cmd.arg("--resume").arg(sid);
    }

    cmd.arg("--output-format").arg("text")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(m) = model { cmd.arg("--model").arg(m); }

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {}", e))?;

    // Write the message to stdin asynchronously, then close to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(message.as_bytes()).await.map_err(|e| format!("stdin write: {}", e))?;
        stdin.write_all(b"\n").await.ok();
        drop(stdin);
    }

    let output = child.wait_with_output().await.map_err(|e| format!("wait failed: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exit {}: {}", output.status.code().unwrap_or(-1), stderr.trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { Err("empty response".into()) } else { Ok(text) }
}

pub fn probe_claude() -> (bool, String) {
    let path = std::env::var("CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    (check_claude_available(&path), path)
}
