//! WebRTC Plugin — 实时音视频通信插件
//!
//! 提供 WebRTC 点对点连接能力：
//! - 创建/管理 PeerConnection
//! - 信令交换（通过事件总线）
//! - 数据通道（DataChannel）支持文本消息
//! - 注册为宿主工具，供 LLM 调用

use plugin_core::{
    AgentEvent, EventType, HostContext, Plugin, PluginError, PluginMeta,
    ToolDef, ToolExecutor, ToolResult,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

mod peer;
mod signaling;

pub struct WebrtcPlugin {
    meta: PluginMeta,
    ctx: Option<HostContext>,
    /// peer_id → PeerConnection 句柄
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    /// 信令处理器
    signaling: Option<signaling::SignalingHandler>,
}

impl WebrtcPlugin {
    pub fn new() -> Self {
        Self {
            meta: PluginMeta {
                name: "webrtc-plugin".into(),
                version: "0.1.0".into(),
                description: "WebRTC 实时音视频通信插件".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            ctx: None,
            peers: Arc::new(Mutex::new(HashMap::new())),
            signaling: None,
        }
    }
}

impl Plugin for WebrtcPlugin {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn init(&mut self, ctx: &HostContext) -> Result<(), PluginError> {
        ctx.log_info("webrtc", "WebrtcPlugin 初始化完成");

        // 注册工具到宿主
        if let Some(ref registry) = ctx.tool_registry {
            let mut reg = registry.lock().map_err(|e| {
                PluginError::InitError(format!("无法获取 ToolRegistry 锁: {}", e))
            })?;

            let peers = self.peers.clone();
            let emitter = ctx.emitter.clone();
            let logger = ctx.logger.clone();

            reg.register(Arc::new(WebrtcCreatePeerTool {
                peers: peers.clone(),
                emitter: emitter.clone(),
                logger: logger.clone(),
            }));
            reg.register(Arc::new(WebrtcAnswerPeerTool {
                peers: peers.clone(),
                emitter: emitter.clone(),
                logger: logger.clone(),
            }));
            reg.register(Arc::new(WebrtcSendDataTool {
                peers: peers.clone(),
                logger: logger.clone(),
            }));
            reg.register(Arc::new(WebrtcListPeersTool {
                peers: peers.clone(),
            }));
            reg.register(Arc::new(WebrtcClosePeerTool {
                peers: peers.clone(),
                logger: logger.clone(),
            }));

            ctx.log_info("webrtc", "已注册 5 个 WebRTC 工具");
        }

        self.ctx = Some(ctx.clone());
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| PluginError::InitError("未初始化".into()))?;

        ctx.log_info("webrtc", "WebrtcPlugin 已启动");

        // 初始化信令处理器
        let emitter = ctx
            .emitter
            .clone()
            .ok_or_else(|| PluginError::InitError("EventEmitter 未注入".into()))?;

        self.signaling = Some(signaling::SignalingHandler::new(
            emitter.clone(),
            self.peers.clone(),
            ctx.logger.clone(),
        ));

        // 启动内置信令服务器
        let signaling_port: u16 = std::env::var("WEBRTC_SIGNALING_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9876);
        signaling::start_builtin_signaling_server(
            emitter,
            self.peers.clone(),
            ctx.logger.clone(),
            signaling_port,
        );

        Ok(())
    }

    fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("webrtc", "WebrtcPlugin 正在停止...");
        }

        // 关闭所有 peer 连接
        let peers = self.peers.clone();
        let logger = self.ctx.as_ref().and_then(|c| c.logger.clone());
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("无法创建 tokio runtime");
            rt.block_on(async {
                let mut guard = peers.lock().await;
                for (id, handle) in guard.iter() {
                    let _ = handle.close().await;
                    if let Some(ref log) = logger {
                        log.log(plugin_core::LogLevel::Info, "webrtc", &format!("已关闭 peer: {}", id));
                    }
                }
                guard.clear();
            });
        });

        Ok(())
    }

    fn on_event(&self, event: &AgentEvent) -> bool {
        // 处理信令消息
        if let Some(ref signaling) = self.signaling {
            signaling.handle_event(event);
        }

        match &event.event_type {
            // TTS 音频发送（来自 asr-tts 插件）
            EventType::Custom(custom) if custom == "webrtc_audio_send" => {
                let peer_id = match event.data.get("peer_id").and_then(|v| v.as_str()) {
                    Some(id) => id.to_string(),
                    None => return true,
                };
                let audio_b64 = match event.data.get("data").and_then(|v| v.as_str()) {
                    Some(d) => d.to_string(),
                    None => return true,
                };

                let audio_data = match base64_decode(&audio_b64) {
                    Ok(d) => d,
                    Err(e) => {
                        if let Some(ref ctx) = self.ctx {
                            ctx.log_warn("webrtc", &format!("base64 解码失败: {}", e));
                        }
                        return true;
                    }
                };

                let peers = self.peers.clone();
                let logger = self.ctx.as_ref().and_then(|c| c.logger.clone());
                tokio::spawn(async move {
                    let guard = peers.lock().await;
                    if let Some(handle) = guard.get(&peer_id) {
                        if let Err(e) = handle.send_audio(&audio_data).await {
                            if let Some(ref log) = logger {
                                log.log(plugin_core::LogLevel::Warn, "webrtc",
                                    &format!("发送音频失败 [{}]: {}", peer_id, e));
                            }
                        }
                    }
                });
            }

            // LLM 回复 → 通过 DataChannel 发回 webrtc 对端
            EventType::AssistantMessage => {
                let source = event.data.get("source").and_then(|v| v.as_str()).unwrap_or("");
                if source != "webrtc" {
                    return true;
                }
                let peer_id = match event.data.get("chat_id").and_then(|v| v.as_str()) {
                    Some(id) => id.to_string(),
                    None => {
                        event.data.get("chat_id").map(|v| v.to_string()).unwrap_or_default()
                    }
                };
                let text = match event.data.get("text").and_then(|v| v.as_str()) {
                    Some(t) => t.to_string(),
                    None => return true,
                };

                let peers = self.peers.clone();
                let logger = self.ctx.as_ref().and_then(|c| c.logger.clone());
                tokio::spawn(async move {
                    let guard = peers.lock().await;
                    if let Some(handle) = guard.get(&peer_id) {
                        // 通过 DataChannel 发送 JSON 格式的聊天回复
                        let msg = serde_json::json!({
                            "type": "chat",
                            "text": text,
                        });
                        if let Err(e) = handle.send_json(&msg).await {
                            if let Some(ref log) = logger {
                                log.log(plugin_core::LogLevel::Warn, "webrtc",
                                    &format!("发送回复失败 [{}]: {}", peer_id, e));
                            }
                        }
                    }
                });
            }
            _ => {}
        }
        true
    }
}

