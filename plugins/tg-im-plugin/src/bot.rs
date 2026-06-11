//! Telegram Bot 核心逻辑
//!
//! 使用 teloxide 库与 Telegram API 交互：
//! - 接收用户私聊消息 → 发射 UserMessage 事件到宿主
//! - 提供 send_message 方法供宿主回复

use plugin_core::{AgentEvent, EventSource, EventType, LogCallback, ToolRegistry};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, InputFile, Recipient};
use teloxide::net::Download;

/// Bot 句柄，用于外部控制（如关闭）
pub struct BotHandle {
    bot: Bot,
    chat_id: Option<i64>,
}

impl BotHandle {
    pub async fn shutdown(&self) {
        if let Some(chat_id) = self.chat_id {
            let _ = self
                .bot
                .send_message(
                    Recipient::Id(teloxide::types::ChatId(chat_id)),
                    "BN Agent 正在关闭...",
                )
                .await;
        }
    }

    /// 发送消息到指定聊天
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), String> {
        self.bot
            .send_message(
                Recipient::Id(teloxide::types::ChatId(chat_id)),
                text,
            )
            .await
            .map_err(|e| format!("发送消息失败: {}", e))?;
        Ok(())
    }

    /// 发送"正在输入..."状态
    pub async fn send_typing(&self, chat_id: i64) -> Result<(), String> {
        self.bot
            .send_chat_action(
                Recipient::Id(teloxide::types::ChatId(chat_id)),
                ChatAction::Typing,
            )
            .await
            .map_err(|e| format!("发送状态失败: {}", e))?;
        Ok(())
    }

    /// 发送"正在录音..."状态
    pub async fn send_record_voice_action(&self, chat_id: i64) -> Result<(), String> {
        self.bot
            .send_chat_action(
                Recipient::Id(teloxide::types::ChatId(chat_id)),
                ChatAction::RecordVoice,
            )
            .await
            .map_err(|e| format!("发送状态失败: {}", e))?;
        Ok(())
    }

    /// 发送语音消息（OGG/OPUS 格式）
    pub async fn send_voice(&self, chat_id: i64, audio_data: Vec<u8>) -> Result<(), String> {
        let file = InputFile::memory(audio_data).file_name("voice.ogg");
        self.bot
            .send_voice(
                Recipient::Id(teloxide::types::ChatId(chat_id)),
                file,
            )
            .await
            .map_err(|e| format!("发送语音失败: {}", e))?;
        Ok(())
    }
}

/// 构建带代理的 reqwest Client（仅 TG 插件使用代理）
fn build_reqwest_client(logger: &Option<Arc<dyn LogCallback>>) -> Result<reqwest::Client, String> {
    let tg_proxy = std::env::var("TG_PROXY_URL").ok();

    let log = |level: plugin_core::LogLevel, msg: &str| {
        if let Some(ref l) = logger {
            l.log(level, "tg-im", msg);
        }
    };

    if let Some(ref url) = tg_proxy {
        let msg = format!("配置代理: {}", url);
        log(plugin_core::LogLevel::Info, &msg);
    } else {
        log(plugin_core::LogLevel::Warn, "未配置代理，使用直连");
    }

    let mut builder = reqwest::Client::builder()
        .no_proxy() // 禁用环境变量/系统代理自动检测
        .timeout(std::time::Duration::from_secs(30));
    
    if let Some(ref url) = tg_proxy {
        let proxy = reqwest::Proxy::all(url)
            .map_err(|e| {
                let msg = format!("代理配置失败 ({}): {}", url, e);
                log(plugin_core::LogLevel::Error, &msg);
                msg
            })?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| {
        let msg = format!("创建 HTTP 客户端失败: {}", e);
        log(plugin_core::LogLevel::Error, &msg);
        msg
    })
}

