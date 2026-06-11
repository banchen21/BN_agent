//! ASR-TTS Plugin — 语音识别与合成插件
//!
//! 通过事件总线处理音频：
//! 1. 监听 webrtc_audio_received / audio_captured → ASR 转文字 → emit UserMessage
//! 2. 监听 AssistantMessage（source=webrtc/local）→ TTS 转语音 → 输出到对应通道

use plugin_core::{
    AgentEvent, EventSource, EventType, HostContext, Plugin, PluginError, PluginMeta,
    ToolDef, ToolExecutor, ToolResult,
};
use std::sync::Arc;

pub struct AsrTtsPlugin {
    meta: PluginMeta,
    ctx: Option<HostContext>,
    /// Deepgram / OpenAI Whisper API key
    asr_api_key: String,
    asr_base_url: String,
    asr_model: String,
    /// TTS API base URL（OpenAI 兼容 TTS）
    tts_base_url: String,
    tts_model: String,
    tts_api_key: String,
    /// TTS 预置音色名（mimo-v2.5-tts 模式）
    tts_voice: String,
    /// TTS 音色描述 / 角色提示词（user message 风格控制）
    tts_voice_desc: String,
    http_client: reqwest::Client,
}

impl AsrTtsPlugin {
    pub fn new() -> Self {
        // 从环境变量读取配置
        let asr_api_key = std::env::var("ASR_API_KEY").unwrap_or_default();
        let asr_base_url = std::env::var("ASR_BASE_URL")
            .or_else(|_| std::env::var("LLM_BASE_URL"))
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let asr_model = std::env::var("ASR_MODEL").unwrap_or_else(|_| "whisper-1".into());
        let tts_api_key = std::env::var("TTS_API_KEY")
            .or_else(|_| std::env::var("LLM_API_KEY"))
            .unwrap_or_default();
        let tts_base_url = std::env::var("TTS_BASE_URL")
            .or_else(|_| std::env::var("LLM_BASE_URL"))
            .unwrap_or_else(|_| "https://api.deepseek.com/v1".into());
        let tts_model = std::env::var("TTS_MODEL").unwrap_or_else(|_| "tts-1".into());
        let tts_voice = std::env::var("TTS_VOICE").unwrap_or_else(|_| "mimo_default".into());
        let tts_voice_desc = std::env::var("TTS_VOICE_DESC")
            .unwrap_or_else(|_| "你是小月，一个元气满满的 AI 助手。声音干净明亮。".into());

        Self {
            meta: PluginMeta {
                name: "asr-tts-plugin".into(),
                version: "0.1.0".into(),
                description: "语音识别(ASR)与语音合成(TTS)插件".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            ctx: None,
            asr_api_key,
            asr_base_url,
            asr_model,
            tts_base_url,
            tts_model,
            tts_api_key,
            tts_voice,
            tts_voice_desc,
            http_client: reqwest::Client::builder().build().unwrap_or_default(),
        }
    }
}

impl Plugin for AsrTtsPlugin {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn init(&mut self, ctx: &HostContext) -> Result<(), PluginError> {
        ctx.log_info("asr-tts", "AsrTtsPlugin 初始化完成");
        self.ctx = Some(ctx.clone());
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("asr-tts", "AsrTtsPlugin 已启动");
            ctx.log_info(
                "asr-tts",
                &format!(
                    "TTS: model={}, base_url={}",
                    self.tts_model, self.tts_base_url
                ),
            );

