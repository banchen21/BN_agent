//! 通话内的「悦悦」：以 agent 身份进房，收用户音频 → ASR → 主后端 /api/chat → TTS → 发回。
//!
//! 与文字聊天共用同一个 agent（同人设、同记忆、同会话），只是换了一条输入/输出通道。
//! 视觉：订阅对方摄像头，缓存最新一帧；用户问「看到我了吗 / 我穿的什么」时，
//! 把帧转 JPEG 调主后端 `image_understand`，再把描述交给 `/api/chat` 口语化回答。

use crate::config::CallConfig;
use crate::jpeg;
use crate::voice::{downmix, pcm16_to_wav, resample_mono, wav_to_pcm16, Vad};
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio_stream::StreamExt;

pub const AGENT_IDENTITY_PREFIX: &str = "agent-yueyue-";
pub const TTS_TRACK_NAME: &str = "yueyue";
const SUBSCRIBE_RATE: u32 = 16_000;
const VAD_SILENCE_MS: u32 = 600;
/// 单句最短 300ms（更短视为噪声丢弃）。
const MIN_UTTERANCE_MS: u32 = 300;
/// 单帧峰值低于该值视为静音（用于提醒对方没开麦）。
const QUIET_PEAK: u16 = 30;
/// 只缓存最近 60 秒的视频帧。
const VIDEO_FRAME_TTL: Duration = Duration::from_secs(60);
const VIDEO_MAX_WIDTH: u32 = 480;
const VISION_TOOL: &str = "image_understand";
const VISION_PROMPT: &str = "这是通话中对方摄像头拍到的一帧画面。请客观描述画面内容，\
     特别是：对方的外貌特征、穿的衣服颜色和样式、发型、正在做什么、背景里有什么。\
     用简体中文，不要寒暄，150 字以内。";

/// 用户在用视觉的问题：「你能看到我吗」「我穿的什么」「画面里有什么」……
pub fn is_vision_question(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "看到",
        "看见",
        "看得见",
        "摄像头",
        "镜头",
        "长什么样",
        "长得",
        "穿的",
        "穿什么",
        "衣服",
        "发型",
        "头发",
        "样子",
        "画面",
        "屏幕里",
        "手里拿",
        "拿着",
        "背景",
        "房间里",
        "我在哪",
        "我的样子",
        "这身",
        "戴着",
        "认得出",
        "看得出",
    ];
    MARKERS.iter().any(|m| text.contains(m))
}

// ── 主后端 / ASR / TTS 客户端 ───────────────────────────────────────────────

#[derive(Clone)]
pub struct AgentBackend {
    pub client: reqwest::Client,
    pub asr_url: String,
    pub tts_url: String,
    pub chat_url: String,
    pub append_url: String,
    pub tools_url: String,
    pub api_key: Option<String>,
}

