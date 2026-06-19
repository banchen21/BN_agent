//! SDK adapter layer — wraps weixin-ilink-sdk for the clawbot plugin system.
//!
//! Provides a non-generic [`WeChatClient`] that encapsulates:
//! - QR login via iLink Bot API
//! - Long-poll message reception
//! - Text + media message sending (image, video, file)
//! - CDN media download + decryption
//! - Voice download + SILK→WAV decoding
//! - Typing indicator
//! - Session persistence (JSON file)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use weixin_ilink_sdk::auth::{LoginHandler, LoginResult};
use weixin_ilink_sdk::types::{GetUpdatesResponse, Message, MessageItem, MessageItemType, TypingStatus};
use weixin_ilink_sdk::ILinkClient;

// ── Session Info ───────────────────────────────────────────────────────────

/// Serializable session data persisted to `data/wechat_session.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WechatSessionInfo {
    pub token: String,
    pub base_url: String,
    pub account_id: String,
    pub user_id: String,
}

// ── Incoming Message ───────────────────────────────────────────────────────

/// A received message with typed SDK items for media access.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub from_user_id: String,
    pub text: String,
    pub context_token: String,
    /// Full typed item list from the SDK (text, image, voice, file, video).
    pub items: Vec<MessageItem>,
}

// ── Poll Result ────────────────────────────────────────────────────────────

/// Result of a single long-poll call.
#[derive(Debug)]
pub struct PollResult {
    pub messages: Vec<IncomingMessage>,
    pub next_buf: String,
}

// ── WeChatClient ───────────────────────────────────────────────────────────

/// Concrete wrapper around the SDK's generic `ILinkClient`.
///
/// All iLink API calls go through this struct. The inner client uses
/// `reqwest::Client` as the HTTP backend. The client is wrapped in `Arc`
/// for sharing across threads (the SDK's `ILinkClient` does not implement `Clone`).
#[derive(Clone)]
pub struct WeChatClient {
    /// The SDK client, Arc'd for thread-safe sharing.
    pub inner: Arc<ILinkClient<reqwest::Client>>,
    /// Serializable session info for file persistence.
    pub session: WechatSessionInfo,
}

impl WeChatClient {
    // ── Login ──────────────────────────────────────────────────────────

    /// Start a QR-code login flow with the given handler.
    ///
    /// The handler receives QR URL → save PNG → set status.
    /// Returns a fully authenticated `WeChatClient`.
    pub async fn login(handler: &dyn LoginHandler) -> Result<Self, String> {
        let client = ILinkClient::builder()
            .login(handler)
            .await
            .map_err(|e| format!("login failed: {e}"))?;

        // Extract LoginResult from the post-login client.
        // The SDK doesn't expose LoginResult after builder().login(),
        // but the token is accessible via client.token().
        let token = client
            .token()
            .ok_or_else(|| "login succeeded but no token".to_string())?
            .to_string();

        Ok(Self {
            inner: Arc::new(client),
            session: WechatSessionInfo {
                token,
                base_url: weixin_ilink_sdk::client::DEFAULT_BASE_URL.to_string(),
                account_id: String::new(),
                user_id: String::new(),
            },
        })
    }

    /// Login with a handler that also captures the full `LoginResult`.
    pub async fn login_with_result(
        handler: &PluginLoginHandler,
    ) -> Result<Self, String> {
        let client = ILinkClient::builder()
            .login(handler as &dyn LoginHandler)
            .await
            .map_err(|e| format!("login failed: {e}"))?;

        let result = handler
            .result
            .lock()
            .map_err(|e| format!("lock: {e}"))?
            .clone()
            .ok_or_else(|| "login completed but no result captured".to_string())?;

        Ok(Self {
            inner: Arc::new(client),
            session: WechatSessionInfo {
                token: result.bot_token,
                base_url: result
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .unwrap_or_else(|| weixin_ilink_sdk::client::DEFAULT_BASE_URL.to_string()),
                account_id: result.ilink_bot_id,
                user_id: result.user_id.unwrap_or_default(),
            },
        })
    }

    /// Restore a client from a saved session (no login needed).
    pub fn restore(session: WechatSessionInfo) -> Self {
        let client = ILinkClient::builder()
            .token(&session.token)
            .base_url(&session.base_url)
            .build();

        Self {
            inner: Arc::new(client),
            session,
        }
    }

    // ── Messaging ──────────────────────────────────────────────────────

