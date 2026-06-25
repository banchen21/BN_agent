//! ASR-TTS Plugin — actor-free port of BN_agent's asr-tts-plugin.
//!
//! Uses MiMo Chat Completions API for:
//! - ASR: input_audio → text
//! - TTS: text → audio via audio modality
//!
//! Registers tools: `asr_transcribe`, `tts_synthesize`

use plugin_interface::*;
use std::sync::Arc;

pub struct AsrTtsPlugin {
    info: PluginInfo,
    asr_api_key: String,
    asr_base_url: String,
    asr_model: String,
    tts_api_key: String,
    tts_base_url: String,
    tts_model: String,
    tts_voice: String,
    tts_voice_desc: String,
    client: reqwest::Client,
    event_bus: Option<Addr<EventBus>>,
    logger: Option<PluginLogger>,
}

impl AsrTtsPlugin {
    pub fn new() -> Self {
        let asr_api_key = std::env::var("ASR_API_KEY").unwrap_or_default();
        let asr_base_url = std::env::var("ASR_BASE_URL")
            .or_else(|_| std::env::var("LLM_BASE_URL"))
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let asr_model = std::env::var("ASR_MODEL").unwrap_or_else(|_| "whisper-1".into());
        let tts_api_key = std::env::var("TTS_API_KEY")
            .or_else(|_| std::env::var("LLM_API_KEY"))
            .unwrap_or_default();
        let tts_base_url = std::env::var("TTS_BASE_URL")
            .or_else(|_| std::env::var("LLM_BASE_URL"))
            .unwrap_or_else(|_| "https://api.deepseek.com/v1".into());
        let tts_model = std::env::var("TTS_MODEL").unwrap_or_else(|_| "tts-1".into());
        let tts_voice = std::env::var("TTS_VOICE").unwrap_or_else(|_| "mimo_default".into());
        let tts_voice_desc = std::env::var("TTS_VOICE_DESC")
            .unwrap_or_else(|_| "你是小月，一个元气满满的 AI 助手。声音干净明亮。".into());

        Self {
            info: PluginInfo {
                name: "asr-tts-plugin".into(),
                version: "0.1.0".into(),
                description: "语音识别(ASR)与语音合成(TTS)".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            asr_api_key,
            asr_base_url,
            asr_model,
            tts_api_key,
            tts_base_url,
            tts_model,
            tts_voice,
            tts_voice_desc,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            event_bus: None,
            logger: None,
        }
    }
}

impl Plugin for AsrTtsPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        self.event_bus = Some(ctx.event_bus.clone());
        self.logger = Some(ctx.logger.clone());

        if let Some(ref reg) = ctx.tool_registry {
            let mut r = reg.lock();

            r.register(Arc::new(TtsTool {
                client: self.client.clone(),
                tts_base_url: self.tts_base_url.clone(),
                tts_model: self.tts_model.clone(),
                tts_api_key: self.tts_api_key.clone(),
                tts_voice: self.tts_voice.clone(),
                tts_voice_desc: self.tts_voice_desc.clone(),
                logger: ctx.logger.clone(),
            }));
            ctx.logger.info("registered tool: tts_synthesize");

            r.register(Arc::new(AsrTool {
                client: self.client.clone(),
                asr_base_url: self.asr_base_url.clone(),
                asr_model: self.asr_model.clone(),
                asr_api_key: self.asr_api_key.clone(),
                asr_language: std::env::var("ASR_LANGUAGE").unwrap_or_else(|_| "zh".into()),
                logger: ctx.logger.clone(),
            }));
            ctx.logger.info("registered tool: asr_transcribe");
        }

        ctx.logger.info("started");
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(ref l) = self.logger {
            l.info("stopped");
        }
    }

    fn on_event(&self, event: &Event) -> bool {
        match event.topic.as_str() {
            "webrtc_audio_received" | "audio_captured" => {
                let source = if event.topic == "webrtc_audio_received" {
                    "webrtc"
                } else {
                    "local"
                };
                self.handle_audio_received(event, source);
            }
            "assistant.message" => {
                let source = event
                    .data
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if source == "webrtc" || source == "local" {
                    self.handle_assistant_for_tts(event, source);
                }
            }
            _ => {}
        }
        true
    }
}

// ─── ASR/TTS 处理 ─────────────────────────────────────────────────