/// 诊断 Telegram 连接问题
fn diagnose_connection() -> String {
    let mut diag = String::new();
    
    if let Ok(_token) = std::env::var("TG_BOT_TOKEN") {
        diag.push_str(&format!("✓ TG_BOT_TOKEN 已设置\n"));
    } else {
        diag.push_str(&format!("✗ TG_BOT_TOKEN 未设置\n"));
    }
    
    if let Ok(proxy) = std::env::var("TG_PROXY_URL") {
        diag.push_str(&format!("✓ TG_PROXY_URL 已设置: {}\n", proxy));
    } else {
        diag.push_str(&format!("⚠ TG_PROXY_URL 未设置（使用直连）\n"));
    }
    
    diag.push_str("可能的问题：\n");
    diag.push_str("1. TG_BOT_TOKEN 无效或已过期\n");
    diag.push_str("2. 网络连接失败，检查代理是否运行\n");
    diag.push_str("3. DNS 解析失败\n");
    diag.push_str("4. Telegram API 服务暂时不可用\n");
    diag.push_str("5. 代理地址错误或无法连接\n");
    
    diag
}

/// 重试次数和延迟配置
const RETRY_COUNT: u32 = 3;
const RETRY_DELAY_MS: u64 = 2000;

/// OGG/OPUS → WAV 转码（TG 语音消息格式 → MiMo ASR 支持格式）
/// TG 语音消息是 OGG 容器 + OPUS 编码，需要转为 WAV(PCM I16)
fn convert_ogg_to_wav(ogg_data: &[u8]) -> Result<Vec<u8>, String> {
    use ogg::PacketReader;
    use opus::{Channels, Decoder as OpusDecoder};

    // 解析 OGG 容器，提取 OPUS 包
    let mut reader = PacketReader::new(std::io::Cursor::new(ogg_data));
    let mut opus_packets: Vec<Vec<u8>> = Vec::new();

    // OPUS 头的采样率通常是 48000 Hz
    let sample_rate = 48000u32;
    let channels = Channels::Mono;

    while let Ok(Some(packet)) = reader.read_packet() {
        if packet.data.is_empty() {
            continue;
        }
        opus_packets.push(packet.data.to_vec());
    }

    if opus_packets.is_empty() {
        return Err("OGG 文件中无音频数据包".to_string());
    }

    // OPUS 解码
    let mut decoder = OpusDecoder::new(sample_rate, channels)
        .map_err(|e| format!("创建 OPUS 解码器失败: {}", e))?;

    let mut all_samples: Vec<i16> = Vec::new();
    let frame_size = 5760; // 120ms @ 48kHz, 最大帧长

    for packet in &opus_packets {
        let mut pcm: Vec<i16> = vec![0i16; frame_size];
        match decoder.decode(packet, &mut pcm, false) {
            Ok(len) => {
                all_samples.extend_from_slice(&pcm[..len]);
            }
            Err(_e) => {
                // 跳过损坏的包
                continue;
            }
        }
    }

    if all_samples.is_empty() {
        return Err("OPUS 解码后无音频数据".to_string());
    }

    // 写入 WAV
    let mut wav_buf = Vec::new();
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(
            std::io::Cursor::new(&mut wav_buf),
            spec,
        )
        .map_err(|e| format!("创建 WAV writer 失败: {}", e))?;
        for &s in &all_samples {
            writer
                .write_sample(s)
                .map_err(|e| format!("写入 WAV 样本失败: {}", e))?;
        }
        writer
            .finalize()
            .map_err(|e| format!("完成 WAV 写入失败: {}", e))?;
    }

    Ok(wav_buf)
}

/// 同步调用 ASR 工具（避免 MutexGuard 跨 await）
fn transcribe_audio(
    registry: &Arc<std::sync::Mutex<ToolRegistry>>,
    audio_b64: &str,
    mime_type: &str,
    logger: &Option<Arc<dyn LogCallback>>,
) -> Result<String, String> {
    let exe = {
        let guard = registry
            .lock()
            .map_err(|e| format!("ToolRegistry 锁失败: {}", e))?;
        guard
            .get_executor("asr_transcribe")
            .ok_or_else(|| "asr_transcribe 工具未注册，请确保 asr-tts-plugin 已加载".to_string())?
    }; // 锁在此释放

    let args = serde_json::json!({ "audio_base64": audio_b64, "mime_type": mime_type });
    let result = exe.execute(&args);

    if !result.success {
        return Err(result.error.unwrap_or_else(|| "未知错误".to_string()));
    }

    if let Some(ref log) = logger {
        log.log(plugin_core::LogLevel::Info, "tg-im", &format!("ASR 结果: {}", result.content));
    }

    Ok(result.content)
}