impl AgentBackend {
    fn new_with(
        config: &CallConfig,
        chat_url: String,
        append_url: String,
        tools_url: String,
    ) -> Self {
        let client = reqwest::Client::builder()
            // 本机系统代理会拦截 127.0.0.1 的请求，显式不走代理。
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            asr_url: config.asr_url.clone(),
            tts_url: config.tts_url.clone(),
            chat_url,
            append_url,
            tools_url,
            api_key: Some(config.backend_api_key.clone()).filter(|k| !k.is_empty()),
        }
    }

    /// 从插件配置派生：主后端接口地址在 `join_agent` 里拼好传进来。
    pub fn from_config(
        config: &CallConfig,
        chat_url: String,
        append_url: String,
        tools_url: String,
    ) -> Self {
        Self::new_with(config, chat_url, append_url, tools_url)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }

    pub async fn transcribe(&self, wav: Vec<u8>) -> Result<String, String> {
        const BOUNDARY: &str = "bncallvad7f3";
        let mut body = Vec::with_capacity(wav.len() + 256);
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"voice.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&wav);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        let resp = self
            .client
            .post(&self.asr_url)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| format!("ASR 请求失败：{e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("ASR 响应读取失败：{e}"))?;
        if !status.is_success() {
            return Err(format!("ASR HTTP {status}: {text}"));
        }
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("ASR 响应解析失败：{e}"))?;
        Ok(value
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string())
    }

    /// 返回 `(PCM16, 采样率, 声道数)`。
    pub async fn synthesize(&self, text: &str) -> Result<(Vec<i16>, u32, u32), String> {
        let resp = self
            .client
            .post(&self.tts_url)
            .json(&serde_json::json!({ "text": text }))
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("TTS 请求失败：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("TTS HTTP {status}: {body}"));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("TTS 响应读取失败：{e}"))?;
        wav_to_pcm16(&bytes)
    }

    pub async fn chat(
        &self,
        message: &str,
        peer_id: &str,
        user_name: &str,
    ) -> Result<String, String> {
        let resp = self
            .auth(self.client.post(&self.chat_url))
            .json(&serde_json::json!({
                "message": message,
                "peer_id": peer_id,
                "user_name": user_name,
                "source": "android",
            }))
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("聊天请求失败：{e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("聊天 HTTP {status}: {text}"));
        }
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("聊天响应解析失败：{e}"))?;
        Ok(value
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string())
    }

    /// 通话结束后把这一通的话写回会话上下文（不产生消息气泡）。
    pub async fn append(&self, peer_id: &str, messages: &[(String, String)]) -> Result<(), String> {
        let payload: Vec<serde_json::Value> = messages
            .iter()
            .map(|(role, content)| serde_json::json!({ "role": role, "content": content }))
            .collect();
        let resp = self
            .auth(self.client.post(&self.append_url))
            .json(&serde_json::json!({ "peer_id": peer_id, "messages": payload }))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("append 请求失败：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("append HTTP {status}: {body}"));
        }
        Ok(())
    }

    /// 通过主后端的 `image_understand` 工具理解一张 JPEG。
    pub async fn understand_image(&self, jpeg_b64: &str, prompt: &str) -> Result<String, String> {
        let resp = self
            .auth(self.client.post(&self.tools_url))
            .json(&serde_json::json!({
                "tool_name": VISION_TOOL,
                "arguments": {
                    "image_base64": jpeg_b64,
                    "mime_type": "image/jpeg",
                    "prompt": prompt,
                },
            }))
            .timeout(Duration::from_secs(90))
            .send()
            .await
            .map_err(|e| format!("视觉请求失败：{e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("视觉 HTTP {status}: {text}"));
        }
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("视觉响应解析失败：{e}"))?;
        let success = value
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !success {
            let err = value
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("视觉工具返回失败");
            return Err(err.to_string());
        }
        Ok(value
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string())
    }
}

// ── 视觉缓存 ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct FrameSnapshot {
    rgb: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    at: Instant,
}

#[derive(Default)]
struct VisionCache {
    latest: Mutex<Option<FrameSnapshot>>,
}

impl VisionCache {
    async fn set(&self, frame: Option<FrameSnapshot>) {
        *self.latest.lock().await = frame;
    }

    /// 取最新帧（必要时缩放到 ≤ 480 宽），编码成 JPEG base64。
    async fn latest_jpeg(&self) -> Option<String> {
        let snap = self.latest.lock().await.clone()?;
        if snap.at.elapsed() > VIDEO_FRAME_TTL {
            return None;
        }
        let (rgb, w, h) = if snap.width > VIDEO_MAX_WIDTH {
            let scale = VIDEO_MAX_WIDTH as f32 / snap.width as f32;
            let nw = VIDEO_MAX_WIDTH;
            let nh = ((snap.height as f32) * scale).max(1.0) as u32;
            (
                resize_rgb(&snap.rgb, snap.width, snap.height, nw, nh),
                nw,
                nh,
            )
        } else {
            ((*snap.rgb).clone(), snap.width, snap.height)
        };
        let image = jpeg::encode_rgb(w, h, &rgb, 70);
        use base64::Engine;
        Some(base64::engine::general_purpose::STANDARD.encode(image))
    }
}

fn resize_rgb(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 3) as usize];
    for y in 0..dh {
        let sy = (y as u64 * sh as u64 / dh as u64).min(sh as u64 - 1) as usize;
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64).min(sw as u64 - 1) as usize;
            let si = (sy * sw as usize + sx) * 3;
            let di = ((y * dw + x) * 3) as usize;
            out[di..di + 3].copy_from_slice(&src[si..si + 3]);
        }
    }
    out
}

// ── 一次通话 ────────────────────────────────────────────────────────────────

/// 由插件注入的日志函数（插件日志走宿主 EventBus）。
#[derive(Clone)]
pub struct LogFn(pub Arc<dyn Fn(bool, String) + Send + Sync>);

