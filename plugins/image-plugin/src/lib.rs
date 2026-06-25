//! image-plugin — 图片理解插件。
//!
//! 基于小米 MiMo API 提供图片描述、分析、对比等能力。
//! 所有图片输入均使用 base64 编码。
//! 自带媒体缓存：收到图片后自动缓存，工具可免传 base64 直接分析最近图片。

use plugin_interface::*;
use serde::Deserialize;
use std::sync::{Arc, LazyLock, Mutex};

/// 最近图片缓存（单会话，只保留最新一张）
static MEDIA_CACHE: LazyLock<Mutex<Option<(String, String)>>> = LazyLock::new(|| Mutex::new(None));

// ── MiMo API 配置 ────────────────────────────────────────────────────────────

struct MiMoConfig {
    api_key: String,
    base_url: String,
    model: String,
}

impl MiMoConfig {
    fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("IMAGE_API_KEY")
            .or_else(|_| std::env::var("LLM_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| "IMAGE_API_KEY / LLM_API_KEY / OPENAI_API_KEY not set".to_string())?;
        let base_url = std::env::var("IMAGE_BASE_URL")
            .or_else(|_| std::env::var("LLM_BASE_URL"))
            .unwrap_or_else(|_| "https://api.xiaomimimo.com/v1".into());
        let model = std::env::var("IMAGE_MODEL").unwrap_or_else(|_| "mimo-v2.5".into());
        Ok(Self {
            api_key,
            base_url,
            model,
        })
    }
}

// ── MiMo API 响应结构 ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MiMoResponse {
    choices: Vec<MiMoChoice>,
}

#[derive(Deserialize)]
struct MiMoChoice {
    message: MiMoMessage,
}

#[derive(Deserialize)]
struct MiMoMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct MiMoError {
    error: Option<MiMoErrorBody>,
}

#[derive(Deserialize)]
struct MiMoErrorBody {
    message: Option<String>,
}

// ── 共享 HTTP 客户端 ─────────────────────────────────────────────────────────

struct MiMoClient {
    config: MiMoConfig,
    http: reqwest::Client,
}

impl MiMoClient {
    fn new(config: MiMoConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    fn chat(&self, messages: Vec<serde_json::Value>, max_tokens: u32) -> Result<String, String> {
        let api_url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "max_completion_tokens": max_tokens,
        });

        let client = self.http.clone();
        let api_key = self.config.api_key.clone();

        // 工具 execute 可能在非 tokio 线程中调用，需要自建 runtime
        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async {
                let response = client
                    .post(&api_url)
                    .header("api-key", &api_key)
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| format!("HTTP error: {}", e))?;

                let status = response.status();
                let text = response
                    .text()
                    .await
                    .map_err(|e| format!("read body: {}", e))?;

                if !status.is_success() {
                    if let Ok(err) = serde_json::from_str::<MiMoError>(&text) {
                        if let Some(msg) = err.error.and_then(|e| e.message) {
                            return Err(format!("MiMo API error ({}): {}", status.as_u16(), msg));
                        }
                    }
                    return Err(format!("MiMo API error ({}): {}", status.as_u16(), text));
                }

                let resp: MiMoResponse =
                    serde_json::from_str(&text).map_err(|e| format!("parse response: {}", e))?;

                let content = resp
                    .choices
                    .first()
                    .and_then(|c| c.message.content.as_deref())
                    .unwrap_or("")
                    .to_string();

                if content.is_empty() {
                    Err("MiMo returned empty response".into())
                } else {
                    Ok(content)
                }
            })
        })
        .join()
        {
            Ok(Ok(t)) => Ok(t),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("thread panic".into()),
        }
    }
}

// ── 辅助 ─────────────────────────────────────────────────────────────────────

fn build_user_content(images: &[(String, Option<String>)], text: &str) -> serde_json::Value {
    let mut parts: Vec<serde_json::Value> = Vec::new();
    for (b64, mime) in images {
        let mime_type = mime.as_deref().unwrap_or("image/jpeg");
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", mime_type, b64)
            }
        }));
    }
    if !text.is_empty() {
        parts.push(serde_json::json!({"type": "text", "text": text}));
    }
    serde_json::json!(parts)
}

/// 从参数或缓存中获取图片数据。
fn resolve_image(args: &serde_json::Value) -> Result<(String, String), String> {
    // 优先用显式传入的 base64
    if let Some(b64) = args.get("image_base64").and_then(|v| v.as_str()) {
        let mime = args
            .get("mime_type")
            .and_then(|v| v.as_str())
            .unwrap_or("image/jpeg")
            .to_string();
        return Ok((b64.to_string(), mime));
    }
    // 从缓存取
    if let Ok(cache) = MEDIA_CACHE.lock() {
        if let Some((b64, mime)) = cache.as_ref() {
            return Ok((b64.clone(), mime.clone()));
        }
    }
    Err("未找到图片数据。请先发送图片，或提供 image_base64 参数。".into())
}

// ── 工具：image_understand ───────────────────────────────────────────────────

struct ImageUnderstandTool {
    client: Arc<MiMoClient>,
}

impl ImageUnderstandTool {
    fn new(client: Arc<MiMoClient>) -> Self {
        Self { client }
    }
}

