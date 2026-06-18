//! Signaling — event-bus signaling handler + built-in WebSocket server.
//!
//! The SignalingHandler listens for `webrtc_signaling` events on EventBus and
//! dispatches Offer/Answer/ICE to the correct PeerHandle.
//!
//! The WebSocket server listens on configurable port (default 9876) for browser
//! clients, auto-creates Offers, and relays signaling messages.

use futures_util::{SinkExt, StreamExt};
use plugin_interface::*;
use std::collections::HashMap as StdHashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::peer;

// ─── Signaling Message ──────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum SignalingMessage {
    Offer { sdp: String },
    Answer { sdp: String },
    IceCandidate { candidate: String, sdp_mid: Option<String>, sdp_mline_index: Option<u16> },
}

// ─── Signaling Handler ──────────────────────────────────────────────

pub struct SignalingHandler {
    peers: Arc<Mutex<StdHashMap<String, peer::PeerHandle>>>,
}

impl SignalingHandler {
    pub fn new(
        peers: Arc<Mutex<StdHashMap<String, peer::PeerHandle>>>,
    ) -> Self {
        Self { peers }
    }

    pub fn handle_event(&self, event: &Event) {
        if event.topic != "webrtc_signaling" { return; }

        let peer_id = match event.data.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(), None => return,
        };

        let sig: SignalingMessage = match serde_json::from_value(
            event.data.get("signaling").cloned().unwrap_or_default(),
        ) {
            Ok(s) => s,
            Err(e) => { log::warn!("[webrtc] signaling parse: {}", e); return; }
        };

        log::debug!("[webrtc] signaling: peer={}, type={:?}", peer_id,
            std::mem::discriminant(&sig));

        let peers = self.peers.clone();
        let pid = peer_id.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("tokio");
            rt.block_on(async {
                let guard = peers.lock().await;
                match guard.get(&pid) {
                    Some(handle) => match sig {
                        SignalingMessage::Answer { sdp } => {
                            if let Err(e) = handle.set_remote_sdp(&sdp).await {
                                log::error!("[webrtc] set remote SDP: {}", e);
                            } else {
                                log::info!("[webrtc] Peer {} connected", pid);
                            }
                        }
                        SignalingMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index } => {
                            if let Err(e) = handle.add_ice_candidate(&candidate, sdp_mid.as_deref(), sdp_mline_index).await {
                                log::warn!("[webrtc] add ICE: {}", e);
                            }
                        }
                        SignalingMessage::Offer { .. } => {
                            log::info!("[webrtc] Got Offer for peer {}. Use webrtc_answer_peer tool.", pid);
                        }
                    },
                    None => log::warn!("[webrtc] unknown peer in signaling: {}", pid),
                }
            });
        });
    }
}

// ─── Built-in WebSocket Signaling Server ──────────────────────────

struct RoomPeer {
    tx: tokio::sync::mpsc::UnboundedSender<Message>,
}

struct SignalingServer {
    rooms: Mutex<StdHashMap<String, StdHashMap<String, RoomPeer>>>,
    eb: Addr<EventBus>,
    peers: Arc<Mutex<StdHashMap<String, peer::PeerHandle>>>,
}

pub fn start_server(
    eb: Addr<EventBus>,
    peers: Arc<Mutex<StdHashMap<String, peer::PeerHandle>>>,
    port: u16,
) {
    let server = Arc::new(SignalingServer {
        rooms: Mutex::new(StdHashMap::new()),
        eb,
        peers,
    });

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all().build().expect("signaling rt");

        rt.block_on(async {
            let addr = format!("127.0.0.1:{}", port);
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => { log::error!("[webrtc-signaling] bind {}: {}", addr, e); return; }
            };
            log::info!("[webrtc-signaling] server at ws://{}", addr);

            while let Ok((stream, peer_addr)) = listener.accept().await {
                let sv = server.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_ws(sv, stream, peer_addr).await {
                        log::warn!("[webrtc-signaling] conn [{}]: {}", peer_addr, e);
                    }
                });
            }
        });
    });
}