impl AsrTtsPlugin {
    fn handle_audio_received(&self, event: &Event, source: &str) {
        let peer_id = event
            .data
            .get("peer_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let audio_b64 = match event.data.get("data").and_then(|v| v.as_str()) {
            Some(d) => d.to_string(),
            None => return,
        };

        let audio_data = match base64_decode(&audio_b64) {
            Ok(d) => d,
            Err(_) => return,
        };

        let client = self.client.clone();
        let api_key = self.asr_api_key.clone();
        let base_url = self.asr_base_url.clone();
        let model = self.asr_model.clone();
        let lang = std::env::var("ASR_LANGUAGE").unwrap_or_else(|_| "zh".into());
        let source_owned = source.to_string();
        let peer = peer_id.to_string();
        let eb = self.event_bus.clone().unwrap();
        let logger = self.logger.clone().unwrap();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                match do_asr(
                    &client,
                    &base_url,
                    &model,
                    &api_key,
                    &audio_data,
                    "audio/opus",
                    &lang,
                    &logger,
                )
                .await
                {
                    Ok(text) => {
                        if text.trim().is_empty() {
                            return;
                        }
                        logger.info(format!("ASR [{}]: {}", peer, text));
                        eb.do_send(Event::new(
                            "user.message",
                            serde_json::json!({
                                "user_name": format!("{}:{}", source_owned, peer),
                                "text": text, "source": source_owned,
                            }),
                            "asr-tts-plugin",
                        ));
                    }
                    Err(e) => logger.warn(format!("ASR failed: {}", e)),
                }
            });
        });
    }

    fn handle_assistant_for_tts(&self, event: &Event, source: &str) {
        // 单会话模式：使用 source 作为 peer 标识
        let peer_id = source.to_string();
        let text = match event.data.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return,
        };
        if text.is_empty() {
            return;
        }
        let logger = self.logger.clone().unwrap();
        logger.info(format!(
            "TTS [{}]: {}...",
            peer_id,
            &text[..text.len().min(60)]
        ));

        let client = self.client.clone();
        let base_url = self.tts_base_url.clone();
        let model = self.tts_model.clone();
        let api_key = self.tts_api_key.clone();
        let voice = self.tts_voice.clone();
        let voice_desc = self.tts_voice_desc.clone();
        let peer = peer_id.clone();
        let source_owned = source.to_string();
        let eb = self.event_bus.clone().unwrap();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async {
                match do_tts(
                    &client,
                    &base_url,
                    &model,
                    &api_key,
                    &text,
                    &voice,
                    Some(&voice_desc),
                )
                .await
                {
                    Ok(audio_data) => {
                        logger.info(format!("TTS done [{}]: {} bytes", peer, audio_data.len()));
                        let topic = match source_owned.as_str() {
                            "webrtc" => "webrtc_audio_send",
                            "local" => "local_audio_play",
                            _ => return,
                        };
                        eb.do_send(Event::new(
                            topic,
                            serde_json::json!({
                                "peer_id": peer, "data": base64_encode(&audio_data),
                            }),
                            "asr-tts-plugin",
                        ));
                    }
                    Err(e) => logger.warn(format!("TTS failed: {}", e)),
                }
            });
        });
    }
}

// ─── OGG→WAV 转换（使用 ffmpeg） ──────────────────────────────────

/// 通过 ffmpeg 将 OGG/Opus 转为 WAV。需要系统安装 ffmpeg。
fn ogg_to_wav(ogg_data: &[u8], logger: &PluginLogger) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    logger.info(format!("ffmpeg: spawning..."));
    let mut child = Command::new("ffmpeg")
        .args(["-y", "-f", "ogg", "-i", "pipe:0", "-f", "wav", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg spawn: {}", e))?;

    logger.info(format!(
        "ffmpeg: writing {} bytes to stdin...",
        ogg_data.len()
    ));
    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(ogg_data)
            .map_err(|e| format!("write stdin: {}", e))?;
    }
    drop(child.stdin.take());
    logger.info("ffmpeg: stdin closed, waiting for output...");

    let output = child
        .wait_with_output()
        .map_err(|e| format!("ffmpeg wait: {}", e))?;
    logger.info(format!(
        "ffmpeg: done, output {} bytes",
        output.stdout.len()
    ));
    if !output.status.success() {
        return Err(format!("ffmpeg exit: {}", output.status));
    }
    Ok(output.stdout)
}

// ─── ASR API ──────────────────────────────────────────────────────

async fn do_asr(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    audio_data: &[u8],
    mime_type: &str,
    language: &str,
    logger: &PluginLogger,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    // OGG/Opus → 先用 ffmpeg 转 WAV（API 只支持 wav/mp3）
    let (audio_bytes, final_mime) = if mime_type == "audio/ogg" || mime_type == "audio/opus" {
        match ogg_to_wav(audio_data, logger) {
            Ok(wav) => (wav, "audio/wav".into()),
            Err(e) => {
                logger.warn(format!("ffmpeg not available ({}), sending raw ogg", e));
                (audio_data.to_vec(), mime_type.to_string())
            }
        }
    } else {
        (audio_data.to_vec(), mime_type.to_string())
    };

    logger.info(format!(
        "sending to ASR ({}, {} bytes)...",
        final_mime,
        audio_bytes.len()
    ));
    let audio_b64 = base64_encode(&audio_bytes);
    let data_url = format!("data:{};base64,{}", final_mime, audio_b64);

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": data_url}}]}],
        "asr_options": { "language": language },
    });

    let resp = client
        .post(&url)
        .header("api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("ASR request failed: {}", e))?;
    let status = resp.status();
    let body_text = resp.text().await.map_err(|e| format!("read: {}", e))?;
    logger.info(format!(
        "ASR status={} body_len={}",
        status,
        body_text.len()
    ));
    if !status.is_success() {
        return Err(format!("ASR {}: {}", status, body_text));
    }

    serde_json::from_str::<serde_json::Value>(&body_text)
        .map_err(|e| format!("parse: {}", e))?
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c["message"]["content"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("bad ASR response: {}", body_text))
}

