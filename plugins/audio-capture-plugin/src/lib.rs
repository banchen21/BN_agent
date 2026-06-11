//! Audio Capture Plugin — 本地音频捕获插件
//!
//! 通过 WASAPI Loopback 捕获系统音频输出（如 Voicemeeter B1 总线），
//! 将 PCM 音频数据通过事件总线发送给 asr-tts-plugin 进行语音识别。
//!
//! 事件流：
//!   系统音频 → cpal loopback 捕获 → emit audio_captured → asr-tts → LLM → TTS
//!
//! 配置（环境变量）：
//!   AUDIO_CAPTURE_DEVICE   - 录音设备名称关键字（默认 "Voicemeeter"）
//!   AUDIO_CAPTURE_CHUNK_MS - 每次发送的音频块时长毫秒（默认 500）

use plugin_core::{
    AgentEvent, EventSource, EventType, HostContext, Plugin, PluginError, PluginMeta,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioCapturePlugin {
    meta: PluginMeta,
    ctx: Option<HostContext>,
    /// 捕获是否运行中
    running: Arc<AtomicBool>,
}

impl AudioCapturePlugin {
    pub fn new() -> Self {
        Self {
            meta: PluginMeta {
                name: "audio-capture-plugin".into(),
                version: "0.1.0".into(),
                description: "本地音频捕获插件（WASAPI Loopback）".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            ctx: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Plugin for AudioCapturePlugin {
    fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    fn init(&mut self, ctx: &HostContext) -> Result<(), PluginError> {
        ctx.log_info("audio-capture", "AudioCapturePlugin 初始化完成");
        self.ctx = Some(ctx.clone());
        Ok(())
    }

    fn start(&mut self) -> Result<(), PluginError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| PluginError::InitError("未初始化".into()))?;

        ctx.log_info("audio-capture", "AudioCapturePlugin 正在启动音频捕获...");

        let emitter = ctx
            .emitter
            .clone()
            .ok_or_else(|| PluginError::InitError("EventEmitter 未注入".into()))?;

        let logger = ctx.logger.clone();
        let running = self.running.clone();

        // 读取配置
        let device_keyword = std::env::var("AUDIO_CAPTURE_DEVICE")
            .unwrap_or_else(|_| "Voicemeeter".into());
        let chunk_ms: u64 = std::env::var("AUDIO_CAPTURE_CHUNK_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        running.store(true, Ordering::SeqCst);

        // 在独立线程中运行音频捕获（cpal 需要自己的事件循环）
        std::thread::spawn(move || {
            if let Err(e) = run_capture_loop(&device_keyword, chunk_ms, emitter, logger, running) {
                if let Some(ref log) = logger {
                    log.log(
                        plugin_core::LogLevel::Error,
                        "audio-capture",
                        &format!("音频捕获失败: {}", e),
                    );
                }
            }
        });

        ctx.log_info(
            "audio-capture",
            &format!(
                "音频捕获已启动: device_keyword={}, chunk_ms={}",
                device_keyword, chunk_ms
            ),
        );

        Ok(())
    }

    fn stop(&mut self) -> Result<(), PluginError> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(ref ctx) = self.ctx {
            ctx.log_info("audio-capture", "AudioCapturePlugin 已停止");
        }
        Ok(())
    }

    fn on_event(&self, event: &AgentEvent) -> bool {
        // 处理 TTS 音频播放（本地通道）
        match &event.event_type {
            EventType::Custom(custom) if custom == "local_audio_play" => {
                let audio_b64 = match event.data.get("data").and_then(|v| v.as_str()) {
                    Some(d) => d.to_string(),
                    None => return true,
                };
                let audio_data = match base64_decode(&audio_b64) {
                    Ok(d) => d,
                    Err(e) => {
                        if let Some(ref ctx) = self.ctx {
                            ctx.log_warn("audio-capture", &format!("base64 解码失败: {}", e));
                        }
                        return true;
                    }
                };

                let logger = self.ctx.as_ref().and_then(|c| c.logger.clone());
                // 异步播放到 VB-Cable 虚拟设备
                tokio::spawn(async move {
                    if let Err(e) = play_audio_to_device(&audio_data).await {
                        if let Some(ref log) = logger {
                            log.log(
                                plugin_core::LogLevel::Warn,
                                "audio-capture",
                                &format!("播放音频失败: {}", e),
                            );
                        }
                    }
                });
            }
            _ => {}
        }
        true
    }
}

// ─── 音频捕获循环 ────────────────────────────────────────────────

fn run_capture_loop(
    device_keyword: &str,
    chunk_ms: u64,
    emitter: Arc<dyn plugin_core::EventEmitter>,
    logger: Option<Arc<dyn plugin_core::LogCallback>>,
    running: Arc<AtomicBool>,
) -> Result<(), String> {
    let host = cpal::default_host();

    // 枚举录音设备，找到匹配关键字的
    let device = find_device(&host, device_keyword)?;

    if let Some(ref log) = logger {
        log.log(
            plugin_core::LogLevel::Info,
            "audio-capture",
            &format!("使用设备: {}", device.name().unwrap_or_default()),
        );
    }

    // 获取默认配置
    let config = device
        .default_input_config()
        .map_err(|e| format!("无法获取设备配置: {}", e))?;

    if let Some(ref log) = logger {
        log.log(
            plugin_core::LogLevel::Info,
            "audio-capture",
            &format!(
                "音频格式: {:?}, {} channels, {} Hz",
                config.sample_format(),
                config.channels(),
                config.sample_rate().0
            ),
        );
    }

    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as u32;
    let samples_per_chunk = (sample_rate * chunk_ms as u32 / 1000) as usize;

    // 音频缓冲区：累积 PCM 样本直到达到 chunk 大小
    let buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let buffer_clone = buffer.clone();
    let emitter_clone = emitter.clone();
    let logger_clone = logger.clone();
    let running_clone = running.clone();

    // 构建输入流
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !running_clone.load(Ordering::SeqCst) {
                        return;
                    }
                    let mut buf = buffer_clone.lock().unwrap();
                    buf.extend_from_slice(data);

                    // 累积够一个 chunk 就发送
                    while buf.len() >= samples_per_chunk * channels as usize {
                        let chunk: Vec<f32> =
                            buf.drain(..samples_per_chunk * channels as usize).collect();

                        // 如果立体声，取左声道或混合
                        let mono: Vec<f32> = if channels >= 2 {
                            chunk
                                .chunks(channels as usize)
                                .map(|frame| frame[0])
                                .collect()
                        } else {
                            chunk
                        };

                        // f32 PCM → i16 PCM（Whisper API 常用格式）
                        let pcm_i16: Vec<i16> = mono
                            .iter()
                            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                            .collect();

                        // i16 → u8 bytes (little-endian)
                        let pcm_bytes: Vec<u8> = pcm_i16
                            .iter()
                            .flat_map(|s| s.to_le_bytes())
                            .collect();

                        let b64 = base64_encode(&pcm_bytes);

                        emitter_clone.emit(AgentEvent::new(
                            EventType::Custom("audio_captured".into()),
                            EventSource::Plugin("audio-capture".into()),
                            serde_json::json!({
                                "data": b64,
                                "sample_rate": sample_rate,
                                "channels": 1,
                                "format": "pcm_i16",
                                "source": "local",
                            }),
                        ));
                    }
                },
                move |err| {
                    if let Some(ref log) = logger_clone {
                        log.log(
                            plugin_core::LogLevel::Warn,
                            "audio-capture",
                            &format!("音频流错误: {}", err),
                        );
                    }
                },
                None,
            )
            .map_err(|e| format!("无法创建音频流: {}", e))?,
        _ => {
            return Err(format!(
                "不支持的采样格式: {:?}（需要 F32）",
                config.sample_format()
            ));
        }
    };

    // 启动流
    stream.play().map_err(|e| format!("无法启动音频流: {}", e))?;

    if let Some(ref log) = logger {
        log.log(
            plugin_core::LogLevel::Info,
            "audio-capture",
            "音频流已启动，等待数据...",
        );
    }

    // 保持线程存活，直到 stop 被调用
    while running.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    drop(stream);

    if let Some(ref log) = logger {
        log.log(
            plugin_core::LogLevel::Info,
            "audio-capture",
            "音频流已关闭",
        );
    }

    Ok(())
}

