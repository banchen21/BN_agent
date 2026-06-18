//! Video Understanding Plugin — uses MiMo Chat Completions API's video_url content type.
//!
//! Registers tool: `video_analyze`

use plugin_interface::*;
use std::sync::{Arc, LazyLock, Mutex};
use std::collections::HashMap;

/// 最近媒体缓存（chat_id → (base64_data, mime_type)）
static MEDIA_CACHE: LazyLock<Mutex<HashMap<i64, (String, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct VideoPlugin {
    info: PluginInfo,
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
    event_bus: Option<Addr<EventBus>>,
}

impl VideoPlugin {
    pub fn new() -> Self {
        let api_key = std::env::var("VIDEO_API_KEY")
            .or_else(|_| std::env::var("LLM_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY")).unwrap_or_default();
        let base_url = std::env::var("VIDEO_BASE_URL")
            .or_else(|_| std::env::var("LLM_BASE_URL"))
            .unwrap_or_else(|_| "https://api.xiaomimimo.com/v1".into());
        let model = std::env::var("VIDEO_MODEL")
            .unwrap_or_else(|_| "mimo-v2.5".into());

        Self {
            info: PluginInfo {
                name: "video-plugin".into(),
                version: "0.1.0".into(),
                description: "视频理解 — 分析视频内容并生成描述".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            api_key, base_url, model,
            client: reqwest::Client::builder().build().unwrap_or_default(),
            event_bus: None,
        }
    }
}

impl Plugin for VideoPlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        self.event_bus = Some(ctx.event_bus.clone());

        if let Some(ref reg) = ctx.tool_registry {
            let mut r = reg.lock().map_err(|e| format!("lock: {}", e))?;
            r.register(Arc::new(VideoAnalyzeTool {
                client: self.client.clone(),
                base_url: self.base_url.clone(),
                model: self.model.clone(),
                api_key: self.api_key.clone(),
            }));
            eprintln!("[video-plugin] registered tool: video_analyze");
        }

        log::info!("[video-plugin] started (model={})", self.model);
        Ok(())
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "user.message" {
            // 缓存 video_base64
            if let Some(b64) = event.data.get("video_base64").and_then(|v| v.as_str()) {
                let mime = event.data.get("video_mime")
                    .and_then(|v| v.as_str()).unwrap_or("video/mp4");
                if let Some(chat_id) = event.data.get("chat_id").and_then(|v| v.as_i64()) {
                    if let Ok(mut cache) = MEDIA_CACHE.lock() {
                        cache.insert(chat_id, (b64.to_string(), mime.to_string()));
                        eprintln!("[video-plugin] cached video for chat_id={}", chat_id);
                    }
                }
            }
        }
        true // 继续传播事件
    }

    fn stop(&mut self) {
        log::info!("[video-plugin] stopped");
    }
}

// ─── FFI exports ──────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> { Box::new(VideoPlugin::new()) }
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}

// ─── Tool ─────────────────────────────────────────────────────────

