//! protocol.rs — 微信 iLink Bot API 协议层
//!
//! 腾讯官方开放的微信个人号 Bot API。
//! 基座地址：https://ilinkai.weixin.qq.com
//!
//! ## 接口
//!
//! | 方法 | 路径 | 用途 |
//! |------|------|------|
//! | GET  | /ilink/bot/get_bot_qrcode?bot_type=3 | 获取登录二维码 |
//! | GET  | /ilink/bot/get_qrcode_status?qrcode=... | 轮询扫码状态 |
//! | POST | /ilink/bot/getupdates | 长轮询收消息 (35s) |
//! | POST | /ilink/bot/sendmessage | 发送文本/媒体消息 |
//!
//! ## 认证
//!
//! - `Authorization: Bearer <bot_token>`
//! - `AuthorizationType: ilink_bot_token`
//! - `X-WECHAT-UIN: base64(string(random_uint32))`
//!
//! ## context_token
//!
//! 每条入站消息携带 context_token，回复时必须原样回传。
//! 没有它，回复无法路由到正确的微信会话。

use qrcode::QrCode;
use reqwest::Client;
use serde::{Deserialize, Serialize};

// ── Constants ────────────────────────────────────────────────────────────────

pub const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
pub const CHANNEL_VERSION: &str = "2.0.0";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// ── Data Types ───────────────────────────────────────────────────────────────

/// iLink Bot 登录会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WechatSession {
    pub token: String,
    pub base_url: String,
    pub account_id: String,
    pub user_id: String,
}

/// 从 get_updates 返回的入站消息。
#[derive(Debug, Clone)]
pub struct WechatIncomingMessage {
    pub from_user_id: String,
    pub text: String,
    pub context_token: String,
    pub msg_type: i32,
}

/// get_updates 响应。
#[derive(Debug)]
pub struct UpdatesResponse {
    pub ret: i32,
    pub messages: Vec<WechatIncomingMessage>,
    pub next_buf: String,
}

/// 获取二维码的响应。
#[derive(Debug)]
pub struct QrCodeResult {
    /// 用于轮询状态的不透明码
    pub qrcode: String,
    /// 用于生成二维码图片的内容（URL 或 clawbot:// 协议）
    pub img_content: String,
}

/// 扫码状态。
#[derive(Debug)]
pub enum QrStatus {
    Wait,
    Scanned,
    Confirmed { bot_token: String, base_url: String, account_id: String, user_id: String },
    Expired,
}

// ── HTTP client ──────────────────────────────────────────────────────────────

pub fn build_client() -> reqwest::Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(60))
        .build()
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// X-WECHAT-UIN：随机 4 字节 → uint32 → 十进制字符串 → base64。
fn random_wechat_uin() -> String {
    let val: u32 = rand::random::<u32>();
    // 将十进制数字符串编码为 base64
    let decimal = val.to_string();
    base64_encode(decimal.as_bytes())
}

/// 简单的 base64 编码（不依赖 base64 crate）。
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn gen_client_id() -> String {
    use rand::Rng;
    let rng = rand::thread_rng();
    let uuid: Vec<u32> = rng.sample_iter(rand::distributions::Standard).take(4).collect();
    format!(
        "wcb-{:08x}-{:04x}-{:04x}-{:04x}-{:04x}{:08x}",
        uuid[0],
        (uuid[1] >> 16) as u16,
        (uuid[1] & 0xFFFF) as u16,
        (uuid[2] >> 16) as u16,
        (uuid[2] & 0xFFFF) as u16,
        uuid[3],
    )
}

// ── 1. 二维码登录 ────────────────────────────────────────────────────────────

/// Step 1: 获取登录二维码。
pub async fn fetch_qrcode(client: &Client) -> Result<QrCodeResult, String> {
    let url = format!("{}/ilink/bot/get_bot_qrcode?bot_type=3", DEFAULT_BASE_URL);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch_qrcode HTTP: {}", e))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("fetch_qrcode JSON: {}", e))?;

    let qrcode = json["qrcode"]
        .as_str()
        .ok_or_else(|| "missing qrcode".to_string())?
        .to_string();
    let img_content = json["qrcode_img_content"]
        .as_str()
        .ok_or_else(|| "missing qrcode_img_content".to_string())?
        .to_string();

    Ok(QrCodeResult { qrcode, img_content })
}

