//! call-livekit-plugin — BN_agent 的 LiveKit 通话后端。
//!
//! 通过宿主 `/api/plugin/call-livekit-plugin/<action>` 暴露：
//! - `POST start`  `{userID, mediaType, identity?}` → 建房 + 参与方 token + dispatch agent
//! - `POST end`    `{room}` → 删除房间
//! - `GET  health` → LiveKit 连通性与配置状态
//! - `GET  rooms`  → 当前房间列表（调试/验收用）
//!
//! 浏览器通话页由 main-app 的 `/call` 路由提供（插件 API 只能返回纯文本 body，
//! 无法设置 `text/html`，因此页面放在 main-app 静态路由里）。

mod config;
mod jwt;
mod livekit;

pub use config::{CallConfig, PLUGIN_NAME};

use livekit::LiveKitAdmin;
use plugin_interface::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

// ── 共享状态 ────────────────────────────────────────────────────────────────

struct SharedState {
    runtime: Runtime,
    config: CallConfig,
    admin: LiveKitAdmin,
    logger: PluginLogger,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn json_resp(status: u16, value: Value) -> (u16, String) {
    (status, value.to_string())
}

fn error_resp(status: u16, message: impl Into<String>) -> (u16, String) {
    json_resp(status, json!({ "error": message.into() }))
}

// ── 请求类型 ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct StartRequest {
    #[serde(rename = "userID", alias = "user_id", alias = "userId", default)]
    user_id: String,
    #[serde(rename = "mediaType", alias = "media_type", default)]
    media_type: Option<String>,
    #[serde(default)]
    identity: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EndRequest {
    #[serde(default)]
    room: String,
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: Option<&str>) -> Result<T, (u16, String)> {
    let raw = body.unwrap_or("").trim();
    if raw.is_empty() {
        return Err(error_resp(400, "请求体不能为空，需要 JSON"));
    }
    serde_json::from_str(raw).map_err(|e| error_resp(400, format!("JSON 解析失败：{e}")))
}

/// 校验并生成 (room, identity, media_type)。抽成纯函数便于单测。
fn resolve_start(
    config: &CallConfig,
    request: &StartRequest,
    timestamp_ms: u64,
) -> Result<(String, String, String), (u16, String)> {
    let user_id = request.user_id.trim();
    if user_id.is_empty() {
        return Err(error_resp(400, "userID 不能为空"));
    }
    if user_id.chars().count() > 128 {
        return Err(error_resp(400, "userID 过长（最多 128 字符）"));
    }

    let media_type = match request.media_type.as_deref() {
        Some(raw) => config::normalize_media_type(raw)
            .ok_or_else(|| error_resp(400, "mediaType 只能是 audio 或 video"))?,
        None => config.default_media_type.clone(),
    };

    let safe_user = config::sanitize_component(user_id);
    let identity = request
        .identity
        .as_deref()
        .map(config::sanitize_component)
        .filter(|s| !s.is_empty() && s != "user")
        .unwrap_or_else(|| format!("user-{safe_user}"));

    let room = format!("{}{}-{}", config.room_prefix, safe_user, timestamp_ms);
    Ok((room, identity, media_type))
}

// ── API 实现 ────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct CallApi {
    state: Arc<Mutex<Option<Arc<SharedState>>>>,
}

impl CallApi {
    pub fn new() -> Self {
        Self::default()
    }

    fn set_state(&self, state: Option<Arc<SharedState>>) {
        *self.state.lock() = state;
    }

    fn current(&self) -> Option<Arc<SharedState>> {
        self.state.lock().clone()
    }