            // 注册 tts 工具
            if let Some(ref registry) = ctx.tool_registry {
                let http_client = self.http_client.clone();
                let tts_base_url = self.tts_base_url.clone();
                let tts_model = self.tts_model.clone();
                let tts_api_key = self.tts_api_key.clone();
                let tts_voice = self.tts_voice.clone();
                let logger = ctx.logger.clone();

                registry
                    .lock()
                    .map_err(|e| PluginError::InitError(format!("{}", e)))?
                    .register(Arc::new(TtsTool {
                        http_client,
                        tts_base_url,
                        tts_model,
                        tts_api_key,
                        tts_voice,
                        tts_voice_desc: self.tts_voice_desc.clone(),
                        logger,
                    }));
                ctx.log_info("asr-tts", "已注册工具: tts_synthesize");

                // 注册 asr 工具
                let asr_http_client = self.http_client.clone();
                let asr_base_url = self.asr_base_url.clone();
                let asr_model = self.asr_model.clone();
                let asr_api_key = self.asr_api_key.clone();
                let asr_language = std::env::var("ASR_LANGUAGE").unwrap_or_else(|_| "zh".into());
                let asr_logger = ctx.logger.clone();

                registry
                    .lock()
                    .map_err(|e| PluginError::InitError(format!("{}", e)))?
                    .register(Arc::new(AsrTool {
                        http_client: asr_http_client,
                        asr_base_url,
                        asr_model,
                        asr_api_key,
                        asr_language,
                        logger: asr_logger,
                    }));
                ctx.log_info("asr-tts", "已注册工具: asr_transcribe");
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("asr-tts", "AsrTtsPlugin 已停止");
        }
        Ok(())
    }

