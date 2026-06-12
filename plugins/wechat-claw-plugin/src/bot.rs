//! 微信 ClawBot / iLink API 客户端 — actor-free port.

use plugin_interface::*;

pub struct BotHandle {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
}

impl BotHandle {
    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let mut req = self.client.post(format!("{}/api/message/send", self.api_url))
            .json(&serde_json::json!({"to": chat_id, "type": "text", "content": text}));
        if !self.api_key.is_empty() { req = req.header("Authorization", format!("Bearer {}", self.api_key)); }
        let resp = req.send().await.map_err(|e| format!("send: {}", e))?;
        if !resp.status().is_success() { return Err(format!("status {}", resp.status())); }
        Ok(())
    }

    pub async fn send_voice(&self, chat_id: &str, audio_b64: &str) -> Result<(), String> {
        let mut req = self.client.post(format!("{}/api/message/send", self.api_url))
            .json(&serde_json::json!({"to": chat_id, "type": "voice", "content": audio_b64}));
        if !self.api_key.is_empty() { req = req.header("Authorization", format!("Bearer {}", self.api_key)); }
        let resp = req.send().await.map_err(|e| format!("voice: {}", e))?;
        if !resp.status().is_success() { return Err(format!("status {}", resp.status())); }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn send_image(&self, chat_id: &str, image_b64: &str) -> Result<(), String> {
        let mut req = self.client.post(format!("{}/api/message/send", self.api_url))
            .json(&serde_json::json!({"to": chat_id, "type": "image", "content": image_b64}));
        if !self.api_key.is_empty() { req = req.header("Authorization", format!("Bearer {}", self.api_key)); }
        let resp = req.send().await.map_err(|e| format!("image: {}", e))?;
        if !resp.status().is_success() { return Err(format!("status {}", resp.status())); }
        Ok(())
    }
}

async fn poll_messages(client: &reqwest::Client, api_url: &str, api_key: &str) -> Result<Vec<serde_json::Value>, String> {
    let mut req = client.get(format!("{}/api/message/poll", api_url));
    if !api_key.is_empty() { req = req.header("Authorization", format!("Bearer {}", api_key)); }
    let resp = req.send().await.map_err(|e| format!("poll: {}", e))?;
    if !resp.status().is_success() { return Err(format!("poll status {}", resp.status())); }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("poll parse: {}", e))?;
    Ok(body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default())
}

fn process_message(msg: &serde_json::Value, eb: &Addr<EventBus>) {
    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("text");
    let chat_id = msg.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sender = msg.get("sender").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if content.is_empty() { return; }

    match msg_type {
        "text" => {
            log::info!("[wechat-claw] text from {}: {}", sender, &content[..content.len().min(60)]);
            eb.do_send(Event::new(
                "user.message",
                serde_json::json!({"platform": "wechat", "chat_id": chat_id, "user_name": sender, "text": format!("[微信]: {}", content)}),
                "wechat-claw-plugin",
            ));
        }
        "voice" => {
            log::info!("[wechat-claw] voice from {}: {}b", sender, content.len());
            eb.do_send(Event::new(
                "audio_captured",
                serde_json::json!({"peer_id": chat_id, "data": content, "source": "wechat"}),
                "wechat-claw-plugin",
            ));
        }
        _ => log::debug!("[wechat-claw] ignore type: {}", msg_type),
    }
}

pub async fn run_bot(api_url: &str, api_key: &str, eb: Addr<EventBus>) -> Result<BotHandle, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build().map_err(|e| format!("client: {}", e))?;

    let handle = BotHandle {
        client: client.clone(),
        api_url: api_url.to_string(),
        api_key: api_key.to_string(),
    };

    match poll_messages(&client, api_url, api_key).await {
        Ok(_) => log::info!("[wechat-claw] API ok: {}", api_url),
        Err(e) => log::warn!("[wechat-claw] API test failed (will retry): {}", e),
    }

    let pc = client.clone();
    let pu = api_url.to_string();
    let pk = api_key.to_string();

    tokio::spawn(async move {
        loop {
            match poll_messages(&pc, &pu, &pk).await {
                Ok(msgs) => { for m in msgs { process_message(&m, &eb); } }
                Err(e) => log::debug!("[wechat-claw] poll: {}", e),
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    });

    Ok(handle)
}