    fn handle_start(&self, state: &SharedState, body: Option<&str>) -> (u16, String) {
        let request: StartRequest = match parse_body(body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        let (room, identity, media_type) = match resolve_start(&state.config, &request, now_ms()) {
            Ok(v) => v,
            Err(resp) => return resp,
        };

        let room_metadata = json!({
            "userID": request.user_id.trim(),
            "identity": identity.clone(),
            "mediaType": media_type.clone(),
        })
        .to_string();

        match state.runtime.block_on(state.admin.create_room(
            &room,
            state.config.empty_timeout_secs,
            &room_metadata,
        )) {
            Ok(_) => {}
            Err(e) => {
                state
                    .logger
                    .error(format!("创建房间 {room} 失败：{e}"));
                return error_resp(502, format!("创建房间失败：{e}"));
            }
        }

        let token = match jwt::sign_token(
            &state.config.api_key,
            &state.config.api_secret,
            &identity,
            jwt::participant_grants(&room),
            state.config.token_ttl_secs,
        ) {
            Ok(token) => token,
            Err(e) => {
                state.logger.error(format!("签发 token 失败：{e}"));
                return error_resp(500, format!("签发 token 失败：{e}"));
            }
        };

        let mut agent_info = json!({
            "name": state.config.agent_name,
            "dispatched": false,
        });
        if !state.config.agent_name.is_empty() {
            let dispatch_metadata = json!({
                "userID": request.user_id.trim(),
                "identity": identity,
                "mediaType": media_type,
                "room": room,
                "source": "bn-call",
            })
            .to_string();
            match state.runtime.block_on(state.admin.create_dispatch(
                &room,
                &state.config.agent_name,
                &dispatch_metadata,
            )) {
                Ok(dispatch) => {
                    state.logger.info(format!(
                        "agent {} 已派发到房间 {} (dispatch={})",
                        state.config.agent_name,
                        room,
                        dispatch
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                    ));
                    agent_info["dispatched"] = json!(true);
                    if let Some(id) = dispatch.get("id") {
                        agent_info["dispatchId"] = id.clone();
                    }
                }
                Err(e) => {
                    // agent worker 尚未上线（call-agent 任务并行开发中）时不阻断通话。
                    state.logger.warn(format!(
                        "agent {} 派发失败（房间 {} 仍可用）：{}",
                        state.config.agent_name, room, e
                    ));
                    agent_info["warning"] = json!(format!(
                        "agent 派发失败：{e}（call-agent 可能尚未启动）"
                    ));
                }
            }
        }

        json_resp(
            200,
            json!({
                "url": state.config.livekit_url,
                "urlLocal": state.config.livekit_url_local,
                "token": token,
                "room": room,
                "identity": identity,
                "mediaType": media_type,
                "agent": agent_info,
                "iceServers": state.config.ice_server_objects(),
                "expiresInSecs": state.config.token_ttl_secs,
            }),
        )
    }

    fn handle_end(&self, state: &SharedState, body: Option<&str>) -> (u16, String) {
        let request: EndRequest = match parse_body(body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        let room = request.room.trim();
        if room.is_empty() {
            return error_resp(400, "room 不能为空");
        }

        match state.runtime.block_on(state.admin.delete_room(room)) {
            Ok(_) => json_resp(
                200,
                json!({ "status": "deleted", "room": room, "deleted": true }),
            ),
            Err(e) if e.is_not_found() => {
                state.logger.info(format!("删除房间 {room}：房间不存在，按已删除处理"));
                json_resp(
                    200,
                    json!({ "status": "ok", "room": room, "deleted": false }),
                )
            }
            Err(e) => {
                state.logger.error(format!("删除房间 {room} 失败：{e}"));
                error_resp(502, format!("删除房间失败：{e}"))
            }
        }
    }

    fn handle_health(&self, state: &SharedState) -> (u16, String) {
        match state.runtime.block_on(state.admin.list_rooms()) {
            Ok(rooms) => json_resp(
                200,
                json!({
                    "status": "ok",
                    "livekit": true,
                    "apiUrl": state.admin.api_url(),
                    "url": state.config.livekit_url,
                    "urlLocal": state.config.livekit_url_local,
                    "agentName": state.config.agent_name,
                    "configured": state.config.credentials_ok(),
                    "rooms": rooms.len(),
                }),
            ),
            Err(e) => {
                state.logger.warn(format!("LiveKit 健康检查失败：{e}"));
                json_resp(
                    503,
                    json!({
                        "status": "error",
                        "livekit": false,
                        "apiUrl": state.admin.api_url(),
                        "url": state.config.livekit_url,
                        "agentName": state.config.agent_name,
                        "configured": state.config.credentials_ok(),
                        "error": e.to_string(),
                    }),
                )
            }
        }
    }

    fn handle_rooms(&self, state: &SharedState) -> (u16, String) {
        match state.runtime.block_on(state.admin.list_rooms()) {
            Ok(rooms) => {
                let rooms: Vec<Value> = rooms
                    .iter()
                    .map(|room| {
                        json!({
                            "name": room.get("name").cloned().unwrap_or(Value::Null),
                            "sid": room.get("sid").cloned().unwrap_or(Value::Null),
                            "numParticipants": room
                                .get("numParticipants")
                                .or_else(|| room.get("num_participants"))
                                .cloned()
                                .unwrap_or(json!(0)),
                            "creationTime": room
                                .get("creationTime")
                                .or_else(|| room.get("creation_time"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        })
                    })
                    .collect();
                json_resp(200, json!({ "rooms": rooms }))
            }
            Err(e) => error_resp(502, format!("获取房间列表失败：{e}")),
        }
    }
}

impl PluginApi for CallApi {
    fn handle_api(&self, method: &str, path: &str, body: Option<&str>) -> Option<(u16, String)> {
        let action = path.split('?').next().unwrap_or("").trim_matches('/');
        let Some(state) = self.current() else {
            return Some(error_resp(503, "call 插件尚未启动（配置加载失败或未加载）"));
        };

        match (method, action) {
            ("POST", "start") => Some(self.handle_start(&state, body)),
            ("POST", "end") => Some(self.handle_end(&state, body)),
            ("GET", "health") | ("GET", "") => Some(self.handle_health(&state)),
            ("GET", "rooms") => Some(self.handle_rooms(&state)),
            ("OPTIONS", _) => Some((204, String::new())),
            (m, a) => Some(error_resp(
                404,
                format!("未知接口：{m} /{a}（可用：POST start、POST end、GET health、GET rooms）"),
            )),
        }
    }
}

// ── 插件主体 ────────────────────────────────────────────────────────────────

pub struct CallLivekitPlugin {
    info: PluginInfo,
    api: CallApi,
}

impl CallLivekitPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: PLUGIN_NAME.into(),
                version: "0.1.0".into(),
                description: "LiveKit 通话后端：创建房间、签发 token、dispatch agent，提供浏览器通话页所需接口".into(),
                author: "BN_agent".into(),
                min_host_version: "0.1.0".into(),
            },
            api: CallApi::new(),
        }
    }
}

