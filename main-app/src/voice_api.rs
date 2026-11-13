//! 语音消息：音频 → 本地 ASR（yueyue SenseVoice）→ 聊天 → 本地 TTS（GSV-TTS）。
//!
//! `POST /api/chat/voice` body：
//! `{audio_base64, mime?, peer_id?, user_name?, source?}`
//! 返回 `{transcript, reply, audio_base64, audio_mime, duration_secs, tts_error?}`。
//!
//! 依赖本机 yueyue 8877（`/api/asr`、`/api/tts`）与 ffmpeg。地址可用
//! `VOICE_ASR_URL` / `VOICE_TTS_URL` / `FFMPEG_BIN` 覆盖。

use crate::{normalize_peer_id, run_chat, AppState};
use actix_web::{web, HttpResponse, Responder};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const DEFAULT_ASR_URL: &str = "http://127.0.0.1:8877/api/asr";
const DEFAULT_TTS_URL: &str = "http://127.0.0.1:8877/api/tts";
const MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize)]
pub struct VoiceChatPayload {
    audio_base64: String,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    peer_id: Option<String>,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

pub async fn voice_chat(
    state: web::Data<AppState>,
    body: web::Json<VoiceChatPayload>,
) -> impl Responder {
    let payload = body.into_inner();
    let audio = match BASE64.decode(payload.audio_base64.trim()) {
        Ok(bytes) => bytes,
        Err(e) => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({ "error": format!("音频 base64 解码失败：{e}") }))
        }
    };
    if audio.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({ "error": "音频为空" }));
    }
    if audio.len() > MAX_AUDIO_BYTES {
        return HttpResponse::PayloadTooLarge()
            .json(serde_json::json!({ "error": "音频过大（上限 16MB）" }));
    }

    let id = uuid::Uuid::new_v4();
    let temp_dir = std::env::temp_dir();
    let input_path =
        temp_dir.join(format!("bn_voice_in_{id}.{}", ext_for_mime(payload.mime.as_deref())));
    let wav_path = temp_dir.join(format!("bn_voice_out_{id}.wav"));

    if let Err(e) = tokio::fs::write(&input_path, &audio).await {
        return HttpResponse::InternalServerError()
            .json(serde_json::json!({ "error": format!("写入临时音频失败：{e}") }));
    }
    let convert = convert_to_wav(&input_path, &wav_path).await;
    let _ = tokio::fs::remove_file(&input_path).await;
    if let Err(e) = convert {
        let _ = tokio::fs::remove_file(&wav_path).await;
        return HttpResponse::BadGateway()
            .json(serde_json::json!({ "error": e, "stage": "ffmpeg" }));
    }
    let wav = match tokio::fs::read(&wav_path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = tokio::fs::remove_file(&wav_path).await;
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({ "error": format!("读取转换后音频失败：{e}") }));
        }
    };
    let _ = tokio::fs::remove_file(&wav_path).await;

    let transcript = match transcribe(wav).await {
        Ok(text) => text,
        Err(e) => {
            return HttpResponse::BadGateway()
                .json(serde_json::json!({ "error": e, "stage": "asr" }))
        }
    };
    if transcript.is_empty() {
        return HttpResponse::UnprocessableEntity().json(serde_json::json!({
            "error": "没有识别到语音内容",
            "stage": "asr",
            "transcript": "",
        }));
    }

    let peer_id = normalize_peer_id(payload.peer_id.as_deref().unwrap_or(""));
    let user_name = payload.user_name.unwrap_or_default();
    let source = payload
        .source
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "android".to_string());

    let reply = match run_chat(&state, &transcript, peer_id, user_name, source).await {
        Ok(resp) => resp.content.trim().to_string(),
        Err((code, message)) => {
            return HttpResponse::build(
                actix_web::http::StatusCode::from_u16(code)
                    .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR),
            )
            .json(serde_json::json!({
                "error": message,
                "stage": "chat",
                "transcript": transcript,
            }))
        }
    };
    if reply.is_empty() {
        return HttpResponse::BadGateway().json(serde_json::json!({
            "error": "模型返回了空回复",
            "stage": "chat",
            "transcript": transcript,
        }));
    }

    let (audio_base64, duration_secs, tts_error) = match synthesize(&reply).await {
        Ok(tts_wav) => {
            let duration = wav_duration_secs(&tts_wav)
                .map(|secs| secs.round().max(1.0) as u32)
                .unwrap_or(1);
            (BASE64.encode(&tts_wav), duration, None)
        }
        Err(e) => {
            log::warn!("[voice] TTS 失败（仅返回文字）：{e}");
            (String::new(), 0, Some(e))
        }
    };

    HttpResponse::Ok().json(serde_json::json!({
        "transcript": transcript,
        "reply": reply,
        "audio_base64": audio_base64,
        "audio_mime": "audio/wav",
        "duration_secs": duration_secs,
        "tts_error": tts_error,
    }))
}