impl LogFn {
    pub fn info(&self, msg: impl Into<String>) {
        (self.0)(false, msg.into());
    }
    pub fn warn(&self, msg: impl Into<String>) {
        (self.0)(true, msg.into());
    }
}

pub struct CallSession {
    pub room: String,
    pub peer_id: String,
    pub user_name: String,
    backend: AgentBackend,
    log: LogFn,
    /// 每次用户插话自增；回复生成完发现代际变了就丢弃这次回复。
    generation: Arc<AtomicU64>,
    ended: AtomicBool,
    /// 本通对话（role, content），挂断后回写主后端。
    history: Mutex<Vec<(String, String)>>,
    /// `/api/chat` 已落库的 (用户原文, 回复原文)。
    synced: Mutex<Vec<(String, String)>>,
    vision: VisionCache,
    /// 悦悦正在说话（1 个 permit 被占）。
    speaking: Arc<Semaphore>,
}

impl CallSession {
    pub fn new(
        room: String,
        peer_id: String,
        user_name: String,
        backend: AgentBackend,
        log: LogFn,
    ) -> Self {
        Self {
            room,
            peer_id,
            user_name,
            backend,
            log,
            generation: Arc::new(AtomicU64::new(0)),
            ended: AtomicBool::new(false),
            history: Mutex::new(Vec::new()),
            synced: Mutex::new(Vec::new()),
            vision: VisionCache::default(),
            speaking: Arc::new(Semaphore::new(1)),
        }
    }

    fn info(&self, msg: impl AsRef<str>) {
        self.log.info(format!("[{}] {}", self.room, msg.as_ref()));
    }

