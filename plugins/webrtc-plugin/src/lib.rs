//! WebRTC Plugin — actor-free port of BN_agent's webrtc-plugin.
//!
//! Manages WebRTC PeerConnections, audio tracks, DataChannels,
//! and a built-in WebSocket signaling server.

mod peer;
mod signaling;

use plugin_interface::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct WebrtcPlugin {
    info: PluginInfo,
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    signaling: Option<signaling::SignalingHandler>,
    event_bus: Option<Addr<EventBus>>,
}

impl WebrtcPlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "webrtc-plugin".into(),
                version: "0.1.0".into(),
                description: "WebRTC real-time audio/video communication".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            peers: Arc::new(Mutex::new(HashMap::new())),
            signaling: None,
            event_bus: None,
        }
    }
}

impl Plugin for WebrtcPlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        self.event_bus = Some(ctx.event_bus.clone());

        // Register 5 tools.
        if let Some(ref reg) = ctx.tool_registry {
            let mut r = reg.lock().map_err(|e| format!("lock: {}", e))?;

            let peers = self.peers.clone();
            let eb = ctx.event_bus.clone();

            r.register(Arc::new(WebrtcCreatePeerTool {
                peers: peers.clone(),
                event_bus: eb.clone(),
            }));
            r.register(Arc::new(WebrtcAnswerPeerTool {
                peers: peers.clone(),
                event_bus: eb.clone(),
            }));
            r.register(Arc::new(WebrtcSendDataTool { peers: peers.clone() }));
            r.register(Arc::new(WebrtcListPeersTool { peers: peers.clone() }));
            r.register(Arc::new(WebrtcClosePeerTool { peers: peers.clone() }));

            log::info!("[webrtc] registered 5 tools");
        }

        // Initialize signaling handler.
        let eb = ctx.event_bus.clone();
        self.signaling = Some(signaling::SignalingHandler::new(
            eb.clone(),
            self.peers.clone(),
        ));

        // Start built-in signaling server.
        let signaling_port: u16 = std::env::var("WEBRTC_SIGNALING_PORT")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(9876);
        signaling::start_server(eb.clone(), self.peers.clone(), signaling_port);

        log::info!("[webrtc] started (signaling port {})", signaling_port);
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[webrtc] stopping...");

        let peers = self.peers.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("tokio");
            rt.block_on(async {
                let mut guard = peers.lock().await;
                for (id, handle) in guard.iter() {
                    let _ = handle.close().await;
                    log::info!("[webrtc] closed peer: {}", id);
                }
                guard.clear();
            });
        });
    }

    fn on_event(&self, event: &Event) -> bool {
        // Route signaling events.
        if let Some(ref sig) = self.signaling {
            sig.handle_event(event);
        }

        match event.topic.as_str() {
            "webrtc_audio_send" => {
                let peer_id = match event.data.get("peer_id").and_then(|v| v.as_str()) {
                    Some(id) => id.to_string(), None => return true,
                };
                let audio_b64 = match event.data.get("data").and_then(|v| v.as_str()) {
                    Some(d) => d.to_string(), None => return true,
                };
                let peers = self.peers.clone();
                tokio::spawn(async move {
                    let guard = peers.lock().await;
                    if let Some(handle) = guard.get(&peer_id) {
                        let audio = match base64_decode(&audio_b64) {
                            Ok(d) => d,
                            Err(e) => { log::warn!("[webrtc] b64: {}", e); return; }
                        };
                        if let Err(e) = handle.send_audio(&audio).await {
                            log::warn!("[webrtc] send audio [{}]: {}", peer_id, e);
                        }
                    }
                });
            }

            "assistant.message" => {
                let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");
                if source != "webrtc" { return true; }
                let peer_id = event.data.get("chat_id").and_then(|v| v.as_str())
                    .map(String::from).unwrap_or_default();
                let text = match event.data.get("text").and_then(|v| v.as_str()) {
                    Some(t) => t.to_string(), None => return true,
                };
                let peers = self.peers.clone();
                tokio::spawn(async move {
                    let guard = peers.lock().await;
                    if let Some(handle) = guard.get(&peer_id) {
                        let msg = serde_json::json!({"type": "chat", "text": text});
                        if let Err(e) = handle.send_json(&msg).await {
                            log::warn!("[webrtc] send reply [{}]: {}", peer_id, e);
                        }
                    }
                });
            }

            _ => {}
        }
        true
    }
}

// ─── Tools ──────────────────────────────────────────────────────────

struct WebrtcCreatePeerTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    event_bus: Addr<EventBus>,
}

impl ToolExecutor for WebrtcCreatePeerTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "webrtc_create_peer".into(),
            description: "Create a WebRTC PeerConnection as Offer, returns local SDP.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object", "properties": {
                    "peer_id": {"type": "string", "description": "Peer identifier"}
                }, "required": ["peer_id"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(), None => return ToolResult::err("missing peer_id"),
        };
        let peers = self.peers.clone();
        let eb = self.event_bus.clone();

        let r = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().map_err(|e| format!("rt: {}", e))?;
            rt.block_on(async {
                let handle = peer::PeerHandle::new_offer(&peer_id, eb)
                    .await.map_err(|e| format!("create: {}", e))?;
                let sdp = handle.local_sdp().await;
                let mut guard = peers.lock().await;
                guard.insert(peer_id.clone(), handle);
                Ok::<_, String>(serde_json::json!({"peer_id": peer_id, "status": "created", "local_sdp": sdp}))
            })
        }).join();
        match r {
            Ok(Ok(j)) => ToolResult::ok(&j.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("panic"),
        }
    }
}

struct WebrtcAnswerPeerTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    event_bus: Addr<EventBus>,
}

impl ToolExecutor for WebrtcAnswerPeerTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "webrtc_answer_peer".into(),
            description: "Answer a WebRTC Offer, returns local Answer SDP.".into(),
            internal: false,
            parameters: serde_json::json!({

                "type": "object", "properties": {
                    "peer_id": {"type": "string", "description": "Peer identifier"},
                    "offer_sdp": {"type": "string", "description": "Remote SDP Offer"}
                }, "required": ["peer_id", "offer_sdp"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(), None => return ToolResult::err("missing peer_id"),
        };
        let offer_sdp = match args.get("offer_sdp").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(), None => return ToolResult::err("missing offer_sdp"),
        };
        let peers = self.peers.clone();
        let eb = self.event_bus.clone();

        let r = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().map_err(|e| format!("rt: {}", e))?;
            rt.block_on(async {
                let handle = peer::PeerHandle::new_answer(&peer_id, &offer_sdp, eb)
                    .await.map_err(|e| format!("answer: {}", e))?;
                let sdp = handle.local_sdp().await;
                let mut guard = peers.lock().await;
                guard.insert(peer_id.clone(), handle);
                Ok::<_, String>(serde_json::json!({"peer_id": peer_id, "status": "answered", "local_sdp": sdp}))
            })
        }).join();
        match r {
            Ok(Ok(j)) => ToolResult::ok(&j.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("panic"),
        }
    }
}

struct WebrtcSendDataTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
}

impl ToolExecutor for WebrtcSendDataTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "webrtc_send_data".into(),
            description: "Send text via WebRTC DataChannel to a peer.".into(),
            internal: false,
            parameters: serde_json::json!({

                "type": "object", "properties": {
                    "peer_id": {"type": "string", "description": "Peer ID"},
                    "message": {"type": "string", "description": "Text message"}
                }, "required": ["peer_id", "message"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(), None => return ToolResult::err("missing peer_id"),
        };
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(), None => return ToolResult::err("missing message"),
        };
        let peers = self.peers.clone();

        let r = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().map_err(|e| format!("rt: {}", e))?;
            rt.block_on(async {
                let guard = peers.lock().await;
                let h = guard.get(&peer_id).ok_or_else(|| format!("peer not found: {}", peer_id))?;
                h.send_text(&message).await.map_err(|e| format!("send: {}", e))?;
                Ok::<_, String>(serde_json::json!({"peer_id": peer_id, "status": "sent"}))
            })
        }).join();
        match r {
            Ok(Ok(j)) => ToolResult::ok(&j.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("panic"),
        }
    }
}

struct WebrtcListPeersTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
}

impl ToolExecutor for WebrtcListPeersTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "webrtc_list_peers".into(),
            description: "List active WebRTC peer connections.".into(),
            internal: false,
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        });
        &DEF
    }

    fn execute(&self, _args: &serde_json::Value) -> ToolResult {
        let peers = self.peers.clone();
        let r = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("rt");
            rt.block_on(async {
                let guard = peers.lock().await;
                let ids: Vec<String> = guard.keys().cloned().collect();
                serde_json::json!({"count": ids.len(), "peers": ids})
            })
        }).join();
        match r {
            Ok(j) => ToolResult::ok(&j.to_string()),
            Err(_) => ToolResult::err("panic"),
        }
    }
}

struct WebrtcClosePeerTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
}

impl ToolExecutor for WebrtcClosePeerTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "webrtc_close_peer".into(),
            description: "Close a WebRTC peer connection.".into(),
            internal: false,
            parameters: serde_json::json!({

                "type": "object", "properties": {
                    "peer_id": {"type": "string", "description": "Peer ID"}
                }, "required": ["peer_id"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(), None => return ToolResult::err("missing peer_id"),
        };
        let peers = self.peers.clone();

        let r = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().map_err(|e| format!("rt: {}", e))?;
            rt.block_on(async {
                let mut guard = peers.lock().await;
                match guard.remove(&peer_id) {
                    Some(h) => { h.close().await; Ok::<_, String>(serde_json::json!({"peer_id": peer_id, "status": "closed"})) }
                    None => Err(format!("peer not found: {}", peer_id)),
                }
            })
        }).join();
        match r {
            Ok(Ok(j)) => ToolResult::ok(&j.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("panic"),
        }
    }
}

// ─── base64 helpers ─────────────────────────────────────────────────

pub(crate) fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub(crate) fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s).map_err(|e| format!("b64: {}", e))
}

// ─── FFI ─────────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> { Box::new(WebrtcPlugin::new()) }

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