impl ToolExecutor for ImageUnderstandTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "image_understand".into(),
            description: "理解/分析图片内容。可传入图片 base64，也可省略（自动使用该对话最近收到的图片）。适用于图片描述、OCR、物体识别、场景分析等。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64": {
                        "type": "string",
                        "description": "Base64 编码的图片数据（可选，不填则自动用该对话最近的图片）"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "关于图片的问题或指令"
                    },
                    "mime_type": {
                        "type": "string",
                        "description": "图片 MIME 类型，默认 image/jpeg"
                    }
                },
                "required": ["prompt"]
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: prompt"),
        };
        let (b64, mime) = match resolve_image(args) {
            Ok(v) => v,
            Err(e) => return ToolResult::err(&e),
        };

        let user_content = build_user_content(&[(b64, Some(mime))], prompt);
        let messages = vec![serde_json::json!({"role": "user", "content": user_content})];

        match self.client.chat(messages, 1024) {
            Ok(text) => ToolResult::ok(&text),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ── 工具：image_describe ─────────────────────────────────────────────────────

struct ImageDescribeTool {
    client: Arc<MiMoClient>,
}

impl ImageDescribeTool {
    fn new(client: Arc<MiMoClient>) -> Self {
        Self { client }
    }
}

impl ToolExecutor for ImageDescribeTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "image_describe".into(),
            description: "描述图片内容。可省略 image_base64，工具会自动使用该对话最近收到的图片。快捷版，无需手动写 prompt。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64": {
                        "type": "string",
                        "description": "Base64 编码的图片数据（可选，不填则自动用该对话最近的图片）"
                    },
                    "mime_type": {
                        "type": "string",
                        "description": "图片 MIME 类型，默认 image/jpeg"
                    }
                }
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let (b64, mime) = match resolve_image(args) {
            Ok(v) => v,
            Err(e) => return ToolResult::err(&e),
        };

        let user_content = build_user_content(
            &[(b64, Some(mime))],
            "Please describe the content of this image in detail.",
        );
        let messages = vec![serde_json::json!({"role": "user", "content": user_content})];

        match self.client.chat(messages, 1024) {
            Ok(text) => ToolResult::ok(&text),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ── 工具：image_compare ──────────────────────────────────────────────────────

struct ImageCompareTool {
    client: Arc<MiMoClient>,
}

impl ImageCompareTool {
    fn new(client: Arc<MiMoClient>) -> Self {
        Self { client }
    }
}

impl ToolExecutor for ImageCompareTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| {
            ToolDef {
            name: "image_compare".into(),
            description: "对比两张图片的异同。传入两张图片 base64（或省略，自动使用该对话最近收到的两张图片）。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64_a": {
                        "type": "string",
                        "description": "第一张图片的 Base64 编码数据（可选）"
                    },
                    "image_base64_b": {
                        "type": "string",
                        "description": "第二张图片的 Base64 编码数据（可选）"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "可选的对比侧重方向"
                    },
                    "mime_type_a": {"type": "string", "description": "第一张图片的 MIME 类型"},
                    "mime_type_b": {"type": "string", "description": "第二张图片的 MIME 类型"}
                }
            }),
        }
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let b64_a = match args.get("image_base64_a").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => match MEDIA_CACHE.lock().ok().and_then(|c| c.clone()) {
                Some((b64, _)) => b64,
                None => return ToolResult::err("missing: image_base64_a（且缓存中无图片）"),
            },
        };
        let b64_b = match args.get("image_base64_b").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => match MEDIA_CACHE.lock().ok().and_then(|c| c.clone()) {
                Some((b64, _)) => b64,
                None => return ToolResult::err("missing: image_base64_b（且缓存中无图片）"),
            },
        };
        let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or(
            "Please describe the similarities and differences between these two images.",
        );
        let mime_a = args
            .get("mime_type_a")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mime_b = args
            .get("mime_type_b")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let user_content = build_user_content(&[(b64_a, mime_a), (b64_b, mime_b)], prompt);
        let messages = vec![serde_json::json!({"role": "user", "content": user_content})];

        match self.client.chat(messages, 1024) {
            Ok(text) => ToolResult::ok(&text),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ── Plugin ───────────────────────────────────────────────────────────────────

struct ImagePlugin {
    info: PluginInfo,
    client: Option<Arc<MiMoClient>>,
}

impl ImagePlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "image-plugin".into(),
                version: "0.2.0".into(),
                description: "图片理解插件 — 基于 MiMo API 的图片描述、分析、对比".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            client: None,
        }
    }
}

impl Plugin for ImagePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let config = MiMoConfig::from_env().map_err(|e| {
            ctx.logger.error(&format!("MiMo config: {}", e));
            e
        })?;

        let client = Arc::new(MiMoClient::new(config));
        self.client = Some(client.clone());

        if let Some(ref reg) = ctx.tool_registry {
            let mut reg = reg.lock();
            reg.register(Arc::new(ImageUnderstandTool::new(client.clone())));
            reg.register(Arc::new(ImageDescribeTool::new(client.clone())));
            reg.register(Arc::new(ImageCompareTool::new(client.clone())));
        }

        ctx.logger
            .info("registered 3 tools: image_understand, image_describe, image_compare");
        Ok(())
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "user.message" {
            if let Some(b64) = event.data.get("image_base64").and_then(|v| v.as_str()) {
                let mime = event
                    .data
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("image/jpeg");
                if let Ok(mut cache) = MEDIA_CACHE.lock() {
                    *cache = Some((b64.to_string(), mime.to_string()));
                }
            }
        }
        true
    }

    fn stop(&mut self) {
        self.client = None;
    }
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ImagePlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