// ─── FFI 导出 ────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    let plugin = Box::new(WebrtcPlugin::new());
    Box::into_raw(plugin)
}

// ─── 工具实现 ────────────────────────────────────────────────

/// webrtc_create_peer — 创建 PeerConnection 并发起连接
struct WebrtcCreatePeerTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    emitter: Option<Arc<dyn plugin_core::EventEmitter>>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for WebrtcCreatePeerTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "webrtc_create_peer".into(),
            description: "创建 WebRTC PeerConnection，生成 SDP Offer 并通过事件总线发送信令。返回 peer_id 和本地 SDP。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "peer_id": {
                        "type": "string",
                        "description": "对端标识符，用于信令路由"
                    }
                },
                "required": ["peer_id"]
            }),
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return ToolResult::err("缺少参数 peer_id"),
        };

        let emitter = match self.emitter.clone() {
            Some(e) => e,
            None => return ToolResult::err("EventEmitter 不可用"),
        };

        let peers = self.peers.clone();
        let logger = self.logger.clone();

        // 需要在异步上下文中执行
        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 runtime 失败: {}", e))?;

            rt.block_on(async {
                let handle = peer::PeerHandle::new_offer(&peer_id, emitter, logger)
                    .await
                    .map_err(|e| format!("创建 PeerConnection 失败: {}", e))?;

                let local_sdp = handle.local_sdp().await;

                let mut guard = peers.lock().await;
                guard.insert(peer_id.clone(), handle);

                Ok::<_, String>(serde_json::json!({
                    "peer_id": peer_id,
                    "status": "created",
                    "local_sdp": local_sdp,
                    "message": "请将 local_sdp 发送给对端，对端设置 remote_sdp 后回复 Answer"
                }))
            })
        })
        .join()
        .map_err(|_| "线程 panic".to_string());

        match result {
            Ok(Ok(json)) => ToolResult::ok(&json.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(e) => ToolResult::err(&e),
        }
    }
}

/// webrtc_answer_peer — 作为 Answer 方接收 Offer 并创建连接
struct WebrtcAnswerPeerTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    emitter: Option<Arc<dyn plugin_core::EventEmitter>>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for WebrtcAnswerPeerTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "webrtc_answer_peer".into(),
            description: "作为 Answer 方接收远端 SDP Offer，创建 PeerConnection 并回复 Answer。返回 peer_id 和本地 Answer SDP。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "peer_id": {
                        "type": "string",
                        "description": "对端标识符"
                    },
                    "offer_sdp": {
                        "type": "string",
                        "description": "远端发来的 SDP Offer 字符串"
                    }
                },
                "required": ["peer_id", "offer_sdp"]
            }),
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return ToolResult::err("缺少参数 peer_id"),
        };
        let offer_sdp = match args.get("offer_sdp").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return ToolResult::err("缺少参数 offer_sdp"),
        };

        let emitter = match self.emitter.clone() {
            Some(e) => e,
            None => return ToolResult::err("EventEmitter 不可用"),
        };

        let peers = self.peers.clone();
        let logger = self.logger.clone();

        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 runtime 失败: {}", e))?;

            rt.block_on(async {
                let handle = peer::PeerHandle::new_answer(&peer_id, &offer_sdp, emitter, logger)
                    .await
                    .map_err(|e| format!("创建 Answer Peer 失败: {}", e))?;

                let local_sdp = handle.local_sdp().await;

                let mut guard = peers.lock().await;
                guard.insert(peer_id.clone(), handle);

                Ok::<_, String>(serde_json::json!({
                    "peer_id": peer_id,
                    "status": "answered",
                    "local_sdp": local_sdp,
                    "message": "Answer 已生成，请将 local_sdp 发送给 Offer 方"
                }))
            })
        })
        .join()
        .map_err(|_| "线程 panic".to_string());

        match result {
            Ok(Ok(json)) => ToolResult::ok(&json.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(e) => ToolResult::err(&e),
        }
    }
}