    fn warn(&self, msg: impl AsRef<str>) {
        self.log.warn(format!("[{}] {}", self.room, msg.as_ref()));
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn is_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// 挂断时在任何后续处理之前调用，避免已结束的通话继续产生日志/回复。
    pub fn mark_ended(&self) {
        self.ended.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    async fn record(&self, role: &str, content: &str) {
        self.history
            .lock()
            .await
            .push((role.to_string(), content.to_string()));
    }

    /// 标记 `/api/chat` 已经落库的那一对 user/assistant 消息（回写时跳过）。
    async fn mark_synced(&self, user: &str, assistant: &str) {
        self.synced
            .lock()
            .await
            .push((user.to_string(), assistant.to_string()));
    }

    /// 把还没被 `/api/chat` 落库的对话整理成 append 列表（按出现顺序）。
    async fn unsynced_messages(&self) -> Vec<(String, String)> {
        let history = self.history.lock().await;
        let mut synced = self.synced.lock().await.clone();
        let mut out = Vec::new();
        let mut i = 0;
        while i < history.len() {
            let (role, content) = &history[i];
            if role == "user" {
                if let Some(pos) = synced
                    .iter()
                    .position(|(u, _)| u == content && history.get(i + 1).is_some())
                {
                    let (_, reply) = synced.remove(pos);
                    // 主后端落的是未清理标签的原文，这里比对清理后的文本。
                    if history.get(i + 1).map(|m| m.1 == reply).unwrap_or(false) {
                        i += 2;
                        continue;
                    }
                }
            }
            out.push((role.clone(), content.clone()));
            i += 1;
        }
        out
    }

    /// 把这一通的对话写回主后端会话上下文。
    pub async fn append_history(&self) {
        // 主后端 /api/chat 已经把这通对话写进会话上下文，这里只补它没落库的部分
        // （例如 /api/chat 失败但回复已播报），否则会在历史里出现两份。
        let history = self.unsynced_messages().await;
        if history.is_empty() {
            return;
        }
        match self.backend.append(&self.peer_id, &history).await {
            Ok(()) => self.info(format!("通话记录已回写会话上下文（{} 条）", history.len())),
            Err(e) => self.warn(format!("通话记录回写失败：{e}")),
        }
    }
}

// ── 进房 + 事件循环 ─────────────────────────────────────────────────────────

enum Turn {
    Utterance(String),
    Failed(String),
}

pub struct AgentRun {
    pub login_url: String,
    pub token: String,
    pub session: Arc<CallSession>,
}

impl AgentRun {
    /// 进房并跑事件循环，直到对方离房/房间断开（随后回写会话上下文）。
    /// `ready` 在 `Room::connect` 有结果时收到一次通知（成功或失败）。
    pub async fn drive(
        self,
        ready: Option<std::sync::mpsc::Sender<Result<(), String>>>,
    ) -> Result<(), String> {
        let notify = |ok: &Result<(), String>| {
            if let Some(tx) = ready.as_ref() {
                let _ = tx.send(ok.clone());
            }
        };
        // 多房间各占一条 PeerConnection，订阅用自动订阅即可。
        let options = RoomOptions::default();
        let (room, mut events) = match Room::connect(&self.login_url, &self.token, options).await {
            Ok(v) => {
                notify(&Ok(()));
                v
            }
            Err(e) => {
                let msg = e.to_string();
                notify(&Err(msg.clone()));
                self.session.warn(format!("悦悦进房失败：{msg}"));
                self.session.mark_ended();
                return Err(msg);
            }
        };
        self.session.info("悦悦已以 agent 身份进房");

        let tts_tx = match publish_tts_track(&room).await {
            Ok(tx) => tx,
            Err(e) => {
                self.session.warn(format!("发布语音轨失败：{e}"));
                let _ = room.close().await;
                self.session.mark_ended();
                return Ok(());
            }
        };

        let (turn_tx, turn_rx) = mpsc::unbounded_channel::<Turn>();
        let turn_driver = tokio::spawn(drive_turns(turn_rx, self.session.clone(), tts_tx));
        let mut audio_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        let mut video_task: Option<tokio::task::JoinHandle<()>> = None;

        while let Some(event) = events.recv().await {
            match event {
                RoomEvent::TrackSubscribed {
                    track, participant, ..
                } => match track {
                    RemoteTrack::Audio(audio) => {
                        self.session.info(format!(
                            "订阅到 {} 的音频轨 {}",
                            participant.identity().to_string(),
                            audio.name()
                        ));
                        audio_tasks.push(spawn_audio_pipeline(
                            audio,
                            self.session.clone(),
                            turn_tx.clone(),
                        ));
                    }
                    RemoteTrack::Video(video) => {
                        self.session.info(format!(
                            "订阅到 {} 的视频轨 {}",
                            participant.identity().to_string(),
                            video.name()
                        ));
                        video_task = Some(spawn_video_pipeline(video, self.session.clone()));
                    }
                },
                RoomEvent::ParticipantDisconnected(participant) => {
                    self.session
                        .info(format!("对方离房：{}", participant.identity().to_string()));
                    break;
                }
                RoomEvent::Disconnected { reason } => {
                    self.session.info(format!("房间断开：{reason:?}"));
                    break;
                }
                _ => {}
            }
        }
        // 收尾：停采音视频 → 处理完残留句子 → 关房 → 回写会话上下文。
        for task in audio_tasks {
            task.abort();
        }
        if let Some(task) = video_task {
            task.abort();
        }
        drop(turn_tx);
        let _ = tokio::time::timeout(Duration::from_secs(30), turn_driver).await;
        let _ = room.close().await;
        self.session.mark_ended();
        self.session.append_history().await;
        self.session.info("本通通话已收尾");
        Ok(())
    }
}

/// agent identity：`agent-yueyue-<room>`。
pub fn agent_identity(room: &str) -> String {
    format!("{AGENT_IDENTITY_PREFIX}{room}")
}

async fn publish_tts_track(room: &Room) -> Result<mpsc::Sender<Vec<i16>>, String> {
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 2000);
    let track =
        LocalAudioTrack::create_audio_track(TTS_TRACK_NAME, RtcAudioSource::Native(source.clone()));
    let options = TrackPublishOptions {
        source: TrackSource::Microphone,
        ..Default::default()
    };
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), options)
        .await
        .map_err(|e| e.to_string())?;

    let (tx, mut rx) = mpsc::channel::<Vec<i16>>(200);
    tokio::spawn(async move {
        while let Some(pcm) = rx.recv().await {
            for chunk in pcm.chunks(480) {
                let frame = AudioFrame {
                    data: std::borrow::Cow::Borrowed(chunk),
                    sample_rate: 48_000,
                    num_channels: 1,
                    samples_per_channel: chunk.len() as u32,
                };
                if source.capture_frame(&frame).await.is_err() {
                    break;
                }
            }
        }
    });
    Ok(tx)
}

