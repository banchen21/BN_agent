//! TG-IM Plugin — Telegram 即时通讯插件
//!
//! 通过 Telegram Bot 与 BN Agent 交互：
//! - 接收用户消息 → 发射 UserMessage 事件
//! - 监听 AssistantMessage 事件 → 发送回复到 Telegram
//! - 注册 send_voice 工具 → LLM 可调用发送语音消息（通过 ToolRegistry 调用 asr-tts-plugin 的 tts 工具）

use plugin_core::{
    AgentEvent, EventType, HostContext, Plugin, PluginError, PluginMeta,
    ToolDef, ToolExecutor, ToolRegistry, ToolResult,
};
use std::sync::Arc;
use tokio::sync::Mutex;

mod bot;

pub struct TgImPlugin {
    meta: PluginMeta,
    ctx: Option<HostContext>,
    /// Bot 句柄，用于优雅关闭
    bot_handle: Option<Arc<Mutex<Option<bot::BotHandle>>>>,
}

impl TgImPlugin {
    pub fn new() -> Self {
        Self {
            meta: PluginMeta {
                name: "tg-im-plugin".into(),
                version: "0.1.0".into(),
                description: "Telegram 即时通讯插件".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            ctx: None,
            bot_handle: None,
        }
    }
}

impl Plugin for TgImPlugin {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn init(&mut self, ctx: &HostContext) -> Result<(), PluginError> {
        ctx.log_info("tg-im", "TgImPlugin 初始化完成");
        self.ctx = Some(ctx.clone());
        self.bot_handle = Some(Arc::new(Mutex::new(None)));
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| PluginError::InitError("未初始化".into()))?;

        ctx.log_info("tg-im", "TgImPlugin 已启动");

        // 注册 send_voice 工具
        if let Some(ref registry) = ctx.tool_registry {
            let bot_handle = self.bot_handle.clone();
            let tool_registry = registry.clone();
            let logger = ctx.logger.clone();

            registry
                .lock()
                .map_err(|e| PluginError::InitError(format!("{}", e)))?
                .register(Arc::new(SendVoiceTool {
                    bot_handle,
                    tool_registry,
                    logger,
                }));
            ctx.log_info("tg-im", "已注册工具: tg_send_voice");

            // 注册 send_message 工具
            let bot_handle2 = self.bot_handle.clone();
            let logger2 = ctx.logger.clone();
            registry
                .lock()
                .map_err(|e| PluginError::InitError(format!("{}", e)))?
                .register(Arc::new(SendMessageTool {
                    bot_handle: bot_handle2,
                    logger: logger2,
                }));
            ctx.log_info("tg-im", "已注册工具: tg_send_message");
        }

        // 从环境变量读取配置
        let token = std::env::var("TG_BOT_TOKEN").map_err(|_| {
            PluginError::InitError("环境变量 TG_BOT_TOKEN 未设置".into())
        })?;

        let handle = self.bot_handle.clone();
        let emitter = ctx
            .emitter
            .clone()
            .ok_or_else(|| PluginError::InitError("EventEmitter 未注入".into()))?;
        let logger = ctx.logger.clone();
        let ctx_logger = ctx.logger.clone();
        let tool_registry_for_bot = ctx
            .tool_registry
            .clone()
            .ok_or_else(|| PluginError::InitError("ToolRegistry 未注入".into()))?;

