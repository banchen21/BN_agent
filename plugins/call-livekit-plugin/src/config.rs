//! 插件配置：优先读 WebUI 注入的 `ctx.config`，缺省回退环境变量，最后使用内置默认值。
//!
//! 字段名与 `config_schema.json` 严格一致，WebUI 保存后宿主会重载插件，
//! `start()` 重新读取即生效。

use serde_json::Value;

pub const PLUGIN_NAME: &str = "call-livekit-plugin";

pub const DEFAULT_LIVEKIT_URL: &str = "wss://192.168.0.101:7443";
pub const DEFAULT_LIVEKIT_URL_LOCAL: &str = "wss://127.0.0.1:7443";
pub const DEFAULT_API_URL: &str = "http://127.0.0.1:7880";
pub const DEFAULT_AGENT_NAME: &str = "yueyue";
pub const DEFAULT_ROOM_PREFIX: &str = "ai-call-";
pub const DEFAULT_MEDIA_TYPE: &str = "audio";
pub const DEFAULT_SIGNALING_URL: &str = "ws://127.0.0.1:7880";
pub const DEFAULT_ASR_URL: &str = "http://127.0.0.1:8877/api/asr";
pub const DEFAULT_TTS_URL: &str = "http://127.0.0.1:8877/api/tts";
pub const DEFAULT_BACKEND_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Clone, PartialEq)]
pub struct CallConfig {
    /// 浏览器/手机使用的 WS 地址（局域网 IP）。
    pub livekit_url: String,
    /// 本机浏览器使用的 WS 地址。
    pub livekit_url_local: String,
    /// 服务端 Twirp API 地址（http/https）。
    pub api_url: String,
    pub api_key: String,
    pub api_secret: String,
    /// 服务端 Twirp 走 https 时用于校验的 CA 证书路径（空 = 使用系统根证书）。
    pub api_ca_cert: String,
    pub agent_name: String,
    pub room_prefix: String,
    pub default_media_type: String,
    pub ice_servers: Vec<String>,
    pub turn_username: String,
    pub turn_password: String,
    pub empty_timeout_secs: u64,
    pub token_ttl_secs: u64,
    /// 插件是否以 agent 身份进房接话（默认开）。
    pub backend_agent_enabled: bool,
    /// agent 进房用的信令地址（本机直连，不走代理）。
    pub signaling_url: String,
    /// yueyue ASR / TTS 地址。
    pub asr_url: String,
    pub tts_url: String,
    /// 主后端地址（派生 /api/chat、/api/chat/append、/api/tools/call）。
    pub backend_url: String,
    /// 主后端的 API_KEY（为空则不带鉴权头）。
    pub backend_api_key: String,
}