fn spawn_audio_pipeline(
    track: RemoteAudioTrack,
    session: Arc<CallSession>,
    turns: mpsc::UnboundedSender<Turn>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = NativeAudioStream::new(track.rtc_track(), SUBSCRIBE_RATE as i32, 1);
        let mut vad = Vad::new(SUBSCRIBE_RATE).with_silence_ms(VAD_SILENCE_MS);
        let mut last_peak_warn = Instant::now();
        // 自接入以来是否听到过真实音量（阈值远低于 VAD 的 380，只用来判断链路是否通）。
        let mut heard_sound = false;
        // 两句之间的最长静音与最近一次收句时长，收尾时打一行，便于排查「话被吞了」。
        let mut longest_gap = 0.0f32;
        let mut quiet_secs = 0.0f32;
        let mut last_sentence = "无".to_string();
        while let Some(frame) = stream.next().await {
            if session.is_ended() {
                return;
            }
            let pcm: Vec<i16> = frame.data.iter().copied().collect();
            let frame_secs = pcm.len() as f32 / SUBSCRIBE_RATE as f32;
            quiet_secs += frame_secs;
            longest_gap = longest_gap.max(quiet_secs);
            // 音量自检：连续 3 秒几乎没声音就提醒一次（多半是对方麦克风静音 / 音频权限没给）。
            if !heard_sound {
                if pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) >= QUIET_PEAK {
                    heard_sound = true;
                } else if last_peak_warn.elapsed() >= Duration::from_secs(3) {
                    last_peak_warn = Instant::now();
                    session.warn("收到的音频接近静音（检查对方麦克风 / 音频权限）");
                }
            }
            for sentence in vad.push(&pcm) {
                last_sentence = format!("{:.1}s", sentence.len() as f32 / SUBSCRIBE_RATE as f32);
                quiet_secs = 0.0;
                handle_sentence(&session, sentence, &turns).await;
            }
        }
        session.info(format!(
            "音频管线结束：最长静音间隔 {longest_gap:.1}s，最后一句 {last_sentence}"
        ));
        if let Some(sentence) = vad.flush() {
            if !session.is_ended() {
                handle_sentence(&session, sentence, &turns).await;
            }
        }
    })
}

async fn handle_sentence(
    session: &Arc<CallSession>,
    pcm: Vec<i16>,
    turns: &mpsc::UnboundedSender<Turn>,
) {
    if pcm.len() as u32 * 1000 / SUBSCRIBE_RATE < MIN_UTTERANCE_MS {
        return;
    }
    // 悦悦在说话：丢弃这一句并让当前回复收尾（简单打断）。
    let permit = match session.speaking.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            session.generation.fetch_add(1, Ordering::SeqCst);
            session.info("用户插话：跳过本句并让悦悦收尾");
            return;
        }
    };
    let seconds = pcm.len() as f32 / SUBSCRIBE_RATE as f32;
    let wav = pcm16_to_wav(&pcm, SUBSCRIBE_RATE);
    let result = session.backend.transcribe(wav).await;
    drop(permit);
    match result {
        Ok(text) if !text.is_empty() => {
            session.info(format!("用户说：{text}"));
            let _ = turns.send(Turn::Utterance(text));
        }
        // 空结果也要留痕，否则现场看起来就是「这句凭空消失了」。
        Ok(_) => session.info(format!("ASR 没有识别出文字（{seconds:.1}s 音频）")),
        Err(e) => {
            session.warn(format!("ASR 失败（{seconds:.1}s）：{e}"));
            let _ = turns.send(Turn::Failed(e));
        }
    }
}