impl Plugin for CallLivekitPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let config = CallConfig::from_json(&ctx.config);
        ctx.logger.info(format!(
            "call-livekit-plugin 启动：url={} api={} agent={} prefix={} media={}",
            config.livekit_url,
            config.api_url,
            config.agent_name,
            config.room_prefix,
            config.default_media_type
        ));
        if !config.credentials_ok() {
            ctx.logger.warn(
                "LiveKit api_key/api_secret 为空：建房与 health 会失败，请在 WebUI 配置或设置 LIVEKIT_API_KEY / LIVEKIT_API_SECRET",
            );
        }

        let runtime = Runtime::new()
            .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;
        let admin = LiveKitAdmin::new(&config);
        self.api.set_state(Some(Arc::new(SharedState {
            runtime,
            config,
            admin,
            logger: ctx.logger.clone(),
        })));
        ctx.logger.info(
            "call-livekit-plugin 启动完成 — API: POST /api/call/start, POST /api/call/end, GET /api/call/health",
        );
        Ok(())
    }

    fn stop(&mut self) {
        self.api.set_state(None);
    }

    fn api_handler(&self) -> Option<&dyn PluginApi> {
        Some(&self.api)
    }

    fn config_schema(&self) -> Option<PluginConfigSchema> {
        serde_json::from_str::<PluginConfigSchema>(include_str!("../config_schema.json")).ok()
    }
}

// ── FFI ─────────────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(CallLivekitPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}

// ── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_interface::config::merge_with_defaults;

    fn config() -> CallConfig {
        CallConfig::from_json(&json!({
            "api_key": "key",
            "api_secret": "secret",
            "room_prefix": "ai-call-",
            "default_media_type": "audio",
        }))
    }

    #[test]
    fn schema_parses_and_covers_runtime_fields() {
        let schema = CallLivekitPlugin::new()
            .config_schema()
            .expect("schema should parse");
        assert_eq!(schema.version, 1);
        for name in [
            "livekit_url",
            "livekit_url_local",
            "api_url",
            "api_key",
            "api_secret",
            "api_ca_cert",
            "agent_name",
            "room_prefix",
            "default_media_type",
            "ice_servers",
            "turn_username",
            "turn_password",
            "empty_timeout_secs",
            "token_ttl_secs",
        ] {
            assert!(schema.fields.contains_key(name), "missing field {name}");
        }
        assert!(schema.fields["api_key"].secret);
        assert!(schema.fields["api_secret"].secret);
        assert!(schema.fields["turn_password"].secret);
        assert!(!schema.fields["agent_name"].secret);
    }

    #[test]
    fn schema_defaults_match_runtime_defaults() {
        let schema = CallLivekitPlugin::new().config_schema().expect("schema");
        let merged = merge_with_defaults(&schema, &json!({}));
        let from_schema = CallConfig::from_json(&merged);
        assert_eq!(from_schema, CallConfig::default());
    }

    #[test]
    fn resolve_start_builds_room_identity_and_media() {
        let cfg = config();
        let (room, identity, media) = resolve_start(
            &cfg,
            &StartRequest {
                user_id: "alice-01".into(),
                media_type: Some("video".into()),
                identity: None,
            },
            1_700_000_000_123,
        )
        .unwrap();
        assert_eq!(room, "ai-call-alice-01-1700000000123");
        assert_eq!(identity, "user-alice-01");
        assert_eq!(media, "video");
    }

    #[test]
    fn resolve_start_defaults_media_type_and_custom_identity() {
        let cfg = config();
        let (_, identity, media) = resolve_start(
            &cfg,
            &StartRequest {
                user_id: "bob".into(),
                media_type: None,
                identity: Some("phone-a".into()),
            },
            1,
        )
        .unwrap();
        assert_eq!(identity, "phone-a");
        assert_eq!(media, "audio");
    }

    #[test]
    fn resolve_start_rejects_bad_input() {
        let cfg = config();
        let cases = [
            ("", None),
            ("   ", None),
            ("alice", Some("screen")),
        ];
        for (user_id, media) in cases {
            let result = resolve_start(
                &cfg,
                &StartRequest {
                    user_id: user_id.into(),
                    media_type: media.map(str::to_string),
                    identity: None,
                },
                1,
            );
            assert!(result.is_err(), "user_id={user_id:?}");
            assert_eq!(result.unwrap_err().0, 400);
        }
    }

    #[test]
    fn api_without_state_returns_503() {
        let api = CallApi::new();
        let (status, body) = api.handle_api("GET", "health", None).unwrap();
        assert_eq!(status, 503);
        assert!(body.contains("尚未启动"));
    }

    #[test]
    fn validation_errors_do_not_touch_network() {
        // 在独立 actix system 上创建 EventBus，拿到 PluginLogger 后 system 即可退出；
        // 校验路径不会写日志，logger 只需存在。
        let logger = {
            let system = actix_rt::System::new();
            system.block_on(async { PluginLogger::new(EventBus::new().start(), "test".into()) })
        };
        let runtime = Runtime::new().unwrap();
        let cfg = CallConfig::from_json(&json!({ "api_key": "k", "api_secret": "s" }));
        let api = CallApi::new();
        api.set_state(Some(Arc::new(SharedState {
            runtime,
            admin: LiveKitAdmin::new(&cfg),
            config: cfg,
            logger,
        })));

        let (status, _) = api.handle_api("POST", "end", Some("{}")).unwrap();
        assert_eq!(status, 400);
        let (status, _) = api.handle_api("POST", "start", Some("{}")).unwrap();
        assert_eq!(status, 400);
        let (status, _) = api.handle_api("POST", "start", Some("not-json")).unwrap();
        assert_eq!(status, 400);
        let (status, _) = api.handle_api("GET", "nope", None).unwrap();
        assert_eq!(status, 404);
    }
}