    /// Long-poll for new messages.
    pub async fn poll_messages(
        &self,
        buf: &str,
        timeout_secs: u64,
    ) -> Result<PollResult, String> {
        let timeout = Duration::from_secs(timeout_secs);
        let resp: GetUpdatesResponse = self
            .inner
            .get_updates(buf, Some(timeout))
            .await
            .map_err(|e| format!("get_updates: {e}"))?;

        let next_buf = resp.get_updates_buf.clone().unwrap_or_default();
        let mut messages = Vec::new();

        if let Some(msgs) = resp.msgs {
            for msg in msgs {
                let from_user_id = msg.from_user_id.clone().unwrap_or_default();
                let context_token = msg.context_token.clone().unwrap_or_default();

                if from_user_id.is_empty() {
                    continue;
                }

                let text = msg.extract_text().unwrap_or_default();

                let items = msg.item_list.clone().unwrap_or_default();

                messages.push(IncomingMessage {
                    from_user_id,
                    text,
                    context_token,
                    items,
                });
            }
        }

        Ok(PollResult {
            messages,
            next_buf,
        })
    }

    /// Send a plain text message.
    pub async fn send_text(
        &self,
        to: &str,
        text: &str,
        context_token: &str,
    ) -> Result<(), String> {
        self.inner
            .send_text(to, text, context_token)
            .await
            .map_err(|e| format!("send_text: {e}"))?;
        Ok(())
    }

    /// Send an image (local file → CDN upload → send).
    pub async fn send_image(
        &self,
        to: &str,
        path: &Path,
        text: &str,
        context_token: &str,
    ) -> Result<(), String> {
        self.inner
            .send_image(to, path, text, context_token)
            .await
            .map_err(|e| format!("send_image: {e}"))?;
        Ok(())
    }

    /// Send a video (local file → CDN upload → send).
    pub async fn send_video(
        &self,
        to: &str,
        path: &Path,
        text: &str,
        context_token: &str,
    ) -> Result<(), String> {
        self.inner
            .send_video(to, path, text, context_token)
            .await
            .map_err(|e| format!("send_video: {e}"))?;
        Ok(())
    }

    /// Send a file attachment (local file → CDN upload → send).
    pub async fn send_file(
        &self,
        to: &str,
        path: &Path,
        text: &str,
        context_token: &str,
    ) -> Result<(), String> {
        self.inner
            .send_file(to, path, text, context_token)
            .await
            .map_err(|e| format!("send_file: {e}"))?;
        Ok(())
    }

    /// Send media, auto-detecting type from file extension.
    pub async fn send_media(
        &self,
        to: &str,
        path: &Path,
        text: &str,
        context_token: &str,
    ) -> Result<(), String> {
        self.inner
            .send_media(to, path, text, context_token)
            .await
            .map_err(|e| format!("send_media: {e}"))?;
        Ok(())
    }

    // ── Typing Indicator ───────────────────────────────────────────────

    /// Get the typing ticket for a user (cached ~24h by the server).
    pub async fn get_typing_ticket(
        &self,
        user_id: &str,
        context_token: &str,
    ) -> Result<String, String> {
        let resp = self
            .inner
            .get_config(user_id, Some(context_token))
            .await
            .map_err(|e| format!("get_config: {e}"))?;

        resp.typing_ticket
            .ok_or_else(|| "no typing_ticket in response".to_string())
    }

    /// Send typing status to a user (1 = start, 2 = stop).
    pub async fn send_typing(
        &self,
        user_id: &str,
        typing_ticket: &str,
        status: i32,
    ) -> Result<(), String> {
        let sdk_status = match status {
            1 => TypingStatus::Typing,
            2 => TypingStatus::Cancel,
            _ => return Err(format!("invalid typing status: {status}")),
        };

        self.inner
            .send_typing(user_id, typing_ticket, sdk_status)
            .await
            .map_err(|e| format!("send_typing: {e}"))?;
        Ok(())
    }

    // ── Context Token ──────────────────────────────────────────────────

    /// Get the cached context token for a user.
    pub fn get_context_token(&self, user_id: &str) -> Option<String> {
        self.inner.get_context_token(user_id)
    }

    // ── CDN Media Download ─────────────────────────────────────────────

    /// Download and decrypt a CDN media file using base64-encoded AES key.
    pub async fn download_media(
        &self,
        encrypt_query_param: &str,
        aes_key_base64: &str,
    ) -> Result<Vec<u8>, String> {
        weixin_ilink_sdk::cdn::download_and_decrypt(&self.inner, encrypt_query_param, aes_key_base64)
            .await
            .map_err(|e| format!("cdn download: {e}"))
    }

    /// Download and decrypt a CDN media file using hex-encoded AES key (images).
    pub async fn download_media_hex_key(
        &self,
        encrypt_query_param: &str,
        aeskey_hex: &str,
    ) -> Result<Vec<u8>, String> {
        weixin_ilink_sdk::cdn::download_and_decrypt_hex_key(&self.inner, encrypt_query_param, aeskey_hex)
            .await
            .map_err(|e| format!("cdn download (hex): {e}"))
    }