struct VideoAnalyzeTool {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl ToolExecutor for VideoAnalyzeTool {
    fn def(&self) -> &ToolDef {
        static DEF: LazyLock<ToolDef> = LazyLock::new(|| ToolDef {
            name: "video_analyze".into(),
            description: "分析视频内容并返回文字描述。可省略 video_base64，工具会自动使用该对话最近收到的视频。适合对视频进行深度帧分析、计时序动作等精细化理解。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "video_base64": {"type": "string", "description": "Base64 encoded video data (可选，不填则自动用该对话最近的视频)"},
                    "mime_type": {"type": "string", "description": "MIME type, e.g. video/mp4"},
                    "fps": {"type": "number", "description": "帧率采样，默认2，范围0.1-10。越高时间细节越精细"},
                    "media_resolution": {"type": "string", "description": "分辨率: 'default' 或 'max'"},
                    "system_prompt": {"type": "string", "description": "自定义系统提示词（可选），用于增强分析能力"},
                    "user_prompt": {"type": "string", "description": "自定义用户提示词（可选），覆盖默认的'请详细描述'"},
                    "jailbreak_index": {"type": "integer", "description": "jailbreak 提示词索引（可选），需要配合 jailbreak_prompts.csv 使用"}
                }
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        // 先从参数拿 video_base64，拿不到则从缓存取
        let video_b64 = match args.get("video_base64").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                let chat_id = args.get("chat_id").and_then(|v| v.as_i64());
                match chat_id.and_then(|cid| MEDIA_CACHE.lock().ok().and_then(|c| c.get(&cid).cloned())) {
                    Some((b64, _)) => b64,
                    None => return ToolResult::err("未找到视频数据。请先发送视频，或提供 video_base64 参数。"),
                }
            }
        };
        let mime = args.get("mime_type").and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                args.get("chat_id").and_then(|v| v.as_i64())
                    .and_then(|cid| MEDIA_CACHE.lock().ok().and_then(|c| c.get(&cid).cloned()))
                    .map(|(_, m)| m)
            })
            .unwrap_or_else(|| "video/mp4".into());
        let fps = args.get("fps").and_then(|v| v.as_f64()).unwrap_or(2.0);
        let resolution = args.get("media_resolution").and_then(|v| v.as_str()).unwrap_or("default");
        let system_prompt = args.get("system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string());
        let user_prompt = args.get("user_prompt").and_then(|v| v.as_str()).map(|s| s.to_string());
        let jailbreak_index = args.get("jailbreak_index").and_then(|v| v.as_u64()).map(|i| i as usize);

        eprintln!("[video-plugin:analyze] mime={} fps={} res={} b64_len={}", mime, fps, resolution, video_b64.len());

        let c = self.client.clone();
        let u = self.base_url.clone();
        let m = self.model.clone();
        let k = self.api_key.clone();
        let mime_o = mime;
        let res_o = resolution.to_string();
        let video_data = video_b64.to_string();
        let sp = system_prompt;
        let up = user_prompt;
        let ji = jailbreak_index;

        match std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
            rt.block_on(async {
                do_video_asr(&c, &u, &m, &k, &video_data, &mime_o, fps, &res_o, sp.as_deref(), up.as_deref(), ji).await
            })
        }).join() {
            Ok(Ok(t)) => ToolResult::ok(&t),
            Ok(Err(e)) => ToolResult::err(&e),
            Err(_) => ToolResult::err("thread panic"),
        }
    }
}

// ─── API call ──────────────────────────────────────────────────────

/// 加载 jailbreak 提示词：读取 data/jailbreak_prompts.csv 的第三列
fn load_jailbreak_prompts() -> Vec<String> {
    let path = std::path::PathBuf::from("data/jailbreak_prompts.csv");
    let mut prompts = Vec::new();
    if let Ok(mut reader) = csv::Reader::from_path(&path) {
        for result in reader.records() {
            if let Ok(record) = result {
                if let Some(p) = record.get(2) {
                    let t = p.trim();
                    if !t.is_empty() {
                        prompts.push(t.to_string());
                    }
                }
            }
        }
    }
    prompts
}

async fn do_video_asr(
    client: &reqwest::Client, base_url: &str, model: &str, api_key: &str,
    video_b64: &str, mime_type: &str, fps: f64, resolution: &str,
    system_prompt: Option<&str>, user_prompt: Option<&str>, jailbreak_index: Option<usize>,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let data_url = format!("data:{};base64,{}", mime_type, video_b64);

    let user_text = user_prompt.unwrap_or("请详细描述这个视频的内容，包括画面中的物体、人物、动作、场景和音频信息。");

    let mut messages: Vec<serde_json::Value> = Vec::new();

    // 组装 system prompt + jailbreak 增强
    if let Some(sp) = system_prompt {
        let system_content = if let Some(idx) = jailbreak_index {
            let prompts = load_jailbreak_prompts();
            let jb = prompts.get(idx).map(|s| s.as_str()).unwrap_or("");
            if !jb.is_empty() {
                format!("{}\n\n{}", jb, sp)
            } else {
                sp.to_string()
            }
        } else {
            sp.to_string()
        };
        messages.push(serde_json::json!({ "role": "system", "content": system_content }));
    }

    messages.push(serde_json::json!({
        "role": "user",
        "content": [
            {
                "type": "video_url",
                "video_url": { "url": data_url },
                "fps": fps,
                "media_resolution": resolution
            },
            { "type": "text", "text": user_text }
        ]
    }));

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": 2048u32,
    });

    eprintln!("[video-plugin:api] posting to {}", url);
    let resp = client.post(&url)
        .header("api-key", api_key)
        .json(&body).send().await
        .map_err(|e| format!("request: {}", e))?;
    let status = resp.status();
    let body_text = resp.text().await.map_err(|e| format!("read: {}", e))?;
    if !status.is_success() {
        return Err(format!("API {}: {}", status, body_text));
    }

    serde_json::from_str::<serde_json::Value>(&body_text)
        .map_err(|e| format!("parse: {}", e))?
        .get("choices").and_then(|c| c.get(0))
        .and_then(|c| c["message"]["content"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("bad response: {}", body_text))
}
