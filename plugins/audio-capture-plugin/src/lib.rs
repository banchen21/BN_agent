//! Audio Capture Plugin — actor-free port.
//!
//! Captures system audio via WASAPI Loopback (cpal) and emits `audio_captured`
//! events so asr-tts-plugin can transcribe them.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plugin_interface::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioCapturePlugin {
    info: PluginInfo,
    running: Arc<AtomicBool>,
    event_bus: Option<Addr<EventBus>>,
}

impl AudioCapturePlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "audio-capture-plugin".into(),
                version: "0.1.0".into(),
                description: "本地音频捕获 (WASAPI Loopback)".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            running: Arc::new(AtomicBool::new(false)),
            event_bus: None,
        }
    }
}

impl Plugin for AudioCapturePlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        self.event_bus = Some(ctx.event_bus.clone());
        let eb = ctx.event_bus.clone();
        let running = self.running.clone();
        let device_keyword = std::env::var("AUDIO_CAPTURE_DEVICE")
            .unwrap_or_else(|_| "Voicemeeter".into());
        let chunk_ms: u64 = std::env::var("AUDIO_CAPTURE_CHUNK_MS")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(500);
        let dk = device_keyword.clone();

        running.store(true, Ordering::SeqCst);

        std::thread::spawn(move || {
            if let Err(e) = run_capture(&dk, chunk_ms, eb, running) {
                log::error!("[audio-capture] {}", e);
            }
        });

        log::info!("[audio-capture] started: device={} chunk={}ms", device_keyword, chunk_ms);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        log::info!("[audio-capture] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "local_audio_play" {
            let audio_b64 = match event.data.get("data").and_then(|v| v.as_str()) {
                Some(d) => d.to_string(), None => return true,
            };
            let audio = match base64_decode(&audio_b64) {
                Ok(d) => d, Err(e) => { log::warn!("[audio-capture] b64: {}", e); return true; }
            };
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all().build().expect("tokio");
                rt.block_on(async { play_audio(&audio).await; });
            });
        }
        true
    }
}

// ─── 音频捕获 ──────────────────────────────────────────────────────

fn run_capture(
    keyword: &str, chunk_ms: u64,
    eb: Addr<EventBus>, running: Arc<AtomicBool>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = find_input_device(&host, keyword)?;

    let config = device.default_input_config()
        .map_err(|e| format!("config: {}", e))?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as u32;
    let samples_per_chunk = (sample_rate * chunk_ms as u32 / 1000) as usize;

    let buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let buf_clone = buffer.clone();
    let eb_clone = eb.clone();
    let running_clone = running.clone();

    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("unsupported sample format: {:?}", config.sample_format()));
    }

    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if !running_clone.load(Ordering::SeqCst) { return; }
            let mut buf = buf_clone.lock().unwrap();
            buf.extend_from_slice(data);
            while buf.len() >= samples_per_chunk * channels as usize {
                let chunk: Vec<f32> = buf.drain(..samples_per_chunk * channels as usize).collect();
                let mono: Vec<f32> = if channels >= 2 {
                    chunk.chunks(channels as usize).map(|f| f[0]).collect()
                } else { chunk };
                let pcm_i16: Vec<i16> = mono.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
                let pcm_bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();

                eb_clone.do_send(Event::new(
                    "audio_captured",
                    serde_json::json!({
                        "data": base64_encode(&pcm_bytes),
                        "sample_rate": sample_rate,
                        "channels": 1,
                        "format": "pcm_i16",
                        "source": "local",
                    }),
                    "audio-capture-plugin",
                ));
            }
        },
        move |err| log::warn!("[audio-capture] stream error: {}", err),
        None,
    ).map_err(|e| format!("build stream: {}", e))?;

    stream.play().map_err(|e| format!("play: {}", e))?;
    log::info!("[audio-capture] streaming ({}Hz, {}ch)", sample_rate, channels);

    while running.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    drop(stream);
    log::info!("[audio-capture] stream closed");
    Ok(())
}

fn find_input_device(host: &cpal::Host, keyword: &str) -> Result<cpal::Device, String> {
    let kw = keyword.to_lowercase();
    for d in host.input_devices().map_err(|e| format!("enum: {}", e))? {
        if let Ok(name) = d.name() {
            if name.to_lowercase().contains(&kw) { return Ok(d); }
        }
    }
    host.default_input_device()
        .ok_or_else(|| {
            let list: Vec<String> = host.input_devices().ok()
                .map(|iter| iter.filter_map(|d| d.name().ok()).collect())
                .unwrap_or_default();
            format!("device '{}' not found; available: [{}]", keyword, list.join(", "))
        })
}

// ─── 音频播放（TTS输出到VB-Cable） ──────────────────────────────

async fn play_audio(audio_data: &[u8]) {
    if audio_data.len() < 2 { return; }
    let samples: Vec<f32> = audio_data.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0)
        .collect();

    let device_keyword = std::env::var("AUDIO_PLAYBACK_DEVICE").unwrap_or_else(|_| "CABLE".into());
    let host = cpal::default_host();
    let device = match find_output_device(&host, &device_keyword) {
        Ok(d) => d, Err(e) => { log::warn!("[audio-capture] play device: {}", e); return; }
    };
    let config = match device.default_output_config() {
        Ok(c) => c, Err(e) => { log::warn!("[audio-capture] play config: {}", e); return; }
    };
    let rate = config.sample_rate().0;

    let samples = std::sync::Arc::new(samples);
    let sc = samples.clone();
    let stream = match device.build_output_stream(
        &config.into(),
        move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
            for (out, inp) in output.iter_mut().zip(sc.iter().cycle()) { *out = *inp; }
        },
        |err| log::warn!("[audio-capture] play err: {}", err),
        None,
    ) {
        Ok(s) => s,
        Err(e) => { log::warn!("[audio-capture] build output: {}", e); return; }
    };
    let _ = stream.play();
    let dur = (samples.len() as u64 * 1000 / rate as u64) + 200;
    tokio::time::sleep(tokio::time::Duration::from_millis(dur)).await;
}

fn find_output_device(host: &cpal::Host, keyword: &str) -> Result<cpal::Device, String> {
    let kw = keyword.to_lowercase();
    for d in host.output_devices().map_err(|e| format!("enum: {}", e))? {
        if let Ok(name) = d.name() {
            if name.to_lowercase().contains(&kw) { return Ok(d); }
        }
    }
    host.default_output_device()
        .ok_or_else(|| format!("output '{}' not found", keyword))
}

// ─── base64 ────────────────────────────────────────────────────────

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
        .map_err(|e| format!("base64: {}", e))
}

// ─── FFI ─────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> { Box::new(AudioCapturePlugin::new()) }

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