    /// Download a voice message: CDN → decrypt → decode SILK → WAV.
    pub async fn download_voice(
        &self,
        voice_item: &weixin_ilink_sdk::types::VoiceItem,
    ) -> Result<Vec<u8>, String> {
        #[cfg(feature = "voice")]
        let decoder: Option<&voice::DefaultSilkDecoder> = Some(&voice::DefaultSilkDecoder);
        #[cfg(not(feature = "voice"))]
        let decoder: Option<&dyn voice::SilkDecoder> = None;

        let data = voice::download_voice(
            &self.inner,
            voice_item,
            decoder,
        )
        .await
        .map_err(|e| format!("voice download: {e}"))?;
        Ok(data.data)
    }

    // ── Session Persistence ────────────────────────────────────────────

    /// Save session to `data/wechat_session.json`.
    pub fn save_to_file(&self) -> Result<(), String> {
        session_persist::save(&self.session)
    }

    /// Load session from `data/wechat_session.json`.
    pub fn load_from_file() -> Option<WechatSessionInfo> {
        session_persist::load()
    }

    /// Delete the saved session file.
    pub fn clear_saved() {
        session_persist::clear();
    }
}

// ── PluginLoginHandler ─────────────────────────────────────────────────────

/// `LoginHandler` implementation that captures QR code events and final result
/// into shared state, accessible by the plugin main loop.
pub struct PluginLoginHandler {
    /// Current status text for the plugin UI.
    pub status_text: Arc<Mutex<String>>,
    /// Path to the most recently saved QR code PNG.
    pub qr_path: Arc<Mutex<Option<String>>>,
    /// Final login result (populated on confirmation).
    pub result: Arc<Mutex<Option<LoginResult>>>,
    /// Whether the QR code has been scanned.
    pub scanned: Arc<Mutex<bool>>,
}

impl PluginLoginHandler {
    pub fn new() -> Self {
        Self {
            status_text: Arc::new(Mutex::new(String::new())),
            qr_path: Arc::new(Mutex::new(None)),
            result: Arc::new(Mutex::new(None)),
            scanned: Arc::new(Mutex::new(false)),
        }
    }
}

impl LoginHandler for PluginLoginHandler {
    fn on_qrcode(&self, url: &str) {
        // Generate QR PNG and save to data/wechat_qrcode.png
        let path = save_qr_png(url);
        if let Some(ref p) = path {
            *self.qr_path.lock().unwrap() = Some(p.clone());
        }
        *self.status_text.lock().unwrap() = format!("qr_ready");
    }

    fn on_scanned(&self) {
        *self.scanned.lock().unwrap() = true;
    }

    fn on_expired(&self, refresh_count: u32, max_refreshes: u32) {
        log::warn!("[wechat] QR expired ({}/{})", refresh_count, max_refreshes);
    }
}

// ── QR PNG Generation ──────────────────────────────────────────────────────

/// Generate a QR code PNG image from the content string and save to disk.
fn save_qr_png(content: &str) -> Option<String> {
    use qrcode::QrCode;

    let code = match QrCode::new(content.as_bytes()) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[wechat] qrcode encode: {e}");
            return None;
        }
    };

    // Render to image::Luma8
    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(300, 300)
        .build();

    let qr_path = Path::new("data").join("wechat_qrcode.png");
    if let Some(parent) = qr_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    if let Err(e) = img.write_to(&mut buf, image::ImageFormat::Png) {
        log::error!("[wechat] qrcode PNG: {e}");
        return None;
    }

    let png_bytes = buf.into_inner();
    if let Err(e) = std::fs::write(&qr_path, &png_bytes) {
        log::error!("[wechat] save qrcode: {e}");
        return None;
    }

    let abs_path = std::fs::canonicalize(&qr_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| qr_path.to_string_lossy().to_string());

    log::info!("[wechat] QR code saved: {abs_path}");
    Some(abs_path)
}

// ── Session File Persistence ───────────────────────────────────────────────

mod session_persist {
    use super::WechatSessionInfo;
    use std::path::Path;

    fn path() -> std::path::PathBuf {
        Path::new("data").join("wechat_session.json")
    }

    pub fn save(session: &WechatSessionInfo) -> Result<(), String> {
        let p = path();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(session).map_err(|e| format!("json: {e}"))?;
        std::fs::write(&p, &json).map_err(|e| format!("write: {e}"))?;
        log::info!("[wechat] session saved to {:?}", p);
        Ok(())
    }

    pub fn load() -> Option<WechatSessionInfo> {
        let p = path();
        if !p.exists() {
            return None;
        }
        let raw = std::fs::read_to_string(&p).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn clear() {
        let p = path();
        let _ = std::fs::remove_file(&p);
        log::info!("[wechat] session cleared");
    }
}

// ── Re-exports for convenience ─────────────────────────────────────────────

pub use weixin_ilink_sdk::types::MessageItemType as SdkMessageItemType;
pub use weixin_ilink_sdk::voice;