/// 查找名称包含指定关键字的录音设备
fn find_device(
    host: &cpal::Host,
    keyword: &str,
) -> Result<cpal::Device, String> {
    let keyword_lower = keyword.to_lowercase();

    // 先尝试精确匹配
    for device in host.input_devices().map_err(|e| format!("枚举设备失败: {}", e))? {
        if let Ok(name) = device.name() {
            if name.to_lowercase().contains(&keyword_lower) {
                return Ok(device);
            }
        }
    }

    // 没找到匹配的，列出所有可用设备并返回默认
    let devices: Vec<String> = host
        .input_devices()
        .map(|iter| {
            iter.filter_map(|d| d.name().ok())
                .collect()
        })
        .unwrap_or_default();

    let device_list = devices.join(", ");

    // 回退到默认输入设备
    host.default_input_device()
        .ok_or_else(|| {
            format!(
                "未找到包含 '{}' 的录音设备，且无默认设备。可用设备: [{}]",
                keyword, device_list
            )
        })
}

// ─── 音频播放（TTS 输出到 VB-Cable） ─────────────────────────────

async fn play_audio_to_device(audio_data: &[u8]) -> Result<(), String> {
    // 将 PCM i16 字节转回 f32 样本
    if audio_data.len() < 2 {
        return Ok(());
    }

    let samples: Vec<f32> = audio_data
        .chunks_exact(2)
        .map(|chunk| {
            let val = i16::from_le_bytes([chunk[0], chunk[1]]);
            val as f32 / 32767.0
        })
        .collect();

    let device_keyword = std::env::var("AUDIO_PLAYBACK_DEVICE")
        .unwrap_or_else(|_| "CABLE".into());

    let host = cpal::default_host();
    let device = find_output_device(&host, &device_keyword)?;

    let config = device
        .default_output_config()
        .map_err(|e| format!("无法获取播放设备配置: {}", e))?;

    let sample_rate = config.sample_rate();

    // 使用 spawn_blocking 因为 cpal 的流操作是阻塞的
    let samples_clone = samples.clone();
    tokio::task::spawn_blocking(move || {
        let stream = device
            .build_output_stream(
                &config.into(),
                move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // 简单地把样本复制到输出缓冲区（循环播放）
                    for (out_sample, in_sample) in output.iter_mut().zip(
                        samples_clone.iter().cycle()
                    ) {
                        *out_sample = *in_sample;
                    }
                },
                |err| {
                    eprintln!("播放错误: {}", err);
                },
                None,
            )
            .map_err(|e| format!("无法创建播放流: {}", e))?;

        stream.play().map_err(|e| format!("无法启动播放流: {}", e))?;

        // 播放足够长的时间（根据样本数估算）
        let duration_ms = (samples_clone.len() as u64 * 1000 / sample_rate.0 as u64) + 200;
        std::thread::sleep(std::time::Duration::from_millis(duration_ms));

        Ok::<_, String>(())
    })
    .await
    .map_err(|e| format!("播放线程 panic: {}", e))?
}

fn find_output_device(host: &cpal::Host, keyword: &str) -> Result<cpal::Device, String> {
    let keyword_lower = keyword.to_lowercase();

    for device in host.output_devices().map_err(|e| format!("枚举输出设备失败: {}", e))? {
        if let Ok(name) = device.name() {
            if name.to_lowercase().contains(&keyword_lower) {
                return Ok(device);
            }
        }
    }

    host.default_output_device()
        .ok_or_else(|| format!("未找到包含 '{}' 的输出设备，且无默认设备", keyword))
}

// ─── base64 ──────────────────────────────────────────────────────

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

// ─── FFI 导出 ────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(AudioCapturePlugin::new()))
}
