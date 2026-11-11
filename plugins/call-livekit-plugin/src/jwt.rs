//! LiveKit JWT（HS256）签发。
//!
//! LiveKit 校验 `iss` == API Key，签名使用 API Secret；`video` claim 携带授权。
//! 这里不引入 jsonwebtoken，直接用 hmac + sha2 + base64url 实现，减少插件依赖。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// 签发一个 LiveKit token。
pub fn sign_token(
    api_key: &str,
    api_secret: &str,
    identity: &str,
    grants: Value,
    ttl_secs: u64,
) -> Result<String, String> {
    if api_key.is_empty() || api_secret.is_empty() {
        return Err("LiveKit api_key/api_secret 未配置".into());
    }
    let now = now_secs();
    let header = json!({ "alg": "HS256", "typ": "JWT" });
    let payload = json!({
        "iss": api_key,
        "sub": identity,
        "nbf": now.saturating_sub(10),
        "exp": now.saturating_add(ttl_secs.max(60)),
        "video": grants,
    });

    let header_b64 = b64url(header.to_string().as_bytes());
    let payload_b64 = b64url(payload.to_string().as_bytes());
    let signing_input = format!("{header_b64}.{payload_b64}");

    let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
        .map_err(|e| format!("HMAC key error: {e}"))?;
    mac.update(signing_input.as_bytes());
    let signature = b64url(&mac.finalize().into_bytes());

    Ok(format!("{signing_input}.{signature}"))
}

/// 服务端管理 token：建房 / 列房 / 删房。
pub fn server_grants() -> Value {
    json!({
        "roomCreate": true,
        "roomList": true,
        "roomAdmin": true,
    })
}

/// dispatch 专用 token：AgentDispatchService 要求 **房间级** roomAdmin
/// （`roomAdmin: true` 必须与 `room: <房间名>` 同时出现，否则 401）。
pub fn dispatch_grants(room: &str) -> Value {
    json!({
        "roomAdmin": true,
        "room": room,
    })
}

/// 参与方 token：只能进指定房间，可发布/订阅音视频与 data。
pub fn participant_grants(room: &str) -> Value {
    json!({
        "roomJoin": true,
        "room": room,
        "canPublish": true,
        "canSubscribe": true,
        "canPublishData": true,
        "canUpdateOwnMetadata": true,
        "hidden": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_segment(segment: &str) -> Value {
        let bytes = URL_SAFE_NO_PAD.decode(segment.as_bytes()).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn verify(token: &str, secret: &str) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        let expected = b64url(&mac.finalize().into_bytes());
        expected == parts[2]
    }

    #[test]
    fn token_has_livekit_claims_and_valid_signature() {
        let token = sign_token("key123", "secret456", "user-alice", participant_grants("room-a"), 600)
            .unwrap();
        assert!(verify(&token, "secret456"));
        assert!(!verify(&token, "wrong-secret"));

        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(decode_segment(parts[0])["alg"], "HS256");
        let payload = decode_segment(parts[1]);
        assert_eq!(payload["iss"], "key123");
        assert_eq!(payload["sub"], "user-alice");
        assert_eq!(payload["video"]["room"], "room-a");
        assert_eq!(payload["video"]["roomJoin"], true);
        assert_eq!(payload["video"]["canPublish"], true);
        assert!(payload["exp"].as_u64().unwrap() > payload["nbf"].as_u64().unwrap());
    }

    #[test]
    fn empty_credentials_rejected() {
        assert!(sign_token("", "secret", "u", json!({}), 60).is_err());
        assert!(sign_token("key", "", "u", json!({}), 60).is_err());
    }

    #[test]
    fn server_grants_cover_admin_ops() {
        let grants = server_grants();
        assert_eq!(grants["roomCreate"], true);
        assert_eq!(grants["roomList"], true);
        assert_eq!(grants["roomAdmin"], true);
    }

    #[test]
    fn dispatch_grants_are_room_scoped() {
        let grants = dispatch_grants("ai-call-x-1");
        assert_eq!(grants["roomAdmin"], true);
        assert_eq!(grants["room"], "ai-call-x-1");
    }
}
