//! PeerConnection management — actor-free port.
//!
//! Wraps webrtc-rs PeerConnection with DataChannel + audio track support.

use bytes::Bytes;
use plugin_interface::*;
use std::sync::Arc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::media::Sample;
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
        let sample = Sample {
            data: Bytes::copy_from_slice(data),
            duration: std::time::Duration::from_millis(20),
            ..Default::default()
        };
        self.track
            .write_sample(&sample)
            .await
            .map_err(|e| format!("write sample: {}", e))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerRole {
    Offer,
    Answer,
}

pub struct PeerHandle {
    peer_id: String,
    _role: PeerRole,
    connection: Arc<RTCPeerConnection>,
    data_channel: Option<Arc<RTCDataChannel>>,
    audio_sender: Option<AudioTrackSender>,
    local_sdp: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl PeerHandle {
    async fn build_api() -> Result<webrtc::api::API, String> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()
            .map_err(|e| format!("codecs: {}", e))?;
        let registry = interceptor::registry::Registry::new();
        let registry = register_default_interceptors(registry, &mut m)
            .map_err(|e| format!("interceptors: {}", e))?;
        Ok(APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build())
    }

    fn default_config() -> RTCConfiguration {
        RTCConfiguration {
            ice_servers: vec![],
            ..Default::default()
        }
    }

    pub async fn new_offer(peer_id: &str, eb: Addr<EventBus>) -> Result<Self, String> {
        let api = Self::build_api().await?;
        let connection = Arc::new(
            api.new_peer_connection(Self::default_config())
                .await
                .map_err(|e| format!("create PC: {}", e))?,
        );
        let local_sdp = Arc::new(tokio::sync::Mutex::new(None));

        let dc = connection
            .create_data_channel("data", None)
            .await
            .map_err(|e| format!("create DC: {}", e))?;
        let audio_sender = Self::add_audio_track(&connection).await?;

        Self::setup_dc_handler(&dc, peer_id, &eb);
        Self::setup_state_handler(&connection, peer_id, &eb);
        Self::setup_ice_handler(&connection, peer_id, &eb);
        Self::setup_track_handler(&connection, peer_id, &eb);

        let offer = connection
            .create_offer(None)
            .await
            .map_err(|e| format!("create offer: {}", e))?;
        connection
            .set_local_description(offer.clone())
            .await
            .map_err(|e| format!("set local desc: {}", e))?;

        let sdp = offer.sdp.clone();
        *local_sdp.lock().await = Some(sdp.clone());

        eb.do_send(Event::new(
            "webrtc_signaling",
            serde_json::json!({"peer_id": peer_id, "signaling": {
                "type": "Offer", "payload": {"sdp": sdp}
            }}),
            "webrtc-plugin",
        ));
        log::info!("[webrtc] Offer peer {} created", peer_id);

        Ok(Self {
            peer_id: peer_id.to_string(),
            _role: PeerRole::Offer,
            connection,
            data_channel: Some(dc),
            audio_sender: Some(audio_sender),
            local_sdp,
        })
    }