/// 启动 Telegram Bot（带重试机制）
pub async fn run_bot(
    token: &str,
    emitter: Arc<dyn plugin_core::EventEmitter>,
    logger: Option<Arc<dyn LogCallback>>,
    tool_registry: Arc<std::sync::Mutex<ToolRegistry>>,
) -> Result<BotHandle, String> {
    let client = build_reqwest_client(&logger)?;
    let bot = Bot::with_client(token, client);

    let log = |level: plugin_core::LogLevel, msg: &str| {
        if let Some(ref l) = logger {
            l.log(level, "tg-im", msg);
        }
    };

    // 获取 bot 信息并打印（带重试）
    let mut last_error = String::new();
    for attempt in 1..=RETRY_COUNT {
        if let Some(ref log) = logger {
            if attempt > 1 {
                log.log(
                    plugin_core::LogLevel::Warn,
                    "tg-im",
                    &format!("重试连接 ({}/{})", attempt, RETRY_COUNT),
                );
            }
        }
        
        match bot.get_me().await {
            Ok(me) => {
                if let Some(ref log) = logger {
                    log.log(
                        plugin_core::LogLevel::Info,
                        "tg-im",
                        &format!("Bot @{} 已连接", me.username.as_deref().unwrap_or("unknown")),
                    );
                }
                
                // 连接成功，启动消息处理器
                let bot_clone = bot.clone();
                let emitter_clone = emitter.clone();
                let logger_clone = logger.clone();

                // 消息处理闭包
                let handler = move |msg: Message, bot: Bot| {
                    let emitter = emitter_clone.clone();
                    let logger = logger_clone.clone();
                    let registry = tool_registry.clone();

                    async move {
                        let chat_id = msg.chat.id.0;
                        let user_name = msg
                            .from
                            .as_ref()
                            .map(|u| u.username.as_deref().unwrap_or("unknown"))
                            .unwrap_or("unknown");

                        // ─── 语音消息 → ASR 转文字 → UserMessage ───
                        if let Some(voice) = msg.voice() {
                            if let Some(ref log) = logger {
                                log.log(
                                    plugin_core::LogLevel::Info,
                                    "tg-im",
                                    &format!("收到来自 @{} 的语音消息 ({}s)", user_name, voice.duration),
                                );
                            }

                            // 发送"正在录音..."状态
                            let _ = bot
                                .send_chat_action(
                                    Recipient::Id(teloxide::types::ChatId(chat_id)),
                                    ChatAction::Typing,
                                )
                                .await;

                            // 下载语音文件
                            match bot.get_file(voice.file.id.clone()).await {
                                Ok(tg_file) => {
                                    let mut ogg_data = Vec::new();
                                    match bot.download_file(&tg_file.path, &mut ogg_data).await {
                                        Ok(()) => {
                                            if let Some(ref log) = logger {
                                                log.log(
                                                    plugin_core::LogLevel::Debug,
                                                    "tg-im",
                                                    &format!("语音文件已下载: {} bytes (OGG)", ogg_data.len()),
                                                );
                                            }

                                            // OGG → WAV 转码（MiMo ASR 要求 wav/mp3）
                                            let wav_data = match convert_ogg_to_wav(&ogg_data) {
                                                Ok(d) => d,
                                                Err(e) => {
                                                    if let Some(ref log) = logger {
                                                        log.log(plugin_core::LogLevel::Warn, "tg-im",
                                                            &format!("OGG→WAV 转码失败: {}", e));
                                                    }
                                                    let _ = bot.send_message(
                                                        Recipient::Id(teloxide::types::ChatId(chat_id)),
                                                        "音频转码失败，请重试",
                                                    ).await;
                                                    return Ok(());
                                                }
                                            };

                                            // base64 编码 WAV
                                            let audio_b64 = {
                                                use base64::Engine;
                                                base64::engine::general_purpose::STANDARD.encode(&wav_data)
                                            };

                                            match transcribe_audio(&registry, &audio_b64, "audio/wav", &logger) {
                                                Ok(text) => {
                                                    if text.trim().is_empty() {
                                                        let _ = bot.send_message(
                                                            Recipient::Id(teloxide::types::ChatId(chat_id)),
                                                            "未识别到语音内容",
                                                        ).await;
                                                        return Ok(());
                                                    }

                                                    // 直接发射 UserMessage 事件给 LLM（不单独回复识别结果）
                                                    emitter.emit(AgentEvent::new(
                                                        EventType::UserMessage,
                                                        EventSource::Plugin("tg-im".into()),
                                                        serde_json::json!({
                                                            "platform": "telegram",
                                                            "chat_id": chat_id,
                                                            "user_name": user_name,
                                                            "text": format!("[Telegram]: {}", text),
                                                        }),
                                                    ));
                                                }
                                                Err(e) => {
                                                    if let Some(ref log) = logger {
                                                        log.log(plugin_core::LogLevel::Warn, "tg-im",
                                                            &format!("ASR 失败: {}", e));
                                                    }
                                                    let _ = bot.send_message(
                                                        Recipient::Id(teloxide::types::ChatId(chat_id)),
                                                        &format!("语音识别失败: {}", e),
                                                    ).await;
                                                }
                                            }

                                            return Ok(());
                                        }
                                        Err(e) => {
                                            if let Some(ref log) = logger {
                                                log.log(plugin_core::LogLevel::Warn, "tg-im",
                                                    &format!("下载语音文件失败: {}", e));
                                            }
                                            let _ = bot.send_message(
                                                Recipient::Id(teloxide::types::ChatId(chat_id)),
                                                "下载语音文件失败，请重试",
                                            ).await;
                                            return Ok(());
                                        }
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref log) = logger {
                                        log.log(plugin_core::LogLevel::Warn, "tg-im",
                                            &format!("获取语音文件失败: {}", e));
                                    }
                                    let _ = bot.send_message(
                                        Recipient::Id(teloxide::types::ChatId(chat_id)),
                                        "获取语音文件失败，请重试",
                                    ).await;
                                    return Ok(());
                                }
                            }
                        }

                        // ─── 文本消息（原逻辑） ───
                        let text = match msg.text() {
                            Some(t) => t.to_string(),
                            None => return Ok::<(), teloxide::RequestError>(()),
                        };

                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Info,
                                "tg-im",
                                &format!("收到来自 @{} 的消息: {}", user_name, text),
                            );
                        }

                        // 发送"正在输入..."状态
                        let _ = bot
                            .send_chat_action(
                                Recipient::Id(teloxide::types::ChatId(chat_id)),
                                ChatAction::Typing,
                            )
                            .await;

                        // 发射 UserMessage 事件到宿主（LLM 回复会通过 AssistantMessage 事件发回）
                        emitter.emit(AgentEvent::new(
                            EventType::UserMessage,
                            EventSource::Plugin("tg-im".into()),
                            serde_json::json!({
                                "platform": "telegram",
                                "chat_id": chat_id,
                                "user_name": user_name,
                                "text": format!("[Telegram]: {}", text),
                            }),
                        ));

                        Ok(())
                    }
                };

                // 创建 Dispatcher
                let mut dispatcher = Dispatcher::builder(bot_clone, Update::filter_message().branch(dptree::endpoint(handler)))
                    .build();

                let handle = BotHandle {
                    bot: bot.clone(),
                    chat_id: None,
                };

                tokio::spawn(async move {
                    dispatcher.dispatch().await;
                });

                return Ok(handle);
            }
            Err(e) => {
                last_error = format!("{}", e);
                if attempt < RETRY_COUNT {
                    tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                }
            }
        }
    }
    
    let error_msg = format!(
        "获取 Bot 信息失败（已重试 {} 次）: {}\n\n诊断信息：\n{}",
        RETRY_COUNT,
        last_error,
        diagnose_connection()
    );
    
    log(plugin_core::LogLevel::Error, &error_msg);

    Err(error_msg)
}

#[cfg(test)]
mod tests {
    
}
