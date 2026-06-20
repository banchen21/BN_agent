//! 飞书 Bot 核心逻辑 — actor-free port.
//!
//! Uses Feishu Open API v2: tenant_access_token + IM message polling/sending.

use plugin_interface::*;

const API_BASE: &str = "https://open.feishu.cn/open-apis";

pub struct BotHandle {
    client: reqwest::Client,
    app_id: String,
    app_secret: String,
    token: tokio::sync::Mutex<Option<String>>,
}

impl BotHandle {
    async fn get_token(&self) -> Result<String, String> {
        let mut guard = self.token.lock().await;
        if let Some(ref t) = *guard {
            return Ok(t.clone());
        }

        let resp = self
            .client
            .post(format!("{}/auth/v3/tenant_access_token/internal", API_BASE))
            .json(&serde_json::json!({"app_id": self.app_id, "app_secret": self.app_secret}))
            .send()
            .await
            .map_err(|e| format!("token req: {}", e))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("token parse: {}", e))?;
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(format!(
                "token error (code={}): {}",
                code,
                body.get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            ));
        }
        let token = body["tenant_access_token"]
            .as_str()
            .ok_or("no token")?
            .to_string();
        *guard = Some(token.clone());
        Ok(token)
    }

    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let token = self.get_token().await?;
        let resp = self
            .client
            .post(format!("{}/im/v1/messages", API_BASE))
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("receive_id_type", "chat_id")])
            .json(&serde_json::json!({
                "receive_id": chat_id,
                "msg_type": "text",
                "content": serde_json::json!({"text": text}).to_string(),
            }))
            .send()
            .await
            .map_err(|e| format!("send req: {}", e))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("send parse: {}", e))?;
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            Err(format!(
                "send error (code={}): {}",
                code,
                body.get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            ))
        } else {
            Ok(())
        }
    }

    #[allow(dead_code)]
    pub async fn send_card(&self, chat_id: &str, card: &serde_json::Value) -> Result<(), String> {
        let token = self.get_token().await?;
        let resp = self
            .client
            .post(format!("{}/im/v1/messages", API_BASE))
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("receive_id_type", "chat_id")])
            .json(&serde_json::json!({
                "receive_id": chat_id,
                "msg_type": "interactive",
                "content": card.to_string(),
            }))
            .send()
            .await
            .map_err(|e| format!("card req: {}", e))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("card parse: {}", e))?;
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            Err(format!(
                "card error (code={}): {}",
                code,
                body.get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            ))
        } else {
            Ok(())
        }
    }
}

async fn poll_messages(
    client: &reqwest::Client,
    token: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let resp = client
        .get(format!("{}/im/v1/messages", API_BASE))
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("container_id_type", "chat"), ("page_size", "20")])
        .send()
        .await
        .map_err(|e| format!("poll: {}", e))?;
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("poll parse: {}", e))?;
    Ok(body
        .get("data")
        .and_then(|d| d.get("items"))
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default())
}

fn process_message(msg: &serde_json::Value, eb: &Addr<EventBus>) {
    let msg_type = msg.get("msg_type").and_then(|v| v.as_str()).unwrap_or("");
    if msg_type != "text" {
        return;
    }

    let chat_id = msg
        .get("chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sender = msg
        .get("sender")
        .and_then(|s| s.get("sender_id"))
        .and_then(|id| id.get("open_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let content = msg
        .get("body")
        .and_then(|b| b.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|j| j.get("text").and_then(|v| v.as_str().map(String::from)))
        .unwrap_or_else(|| content.to_string());

    if text.is_empty() {
        return;
    }
    log::info!(
        "[feishu-im] msg from {}: {}",
        sender,
        &text[..text.len().min(60)]
    );

    eb.do_send(Event::new(
        "user.message",
        serde_json::json!({
            "platform": "feishu",
            "source": "feishu",
            "peer_id": format!("feishu:{}", chat_id),
            "chat_id": chat_id,
            "user_name": sender,
            "text": text
        }),
        "feishu-im-plugin",
    ));
}

pub async fn run_bot(
    app_id: &str,
    app_secret: &str,
    eb: Addr<EventBus>,
) -> Result<BotHandle, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("client: {}", e))?;

    let handle = BotHandle {
        client: client.clone(),
        app_id: app_id.to_string(),
        app_secret: app_secret.to_string(),
        token: tokio::sync::Mutex::new(None),
    };
    handle
        .get_token()
        .await
        .map_err(|e| format!("auth failed: {}", e))?;
    log::info!("[feishu-im] API auth ok");

    let poll_client = client.clone();
    let poll_id = app_id.to_string();
    let poll_secret = app_secret.to_string();

    tokio::spawn(async move {
        let poll_handle = BotHandle {
            client: poll_client.clone(),
            app_id: poll_id,
            app_secret: poll_secret,
            token: tokio::sync::Mutex::new(None),
        };
        loop {
            match poll_handle.get_token().await {
                Ok(token) => match poll_messages(&poll_handle.client, &token).await {
                    Ok(msgs) => {
                        for m in msgs {
                            process_message(&m, &eb);
                        }
                    }
                    Err(e) => {
                        log::debug!("[feishu-im] poll: {}", e);
                        *poll_handle.token.lock().await = None;
                    }
                },
                Err(e) => log::warn!("[feishu-im] token: {}", e),
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    });

    Ok(handle)
}
