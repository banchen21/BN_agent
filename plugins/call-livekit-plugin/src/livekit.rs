//! LiveKit Server Twirp API 客户端（RoomService / AgentDispatchService）。
//!
//! 全部走 `POST {api_url}/twirp/livekit.<Service>/<Method>`，
//! 使用服务端管理 token（HS256）作为 Bearer。

use crate::config::CallConfig;
use crate::jwt;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TwirpError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for TwirpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LiveKit {} ({}): {}",
            self.status, self.code, self.message
        )
    }
}

impl std::error::Error for TwirpError {}

impl TwirpError {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            status: 0,
            code: "transport_error".into(),
            message: message.into(),
        }
    }

    pub fn is_not_found(&self) -> bool {
        self.status == 404 || self.code == "not_found"
    }

    pub fn is_already_exists(&self) -> bool {
        self.status == 409 || self.code == "already_exists"
    }
}

#[derive(Clone)]
pub struct LiveKitAdmin {
    api_url: String,
    api_key: String,
    api_secret: String,
    client: reqwest::Client,
}

impl LiveKitAdmin {
    pub fn new(config: &CallConfig) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(3));

        // https Twirp：用本地 CA 校验（rustls 不读取 Windows 证书store）。
        if !config.api_ca_cert.trim().is_empty() {
            let raw = config.api_ca_cert.trim();
            let candidates = [
                std::path::PathBuf::from(raw),
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(raw),
            ];
            if let Some(path) = candidates.iter().find(|p| p.is_file()) {
                match std::fs::read(path)
                    .map_err(|e| e.to_string())
                    .and_then(|pem| reqwest::Certificate::from_pem(&pem).map_err(|e| e.to_string()))
                {
                    Ok(cert) => builder = builder.add_root_certificate(cert),
                    Err(_) => {}
                }
            }
        }

        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_url: config.api_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
            api_secret: config.api_secret.clone(),
            client,
        }
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    async fn twirp(&self, method: &str, body: Value) -> Result<Value, TwirpError> {
        self.twirp_with_grants(method, body, jwt::server_grants())
            .await
    }

    async fn twirp_with_grants(
        &self,
        method: &str,
        body: Value,
        grants: Value,
    ) -> Result<Value, TwirpError> {
        let token = jwt::sign_token(
            &self.api_key,
            &self.api_secret,
            "bn-call-server",
            grants,
            600,
        )
        .map_err(TwirpError::transport)?;
        let url = format!("{}/twirp/livekit.{}", self.api_url, method);
        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| TwirpError::transport(format!("POST {url}: {e}")))?;

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| TwirpError::transport(format!("read {url} response: {e}")))?;

        if (200..300).contains(&status) {
            if text.trim().is_empty() {
                return Ok(json!({}));
            }
            serde_json::from_str(&text).map_err(|e| TwirpError {
                status,
                code: "invalid_json".into(),
                message: format!("{e}: {}", truncate(&text, 300)),
            })
        } else {
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            Err(TwirpError {
                status,
                code: parsed
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("twirp_error")
                    .to_string(),
                message: parsed
                    .get("msg")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| truncate(&text, 300)),
            })
        }
    }

    /// 建房。房间已存在时返回 `Ok(None)`（不视为错误）。
    pub async fn create_room(
        &self,
        room: &str,
        empty_timeout_secs: u64,
        metadata: &str,
    ) -> Result<Option<Value>, TwirpError> {
        match self
            .twirp(
                "RoomService/CreateRoom",
                json!({
                    "name": room,
                    "empty_timeout": empty_timeout_secs,
                    "max_participants": 4,
                    "metadata": metadata,
                }),
            )
            .await
        {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.is_already_exists() => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn delete_room(&self, room: &str) -> Result<Value, TwirpError> {
        self.twirp("RoomService/DeleteRoom", json!({ "room": room }))
            .await
    }

    pub async fn list_rooms(&self) -> Result<Vec<Value>, TwirpError> {
        let value = self.twirp("RoomService/ListRooms", json!({})).await?;
        Ok(value
            .get("rooms")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}
