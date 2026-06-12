//! plugin-interface — shared contract between the main application and plugins.
//!
//! ## Architecture
//!
//! ```text
//! PluginManager (Actor) ──owns──▶ LoadedPlugin (Box<dyn Plugin>)
//!        │                              │
//!        └── holds Addr ──▶ EventBus (Actor) ◀── subscribe/publish
//!        │                     │                      │
//!        │             PipelineActor            plugin actors
//!        │           (LLM + tool loop)         (internal actors)
//!        │
//!        └── holds ToolRegistry ──▶ shared tool executors
//! ```
//!
//! Every plugin is a `cdylib` that exports two `extern "C"` symbols:
//! - `plugin_create() -> Box<dyn Plugin>`
//! - `plugin_destroy(plugin: Box<dyn Plugin>)`

pub use actix::prelude::*;
use serde::{Deserialize, Serialize};
pub use serde_json;
pub use log;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Event ────────────────────────────────────────────────────────────────────

/// The core message that flows through the entire system.
#[derive(Message, Clone, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct Event {
    /// Topic string — subscribers match on this (e.g. `"greeting"`, `"user.message"`).
    pub topic: String,
    /// Arbitrary JSON payload.
    pub data: serde_json::Value,
    /// Name of the plugin that published this event.
    pub source: String,
    /// Millisecond timestamp (filled by EventBus on dispatch).
    pub timestamp: u64,
}

impl Event {
    pub fn new(
        topic: impl Into<String>,
        data: serde_json::Value,
        source: impl Into<String>,
    ) -> Self {
        Self { topic: topic.into(), data, source: source.into(), timestamp: 0 }
    }
}

// ── EventBus messages ────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "()")]
pub struct Subscribe { pub topic: String, pub recipient: Recipient<Event> }

#[derive(Message)]
#[rtype(result = "()")]
pub struct Unsubscribe { pub topic: String, pub recipient: Recipient<Event> }

// ── EventBus actor ───────────────────────────────────────────────────────────

pub struct EventBus {
    subscribers: HashMap<String, Vec<Recipient<Event>>>,
}

impl EventBus {
    pub fn new() -> Self { Self { subscribers: HashMap::new() } }
}

impl Actor for EventBus {
    type Context = Context<Self>;
}

impl Handler<Event> for EventBus {
    type Result = ();

    fn handle(&mut self, mut event: Event, _ctx: &mut Self::Context) {
        event.timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        log::info!("[EventBus] dispatching topic='{}' from='{}'", event.topic, event.source);

        let dispatch = |topic: &str| {
            if let Some(recipients) = self.subscribers.get(topic) {
                for r in recipients {
                    // do_send 使用无界邮箱，避免高频事件丢消息
                    r.do_send(event.clone());
                }
            }
        };
        dispatch(&event.topic);
        if event.topic != "*" {
            dispatch("*");
        }
    }
}

impl Handler<Subscribe> for EventBus {
    type Result = ();
    fn handle(&mut self, msg: Subscribe, _: &mut Self::Context) {
        log::info!("[EventBus] +subscribe topic='{}'", msg.topic);
        self.subscribers.entry(msg.topic).or_default().push(msg.recipient);
    }
}

impl Handler<Unsubscribe> for EventBus {
    type Result = ();
    fn handle(&mut self, msg: Unsubscribe, _: &mut Self::Context) {
        if let Some(recipients) = self.subscribers.get_mut(&msg.topic) {
            recipients.retain(|r| r.connected());
        }
        log::info!("[EventBus] -unsubscribe topic='{}'", msg.topic);
    }
}

// ── Tool system ──────────────────────────────────────────────────────────────

/// A tool definition exposed to the LLM for function calling.
#[derive(Clone, Debug, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's parameters.
    pub parameters: serde_json::Value,
    /// Internal tools are hidden from the LLM, only callable by other plugins.
    pub internal: bool,
}

/// Result of executing a tool.
#[derive(Clone, Debug)]
pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(content: &str) -> Self {
        Self { success: true, content: content.to_string(), error: None }
    }
    pub fn err(msg: &str) -> Self {
        Self { success: false, content: String::new(), error: Some(msg.to_string()) }
    }
}

/// Trait implemented by every tool.
pub trait ToolExecutor: Send + Sync {
    fn def(&self) -> &ToolDef;
    fn execute(&self, args: &serde_json::Value) -> ToolResult;
}

/// Thread-safe registry of tools.
pub struct ToolRegistry {
    executors: HashMap<String, Arc<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { executors: HashMap::new() } }

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

    /// Clone the Arc so the caller can execute without holding the lock.
    pub fn get_executor(&self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.executors.get(name).cloned()
    }

    pub fn unregister(&mut self, name: &str) { self.executors.remove(name); }
    pub fn clear(&mut self) { self.executors.clear(); }
    pub fn is_empty(&self) -> bool { self.executors.is_empty() }
}

// ── LLM types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