fn spawn_video_pipeline(
    track: RemoteVideoTrack,
    session: Arc<CallSession>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = NativeVideoStream::new(track.rtc_track());
        let mut last: Option<Instant> = None;
        let mut logged = false;
        while let Some(frame) = stream.next().await {
            if session.is_ended() {
                return;
            }
            if let Some(t) = last {
                if t.elapsed() < Duration::from_millis(500) {
                    continue;
                }
            }
            last = Some(Instant::now());
            let (w, h) = (
                VideoBuffer::width(&*frame.buffer),
                VideoBuffer::height(&*frame.buffer),
            );
            if w == 0 || h == 0 {
                continue;
            }
            let mut argb = vec![0u8; (w * h * 4) as usize];
            // 取 I420 平面自行转 RGB（公开 API 里没有跨平台 to_argb）。
            let i420 = frame.buffer.to_i420();
            let (y_plane, u_plane, v_plane) = i420.data();
            let (sy, su, sv) = i420.strides();
            for row in 0..h as usize {
                for col in 0..w as usize {
                    let y = y_plane[row * sy as usize + col] as f32;
                    let u = u_plane[(row / 2) * su as usize + col / 2] as f32 - 128.0;
                    let v = v_plane[(row / 2) * sv as usize + col / 2] as f32 - 128.0;
                    let i = (row * w as usize + col) * 4;
                    argb[i] = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
                    argb[i + 1] = (y - 0.344_136 * u - 0.714_136 * v).clamp(0.0, 255.0) as u8;
                    argb[i + 2] = (y + 1.772 * u).clamp(0.0, 255.0) as u8;
                    argb[i + 3] = 255;
                }
            }
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for px in argb.chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
            }
            session
                .vision
                .set(Some(FrameSnapshot {
                    rgb: Arc::new(rgb),
                    width: w,
                    height: h,
                    at: Instant::now(),
                }))
                .await;
            if !logged {
                logged = true;
                session.info(format!("已能取到视频帧：{w}x{h}"));
            }
        }
    })
}