// ─── TTS API ──────────────────────────────────────────────────────

async fn do_tts(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    text: &str,
    voice: &str,
    voice_desc: Option<&str>,
) -> Result<Vec<u8>, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let is_voicedesign = model.contains("voicedesign");

    let mut messages = Vec::new();
    if is_voicedesign {
        if let Some(desc) = voice_desc {
            messages.push(serde_json::json!({"role": "user", "content": desc}));
        }
    }
    messages.push(serde_json::json!({"role": "assistant", "content": text}));

    let mut audio = serde_json::json!({"format": "wav"});
    if !is_voicedesign {
        audio["voice"] = serde_json::json!(voice);
    }

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "audio": audio,
    });

    let resp = client
        .post(&url)
        .header("api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("TTS request: {}", e))?;
    let status = resp.status();
    let resp_text = resp.text().await.map_err(|e| format!("read: {}", e))?;
    if !status.is_success() {
        return Err(format!(
            "TTS {}: {}",
            status,
            &resp_text[..resp_text.len().min(500)]
        ));
    }

    let audio_b64 = serde_json::from_str::<serde_json::Value>(&resp_text)
        .map_err(|e| format!("parse: {}", e))?
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c["message"]["audio"]["data"].as_str())
        .ok_or_else(|| {
            format!(
                "bad TTS response: {}",
                &resp_text[..resp_text.len().min(300)]
            )
        })?
        .to_string();

    base64_decode(&audio_b64)
}

// ─── 工具 ─────────────────────────────────────────────────────────

struct AsrTool {
    client: reqwest::Client,
    asr_base_url: String,
    asr_model: String,
    asr_api_key: String,
    asr_language: String,
    logger: PluginLogger,
}

impl ToolExecutor for AsrTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "asr_transcribe".into(),
            description: "Transcribe audio to text. Input: base64 audio + MIME type. Output: recognized text.".into(),
            internal: true,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "audio_base64": {"type": "string", "description": "Base64 audio data"},
                    "mime_type": {"type": "string", "description": "MIME type, e.g. audio/wav"}
                },
                "required": ["audio_base64"]
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        self.logger.info("ASR execute called");
        let audio_b64 = match args.get("audio_base64").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: audio_base64"),
        };
        let mime = args
            .get("mime_type")
            .and_then(|v| v.as_str())
            .unwrap_or("audio/ogg");
        self.logger
            .info(format!("mime={} audio_b64_len={}", mime, audio_b64.len()));
        let audio = match base64_decode(audio_b64) {
            Ok(d) => d,
            Err(e) => {
                self.logger.error(format!("base64 decode failed: {}", e));
                return ToolResult::err(&format!("base64: {}", e));
            }
        };
        self.logger.info(format!("decoded {} bytes", audio.len()));

        let c = self.client.clone();
        let u = self.asr_base_url.clone();
        let m = self.asr_model.clone();
        let k = self.asr_api_key.clone();
        let lang = self.asr_language.clone();
        let mime_o = mime.to_string();
        let logger = self.logger.clone();

        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async { do_asr(&c, &u, &m, &k, &audio, &mime_o, &lang, &logger).await })
        })
        .join()
        {
            Ok(Ok(t)) => ToolResult::ok(&t),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("thread panic"),
        }
    }
}

struct TtsTool {
    client: reqwest::Client,
    tts_base_url: String,
    tts_model: String,
    tts_api_key: String,
    tts_voice: String,
    tts_voice_desc: String,
    logger: PluginLogger,
}

impl ToolExecutor for TtsTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tts_synthesize".into(),
            description: "Convert text to speech. Returns base64-encoded audio data.".into(),
            internal: true,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to synthesize"},
                    "voice_desc": {"type": "string", "description": "Optional voice description"}
                },
                "required": ["text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("missing: text"),
        };
        let custom_desc = args
            .get("voice_desc")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let c = self.client.clone();
        let u = self.tts_base_url.clone();
        let m = self.tts_model.clone();
        let k = self.tts_api_key.clone();
        let v = self.tts_voice.clone();
        let desc = custom_desc.unwrap_or(&self.tts_voice_desc).to_string();

        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            rt.block_on(async { do_tts(&c, &u, &m, &k, &text, &v, Some(&desc)).await })
        })
        .join()
        {
            Ok(Ok(audio)) => ToolResult::ok(&base64_encode(&audio)),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("thread panic"),
        }
    }
}

// ─── base64 ───────────────────────────────────────────────────────

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("base64: {}", e))
}

// ─── FFI ─────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(AsrTtsPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