/// Simple LLM request (used by plugins directly).
#[derive(Message)]
#[rtype(result = "Result<LlmResponse, String>")]
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// Full-featured chat request sent by PipelineActor.
#[derive(Message)]
#[rtype(result = "Result<LlmResponse, String>")]
pub struct ChatRequest {
    pub chat_id: i64,
    pub message: String,
    pub tools: Vec<serde_json::Value>,
    pub skip_store: bool,
    /// Plugin passive contexts (snapshots) injected into messages.
    pub contexts: Vec<String>,
    /// Index into the jailbreak prompt list, None = no injection.
    pub jailbreak_index: Option<usize>,
    /// Base64-encoded image to include as multimodal input.
    pub image_base64: Option<String>,
    /// Base64-encoded video to include as multimodal input.
    pub video_base64: Option<String>,
    /// MIME type for the video (e.g. video/mp4).
    pub video_mime: Option<String>,
    /// Base64-encoded file data (generic documents).
    pub file_base64: Option<String>,
    /// Original file name.
    pub file_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Tool calls extracted from the LLM response.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

// ── PluginError ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PluginError {
    LoadError(String),
    InitError(String),
    NotFound(String),
    AlreadyLoaded(String),
    VersionMismatch(String),
    Io(std::io::Error),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadError(s) => write!(f, "load failed: {}", s),
            Self::InitError(s) => write!(f, "init failed: {}", s),
            Self::NotFound(s) => write!(f, "not found: {}", s),
            Self::AlreadyLoaded(s) => write!(f, "already loaded: {}", s),
            Self::VersionMismatch(s) => write!(f, "version mismatch: {}", s),
            Self::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for PluginError {}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

// ── Plugin trait ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    /// Minimum host version required.
    pub min_host_version: String,
}

/// Plugin HTTP API handler trait.
pub trait PluginApi: Send + Sync {
    /// Handle an HTTP request. Returns `(status_code, body)` or `None`.
    fn handle_api(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Option<(u16, String)> {
        let _ = (method, path, body);
        None
    }
}

/// Context handed to a plugin when `start()` is called.
pub struct PluginContext {
    pub event_bus: Addr<EventBus>,
    pub plugin_name: String,
    pub llm: Option<Recipient<LlmRequest>>,
    /// Tool registry shared across all plugins and the host.
    pub tool_registry: Option<Arc<Mutex<ToolRegistry>>>,
}

/// The **object-safe** trait every plugin must implement.
///
/// # Lifecycle
///
/// 1. `PluginManager` loads the `.so` / `.dll`, calls `plugin_create()` →
///    gets a `Box<dyn Plugin>`.
/// 2. `info()` is called to obtain metadata.
/// 3. `start(ctx)` is called — the plugin creates its internal actix actors,
///    subscribes to events, and registers tools.
/// 4. At runtime, `on_event()` is called for every broadcast event, and
///    `snapshot()` is polled before each LLM request.
/// 5. `stop()` is called → plugin stops all its actors, then the `Box` is
///    dropped and the library is unloaded.
pub trait Plugin: Send {
    /// Return static metadata.
    fn info(&self) -> PluginInfo;

    /// Called once after loading.
    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>>;

    /// Called before the plugin is unloaded.
    fn stop(&mut self);

    /// Receive a broadcast event. Return `true` to continue propagation,
    /// `false` to intercept (no further plugins receive this event).
    fn on_event(&self, _event: &Event) -> bool { true }

    /// Passive context injected into LLM messages before each request.
    /// Format: `【plugin_name】details`. Not persisted to chat history.
    fn snapshot(&self) -> Option<String> { None }

    /// Return an HTTP API handler, if this plugin exposes endpoints.
    fn api_handler(&self) -> Option<&dyn PluginApi> { None }
}

// ── FFI helpers ──────────────────────────────────────────────────────────────

#[allow(improper_ctypes_definitions)]
pub type PluginCreateFn = unsafe extern "C" fn() -> Box<dyn Plugin>;

#[allow(improper_ctypes_definitions)]
pub type PluginDestroyFn = unsafe extern "C" fn(plugin: Box<dyn Plugin>);

// ── PluginManager messages ───────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "Result<PluginInfo, String>")]
pub struct LoadPlugin { pub path: String }

#[derive(Message)]
#[rtype(result = "Result<(), String>")]
pub struct UnloadPlugin { pub name: String }

#[derive(Message)]
#[rtype(result = "Result<PluginInfo, String>")]
pub struct ReloadPlugin { pub name: String }

#[derive(Message)]
#[rtype(result = "Vec<PluginInfo>")]
pub struct ListPlugins;

/// Scan a directory and load all `.dll` / `.so` / `.dylib` files.
#[derive(Message)]
#[rtype(result = "Result<usize, String>")]
pub struct ScanAndLoad {
    pub plugin_dir: String,
    pub host_context: PluginContext,
}

/// Broadcast an event to all loaded plugins (calls `on_event` on each).
#[derive(Message)]
#[rtype(result = "()")]
pub struct BroadcastEvent(pub Event);

/// Refresh passive context snapshots from all plugins.
#[derive(Message)]
#[rtype(result = "()")]
pub struct RefreshSnapshots;

/// Proxy an HTTP request to a plugin's API handler.
#[derive(Message)]
#[rtype(result = "Option<(u16, String)>")]
pub struct ApiRequest {
    pub plugin: String,
    pub method: String,
    pub path: String,
    pub body: Option<String>,
}

/// Stop all loaded plugins.
#[derive(Message)]
#[rtype(result = "()")]
pub struct StopAll;