/// 生成二维码 PNG 字节。
pub fn gen_qrcode(content: &str) -> Result<Vec<u8>, String> {
    let code =
        QrCode::new(content.as_bytes()).map_err(|e| format!("qrcode encode: {}", e))?;
    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(300, 300)
        .build();
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("qrcode PNG: {}", e))?;
    Ok(buf.into_inner())
}

/// Step 2: 轮询扫码状态。
pub async fn poll_qrcode(client: &Client, qrcode: &str) -> Result<QrStatus, String> {
    let url = format!(
        "{}/ilink/bot/get_qrcode_status?qrcode={}",
        DEFAULT_BASE_URL,
        urlencode(qrcode)
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("poll_qrcode HTTP: {}", e))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("poll_qrcode JSON: {}", e))?;

    let status = json["status"]
        .as_str()
        .unwrap_or("wait")
        .to_string();

    match status.as_str() {
        "wait" => Ok(QrStatus::Wait),
        "scaned" => Ok(QrStatus::Scanned),
        "expired" => Ok(QrStatus::Expired),
        "confirmed" => {
            let bot_token = json["bot_token"]
                .as_str()
                .ok_or_else(|| "missing bot_token".to_string())?
                .to_string();
            let base_url = json["baseurl"]
                .as_str()
                .unwrap_or(DEFAULT_BASE_URL)
                .to_string();
            let account_id = json["ilink_bot_id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let user_id = json["ilink_user_id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            Ok(QrStatus::Confirmed { bot_token, base_url, account_id, user_id })
        }
        _ => Ok(QrStatus::Wait),
    }
}

// ── 2. 消息收发 ──────────────────────────────────────────────────────────────

/// 构建 iLink API 通用请求头。
fn build_headers(token: Option<&str>) -> reqwest::header::HeaderMap {
    use reqwest::header::*;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
    headers.insert("X-WECHAT-UIN", random_wechat_uin().parse().unwrap());
    if let Some(t) = token {
        headers.insert(AUTHORIZATION, format!("Bearer {}", t).parse().unwrap());
    }
    headers
}

/// Base request info injected into every POST body.
fn base_info() -> serde_json::Value {
    serde_json::json!({ "channel_version": CHANNEL_VERSION })
}

/// Step 3: 长轮询收消息（35 秒挂起）。
pub async fn get_updates(
    client: &Client,
    token: &str,
    base_url: &str,
    buf: &str,
) -> Result<UpdatesResponse, String> {
    let url = format!("{}/ilink/bot/getupdates", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "get_updates_buf": buf,
        "base_info": base_info(),
    });

    let resp = client
        .post(&url)
        .headers(build_headers(Some(token)))
        .json(&body)
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| format!("getupdates HTTP: {}", e))?;

    let status_code = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("getupdates JSON ({}): {}", status_code, e))?;

    // 检查错误码
    if let Some(errcode) = json.get("errcode") {
        let code = errcode.as_i64().unwrap_or(0);
        if code == -14 {
            return Err("session timeout".to_string());
        }
    }
    if let Some(ret) = json.get("ret") {
        let code = ret.as_i64().unwrap_or(0);
        if code == -14 {
            return Err("session timeout".to_string());
        }
        if code != 0 {
            return Err(format!("getupdates ret={}", code));
        }
    }

    let next_buf = json["get_updates_buf"].as_str().unwrap_or("").to_string();
    let mut messages = Vec::new();

    if let Some(msgs) = json["msgs"].as_array() {
        for msg in msgs {
            let from_user_id = msg["from_user_id"].as_str().unwrap_or("").to_string();
            let context_token = msg["context_token"].as_str().unwrap_or("").to_string();
            let msg_type = msg["message_type"].as_i64().unwrap_or(0) as i32;

            // 提取文本（支持 text_item、voice_item 等）
            let text = extract_text(msg);

            if from_user_id.is_empty() && text.is_empty() {
                continue;
            }

            messages.push(WechatIncomingMessage {
                from_user_id,
                text,
                context_token,
                msg_type,
            });
        }
    }

    Ok(UpdatesResponse {
        ret: 0,
        messages,
        next_buf,
    })
}