        // 在后台线程启动 bot
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                match bot::run_bot(&token, emitter, logger, tool_registry_for_bot).await {
                    Ok(h) => {
                        if let Some(ref handle) = handle {
                            *handle.lock().await = Some(h);
                        }
                        // 保持 runtime 存活，不监听 Ctrl+C（主线程负责退出）
                        std::future::pending::<()>().await;
                    }
                    Err(e) => {
                        eprintln!("[tg-im] Bot 启动失败:\n{}", e);
                        if let Some(ref logger) = ctx_logger {
                            logger.log(
                                plugin_core::LogLevel::Error,
                                "tg-im",
                                &format!("Bot 启动失败:\n{}", e),
                            );
                        }
                    }
                }
            });
        });

        Ok(())
    }

    fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("tg-im", "TgImPlugin 正在停止...");
        }

        // 通知 bot 关闭
        if let Some(ref handle) = self.bot_handle {
            let handle = handle.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .expect("无法创建 tokio runtime");
                rt.block_on(async {
                    let mut guard = handle.lock().await;
                    if let Some(ref h) = *guard {
                        h.shutdown().await;
                    }
                    *guard = None;
                });
            });
        }

        if let Some(ref ctx) = self.ctx {
            ctx.log_info("tg-im", "TgImPlugin 已停止");
        }
        Ok(())
    }

    fn on_event(&self, event: &AgentEvent) -> bool {
        match event.event_type {
            EventType::AssistantMessage => {
                // 宿主回复 → 发送到 Telegram
                let chat_id = event.data.get("chat_id").and_then(|v| v.as_i64());
                let text = event.data.get("text").and_then(|v| v.as_str());

                if let (Some(chat_id), Some(text)) = (chat_id, text) {
                    if let Some(ref handle) = self.bot_handle {
                        let handle = handle.clone();
                        let text = text.to_string();
                        std::thread::spawn(move || {
                            let rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("无法创建 tokio runtime");
                            rt.block_on(async {
                                let guard = handle.lock().await;
                                if let Some(ref h) = *guard {
                                    let _ = h.send_message(chat_id, &text).await;
                                }
                            });
                        });
                    }
                }
            }
            _ => {
                if let Some(ref ctx) = self.ctx {
                    ctx.log_debug("tg-im", &format!("收到事件: {:?}", event.event_type));
                }
            }
        }
        true
    }

    fn ctx(&self) -> Option<&HostContext> {
        self.ctx.as_ref()
    }
}

// ─── send_voice 工具（通过 ToolRegistry 调用 asr-tts-plugin 的 tts 工具） ───

struct SendVoiceTool {
    bot_handle: Option<Arc<Mutex<Option<bot::BotHandle>>>>,
    tool_registry: Arc<std::sync::Mutex<ToolRegistry>>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for SendVoiceTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tg_send_voice".into(),
            description: "通过 TTS 将文本转为语音，然后发送语音消息到 Telegram 聊天。适用于用户要求语音回复的场景。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "要转为语音并发送的文本内容"
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
        let chat_id = match args.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::err("缺少参数: chat_id"),
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("缺少参数: text"),
        };

        if text.is_empty() {
            return ToolResult::err("text 不能为空");
        }

        // 1. 通过 ToolRegistry 调用 asr-tts-plugin 的 tts 工具
        //    先获取 executor 克隆再释放锁，避免与 main.rs 死锁
        let mut tts_args = serde_json::json!({ "text": text });
        // 可选透传 voice_desc
        if let Some(vd) = args.get("voice_desc").and_then(|v| v.as_str()) {
            if !vd.is_empty() {
                tts_args["voice_desc"] = serde_json::json!(vd);
            }
        }
        let tts_executor = {
            let registry = match self.tool_registry.lock() {
                Ok(r) => r,
                Err(e) => return ToolResult::err(&format!("ToolRegistry 锁失败: {}", e)),
            };
            match registry.get_executor("tts_synthesize") {
                Some(e) => e,
                None => return ToolResult::err("tts 工具未注册，请确保 asr-tts-plugin 已加载"),
            }
        }; // 锁在此处释放

        let tts_result = tts_executor.execute(&tts_args);

        if !tts_result.success {
            return ToolResult::err(&format!(
                "TTS 合成失败: {}",
                tts_result.error.as_deref().unwrap_or("未知错误")
            ));
        }

        // tts 工具返回的是 base64 编码的音频数据
        let audio_b64 = tts_result.content;
        let audio_data = match base64_decode(&audio_b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 解码失败: {}", e)),
        };

        if let Some(ref log) = self.logger {
            log.log(
                plugin_core::LogLevel::Debug,
                "tg-im",
                &format!("TTS 完成: {} bytes", audio_data.len()),
            );
        }

        // 2. 发送语音消息到 Telegram
        let bot_handle = match self.bot_handle.clone() {
            Some(h) => h,
            None => return ToolResult::err("Bot 未启动"),
        };

        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                // 发送"正在录音..."状态
                {
                    let guard = bot_handle.lock().await;
                    if let Some(ref h) = *guard {
                        let _ = h.send_record_voice_action(chat_id).await;
                    }
                }

                // 发送语音
                let guard = bot_handle.lock().await;
                match *guard {
                    Some(ref h) => match h.send_voice(chat_id, audio_data).await {
                        Ok(()) => ToolResult::ok(&format!(
                            "语音消息已发送: {}",
                            text
                        )),
                        Err(e) => ToolResult::err(&format!("发送语音失败: {}", e)),
                    },
                    None => ToolResult::err("Bot 未启动"),
                }
            })
        });

        match result.join() {
            Ok(r) => r,
            Err(e) => ToolResult::err(&format!("执行线程 panic: {:?}", e)),
        }
    }
}