impl Default for CallConfig {
    fn default() -> Self {
        Self {
            livekit_url: DEFAULT_LIVEKIT_URL.to_string(),
            livekit_url_local: DEFAULT_LIVEKIT_URL_LOCAL.to_string(),
            api_url: DEFAULT_API_URL.to_string(),
            api_key: String::new(),
            api_secret: String::new(),
            api_ca_cert: String::new(),
            agent_name: DEFAULT_AGENT_NAME.to_string(),
            room_prefix: DEFAULT_ROOM_PREFIX.to_string(),
            default_media_type: DEFAULT_MEDIA_TYPE.to_string(),
            ice_servers: Vec::new(),
            turn_username: String::new(),
            turn_password: String::new(),
            empty_timeout_secs: 300,
            token_ttl_secs: 3600,
            backend_agent_enabled: true,
            signaling_url: DEFAULT_SIGNALING_URL.to_string(),
            asr_url: DEFAULT_ASR_URL.to_string(),
            tts_url: DEFAULT_TTS_URL.to_string(),
            backend_url: DEFAULT_BACKEND_URL.to_string(),
            backend_api_key: String::new(),
        }
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn u64_field(value: &Value, name: &str) -> Option<u64> {
    value.get(name).and_then(Value::as_u64)
}

fn bool_field(value: &Value, name: &str) -> Option<bool> {
    value.get(name).and_then(Value::as_bool)
}

/// 布尔配置：WebUI 值 > 环境变量（1/true/0/false）> 默认值。
fn bool_or(value: &Value, name: &str, env: &str, default: bool) -> bool {
    if let Some(v) = bool_field(value, name) {
        return v;
    }
    match env_nonempty(env).map(|v| v.to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => false,
        _ => default,
    }
}

fn trim_url(url: String) -> String {
    url.trim_end_matches('/').to_string()
}

fn read_or(value: &Value, name: &str, env: &str, default: &str) -> String {
    string_field(value, name)
        .or_else(|| env_nonempty(env))
        .unwrap_or_else(|| default.to_string())
}

/// 把 ws:// / wss:// 地址转成 http:// / https://，用于服务端 Twirp 调用。
pub fn derive_api_url(livekit_url: &str) -> String {
    let url = livekit_url.trim();
    if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("http://{url}")
    }
}

impl CallConfig {
    pub fn from_json(value: &Value) -> Self {
        let mut config = Self::default();

        config.livekit_url = read_or(value, "livekit_url", "LIVEKIT_URL", DEFAULT_LIVEKIT_URL);
        config.livekit_url_local = read_or(
            value,
            "livekit_url_local",
            "LIVEKIT_URL_LOCAL",
            DEFAULT_LIVEKIT_URL_LOCAL,
        );
        config.api_url = string_field(value, "api_url")
            .or_else(|| env_nonempty("LIVEKIT_API_URL"))
            .unwrap_or_else(|| derive_api_url(&config.livekit_url));

        config.api_key = string_field(value, "api_key")
            .or_else(|| env_nonempty("LIVEKIT_API_KEY"))
            .unwrap_or_default();
        config.api_secret = string_field(value, "api_secret")
            .or_else(|| env_nonempty("LIVEKIT_API_SECRET"))
            .unwrap_or_default();
        config.api_ca_cert = string_field(value, "api_ca_cert")
            .or_else(|| env_nonempty("LIVEKIT_API_CA_CERT"))
            .unwrap_or_default();

        config.agent_name = read_or(
            value,
            "agent_name",
            "LIVEKIT_AGENT_NAME",
            DEFAULT_AGENT_NAME,
        );
        config.room_prefix = read_or(
            value,
            "room_prefix",
            "LIVEKIT_ROOM_PREFIX",
            DEFAULT_ROOM_PREFIX,
        );
        config.default_media_type = normalize_media_type(&read_or(
            value,
            "default_media_type",
            "LIVEKIT_DEFAULT_MEDIA_TYPE",
            DEFAULT_MEDIA_TYPE,
        ))
        .unwrap_or_else(|| DEFAULT_MEDIA_TYPE.to_string());

        config.ice_servers = value
            .get("ice_servers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        config.turn_username = string_field(value, "turn_username").unwrap_or_default();
        config.turn_password = string_field(value, "turn_password").unwrap_or_default();

        config.empty_timeout_secs = u64_field(value, "empty_timeout_secs")
            .unwrap_or(300)
            .clamp(10, 86400);
        config.token_ttl_secs = u64_field(value, "token_ttl_secs")
            .unwrap_or(3600)
            .clamp(60, 86400);

        config.backend_agent_enabled = bool_or(
            value,
            "backend_agent_enabled",
            "LIVEKIT_BACKEND_AGENT_ENABLED",
            true,
        );
        config.signaling_url = trim_url(read_or(
            value,
            "signaling_url",
            "LIVEKIT_SIGNALING_URL",
            DEFAULT_SIGNALING_URL,
        ));
        config.asr_url = read_or(value, "asr_url", "VOICE_ASR_URL", DEFAULT_ASR_URL);
        config.tts_url = read_or(value, "tts_url", "VOICE_TTS_URL", DEFAULT_TTS_URL);
        config.backend_url = trim_url(read_or(
            value,
            "backend_url",
            "BN_BACKEND_URL",
            DEFAULT_BACKEND_URL,
        ));
        config.backend_api_key = string_field(value, "backend_api_key")
            .or_else(|| env_nonempty("BN_BACKEND_API_KEY"))
            .unwrap_or_default();

        config
    }

    /// key/secret 是否已配置。
    pub fn credentials_ok(&self) -> bool {
        !self.api_key.is_empty() && !self.api_secret.is_empty()
    }

    /// 浏览器端 RTCIceServer 数组。
    pub fn ice_server_objects(&self) -> Vec<Value> {
        self.ice_servers
            .iter()
            .map(|url| {
                let is_turn = url.starts_with("turn:") || url.starts_with("turns:");
                if is_turn && (!self.turn_username.is_empty() || !self.turn_password.is_empty()) {
                    serde_json::json!({
                        "urls": [url],
                        "username": self.turn_username,
                        "credential": self.turn_password,
                    })
                } else {
                    serde_json::json!({ "urls": [url] })
                }
            })
            .collect()
    }
}

/// 只接受 audio / video（大小写与常见别名兼容）。
pub fn normalize_media_type(raw: &str) -> Option<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "audio" | "voice" | "mic" => Some("audio".into()),
        "video" | "camera" => Some("video".into()),
        _ => None,
    }
}