    fn on_event(&self, event: &AgentEvent) -> bool {
        match &event.event_type {
            EventType::Custom(custom) if custom == "webrtc_audio_received" => {
                // 收到 WebRTC 音频 → ASR
                self.handle_audio_received(event, "webrtc");
            }
            EventType::Custom(custom) if custom == "audio_captured" => {
                // 收到本地音频捕获 → ASR
                self.handle_audio_received(event, "local");
            }
            EventType::AssistantMessage => {
                // LLM 回复 → 检查来源 → TTS
                let source = event
                    .data
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match source {
                    "webrtc" => self.handle_assistant_for_tts(event, "webrtc"),
                    "local" => self.handle_assistant_for_tts(event, "local"),
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}

impl AsrTtsPlugin {
    fn log(&self, level: plugin_core::LogLevel, msg: &str) {
        if let Some(ref ctx) = self.ctx {
            if let Some(ref logger) = ctx.logger {
                logger.log(level, "asr-tts", msg);
            }
        }
    }

    fn emitter(&self) -> Option<&Arc<dyn plugin_core::EventEmitter>> {
        self.ctx.as_ref().and_then(|c| c.emitter.as_ref())
    }

    /// 处理收到的音频 → ASR → UserMessage
    fn handle_audio_received(&self, event: &AgentEvent, source: &str) {
        let peer_id = event
            .data
            .get("peer_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{}-user", source));
        let audio_b64 = match event.data.get("data").and_then(|v| v.as_str()) {
            Some(d) => d.to_string(),
            None => return,
        };

        self.log(plugin_core::LogLevel::Debug, &format!("收到音频 [{}]: {} bytes base64", peer_id, audio_b64.len()));

        // 解码 base64
        let audio_data = match base64_decode(&audio_b64) {
            Ok(d) => d,
            Err(e) => {
                self.log(plugin_core::LogLevel::Warn, &format!("base64 解码失败: {}", e));
                return;
            }
        };

        // 异步执行 ASR
        let client = self.http_client.clone();
        let asr_api_key = self.asr_api_key.clone();
        let asr_base_url = self.asr_base_url.clone();
        let asr_model = self.asr_model.clone();
        let mime = "audio/opus";
        let lang = std::env::var("ASR_LANGUAGE").unwrap_or_else(|_| "zh".into());
        let emitter = match self.emitter() {
            Some(e) => Arc::clone(e),
            None => return,
        };
        let peer_id_clone = peer_id.clone();
        let logger = self.ctx.as_ref().and_then(|c| c.logger.clone());
        let source_owned = source.to_string();

        tokio::spawn(async move {
            match do_asr(&client, &asr_base_url, &asr_model, &asr_api_key, &audio_data, mime, &lang).await {
                Ok(text) => {
                    if text.trim().is_empty() {
                        return;
                    }
                    if let Some(ref log) = logger {
                        log.log(plugin_core::LogLevel::Info, "asr-tts", &format!("ASR [{}]: {}", peer_id_clone, text));
                    }
                    // emit UserMessage → LLM 处理
                    emitter.emit(AgentEvent::new(
                        EventType::UserMessage,
                        EventSource::Plugin("asr-tts".into()),
                        serde_json::json!({
                            "chat_id": peer_id_clone,
                            "user_name": format!("{}:{}", source_owned, peer_id_clone),
                            "text": text,
                            "source": source_owned,
                        }),
                    ));
                }
                Err(e) => {
                    if let Some(ref log) = logger {
                        log.log(plugin_core::LogLevel::Warn, "asr-tts", &format!("ASR 失败: {}", e));
                    }
                }
            }
        });
    }

    /// 处理 LLM 回复 → TTS → 发送音频到对应通道
    fn handle_assistant_for_tts(&self, event: &AgentEvent, source: &str) {
        let peer_id = match event.data.get("chat_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                event.data.get("chat_id").map(|v| v.to_string()).unwrap_or_default()
            }
        };
        let text = match event.data.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return,
        };

        if text.is_empty() {
            return;
        }

        self.log(plugin_core::LogLevel::Info, &format!("TTS [{}]: {}", peer_id, &text[..text.len().min(60)]));

        let client = self.http_client.clone();
        let tts_base_url = self.tts_base_url.clone();
        let tts_model = self.tts_model.clone();
        let tts_api_key = self.tts_api_key.clone();
        let tts_voice = self.tts_voice.clone();
        let tts_voice_desc = self.tts_voice_desc.clone();
        let emitter = match self.emitter() {
            Some(e) => Arc::clone(e),
            None => return,
        };
        let peer_id_clone = peer_id.clone();
        let logger = self.ctx.as_ref().and_then(|c| c.logger.clone());
        let source_owned = source.to_string();

        tokio::spawn(async move {
            match do_tts(&client, &tts_base_url, &tts_model, &tts_api_key, &text, &tts_voice, Some(&tts_voice_desc)).await {
                Ok(audio_data) => {
                    if let Some(ref log) = logger {
                        log.log(plugin_core::LogLevel::Debug, "asr-tts",
                            &format!("TTS 完成 [{}]: {} bytes", peer_id_clone, audio_data.len()));
                    }

                    match source_owned.as_str() {
                        "webrtc" => {
                            // WebRTC 通道：emit 事件让 webrtc-plugin 发送
                            emitter.emit(AgentEvent::new(
                                EventType::Custom("webrtc_audio_send".into()),
                                EventSource::Plugin("asr-tts".into()),
                                serde_json::json!({
                                    "peer_id": peer_id_clone,
                                    "data": base64_encode(&audio_data),
                                }),
                            ));
                        }
                        "local" => {
                            // 本地通道：emit 事件让 audio-capture-plugin（或专门播放）处理
                            // 同时写入 VB-Cable 虚拟播放设备
                            emitter.emit(AgentEvent::new(
                                EventType::Custom("local_audio_play".into()),
                                EventSource::Plugin("asr-tts".into()),
                                serde_json::json!({
                                    "peer_id": peer_id_clone,
                                    "data": base64_encode(&audio_data),
                                    "source": "local",
                                }),
                            ));
                        }
                        _ => {
                            if let Some(ref log) = logger {
                                log.log(plugin_core::LogLevel::Warn, "asr-tts",
                                    &format!("未知 source: {}, 跳过 TTS 输出", source_owned));
                            }
                        }
                    }
                }
                Err(e) => {
                    if let Some(ref log) = logger {
                        log.log(plugin_core::LogLevel::Warn, "asr-tts", &format!("TTS 失败: {}", e));
                    }
                }
            }
        });
    }
}

// ─── ASR：使用 MiMo Chat Completions API（input_audio 格式） ───
///
/// MiMo ASR 通过 Chat Completions 接口实现：
/// - endpoint: /v1/chat/completions
/// - model: mimo-v2.5-asr
/// - messages: [{role:"user", content:[{type:"input_audio", input_audio:{data:"data:{mime};base64,..."}}]}]
/// - asr_options: {language: "zh"|"en"|"auto"}
/// - 响应: choices[0].message.content (文本)

async fn do_asr(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    audio_data: &[u8],
    mime_type: &str,
    language: &str,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("ASR_API_KEY 未配置".into());
    }

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let audio_b64 = base64_encode(audio_data);
    let data_url = format!("data:{};base64,{}", mime_type, audio_b64);

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_audio",
                        "input_audio": {
                            "data": data_url
                        }
                    }
                ]
            }
        ],
        "asr_options": {
            "language": language
        }
    });

    let resp = client
        .post(&url)
        .header("api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("ASR 请求失败: {}", e))?;

    let status = resp.status();
    let resp_body = resp.text().await.map_err(|e| format!("读取 ASR 响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("ASR API 错误 ({}): {}", status, resp_body));
    }

    let json: serde_json::Value =
        serde_json::from_str(&resp_body).map_err(|e| format!("解析 ASR 响应失败: {}", e))?;

    // MiMo Chat Completions 响应格式: choices[0].message.content
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("ASR 响应格式异常: {}", resp_body))
}

