//! 信令处理 — 通过事件总线交换 SDP Offer/Answer/ICE Candidate

use plugin_core::{AgentEvent, EventSource, EventType, LogCallback};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::peer;

/// 信令消息类型
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum SignalingMessage {
    /// SDP Offer
    Offer {
        sdp: String,
    },
    /// SDP Answer
    Answer {
        sdp: String,
    },
    /// ICE Candidate
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

/// 信令处理器 — 监听事件总线中的信令消息并分发给对应 peer
pub struct SignalingHandler {
    emitter: Arc<dyn plugin_core::EventEmitter>,
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    logger: Option<Arc<dyn LogCallback>>,
}

impl SignalingHandler {
    pub fn new(
        emitter: Arc<dyn plugin_core::EventEmitter>,
        peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
        logger: Option<Arc<dyn LogCallback>>,
    ) -> Self {
        Self {
            emitter,
            peers,
            logger,
        }
    }

    /// 处理来自事件总线的事件
    pub fn handle_event(&self, event: &AgentEvent) {
        // 只处理来自其他插件的 WebRTC 信令事件
        if event.event_type != EventType::PluginNotification
            && event.event_type != EventType::Custom("webrtc_signaling".into())
        {
            return;
        }

        let peer_id = match event.data.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return,
        };

        let signaling: SignalingMessage = match serde_json::from_value(event.data.get("signaling").cloned().unwrap_or_default()) {
            Ok(s) => s,
            Err(e) => {
                if let Some(ref log) = self.logger {
                    log.log(
                        plugin_core::LogLevel::Warn,
                        "webrtc",
                        &format!("信令解析失败: {}", e),
                    );
                }
                return;
            }
        };

        if let Some(ref log) = self.logger {
            log.log(
                plugin_core::LogLevel::Debug,
                "webrtc",
                &format!("收到信令: peer={}, type={:?}", peer_id, std::mem::discriminant(&signaling)),
            );
        }

        let peers = self.peers.clone();
        let _emitter = self.emitter.clone();
        let logger = self.logger.clone();
        let peer_id_clone = peer_id.clone();

        // 在后台线程处理信令
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                let guard = peers.lock().await;
                match guard.get(&peer_id_clone) {
                    Some(handle) => {
                        match signaling {
                            SignalingMessage::Answer { sdp } => {
                                if let Err(e) = handle.set_remote_sdp(&sdp).await {
                                    if let Some(ref log) = logger {
                                        log.log(
                                            plugin_core::LogLevel::Error,
                                            "webrtc",
                                            &format!("设置 remote SDP 失败: {}", e),
                                        );
                                    }
                                } else if let Some(ref log) = logger {
                                    log.log(
                                        plugin_core::LogLevel::Info,
                                        "webrtc",
                                        &format!("Peer {} 连接已建立", peer_id_clone),
                                    );
                                }
                            }
                            SignalingMessage::IceCandidate {
                                candidate,
                                sdp_mid,
                                sdp_mline_index,
                            } => {
                                if let Err(e) = handle.add_ice_candidate(&candidate, sdp_mid.as_deref(), sdp_mline_index).await {
                                    if let Some(ref log) = logger {
                                        log.log(
                                            plugin_core::LogLevel::Warn,
                                            "webrtc",
                                            &format!("添加 ICE candidate 失败: {}", e),
                                        );
                                    }
                                }
                            }
                            SignalingMessage::Offer { sdp: _ } => {
                                // Answer 方收到 Offer → 创建 Answer
                                // 注意：这里需要 emitter 来创建 PeerHandle，但当前架构中
                                // Offer 信令通常由外部（如信令服务器）转发。
                                // 在局域网场景中，Offer 通过 DataChannel 或手动交换。
                                // 此处仅记录日志，实际 Answer 创建由 webrtc_answer_peer 工具完成。
                                if let Some(ref log) = logger {
                                    log.log(
                                        plugin_core::LogLevel::Info,
                                        "webrtc",
                                        &format!("收到 Offer 信令 (peer={})，请调用 webrtc_answer_peer 应答", peer_id_clone),
                                    );
                                }
                            }
                        }
                    }
                    None => {
                        // peer 不存在，可能是新连接请求
                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Warn,
                                "webrtc",
                                &format!("收到未知 peer {} 的信令", peer_id_clone),
                            );
                        }
                    }
                }
            });
        });
    }

    /// 发送信令消息到事件总线
    pub fn send_signaling(&self, peer_id: &str, msg: SignalingMessage) {
        self.emitter.emit(AgentEvent::new(
            EventType::Custom("webrtc_signaling".into()),
            EventSource::Plugin("webrtc".into()),
            serde_json::json!({
                "peer_id": peer_id,
                "signaling": msg,
            }),
        ));
    }
}

// ─── 内置信令服务器（WebSocket） ──────────────────────────────────

use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap as StdHashMap;
use std::sync::Arc as StdArc;
use tokio::sync::Mutex as TokioMutex;
use tokio_tungstenite::tungstenite::Message;