/// 把 userID 清洗成可安全用于房间名/identity 的片段。
/// 保留 ASCII 字母数字、`-`、`_`、`.`；其他字符（含中文、空格）替换为 `_`。
pub fn sanitize_component(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "user".to_string()
    } else {
        trimmed.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_schema_defaults() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../config_schema.json")).unwrap();
        let config = CallConfig::from_json(&json!({}));
        assert_eq!(
            config.livekit_url,
            schema["fields"]["livekit_url"]["default"]
        );
        assert_eq!(config.agent_name, schema["fields"]["agent_name"]["default"]);
        assert_eq!(
            config.room_prefix,
            schema["fields"]["room_prefix"]["default"]
        );
        assert_eq!(config.default_media_type, "audio");
        assert_eq!(config.empty_timeout_secs, 300);
        assert_eq!(config.token_ttl_secs, 3600);
        assert!(!config.credentials_ok());
    }

    #[test]
    fn config_reads_fields_and_derives_api_url() {
        let config = CallConfig::from_json(&json!({
            "livekit_url": "wss://lk.example.com",
            "api_key": "key",
            "api_secret": "secret",
            "agent_name": "yueyue",
            "room_prefix": "test-",
            "ice_servers": ["stun:stun.example.com:3478"],
            "turn_username": "u",
            "turn_password": "p",
            "empty_timeout_secs": 60,
            "token_ttl_secs": 120
        }));
        assert_eq!(config.api_url, "https://lk.example.com");
        assert!(config.credentials_ok());
        assert_eq!(config.empty_timeout_secs, 60);
        assert_eq!(config.token_ttl_secs, 120);
        let ice = config.ice_server_objects();
        assert_eq!(ice[0]["urls"][0], "stun:stun.example.com:3478");
    }

    #[test]
    fn turn_urls_carry_credentials() {
        let config = CallConfig::from_json(&json!({
            "ice_servers": ["turn:turn.example.com:3478?transport=udp"],
            "turn_username": "user",
            "turn_password": "pass"
        }));
        let ice = config.ice_server_objects();
        assert_eq!(ice[0]["username"], "user");
        assert_eq!(ice[0]["credential"], "pass");
    }

    #[test]
    fn media_type_aliases_and_invalid() {
        assert_eq!(normalize_media_type("AUDIO").as_deref(), Some("audio"));
        assert_eq!(normalize_media_type("video").as_deref(), Some("video"));
        assert_eq!(normalize_media_type("camera").as_deref(), Some("video"));
        assert_eq!(normalize_media_type("screen"), None);
    }

    #[test]
    fn sanitize_component_handles_unicode_and_empty() {
        assert_eq!(sanitize_component("alice-01"), "alice-01");
        assert_eq!(sanitize_component("小悦"), "user");
        assert_eq!(sanitize_component(" hello world "), "hello_world");
        assert_eq!(sanitize_component(""), "user");
    }

    #[test]
    fn derive_api_url_from_various_schemes() {
        assert_eq!(
            derive_api_url("ws://127.0.0.1:7880"),
            "http://127.0.0.1:7880"
        );
        assert_eq!(
            derive_api_url("wss://lk.example.com"),
            "https://lk.example.com"
        );
        assert_eq!(derive_api_url("http://x:1"), "http://x:1");
        assert_eq!(derive_api_url("127.0.0.1:7880"), "http://127.0.0.1:7880");
    }
}