    pub async fn new_answer(
        peer_id: &str,
        remote_offer_sdp: &str,
        eb: Addr<EventBus>,
    ) -> Result<Self, String> {
        let api = Self::build_api().await?;
        let connection = Arc::new(
            api.new_peer_connection(Self::default_config())
                .await
                .map_err(|e| format!("create PC: {}", e))?,
        );
        let local_sdp = Arc::new(tokio::sync::Mutex::new(None));

        let audio_sender = Self::add_audio_track(&connection).await?;

        Self::setup_state_handler(&connection, peer_id, &eb);
        Self::setup_ice_handler(&connection, peer_id, &eb);
        Self::setup_track_handler(&connection, peer_id, &eb);

        {
            let eb = eb.clone();
            let pid = peer_id.to_string();
            connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let eb = eb.clone();
                let pid = pid.clone();
                Box::pin(async move {
                    log::info!("[webrtc] Answer remote DC: {}", dc.label());
                    PeerHandle::setup_dc_handler(&dc, &pid, &eb);
                })
            }));
        }

        let offer = RTCSessionDescription::offer(remote_offer_sdp.to_string())
            .map_err(|e| format!("parse offer: {}", e))?;
        connection
            .set_remote_description(offer)
            .await
            .map_err(|e| format!("set remote: {}", e))?;

        let answer = connection
            .create_answer(None)
            .await
            .map_err(|e| format!("create answer: {}", e))?;
        connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| format!("set local desc: {}", e))?;

        let sdp = answer.sdp.clone();
        *local_sdp.lock().await = Some(sdp.clone());

        eb.do_send(Event::new(
            "webrtc_signaling",
            serde_json::json!({"peer_id": peer_id, "signaling": {
                "type": "Answer", "payload": {"sdp": sdp}
            }}),
            "webrtc-plugin",
        ));
        log::info!("[webrtc] Answer peer {} created", peer_id);

        Ok(Self {
            peer_id: peer_id.to_string(),
            _role: PeerRole::Answer,
            connection,
            data_channel: None,
            audio_sender: Some(audio_sender),
            local_sdp,
        })
    }

    async fn add_audio_track(
        connection: &Arc<RTCPeerConnection>,
    ) -> Result<AudioTrackSender, String> {
        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "audio/opus".to_string(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: vec![],
            },
            "audio".to_string(),
            "bn-agent-audio".to_string(),
        ));
        connection
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| format!("add track: {}", e))?;
        Ok(AudioTrackSender { track })
    }

    pub async fn send_audio(&self, opus_data: &[u8]) -> Result<(), String> {
        match &self.audio_sender {
            Some(s) => s.write_samples(opus_data).await,
            None => Err("audio sender not ready".into()),
        }
    }

    fn setup_dc_handler(dc: &Arc<RTCDataChannel>, peer_id: &str, eb: &Addr<EventBus>) {
        let eb = eb.clone();
        let pid = peer_id.to_string();
        dc.on_message(Box::new(move |msg| {
            let text = String::from_utf8_lossy(&msg.data).to_string();
            let eb = eb.clone();
            let pid = pid.clone();
            Box::pin(async move {
                log::info!("[webrtc] DC [{}]: {}", pid, text);
                // Check for JSON chat message
                if let Ok(ctrl) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(t) = ctrl.get("type").and_then(|v| v.as_str()) {
                        if t == "chat" {
                            let content =
                                ctrl.get("text").and_then(|v| v.as_str()).unwrap_or(&text);
                            eb.do_send(Event::new(
                                "user.message",
                                serde_json::json!({
                                    "chat_id": pid, "user_name": format!("webrtc:{}", pid),
                                    "text": content, "source": "webrtc",
                                }),
                                "webrtc-plugin",
                            ));
                            return;
                        }
                    }
                }
                eb.do_send(Event::new(
                    "user.message",
                    serde_json::json!({
                        "chat_id": pid, "user_name": format!("webrtc:{}", pid),
                        "text": text, "source": "webrtc",
                    }),
                    "webrtc-plugin",
                ));
            })
        }));
    }

    fn setup_state_handler(
        connection: &Arc<RTCPeerConnection>,
        peer_id: &str,
        eb: &Addr<EventBus>,
    ) {
        let eb = eb.clone();
        let pid = peer_id.to_string();
        connection.on_peer_connection_state_change(Box::new(
            move |state: RTCPeerConnectionState| {
                let eb = eb.clone();
                let pid = pid.clone();
                Box::pin(async move {
                    log::info!("[webrtc] Peer {} state: {:?}", pid, state);
                    eb.do_send(Event::new(
                        "webrtc_state_change",
                        serde_json::json!({"peer_id": pid, "state": format!("{:?}", state)}),
                        "webrtc-plugin",
                    ));
                })
            },
        ));
    }

    fn setup_ice_handler(connection: &Arc<RTCPeerConnection>, peer_id: &str, eb: &Addr<EventBus>) {
        let eb = eb.clone();
        let pid = peer_id.to_string();
        connection.on_ice_candidate(Box::new(
            move |candidate: Option<webrtc::ice_transport::ice_candidate::RTCIceCandidate>| {
                let eb = eb.clone();
                let pid = pid.clone();
                Box::pin(async move {
                    if let Some(c) = candidate {
                        if let Ok(json) = c.to_json() {
                            eb.do_send(Event::new(
                                "webrtc_signaling",
                                serde_json::json!({"peer_id": pid, "signaling": {
                                    "type": "IceCandidate",
                                    "payload": {
                                        "candidate": json.candidate,
                                        "sdp_mid": json.sdp_mid,
                                        "sdp_mline_index": json.sdp_mline_index,
                                    }
                                }}),
                                "webrtc-plugin",
                            ));
                        }
                    }
                })
            },
        ));
    }

    fn setup_track_handler(
        connection: &Arc<RTCPeerConnection>,
        peer_id: &str,
        eb: &Addr<EventBus>,
    ) {
        let eb = eb.clone();
        let pid = peer_id.to_string();
        connection.on_track(Box::new(
            move |track: Arc<TrackRemote>, _receiver, _transceiver| {
                let eb = eb.clone();
                let pid = pid.clone();
                Box::pin(async move {
                    let kind = track.kind().to_string();
                    let codec = track.codec();
                    let mime = codec.capability.mime_type.clone();
                    log::info!(
                        "[webrtc] remote track [{}]: kind={}, codec={}",
                        pid,
                        kind,
                        mime
                    );

                    if kind == "audio" {
                        tokio::spawn(async move {
                            loop {
                                match track.read_rtp().await {
                                    Ok((pkt, _)) => {
                                        log::debug!(
                                            "[webrtc] audio RTP [{}]: {} bytes",
                                            pid,
                                            pkt.payload.len()
                                        );
                                        eb.do_send(Event::new(
                                            "webrtc_audio_received",
                                            serde_json::json!({
                                                "peer_id": pid,
                                                "codec": mime,
                                                "data": crate::base64_encode(&pkt.payload),
                                            }),
                                            "webrtc-plugin",
                                        ));
                                    }
                                    Err(e) => {
                                        log::debug!(
                                            "[webrtc] audio track ended [{}]: {:?}",
                                            pid,
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                        });
                    }
                })
            },
        ));
    }

    pub async fn local_sdp(&self) -> Option<String> {
        self.local_sdp.lock().await.clone()
    }

    pub async fn set_remote_sdp(&self, sdp: &str) -> Result<(), String> {
        let desc = RTCSessionDescription::answer(sdp.to_string())
            .map_err(|e| format!("parse answer: {}", e))?;
        self.connection
            .set_remote_description(desc)
            .await
            .map_err(|e| format!("set remote: {}", e))?;
        Ok(())
    }

    pub async fn add_ice_candidate(
        &self,
        candidate: &str,
        sdp_mid: Option<&str>,
        sdp_mline_index: Option<u16>,
    ) -> Result<(), String> {
        let init = RTCIceCandidateInit {
            candidate: candidate.to_string(),
            sdp_mid: sdp_mid.map(|s| s.to_string()),
            sdp_mline_index,
            username_fragment: None,
        };
        self.connection
            .add_ice_candidate(init)
            .await
            .map_err(|e| format!("add ICE: {}", e))?;
        Ok(())
    }

    pub async fn send_text(&self, text: &str) -> Result<(), String> {
        let dc = self
            .data_channel
            .as_ref()
            .ok_or_else(|| "DataChannel not ready".to_string())?;
        dc.send_text(text.to_string())
            .await
            .map_err(|e| format!("send: {}", e))?;
        Ok(())
    }

    pub async fn send_json(&self, msg: &serde_json::Value) -> Result<(), String> {
        let text = serde_json::to_string(msg).map_err(|e| format!("serialize: {}", e))?;
        self.send_text(&text).await
    }

    pub async fn close(&self) {
        log::info!("[webrtc] closing peer: {}", self.peer_id);
        let _ = self.connection.close().await;
    }
}