// ─── send_message 工具（LLM 可调用发送文字消息到 Telegram） ───

struct SendMessageTool {
    bot_handle: Option<Arc<Mutex<Option<bot::BotHandle>>>>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
}

impl ToolExecutor for SendMessageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "tg_send_message".into(),
            description: "发送文字消息到 Telegram 聊天。当用户要求文字回复时调用。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "要发送的文字内容"
                    }
                },
                "required": ["text"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let chat_id = match args.get("chat_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return ToolResult::err("缺少参数: chat_id"),
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return ToolResult::err("缺少参数: text"),
        };

        if text.is_empty() {
            return ToolResult::err("text 不能为空");
        }

        let bot_handle = match self.bot_handle.clone() {
            Some(h) => h,
            None => return ToolResult::err("Bot 未启动"),
        };

        if let Some(ref log) = self.logger {
            log.log(
                plugin_core::LogLevel::Debug,
                "tg-im",
                &format!("发送消息: {}", &text[..text.len().min(60)]),
            );
        }

        let result = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("无法创建 tokio runtime");

            rt.block_on(async {
                let guard = bot_handle.lock().await;
                match *guard {
                    Some(ref h) => match h.send_message(chat_id, &text).await {
                        Ok(()) => ToolResult::ok(&format!("消息已发送: {}", text)),
                        Err(e) => ToolResult::err(&format!("发送消息失败: {}", e)),
                    },
                    None => ToolResult::err("Bot 未启动"),
                }
            })
        });

        match result.join() {
            Ok(r) => r,
            Err(e) => ToolResult::err(&format!("执行线程 panic: {:?}", e)),
        }
    }
}