async fn handle_ws(
    server: Arc<SignalingServer>,
    stream: tokio::net::TcpStream,
    peer_addr: std::net::SocketAddr,
) -> Result<(), String> {
    let uri_store = Arc::new(std::sync::Mutex::new(String::new()));
    let uri_clone = uri_store.clone();

    let ws_stream = tokio_tungstenite::accept_hdr_async(
        stream,
        move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
              resp: tokio_tungstenite::tungstenite::handshake::server::Response|
              -> Result<_, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse> {
            *uri_clone.lock().unwrap() = req.uri().to_string();
            Ok(resp)
        },
    )
    .await.map_err(|e| format!("ws handshake: {}", e))?;

    let uri = uri_store.lock().unwrap().clone();
    let params = parse_query(&uri);
    let room_id = params.get("room").cloned().unwrap_or_else(|| "default".into());
    let role = params.get("role").cloned().unwrap_or_else(|| "answer".into());

    log::info!("[webrtc-signaling] new [{}]: room={}, role={}", peer_addr, room_id, role);

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

    // Register in room.
    {
        let mut rooms = server.rooms.lock().await;
        let room = rooms.entry(room_id.clone()).or_default();
        room.insert(role.clone(), RoomPeer { tx: tx.clone() });
    }

    // Auto-create Offer if answer side connects.
    if role == "answer" {
        let peer_id = format!("browser-{}", room_id);
        let eb = server.eb.clone();
        let peers = server.peers.clone();
        let tx_for_offer = tx.clone();
        let _eb2 = eb.clone();

        tokio::spawn(async move {
            match peer::PeerHandle::new_offer(&peer_id, eb).await {
                Ok(handle) => {
                    let sdp = handle.local_sdp().await;
                    let offer_msg = serde_json::json!({
                        "peer_id": peer_id,
                        "signaling": { "type": "Offer", "payload": { "sdp": sdp } }
                    });
                    let _ = tx_for_offer.send(Message::Text(offer_msg.to_string()));
                    let mut guard = peers.lock().await;
                    guard.insert(peer_id.clone(), handle);
                    log::info!("[webrtc-signaling] auto-offer: peer={}", peer_id);
                }
                Err(e) => log::error!("[webrtc-signaling] auto-offer failed: {}", e),
            }
        });
    }

    // Forward thread: mpsc → WebSocket.
    let fwd = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_tx.send(msg).await.is_err() { break; }
        }
    });

    // Read loop.
    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(m) => m, Err(_) => break,
        };

        if msg.is_binary() {
            let audio_data = msg.into_data();
            let peer_id = format!("browser-{}", room_id);
            server.eb.do_send(Event::new(
                "webrtc_audio_received",
                serde_json::json!({"peer_id": peer_id, "codec": "pcm_i16", "data": crate::base64_encode(&audio_data)}),
                "webrtc-plugin",
            ));
            continue;
        }

        if msg.is_text() {
            let text = msg.to_text().unwrap_or("").to_string();
            if text.is_empty() { continue; }

            // Parse signaling and route to PeerHandle.
            if let Ok(sig) = serde_json::from_str::<serde_json::Value>(&text) {
                let pid = sig.get("peer_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let sig_type = sig["signaling"]["type"].as_str().unwrap_or("").to_string();

                if !pid.is_empty() {
                    let peers = server.peers.clone();
                    let sig_clone = sig.clone();
                    tokio::spawn(async move {
                        let guard = peers.lock().await;
                        if let Some(handle) = guard.get(&pid) {
                            match sig_type.as_str() {
                                "Answer" => {
                                    if let Some(sdp) = sig_clone["signaling"]["payload"]["sdp"].as_str() {
                                        if let Err(e) = handle.set_remote_sdp(sdp).await {
                                            log::error!("[webrtc-signaling] set SDP: {}", e);
                                        } else {
                                            log::info!("[webrtc-signaling] Answer applied: peer={}", pid);
                                        }
                                    }
                                }
                                "IceCandidate" => {
                                    let c = sig_clone["signaling"]["payload"]["candidate"].as_str().unwrap_or("");
                                    let mid = sig_clone["signaling"]["payload"]["sdp_mid"].as_str().map(String::from);
                                    let idx = sig_clone["signaling"]["payload"]["sdp_mline_index"].as_u64().map(|v| v as u16);
                                    if !c.is_empty() {
                                        let _ = handle.add_ice_candidate(c, mid.as_deref(), idx).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                }

                // Relay to other room member.
                let rooms = server.rooms.lock().await;
                if let Some(room) = rooms.get(&room_id) {
                    for (other, rp) in room.iter() {
                        if *other != role {
                            let _ = rp.tx.send(Message::Text(text.clone()));
                        }
                    }
                }
            }
        }
    }

    // Cleanup.
    {
        let mut rooms = server.rooms.lock().await;
        if let Some(room) = rooms.get_mut(&room_id) {
            room.remove(&role);
            if room.is_empty() { rooms.remove(&room_id); }
        }
    }
    fwd.abort();
    log::info!("[webrtc-signaling] closed [{}]: room={}, role={}", peer_addr, room_id, role);
    Ok(())
}

fn parse_query(uri: &str) -> StdHashMap<String, String> {
    let mut params = StdHashMap::new();
    if let Some(qs) = uri.find('?') {
        for pair in uri[qs + 1..].split('&') {
            let mut parts = pair.splitn(2, '=');
            if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                params.insert(url_decode(k), url_decode(v));
            }
        }
    }
    params
}

fn url_decode(s: &str) -> String {
    s.replace("%20", " ").replace("%3A", ":").replace("%2F", "/")
        .replace("%3D", "=").replace("%26", "&").replace("%3F", "?")
}