/// 房间内的连接信息
struct RoomPeer {
    /// WebSocket 发送端
    tx: tokio::sync::mpsc::UnboundedSender<Message>,
    /// 角色：offer / answer
    role: String,
}

/// 内置信令服务器状态
struct SignalingServer {
    /// room_id → (role → sender)
    rooms: TokioMutex<StdHashMap<String, StdHashMap<String, RoomPeer>>>,
    /// 事件发射器（用于将信令转发到插件内部）
    emitter: StdArc<dyn plugin_core::EventEmitter>,
    /// PeerConnection 句柄（用于自动创建 Offer）
    peers: StdArc<tokio::sync::Mutex<StdHashMap<String, super::peer::PeerHandle>>>,
    /// Tokio runtime handle（所有 PeerConnection 操作在此 runtime 上执行）
    rt_handle: TokioMutex<Option<tokio::runtime::Handle>>,
    logger: Option<StdArc<dyn LogCallback>>,
}

/// 启动内置信令服务器
///
/// 监听 ws://127.0.0.1:9876，对端浏览器连接后自动交换 SDP/ICE。
/// URL 格式: ws://127.0.0.1:9876/ws?room=xxx&role=offer|answer
pub fn start_builtin_signaling_server(
    emitter: StdArc<dyn plugin_core::EventEmitter>,
    peers: StdArc<tokio::sync::Mutex<StdHashMap<String, super::peer::PeerHandle>>>,
    logger: Option<StdArc<dyn LogCallback>>,
    port: u16,
) {
    let server = StdArc::new(SignalingServer {
        rooms: TokioMutex::new(StdHashMap::new()),
        emitter,
        peers,
        rt_handle: TokioMutex::new(None),
        logger,
    });

    // 在独立线程中启动 Tokio runtime（使用 multi_thread 确保长期存活）
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("信令服务器: 无法创建 tokio runtime");

        let rt_handle = rt.handle().clone();

        rt.block_on(async move {
            // 注入 runtime handle
            {
                let mut h = server.rt_handle.lock().await;
                *h = Some(rt_handle);
            }

            let addr = format!("127.0.0.1:{}", port);
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    if let Some(ref log) = server.logger {
                        log.log(
                            plugin_core::LogLevel::Error,
                            "webrtc-signaling",
                            &format!("信令服务器绑定失败 {}: {}", addr, e),
                        );
                    }
                return;
            }
        };

        if let Some(ref log) = server.logger {
            log.log(
                plugin_core::LogLevel::Info,
                "webrtc-signaling",
                &format!("内置信令服务器已启动: ws://{}", addr),
            );
        }

        while let Ok((stream, peer_addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(server, stream, peer_addr).await {
                    eprintln!("信令连接错误 [{}]: {}", peer_addr, e);
                }
            });
        }
        });
    });
}

