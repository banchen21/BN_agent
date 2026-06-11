//! PeerConnection 管理 — 封装 WebRTC PeerConnection、DataChannel 和音频轨道

use bytes::Bytes;
use plugin_core::{AgentEvent, EventSource, EventType, LogCallback};
use std::sync::Arc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_remote::TrackRemote;

pub struct AudioTrackSender {
    track: Arc<TrackLocalStaticSample>,
}

impl AudioTrackSender {
    pub async fn write_samples(&self, data: &[u8]) -> Result<(), String> {
        use webrtc::media::Sample;
        let sample = Sample {
            data: Bytes::copy_from_slice(data),
            duration: std::time::Duration::from_millis(20),
            ..Default::default()
        };
        self.track.write_sample(&sample).await.map_err(|e| format!("写入音频采样失败: {}", e))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerRole { Offer, Answer }

pub struct PeerHandle {
    peer_id: String,
    role: PeerRole,
    connection: Arc<RTCPeerConnection>,
    data_channel: Option<Arc<RTCDataChannel>>,
    audio_sender: Option<AudioTrackSender>,
    emitter: Arc<dyn plugin_core::EventEmitter>,
    logger: Option<Arc<dyn LogCallback>>,
    local_sdp: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl PeerHandle {
    async fn build_api() -> Result<webrtc::api::API, String> {
        let mut m = MediaEngine::default();
        m.register_default_codecs().map_err(|e| format!("注册编解码器失败: {}", e))?;
        let registry = interceptor::registry::Registry::new();
        let registry = register_default_interceptors(registry, &mut m).map_err(|e| format!("注册拦截器失败: {}", e))?;
        Ok(APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build())
    }

    fn default_config() -> RTCConfiguration {
        RTCConfiguration {
            ice_servers: vec![],
            ..Default::default()
        }
    }

    pub async fn new_offer(peer_id: &str, emitter: Arc<dyn plugin_core::EventEmitter>, logger: Option<Arc<dyn LogCallback>>) -> Result<Self, String> {
        let api = Self::build_api().await?;
        let config = Self::default_config();
        let connection = Arc::new(api.new_peer_connection(config).await.map_err(|e| format!("创建 PeerConnection 失败: {}", e))?);
        let local_sdp = Arc::new(tokio::sync::Mutex::new(None));
        let dc = connection.create_data_channel("data", None).await.map_err(|e| format!("创建 DataChannel 失败: {}", e))?;
        let audio_sender = Self::add_audio_track(&connection).await?;
        Self::setup_data_channel_handler(&dc, peer_id, &emitter, &logger);
        Self::setup_state_handler(&connection, peer_id, &emitter, &logger);
        Self::setup_ice_handler(&connection, peer_id, &emitter);
        Self::setup_remote_track_handler(&connection, peer_id, &emitter, &logger);
        let offer = connection.create_offer(None).await.map_err(|e| format!("创建 Offer 失败: {}", e))?;
        connection.set_local_description(offer.clone()).await.map_err(|e| format!("设置本地描述失败: {}", e))?;
        let sdp = offer.sdp.clone();
        *local_sdp.lock().await = Some(sdp.clone());
        emitter.emit(AgentEvent::new(EventType::Custom("webrtc_signaling".into()), EventSource::Plugin("webrtc".into()), serde_json::json!({"peer_id": peer_id, "signaling": {"type": "Offer", "payload": {"sdp": sdp}}})));
        if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("[Offer] Peer {} 已创建", peer_id)); }
        Ok(Self { peer_id: peer_id.to_string(), role: PeerRole::Offer, connection, data_channel: Some(dc), audio_sender: Some(audio_sender), emitter, logger, local_sdp })
    }

    pub async fn new_answer(peer_id: &str, remote_offer_sdp: &str, emitter: Arc<dyn plugin_core::EventEmitter>, logger: Option<Arc<dyn LogCallback>>) -> Result<Self, String> {
        let api = Self::build_api().await?;
        let config = Self::default_config();
        let connection = Arc::new(api.new_peer_connection(config).await.map_err(|e| format!("创建 PeerConnection 失败: {}", e))?);
        let local_sdp = Arc::new(tokio::sync::Mutex::new(None));
        let audio_sender = Self::add_audio_track(&connection).await?;
        Self::setup_state_handler(&connection, peer_id, &emitter, &logger);
        Self::setup_ice_handler(&connection, peer_id, &emitter);
        Self::setup_remote_track_handler(&connection, peer_id, &emitter, &logger);
        {
            let emitter = Arc::clone(&emitter); let peer_id = peer_id.to_string(); let logger = logger.clone();
            connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let emitter = Arc::clone(&emitter); let peer_id = peer_id.clone(); let logger = logger.clone();
                Box::pin(async move {
                    if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("[Answer] 远端 DataChannel: {}", dc.label())); }
                    Self::setup_data_channel_handler(&dc, &peer_id, &emitter, &logger);
                })
            }));
        }
        let offer = RTCSessionDescription::offer(remote_offer_sdp.to_string()).map_err(|e| format!("解析 Offer SDP 失败: {}", e))?;
        connection.set_remote_description(offer).await.map_err(|e| format!("设置远端描述失败: {}", e))?;
        let answer = connection.create_answer(None).await.map_err(|e| format!("创建 Answer 失败: {}", e))?;
        connection.set_local_description(answer.clone()).await.map_err(|e| format!("设置本地描述失败: {}", e))?;
        let sdp = answer.sdp.clone();
        *local_sdp.lock().await = Some(sdp.clone());
        emitter.emit(AgentEvent::new(EventType::Custom("webrtc_signaling".into()), EventSource::Plugin("webrtc".into()), serde_json::json!({"peer_id": peer_id, "signaling": {"type": "Answer", "payload": {"sdp": sdp}}})));
        if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("[Answer] Peer {} 已创建", peer_id)); }
        Ok(Self { peer_id: peer_id.to_string(), role: PeerRole::Answer, connection, data_channel: None, audio_sender: Some(audio_sender), emitter, logger, local_sdp })
    }

    async fn add_audio_track(connection: &Arc<RTCPeerConnection>) -> Result<AudioTrackSender, String> {
        let track = Arc::new(TrackLocalStaticSample::new(RTCRtpCodecCapability { mime_type: "audio/opus".to_string(), clock_rate: 48000, channels: 2, sdp_fmtp_line: "".to_string(), rtcp_feedback: vec![] }, "audio".to_string(), "bn-agent-audio".to_string()));
        connection.add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>).await.map_err(|e| format!("添加音频轨道失败: {}", e))?;
        Ok(AudioTrackSender { track })
    }

    pub async fn send_audio(&self, opus_data: &[u8]) -> Result<(), String> {
        match &self.audio_sender { Some(sender) => sender.write_samples(opus_data).await, None => Err("音频发送器未就绪".into()) }
    }

    fn setup_data_channel_handler(dc: &Arc<RTCDataChannel>, peer_id: &str, emitter: &Arc<dyn plugin_core::EventEmitter>, logger: &Option<Arc<dyn LogCallback>>) {
        let dc = Arc::clone(dc); let emitter = Arc::clone(emitter); let peer_id = peer_id.to_string(); let logger = logger.clone();
        dc.on_message(Box::new(move |msg| {
            let text = String::from_utf8_lossy(&msg.data).to_string();
            let peer_id = peer_id.clone(); let emitter = Arc::clone(&emitter); let logger = logger.clone();
            Box::pin(async move {
                if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("DataChannel [{}]: {}", peer_id, text)); }
                if let Ok(ctrl) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(t) = ctrl.get("type").and_then(|v| v.as_str()) {
                        if t == "chat" {
                            let content = ctrl.get("text").and_then(|v| v.as_str()).unwrap_or(&text);
                            emitter.emit(AgentEvent::new(EventType::UserMessage, EventSource::Plugin("webrtc".into()), serde_json::json!({"chat_id": peer_id, "user_name": format!("webrtc:{}", peer_id), "text": content, "source": "webrtc"})));
                            return;
                        }
                    }
                }
                emitter.emit(AgentEvent::new(EventType::UserMessage, EventSource::Plugin("webrtc".into()), serde_json::json!({"chat_id": peer_id, "user_name": format!("webrtc:{}", peer_id), "text": text, "source": "webrtc"})));
            })
        }));
    }

    fn setup_state_handler(connection: &Arc<RTCPeerConnection>, peer_id: &str, emitter: &Arc<dyn plugin_core::EventEmitter>, logger: &Option<Arc<dyn LogCallback>>) {
        let emitter = Arc::clone(emitter); let peer_id = peer_id.to_string(); let logger = logger.clone();
        connection.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
            let peer_id = peer_id.clone(); let emitter = Arc::clone(&emitter); let logger = logger.clone();
            Box::pin(async move {
                if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("Peer {} 状态: {:?}", peer_id, state)); }
                emitter.emit(AgentEvent::new(EventType::Custom("webrtc_state_change".into()), EventSource::Plugin("webrtc".into()), serde_json::json!({"peer_id": peer_id, "state": format!("{:?}", state)})));
            })
        }));
    }

    fn setup_ice_handler(connection: &Arc<RTCPeerConnection>, peer_id: &str, emitter: &Arc<dyn plugin_core::EventEmitter>) {
        let emitter = Arc::clone(emitter); let peer_id = peer_id.to_string();
        connection.on_ice_candidate(Box::new(move |candidate: Option<webrtc::ice_transport::ice_candidate::RTCIceCandidate>| {
            let peer_id = peer_id.clone(); let emitter = Arc::clone(&emitter);
            Box::pin(async move {
                if let Some(c) = candidate {
                    if let Ok(json) = c.to_json() {
                        emitter.emit(AgentEvent::new(EventType::Custom("webrtc_signaling".into()), EventSource::Plugin("webrtc".into()), serde_json::json!({"peer_id": peer_id, "signaling": {"type": "IceCandidate", "payload": {"candidate": json.candidate, "sdp_mid": json.sdp_mid, "sdp_mline_index": json.sdp_mline_index}}})));
                    }
                }
            })
        }));
    }

    fn setup_remote_track_handler(connection: &Arc<RTCPeerConnection>, peer_id: &str, emitter: &Arc<dyn plugin_core::EventEmitter>, logger: &Option<Arc<dyn LogCallback>>) {
        let emitter = Arc::clone(emitter); let peer_id = peer_id.to_string(); let logger = logger.clone();
        connection.on_track(Box::new(move |track: Arc<TrackRemote>, _receiver, _transceiver| {
            let emitter = Arc::clone(&emitter); let peer_id = peer_id.clone(); let logger = logger.clone();
            Box::pin(async move {
                let kind = track.kind().to_string();
                let codec = track.codec();
                let codec_mime = codec.capability.mime_type.clone();
                if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("远端轨道 [{}]: kind={}, codec={}", peer_id, kind, codec_mime)); }
                if kind == "audio" {
                    let emitter = Arc::clone(&emitter); let peer_id = peer_id.clone(); let logger = logger.clone();
                    tokio::spawn(async move {
                        loop {
                            match track.read_rtp().await {
                                Ok((rtp_packet, _)) => {
                                    if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Debug, "webrtc", &format!("音频 RTP [{}]: {} bytes", peer_id, rtp_packet.payload.len())); }
                                    emitter.emit(AgentEvent::new(EventType::Custom("webrtc_audio_received".into()), EventSource::Plugin("webrtc".into()), serde_json::json!({"peer_id": peer_id, "codec": codec_mime, "data": base64_encode(&rtp_packet.payload)})));
                                }
                                Err(e) => {
                                    if let Some(ref log) = logger { log.log(plugin_core::LogLevel::Debug, "webrtc", &format!("音频轨道结束 [{}]: {:?}", peer_id, e)); }
                                    break;
                                }
                            }
                        }
                    });
                }
            })
        }));
    }

    pub async fn local_sdp(&self) -> Option<String> { self.local_sdp.lock().await.clone() }

    pub async fn set_remote_sdp(&self, sdp: &str) -> Result<(), String> {
        let desc = RTCSessionDescription::answer(sdp.to_string()).map_err(|e| format!("解析 Answer SDP 失败: {}", e))?;
        self.connection.set_remote_description(desc).await.map_err(|e| format!("设置远端描述失败: {}", e))?;
        Ok(())
    }

    pub async fn add_ice_candidate(&self, candidate: &str, sdp_mid: Option<&str>, sdp_mline_index: Option<u16>) -> Result<(), String> {
        let init = RTCIceCandidateInit { candidate: candidate.to_string(), sdp_mid: sdp_mid.map(|s| s.to_string()), sdp_mline_index, username_fragment: None };
        self.connection.add_ice_candidate(init).await.map_err(|e| format!("添加 ICE candidate 失败: {}", e))?;
        Ok(())
    }

    pub async fn send_text(&self, text: &str) -> Result<(), String> {
        let dc = self.data_channel.as_ref().ok_or_else(|| "DataChannel 未就绪".to_string())?;
        dc.send_text(text.to_string()).await.map_err(|e| format!("发送失败: {}", e))?;
        Ok(())
    }

    pub async fn send_json(&self, msg: &serde_json::Value) -> Result<(), String> {
        self.send_text(&serde_json::to_string(msg).map_err(|e| format!("序列化失败: {}", e))?).await
    }

    pub async fn close(&self) {
        if let Some(ref log) = self.logger { log.log(plugin_core::LogLevel::Info, "webrtc", &format!("关闭 peer: {}", self.peer_id)); }
        let _ = self.connection.close().await;
    }
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 { result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char); } else { result.push('='); }
        if chunk.len() > 2 { result.push(CHARS[(triple & 0x3F) as usize] as char); } else { result.push('='); }
    }
    result
}