fn ext_for_mime(mime: Option<&str>) -> &'static str {
    match mime.unwrap_or("").to_ascii_lowercase().as_str() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/webm" => "webm",
        "audio/3gpp" => "3gp",
        _ => "m4a",
    }
}

async fn convert_to_wav(input: &Path, output: &Path) -> Result<(), String> {
    let bin = std::env::var("FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string());
    let mut command = tokio::process::Command::new(bin);
    command
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-ar", "16000", "-ac", "1", "-f", "wav"])
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => Err(format!(
            "ffmpeg 转码失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(Err(e)) => Err(format!("ffmpeg 启动失败：{e}")),
        Err(_) => Err("ffmpeg 转码超时".to_string()),
    }
}

async fn transcribe(wav: Vec<u8>) -> Result<String, String> {
    let url = std::env::var("VOICE_ASR_URL").unwrap_or_else(|_| DEFAULT_ASR_URL.to_string());
    let boundary = format!("----bnvoice{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::with_capacity(wav.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"voice.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let response = reqwest::Client::new()
        .post(&url)
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .timeout(Duration::from_secs(90))
        .body(body)
        .send()
        .await
        .map_err(|e| format!("ASR 请求失败：{e}"))?;
    let status = response.status();
    let text = response
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

pub(crate) async fn synthesize(text: &str) -> Result<Vec<u8>, String> {
    let url = std::env::var("VOICE_TTS_URL").unwrap_or_else(|_| DEFAULT_TTS_URL.to_string());
    let response = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "text": text }))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("TTS 请求失败：{e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("TTS HTTP {status}: {body}"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("TTS 响应读取失败：{e}"))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" {
        return Err("TTS 返回的不是 WAV".to_string());
    }
    Ok(bytes.to_vec())
}

/// 解析 PCM WAV 时长（秒）：按 `fmt ` 的 byte_rate 与 `data` 长度计算。
pub(crate) fn wav_duration_secs(bytes: &[u8]) -> Option<f64> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut byte_rate = 0u32;
    let mut data_len = 0u32;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body_start = pos + 8;
        if id == b"fmt " && size >= 12 && body_start + 12 <= bytes.len() {
            byte_rate = u32::from_le_bytes(
                bytes[body_start + 8..body_start + 12].try_into().ok()?,
            );
        } else if id == b"data" {
            data_len = size.min(bytes.len().saturating_sub(body_start)) as u32;
            break;
        }
        pos = body_start + size + (size & 1);
    }
    if byte_rate == 0 {
        return None;
    }
    Some(data_len as f64 / byte_rate as f64)
}

#[cfg(test)]
mod tests {
    use super::{ext_for_mime, wav_duration_secs};

    fn pcm_wav(byte_rate: u32, data_len: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&16000u32.to_le_bytes()); // sample rate
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(bytes.len() + data_len as usize, 0);
        bytes
    }

    #[test]
    fn wav_duration_parses_pcm() {
        let wav = pcm_wav(32_000, 320_000);
        let secs = wav_duration_secs(&wav).expect("wav duration");
        assert!((secs - 10.0).abs() < 0.001);
    }

    #[test]
    fn wav_duration_rejects_non_wav() {
        assert!(wav_duration_secs(b"not a wav").is_none());
    }

    #[test]
    fn mime_maps_to_extension() {
        assert_eq!(ext_for_mime(Some("audio/wav")), "wav");
        assert_eq!(ext_for_mime(Some("audio/mp4")), "m4a");
        assert_eq!(ext_for_mime(None), "m4a");
    }
}