// ─── TTS：使用 MiMo Chat Completions API（非标准 /audio/speech） ───
///
/// MiMo TTS 通过 Chat Completions 接口实现：
/// - endpoint: /v1/chat/completions
/// - user message: 音色描述 / 风格指令
/// - assistant message: 要合成的文本
/// - audio.format: "opus" | "wav" | "pcm16"
/// - audio.voice: 预置音色名 或 "data:audio/wav;base64,..." (voiceclone)
/// - 响应: choices[0].message.audio.data (base64)

async fn do_tts(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: &str,
    text: &str,
    voice: &str,
    voice_desc: Option<&str>,
) -> Result<Vec<u8>, String> {
    if api_key.is_empty() {
        return Err("TTS_API_KEY 未配置".into());
    }

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    // 检测模型类型
    let is_voiceclone = model.contains("voiceclone");
    let is_voicedesign = model.contains("voicedesign");
    let is_preset = !is_voiceclone && !is_voicedesign;

    // 构建 messages
    let mut messages = Vec::new();

    // user message: 音色描述或风格指令
    //   - preset 模式：用角色提示词控制风格（参照 mc-ai-buddy 的做法）
    //   - voicedesign 模式：用描述文本设计音色
    //   - voiceclone 模式：可为空（音色由音频样本决定）
    let voice_desc_content = if is_preset || is_voicedesign {
        voice_desc.unwrap_or("")
    } else {
        ""
    };
    messages.push(serde_json::json!({
        "role": "user",
        "content": voice_desc_content
    }));

    // assistant message: 要合成的文本
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": text
    }));

    // 构建 audio 参数
    let mut audio = serde_json::json!({
        "format": "wav",
    });

    if is_voiceclone {
        // voiceclone 模式：需要传入 base64 音频样本
        return Err("voiceclone 模式暂不支持，请使用 mimo-v2.5-tts 预设音色模式".into());
    } else if is_voicedesign {
        // voicedesign 模式：audio 中不设 voice，音色由 user message 描述决定
    } else if is_preset {
        // 预置音色模式：使用配置的预置音色名
        audio["voice"] = serde_json::json!(voice);
    }

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "audio": audio,
    });

    let resp = client
        .post(&url)
        .header("api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("TTS 请求失败: {}", e))?;

    let status = resp.status();
    let resp_text = resp.text().await.map_err(|e| format!("读取 TTS 响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("TTS API 错误 ({}): {}", status, &resp_text[..resp_text.len().min(500)]));
    }

    // 解析 Chat Completions 响应，提取 audio.data
    let json: serde_json::Value =
        serde_json::from_str(&resp_text).map_err(|e| format!("解析 TTS 响应失败: {}", e))?;

    let audio_b64 = json["choices"][0]["message"]["audio"]["data"]
        .as_str()
        .ok_or_else(|| format!("TTS 响应格式异常: {}", &resp_text[..resp_text.len().min(300)]))?;

    // base64 解码音频数据
    base64_decode(audio_b64)
}

// ─── asr 工具（供其他插件通过 ToolRegistry 调用） ───

struct AsrTool {
    http_client: reqwest::Client,
    asr_base_url: String,
    asr_model: String,
    asr_api_key: String,
    asr_language: String,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for AsrTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "asr_transcribe".into(),
            description: "将音频数据转为文字，接收 base64 编码的音频和 MIME 类型，返回识别出的文本。".into(),
            internal: true,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "audio_base64": {
                        "type": "string",
                        "description": "base64 编码的音频数据"
                    },
                    "mime_type": {
                        "type": "string",
                        "description": "音频 MIME 类型，如 audio/wav、audio/mp3、audio/ogg"
                    }
                },
                "required": ["audio_base64"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let audio_b64 = match args.get("audio_base64").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return ToolResult::err("缺少参数: audio_base64"),
        };

        if audio_b64.is_empty() {
            return ToolResult::err("audio_base64 不能为空");
        }

        let mime_type = args
            .get("mime_type")
            .and_then(|v| v.as_str())
            .unwrap_or("audio/ogg")
            .to_string();

        let audio_data = match base64_decode(&audio_b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 解码失败: {}", e)),
        };

        let http_client = self.http_client.clone();
        let asr_base_url = self.asr_base_url.clone();
        let asr_model = self.asr_model.clone();
        let asr_api_key = self.asr_api_key.clone();
        let asr_language = self.asr_language.clone();
        let logger = self.logger.clone();

        // 同步执行 ASR
        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                match do_asr(&http_client, &asr_base_url, &asr_model, &asr_api_key, &audio_data, &mime_type, &asr_language).await {
                    Ok(text) => {
                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Info,
                                "asr-tts",
                                &format!("ASR 工具完成: {}", text),
                            );
                        }
                        ToolResult::ok(&text)
                    }
                    Err(e) => {
                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Warn,
                                "asr-tts",
                                &format!("ASR 工具失败: {}", e),
                            );
                        }
                        ToolResult::err(&format!("ASR 识别失败: {}", e))
                    }
                }
            })
        });

        match result.join() {
            Ok(r) => r,
            Err(e) => ToolResult::err(&format!("执行线程 panic: {:?}", e)),
        }
    }
}