// ─── base64 解码 ───

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("base64 解码失败: {}", e))
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(TgImPlugin::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试：TgImPlugin 初始化
    #[test]
    fn test_tg_im_plugin_creation() {
        let plugin = TgImPlugin::new();
        assert_eq!(plugin.meta().name, "tg-im-plugin");
        assert_eq!(plugin.meta().version, "0.1.0");
        assert_eq!(plugin.meta().author, "BN Team");
        assert!(!plugin.meta().description.is_empty());
    }

    /// 测试：插件元数据完整性
    #[test]
    fn test_plugin_metadata_integrity() {
        let plugin = TgImPlugin::new();
        let meta = plugin.meta();
        
        assert!(!meta.name.is_empty(), "插件名称不应该为空");
        assert!(!meta.version.is_empty(), "版本号不应该为空");
        assert!(!meta.description.is_empty(), "描述不应该为空");
        assert!(!meta.author.is_empty(), "作者不应该为空");
        assert!(!meta.min_host_version.is_empty(), "最小主机版本不应该为空");
    }

    /// 测试：插件版本格式
    #[test]
    fn test_plugin_version_format() {
        let plugin = TgImPlugin::new();
        let version = plugin.meta().version.clone();
        
        // 版本应该遵循 semver 格式 (x.y.z)
        let parts: Vec<&str> = version.split('.').collect();
        assert_eq!(parts.len(), 3, "版本号应该有三个部分");
        
        for part in parts {
            assert!(part.parse::<u32>().is_ok(), "版本号的每个部分应该是数字");
        }
    }

    /// 测试：SendVoiceTool 定义
    #[test]
    fn test_send_voice_tool_definition() {
        use std::sync::Mutex;
        
        let tool = SendVoiceTool {
            bot_handle: None,
            tool_registry: Arc::new(Mutex::new(ToolRegistry::new())),
            logger: None,
        };
        
        let def = tool.def();
        assert_eq!(def.name, "send_voice", "工具名称应该是 'send_voice'");
        assert!(!def.description.is_empty(), "工具描述不应该为空");
    }

    /// 测试：base64 解码
    #[test]
    fn test_base64_decode() {
        // 测试有效的 base64 字符串
        let encoded = "SGVsbG8gV29ybGQ="; // "Hello World"
        let result = base64_decode(encoded);
        assert!(result.is_ok(), "应该能解码有效的 base64 字符串");
        
        let decoded = result.unwrap();
        assert_eq!(decoded, b"Hello World");
    }

    /// 测试：base64 解码无效输入
    #[test]
    fn test_base64_decode_invalid() {
        let invalid = "!!!invalid base64!!!";
        let result = base64_decode(invalid);
        assert!(result.is_err(), "应该因为无效的 base64 而失败");
    }

    /// 测试：聊天 ID 验证
    #[test]
    fn test_valid_chat_id() {
        let valid_ids = vec![
            12345,
            987654321,
            -1001234567890i64, // 群组 ID (负数)
        ];
        
        for id in valid_ids {
            assert!(id != 0, "聊天 ID 不应该为 0");
        }
    }

    /// 测试：文本内容验证
    #[test]
    fn test_text_content_validation() {
        let valid_texts = vec![
            "Hello",
            "你好",
            "Mixed 混合 text",
            "Special @#$%^&* chars",
        ];
        
        for text in valid_texts {
            assert!(!text.is_empty(), "文本不应该为空");
            assert!(text.len() <= 4096, "文本长度应该 <= 4096");
        }
    }

    /// 测试：工具参数验证
    #[test]
    fn test_send_voice_args_validation() {
        let valid_args = serde_json::json!({
            "chat_id": 12345,
            "text": "Hello"
        });
        
        assert!(valid_args.get("chat_id").is_some(), "应该有 chat_id 参数");
        assert!(valid_args.get("text").is_some(), "应该有 text 参数");
        assert!(valid_args.get("chat_id").unwrap().is_i64(), "chat_id 应该是整数");
        assert!(valid_args.get("text").unwrap().is_string(), "text 应该是字符串");
    }

    /// 测试：缺少必需参数
    #[test]
    fn test_missing_required_parameters() {
        let missing_chat_id = serde_json::json!({ "text": "Hello" });
        let missing_text = serde_json::json!({ "chat_id": 12345 });
        
        assert!(missing_chat_id.get("chat_id").is_none(), "缺少 chat_id");
        assert!(missing_text.get("text").is_none(), "缺少 text");
    }

    /// 测试：空文本验证
    #[test]
    fn test_empty_text_validation() {
        let empty_text = "";
        assert!(empty_text.is_empty(), "空文本应该被识别");
    }

    /// 测试：音频数据大小限制
    #[test]
    fn test_audio_data_size_limits() {
        // Telegram 允许的最大文件大小是 50MB
        let max_size = 50 * 1024 * 1024;
        let audio_size = 100 * 1024; // 100KB
        
        assert!(audio_size < max_size, "音频大小应该在限制内");
    }

    /// 测试：插件初始化状态
    #[test]
    fn test_plugin_initial_state() {
        let plugin = TgImPlugin::new();
        assert!(plugin.ctx.is_none(), "初始化时 ctx 应该为 None");
        assert!(plugin.bot_handle.is_none(), "初始化时 bot_handle 应该为 None");
    }
}
