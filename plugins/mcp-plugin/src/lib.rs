//! MCP Plugin — bridges external MCP servers into the tool system.
//!
//! Reads `MCP_SERVERS` env (JSON array), spawns servers, discovers tools via
//! JSON-RPC `tools/list`, registers proxy executors.
//! Tools registered as `{server}_{tool_name}`, e.g. `fs_read`.
//!
//! # Env
//! ```env
//! MCP_SERVERS=[{"name":"fs","command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","/tmp"]}]
//! ```

use plugin_interface::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};

// ── Global connection pool ───────────────────────────────────────────────────

static MCP_POOL: LazyLock<Mutex<HashMap<String, Mutex<McpConnection>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct McpConnection {
    process: Child,
    stdin: ChildStdin,
    next_id: u64,
}

impl McpConnection {
    fn spawn(command: &str, args: &[String]) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn '{}': {}", command, e))?;

        let stdin = child.stdin.take().ok_or("stdin not available")?;
        Ok(Self { process: child, stdin, next_id: 1 })
    }

    fn call(&mut self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or(serde_json::Value::Null),
        });

        let mut line = serde_json::to_string(&request).map_err(|e| format!("serialize: {}", e))?;
        line.push('\n');

        self.stdin.write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("write: {}", e))?;

        let stdout = self.process.stdout.as_mut().ok_or("stdout not available")?;
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        loop {
            buf.clear();
            reader.read_line(&mut buf).map_err(|e| format!("read: {}", e))?;
            if buf.trim().is_empty() { continue; }
            let parsed: serde_json::Value = serde_json::from_str(&buf)
                .map_err(|e| format!("parse: {}", e))?;
            if parsed.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = parsed.get("error") {
                    let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown");
                    return Err(msg.to_string());
                }
                return Ok(parsed.get("result").cloned().unwrap_or(serde_json::Value::Null));
            }
        }
    }
}

// ── Plugin ───────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct McpServerConfig {
    name: String,
    command: String,
    args: Vec<String>,
}

pub struct McpPlugin {
    info: PluginInfo,
    servers: Vec<McpServerConfig>,
}

impl McpPlugin {
    pub fn new() -> Self {
        let servers = std::env::var("MCP_SERVERS")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            info: PluginInfo {
                name: "mcp-plugin".into(),
                version: "0.1.0".into(),
                description: "MCP (Model Context Protocol) — connect external MCP servers".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            servers,
        }
    }
}

impl Plugin for McpPlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        if self.servers.is_empty() {
            log::info!("[mcp] no MCP_SERVERS configured, skipping");
            return Ok(());
        }

        log::info!("[mcp] connecting to {} MCP server(s)...", self.servers.len());

        for config in &self.servers {
            log::info!("[mcp:{}] spawning: {} {}", config.name, config.command, config.args.join(" "));

            let mut conn = match McpConnection::spawn(&config.command, &config.args) {
                Ok(c) => c,
                Err(e) => { log::warn!("[mcp:{}] spawn failed: {}", config.name, e); continue; }
            };

            match conn.call("initialize", Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "bn-agent", "version": "0.1.0" },
            }))) {
                Ok(_) => {
                    let _ = conn.call("notifications/initialized", None);
                    log::info!("[mcp:{}] initialized", config.name);
                }
                Err(e) => { log::warn!("[mcp:{}] init failed: {}", config.name, e); continue; }
            }

            let tool_list = match conn.call("tools/list", None) {
                Ok(r) => r.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                Err(e) => { log::warn!("[mcp:{}] tools/list failed: {}", config.name, e); continue; }
            };

            {
                let mut pool = MCP_POOL.lock().unwrap();
                pool.insert(config.name.clone(), Mutex::new(conn));
            }

            if let Some(ref reg) = ctx.tool_registry {
                let mut r = reg.lock().map_err(|e| format!("lock: {}", e))?;
                for td in &tool_list {
                    let tool_name = td.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                    let desc = td.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let input_schema = td.get("inputSchema").cloned()
                        .unwrap_or(serde_json::json!({"type":"object","properties":{}}));

                    let qname = format!("{}__{}", config.name, tool_name);
                    // leak 一个 ToolDef 使其拥有 'static 生命周期
                    let def = Box::new(ToolDef {
                        name: qname,
                        description: format!("[{}] {}", config.name, desc),
                        internal: false,
                        parameters: input_schema,
                    });
                    let leaked: &'static ToolDef = Box::leak(def);

                    r.register(Arc::new(McpToolProxy {
                        server_name: config.name.clone(),
                        tool_name: tool_name.clone(),
                        def_ptr: leaked,
                    }));
                    eprintln!("[mcp:{}] registered tool: {}", config.name, tool_name);
                }
            }

            eprintln!("[mcp:{}] ready ({} tools)", config.name, tool_list.len());
        }

        log::info!("[mcp] started");
        Ok(())
    }

    fn on_event(&self, _event: &Event) -> bool { true }
    fn stop(&mut self) { log::info!("[mcp] stopped"); }
}

// ── Tool proxy ───────────────────────────────────────────────────────────────

struct McpToolProxy {
    server_name: String,
    tool_name: String,
    def_ptr: &'static ToolDef,
}

impl ToolExecutor for McpToolProxy {
    fn def(&self) -> &ToolDef {
        self.def_ptr
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let pool = MCP_POOL.lock().unwrap();
        let mutex = match pool.get(&self.server_name) {
            Some(m) => m,
            None => return ToolResult::err(&format!("MCP server '{}' disconnected", self.server_name)),
        };
        let mut conn = match mutex.lock() {
            Ok(c) => c,
            Err(e) => return ToolResult::err(&format!("lock: {}", e)),
        };

        let mut clean = args.clone();
        if let serde_json::Value::Object(ref mut map) = clean {
            map.remove("chat_id");
        }

        match conn.call("tools/call", Some(serde_json::json!({
            "name": self.tool_name,
            "arguments": clean,
        }))) {
            Ok(result) => {
                let content = result.get("content").and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| serde_json::to_string_pretty(&result).unwrap_or_default());
                ToolResult::ok(&content)
            }
            Err(e) => ToolResult::err(&format!("MCP {} failed: {}", self.tool_name, e)),
        }
    }
}

// ── DLL exports ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(McpPlugin::new())
}

#[no_mangle]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