/// 从消息的 item_list 中提取纯文本。
fn extract_text(msg: &serde_json::Value) -> String {
    if let Some(items) = msg["item_list"].as_array() {
        for item in items {
            let item_type = item["type"].as_i64().unwrap_or(0);
            match item_type {
                1 => {
                    // 文本
                    if let Some(text) = item["text_item"]["text"].as_str() {
                        return text.to_string();
                    }
                }
                3 => {
                    // 语音转文字
                    if let Some(text) = item["voice_item"]["text"].as_str() {
                        return format!("[语音] {}", text);
                    }
                }
                _ => {}
            }
        }
    }
    // 降级：取 item_list 外层可能存在的 text 字段
    msg["text"].as_str().unwrap_or("").to_string()
}

/// Step 4: 发送文本消息。
pub async fn send_message(
    client: &Client,
    token: &str,
    base_url: &str,
    to_user_id: &str,
    text: &str,
    context_token: &str,
) -> Result<(), String> {
    let url = format!("{}/ilink/bot/sendmessage", base_url.trim_end_matches('/'));
    let client_id = gen_client_id();

    let body = serde_json::json!({
        "msg": {
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": client_id,
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [
                { "type": 1, "text_item": { "text": text } }
            ],
        },
        "base_info": base_info(),
    });

    let resp = client
        .post(&url)
        .headers(build_headers(Some(token)))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("sendmessage HTTP: {}", e))?;

    let status_code = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("sendmessage JSON ({}): {}", status_code, e))?;

    // 检查错误
    if let Some(errcode) = json.get("errcode") {
        let code = errcode.as_i64().unwrap_or(0);
        if code == -14 {
            return Err("session timeout".to_string());
        }
        if code != 0 {
            let msg = json["errmsg"].as_str().unwrap_or("unknown");
            return Err(format!("sendmessage errcode={}: {}", code, msg));
        }
    }

    Ok(())
}

// ── 4. 输入状态指示器 ──────────────────────────────────────────────────────────

/// 获取用户的 typing_ticket（缓存 ≈24h）。
/// POST /ilink/bot/getconfig
pub async fn get_typing_ticket(
    client: &Client,
    token: &str,
    base_url: &str,
    to_user_id: &str,
) -> Result<String, String> {
    let url = format!("{}/ilink/bot/getconfig", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "to_user_id": to_user_id,
        "base_info": base_info(),
    });

    let resp = client
        .post(&url)
        .headers(build_headers(Some(token)))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("getconfig HTTP: {}", e))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("getconfig JSON: {}", e))?;

    let ticket = json["typing_ticket"]
        .as_str()
        .ok_or_else(|| "missing typing_ticket".to_string())?
        .to_string();

    log::info!("[wechat] got typing_ticket for {} ({} chars)", to_user_id, ticket.len());
    Ok(ticket)
}

/// 发送输入状态。
/// status: 1 = 开始输入, 2 = 停止输入。
/// POST /ilink/bot/sendtyping
pub async fn send_typing(
    client: &Client,
    token: &str,
    base_url: &str,
    to_user_id: &str,
    typing_ticket: &str,
    status: i32,
) -> Result<(), String> {
    let url = format!("{}/ilink/bot/sendtyping", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "to_user_id": to_user_id,
        "typing_ticket": typing_ticket,
        "status": status,
        "base_info": base_info(),
    });

    let resp = client
        .post(&url)
        .headers(build_headers(Some(token)))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("sendtyping HTTP: {}", e))?;

    let status_code = resp.status();
    if !status_code.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("sendtyping HTTP {}: {}", status_code, text));
    }

    Ok(())
}

// ── Utility ──────────────────────────────────────────────────────────────────

/// 简单 URL 编码（仅编码特殊字符）。
fn urlencode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