// ─── tts 工具（供其他插件通过 ToolRegistry 调用） ───

struct TtsTool {
    http_client: reqwest::Client,
    tts_base_url: String,
    tts_model: String,
    tts_api_key: String,
    tts_voice: String,
    tts_voice_desc: String,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for TtsTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tts_synthesize".into(),
            description: "将文本转为语音，返回 base64 编码的音频数据。".into(),
            internal: true,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "要转为语音的文本内容"
                    },
                    "voice_desc": {
                        "type": "string",
                        "description": "可选：音色描述/风格指令，不传则使用默认音色"
                    }
                },
                "required": ["text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("缺少参数: text"),
        };

        if text.is_empty() {
            return ToolResult::err("text 不能为空");
        }

        // voice_desc 可选：传了就用，没传用默认
        let custom_voice_desc: Option<String> = args
            .get("voice_desc")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let http_client = self.http_client.clone();
        let tts_base_url = self.tts_base_url.clone();
        let tts_model = self.tts_model.clone();
        let tts_api_key = self.tts_api_key.clone();
        let tts_voice = self.tts_voice.clone();
        let default_desc = self.tts_voice_desc.clone();
        let logger = self.logger.clone();

        // 同步执行 TTS（工具调用是同步的，内部用 block_on）
        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                let final_desc = custom_voice_desc.as_deref().unwrap_or(&default_desc);
                match do_tts(&http_client, &tts_base_url, &tts_model, &tts_api_key, &text, &tts_voice, Some(final_desc)).await {
                    Ok(audio_data) => {
                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Debug,
                                "asr-tts",
                                &format!("TTS 工具完成: {} bytes", audio_data.len()),
                            );
                        }
                        // 返回 base64 编码的音频数据
                        ToolResult::ok(&base64_encode(&audio_data))
                    }
                    Err(e) => {
                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Warn,
                                "asr-tts",
                                &format!("TTS 工具失败: {}", e),
                            );
                        }
                        ToolResult::err(&format!("TTS 合成失败: {}", e))
                    }
                }
            })
        });

        match result.join() {
            Ok(r) => r,
            Err(e) => ToolResult::err(&format!("执行线程 panic: {:?}", e)),
        }
    }
}

// ─── FFI ───

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(AsrTtsPlugin::new()))
}

// ─── base64 编解码 ───

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("base64 解码失败: {}", e))
}