/// 处理一句识别文本：可选视觉理解 → 主后端聊天 → TTS → 推流。
async fn handle_sentence_text(
    session: &Arc<CallSession>,
    text: String,
    tts: &mpsc::Sender<Vec<i16>>,
) {
    let generation = session.generation.load(Ordering::SeqCst);
    session.record("user", &text).await;

    let mut message = text.clone();
    if is_vision_question(&text) {
        match session.vision.latest_jpeg().await {
            Some(jpeg_b64) => {
                match session
                    .backend
                    .understand_image(&jpeg_b64, VISION_PROMPT)
                    .await
                {
                    Ok(desc) if !desc.is_empty() => {
                        session.info(format!(
                            "视觉理解（{} 字）：{}",
                            desc.chars().count(),
                            truncate_chars(&desc, 80)
                        ));
                        message = format!(
                            "{text}\n\n【你通过摄像头看到的画面（用户知道你在看）】{desc}\n\
                             请结合这段话，用口语自然地回答用户的问题，不要说「根据描述」之类的话。"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        session.warn(format!("视觉理解失败：{e}"));
                        message = format!("{text}\n\n【摄像头画面暂时看不清】");
                    }
                }
            }
            None => {
                session.info("用户问到了画面，但当前没有可用的视频帧");
                message = format!("{text}\n\n【现在摄像头没有画面】");
            }
        }
    }

    let reply = match session
        .backend
        .chat(&message, &session.peer_id, &session.user_name)
        .await
    {
        Ok(reply) if !reply.is_empty() => reply,
        Ok(_) => {
            session.warn("主后端返回空回复");
            return;
        }
        Err(e) => {
            session.warn(format!("聊天失败：{e}"));
            return;
        }
    };
    session.mark_synced(&text, &reply).await;
    let spoken = strip_role_tags(&reply);
    session.info(format!("悦悦回复：{spoken}"));
    if spoken.is_empty() {
        session.warn("回复只有角色标签，忽略");
        return;
    }
    session.record("assistant", &spoken).await;
    if session.is_ended() || session.generation.load(Ordering::SeqCst) != generation {
        session.info("回复生成期间通话结束或用户插话，跳过播报");
        return;
    }
    speak(session, &spoken, tts, generation).await;
}

async fn speak(
    session: &Arc<CallSession>,
    reply: &str,
    tts: &mpsc::Sender<Vec<i16>>,
    generation: u64,
) {
    let (pcm, rate, channels) = match session.backend.synthesize(reply).await {
        Ok(v) => v,
        Err(e) => {
            session.warn(format!("TTS 失败：{e}"));
            return;
        }
    };
    let mono = downmix(&pcm, channels);
    let mut pcm48 = resample_mono(&mono, rate.max(1), 48_000);
    if pcm48.is_empty() {
        return;
    }
    let fade = 4800.min(pcm48.len());
    for (i, sample) in pcm48.iter_mut().take(fade).enumerate() {
        *sample = ((*sample as f32) * (i as f32 / fade as f32)) as i16;
    }
    // 尾部补 300ms 静音让对端缓冲播完。
    pcm48.extend(std::iter::repeat(0i16).take(14_400));

    let _permit = session.speaking.clone().acquire_owned().await.ok();
    for chunk in pcm48.chunks(480) {
        if session.generation.load(Ordering::SeqCst) != generation || session.is_ended() {
            session.info("悦悦被打断，停止播报");
            return;
        }
        // 不需要人为限速：NativeAudioSource 自带 queue_size_ms 缓冲，
        // 之前的「每 5 帧睡 50ms」会把喂音速度降到约 19k 样本/秒（<24k），
        // 缓冲一空就变成断续/沙哑。
        if tts.send(chunk.to_vec()).await.is_err() {
            return;
        }
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 同一句话的重合收句判定时间窗：VAD 在「超长截断」和「静音收句」两个时机可能把
/// 尾巴同几句一起交出来（第二句尤其容易这样），ASR 结果完全相同时按重复处理。
const DUPLICATE_TURN_WINDOW: Duration = Duration::from_secs(10);

/// 由事件循环串行处理句子（避免多句并发请求打乱对话顺序）。
async fn drive_turns(
    mut turns: mpsc::UnboundedReceiver<Turn>,
    session: Arc<CallSession>,
    tts: mpsc::Sender<Vec<i16>>,
) {
    let mut last: Option<(String, Instant)> = None;
    while let Some(turn) = turns.recv().await {
        if session.is_ended() {
            continue;
        }
        if let Turn::Utterance(text) = &turn {
            if let Some((prev, at)) = &last {
                if prev == text && at.elapsed() < DUPLICATE_TURN_WINDOW {
                    session.info("同一句被重复识别，跳过本次");
                    continue;
                }
            }
            last = Some((text.clone(), Instant::now()));
        }
        match turn {
            Turn::Utterance(text) => handle_sentence_text(&session, text, &tts).await,
            Turn::Failed(e) => session.warn(e),
        }
    }
}

/// TTS 前清理：角色标签里的名字保留（如 `<role>悦悦</role>`），其余标签删除；
/// 括号里的动作提示（如「（笑着说）」「[招手]」）去掉，并压缩多余空行。
pub fn strip_role_tags(text: &str) -> String {
    let mut text = text.to_string();
    for tag in ["role", "name", "speaker", "character"] {
        text = text
            .replace(&format!("<{tag}>"), "")
            .replace(&format!("</{tag}>"), "");
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    loop {
        let Some(start) = rest.find('<') else {
            out.push_str(rest);
            break;
        };
        let Some(end_rel) = rest[start..].find('>') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + end_rel + 1..];
    }
    let mut cleaned = String::with_capacity(out.len());
    let mut depth = 0usize;
    for ch in out.chars() {
        match ch {
            '（' | '(' | '【' | '[' | '〔' | '『' => depth += 1,
            '）' | ')' | '】' | ']' | '〕' | '』' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(ch),
            _ => {}
        }
    }
    let lines: Vec<&str> = cleaned.lines().map(str::trim).collect();
    let mut result = String::with_capacity(cleaned.len());
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_question_detection() {
        assert!(is_vision_question("你能看到我吗"));
        assert!(is_vision_question("我穿的什么颜色衣服"));
        assert!(is_vision_question("画面里有什么呀"));
        assert!(is_vision_question("你看得出来我手里拿着什么吗"));
        assert!(!is_vision_question("今天天气怎么样"));
        assert!(!is_vision_question("给我讲个故事"));
    }

    #[test]
    fn agent_identity_contains_room() {
        assert_eq!(agent_identity("ai-call-x-1"), "agent-yueyue-ai-call-x-1");
    }

    #[test]
    fn role_strip_keeps_response_text() {
        let raw = "<role>悦悦</role>\n\n哈哈哈，你来啦！";
        assert_eq!(strip_role_tags(raw), "悦悦\n哈哈哈，你来啦！");
        assert_eq!(strip_role_tags("普通回复"), "普通回复");
        assert_eq!(strip_role_tags("<role>悦悦</role>早上好"), "悦悦早上好");
        assert_eq!(strip_role_tags("（笑着说）你好呀"), "你好呀");
        assert_eq!(strip_role_tags("[招手] 在干嘛"), "在干嘛");
    }
}