async fn handle_connection(
    server: StdArc<SignalingServer>,
    stream: tokio::net::TcpStream,
    peer_addr: std::net::SocketAddr,
) -> Result<(), String> {
    // 用 accept_hdr_async 捕获请求 URI
    let uri_from_req = StdArc::new(std::sync::Mutex::new(String::new()));
    let uri_clone = uri_from_req.clone();

    let ws_stream = tokio_tungstenite::accept_hdr_async(
        stream,
        move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
              resp: tokio_tungstenite::tungstenite::handshake::server::Response|
              -> Result<tokio_tungstenite::tungstenite::handshake::server::Response,
                        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse> {
            *uri_clone.lock().unwrap() = req.uri().to_string();
            Ok(resp)
        },
    )
    .await
    .map_err(|e| format!("WebSocket 握手失败: {}", e))?;

    let uri = uri_from_req.lock().unwrap().clone();
    let params = parse_query_params(&uri);

    let room_id = params.get("room").cloned().unwrap_or_else(|| "default".into());
    let role = params.get("role").cloned().unwrap_or_else(|| "answer".into());

    if let Some(ref log) = server.logger {
        log.log(
            plugin_core::LogLevel::Info,
            "webrtc-signaling",
            &format!("新连接 [{}]: room={}, role={}", peer_addr, room_id, role),
        );
    }

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

    // 注册到房间
    {
        let mut rooms = server.rooms.lock().await;
        let room = rooms.entry(room_id.clone()).or_default();
        room.insert(role.clone(), RoomPeer { tx: tx.clone(), role: role.clone() });
    }

    // 如果对端是 answer 方（浏览器），自动创建 Offer
    if role == "answer" {
        let peer_id = format!("browser-{}", &room_id);
        let emitter = server.emitter.clone();
        let peers = server.peers.clone();
        let logger = server.logger.clone();
        let tx_for_offer = tx.clone();
        let rt_handle = server.rt_handle.lock().await.clone().expect("rt_handle not set");

        rt_handle.spawn(async move {
            match super::peer::PeerHandle::new_offer(&peer_id, emitter.clone(), logger.clone()).await {
                Ok(handle) => {
                    // 发送 Offer 到浏览器
                    let sdp = handle.local_sdp().await;
                        let offer_msg = serde_json::json!({
                            "peer_id": peer_id,
                            "signaling": {
                                "type": "Offer",
                                "payload": { "sdp": sdp }
                            }
                        });
                        let _ = tx_for_offer.send(Message::Text(offer_msg.to_string()));

                        // 保存 PeerHandle
                        let mut guard = peers.lock().await;
                        guard.insert(peer_id.clone(), handle);

                        if let Some(ref log) = logger {
                            log.log(plugin_core::LogLevel::Info, "webrtc-signaling",
                                &format!("自动创建 Offer: peer={}", peer_id));
                        }
                    }
                    Err(e) => {
                        if let Some(ref log) = logger {
                            log.log(plugin_core::LogLevel::Error, "webrtc-signaling",
                                &format!("自动创建 Offer 失败: {}", e));
                        }
                    }
                }
            });
    }

    // 转发线程：从 mpsc channel 读取消息 → 发送到 WebSocket
    let forward_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 读取 WebSocket 消息
    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };

        // 二进制消息 = 音频数据 → emit webrtc_audio_received
        if msg.is_binary() {
            let audio_data = msg.into_data();
            let peer_id = format!("browser-{}", &room_id);
            server.emitter.emit(AgentEvent::new(
                EventType::Custom("webrtc_audio_received".into()),
                EventSource::Plugin("webrtc".into()),
                serde_json::json!({
                    "peer_id": peer_id,
                    "codec": "pcm_i16",
                    "data": base64_encode(&audio_data),
                }),
            ));
            continue;
        }

        if msg.is_text() {
            let text = msg.to_text().unwrap_or("").to_string();
            if text.is_empty() {
                continue;
            }

            // 解析信令消息
            if let Ok(sig) = serde_json::from_str::<serde_json::Value>(&text) {
                let sig_type = sig["signaling"]["type"].as_str().unwrap_or("").to_string();

                // 处理 Answer / ICE → 应用到 PeerHandle
                let peer_id = sig.get("peer_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !peer_id.is_empty() {
                    let peers = server.peers.clone();
                    let logger = server.logger.clone();
                    let sig_clone = sig.clone();
                    let sig_type_clone = sig_type.clone();
                    let rt_handle = server.rt_handle.lock().await.clone().expect("rt_handle not set");

                    rt_handle.spawn(async move {
                        let guard = peers.lock().await;
                        if let Some(handle) = guard.get(&peer_id) {
                            match sig_type_clone.as_str() {
                                "Answer" => {
                                    if let Some(sdp) = sig_clone["signaling"]["payload"]["sdp"].as_str() {
                                        if let Err(e) = handle.set_remote_sdp(sdp).await {
                                            if let Some(ref log) = logger {
                                                log.log(plugin_core::LogLevel::Error, "webrtc-signaling",
                                                    &format!("设置 remote SDP 失败: {}", e));
                                            }
                                        } else if let Some(ref log) = logger {
                                            log.log(plugin_core::LogLevel::Info, "webrtc-signaling",
                                                &format!("Answer 已应用: peer={}", peer_id));
                                        }
                                    }
                                }
                                "IceCandidate" => {
                                    let candidate = sig_clone["signaling"]["payload"]["candidate"].as_str().unwrap_or("");
                                    let sdp_mid = sig_clone["signaling"]["payload"]["sdp_mid"].as_str().map(|s| s.to_string());
                                    let sdp_mline_index = sig_clone["signaling"]["payload"]["sdp_mline_index"].as_u64().map(|v| v as u16);
                                    if !candidate.is_empty() {
                                        let _ = handle.add_ice_candidate(candidate, sdp_mid.as_deref(), sdp_mline_index).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                }

                // 转发给同房间另一个 peer
                let rooms = server.rooms.lock().await;
                if let Some(room) = rooms.get(&room_id) {
                    for (other_role, peer) in room.iter() {
                        if *other_role != role {
                            let _ = peer.tx.send(Message::Text(text.clone()));
                        }
                    }
                }
            }
        }
    }

    // 清理：从房间移除
    {
        let mut rooms = server.rooms.lock().await;
        if let Some(room) = rooms.get_mut(&room_id) {
            room.remove(&role);
            if room.is_empty() {
                rooms.remove(&room_id);
            }
        }
    }

    forward_handle.abort();

    if let Some(ref log) = server.logger {
        log.log(
            plugin_core::LogLevel::Info,
            "webrtc-signaling",
            &format!("连接断开 [{}]: room={}, role={}", peer_addr, room_id, role),
        );
    }

    Ok(())
}

/// 解析 URI 查询参数
fn parse_query_params(uri: &str) -> StdHashMap<String, String> {
    let mut params = StdHashMap::new();
    if let Some(query_start) = uri.find('?') {
        let query = &uri[query_start + 1..];
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            if let (Some(key), Some(val)) = (parts.next(), parts.next()) {
                params.insert(
                    urlencoding_decode(key),
                    urlencoding_decode(val),
                );
            }
        }
    }
    params
}

fn urlencoding_decode(s: &str) -> String {
    s.replace("%20", " ")
        .replace("%22", "\"")
        .replace("%3A", ":")
        .replace("%2F", "/")
        .replace("%3D", "=")
        .replace("%26", "&")
        .replace("%3F", "?")
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}