/// webrtc_send_data — 通过 DataChannel 发送文本消息
struct WebrtcSendDataTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for WebrtcSendDataTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "webrtc_send_data".into(),
            description: "通过 WebRTC DataChannel 向指定 peer 发送文本消息。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "peer_id": {
                        "type": "string",
                        "description": "目标 peer 标识符"
                    },
                    "message": {
                        "type": "string",
                        "description": "要发送的文本消息"
                    }
                },
                "required": ["peer_id", "message"]
            }),
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return ToolResult::err("缺少参数 peer_id"),
        };
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => return ToolResult::err("缺少参数 message"),
        };

        let peers = self.peers.clone();

        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 runtime 失败: {}", e))?;

            rt.block_on(async {
                let guard = peers.lock().await;
                let handle = guard
                    .get(&peer_id)
                    .ok_or_else(|| format!("未找到 peer: {}", peer_id))?;

                handle.send_text(&message).await.map_err(|e| format!("发送失败: {}", e))?;

                Ok::<_, String>(serde_json::json!({
                    "peer_id": peer_id,
                    "status": "sent",
                    "message": message
                }))
            })
        })
        .join()
        .map_err(|_| "线程 panic".to_string());

        match result {
            Ok(Ok(json)) => ToolResult::ok(&json.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(e) => ToolResult::err(&e),
        }
    }
}

/// webrtc_list_peers — 列出所有活跃的 peer 连接
struct WebrtcListPeersTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
}

impl ToolExecutor for WebrtcListPeersTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "webrtc_list_peers".into(),
            description: "列出所有活跃的 WebRTC PeerConnection。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        })
    }

    fn execute(&self, _args: &serde_json::Value) -> ToolResult {
        let peers = self.peers.clone();

        let result = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => return serde_json::json!({"error": format!("创建 runtime 失败: {}", e)}),
            };

            rt.block_on(async {
                let guard = peers.lock().await;
                let ids: Vec<&String> = guard.keys().collect();
                serde_json::json!({
                    "count": ids.len(),
                    "peers": ids
                })
            })
        })
        .join()
        .map_err(|_| "线程 panic".to_string());

        match result {
            Ok(json) => ToolResult::ok(&json.to_string()),
            Err(e) => ToolResult::err(&e),
        }
    }
}

/// webrtc_close_peer — 关闭指定 peer 连接
struct WebrtcClosePeerTool {
    peers: Arc<Mutex<HashMap<String, peer::PeerHandle>>>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for WebrtcClosePeerTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "webrtc_close_peer".into(),
            description: "关闭指定的 WebRTC PeerConnection。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "peer_id": {
                        "type": "string",
                        "description": "要关闭的 peer 标识符"
                    }
                },
                "required": ["peer_id"]
            }),
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let peer_id = match args.get("peer_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return ToolResult::err("缺少参数 peer_id"),
        };

        let peers = self.peers.clone();

        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 runtime 失败: {}", e))?;

            rt.block_on(async {
                let mut guard = peers.lock().await;
                match guard.remove(&peer_id) {
                    Some(handle) => {
                        handle.close().await;
                        Ok::<_, String>(serde_json::json!({
                            "peer_id": peer_id,
                            "status": "closed"
                        }))
                    }
                    None => Err(format!("未找到 peer: {}", peer_id)),
                }
            })
        })
        .join()
        .map_err(|_| "线程 panic".to_string());

        match result {
            Ok(Ok(json)) => ToolResult::ok(&json.to_string()),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ─── base64 解码（用于 webrtc_audio_send 事件） ───

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let s = s.trim_end_matches('=');
    let mut result = Vec::with_capacity(s.len() * 3 / 4);
    let bytes: Vec<u8> = s.bytes().collect();

    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let mut buf = 0u32;
        for (i, &b) in chunk.iter().enumerate() {
            if let Some(pos) = CHARS.iter().position(|&c| c == b) {
                buf |= (pos as u32) << (6 * (3 - i));
            }
        }
        result.push((buf >> 16) as u8);
        if chunk.len() > 2 {
            result.push((buf >> 8) as u8);
        }
        if chunk.len() > 3 {
            result.push(buf as u8);
        }
    }
    Ok(result)
}
