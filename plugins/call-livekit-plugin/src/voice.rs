//! 音频处理纯函数：重采样到 16k mono、能量 VAD 切句、WAV 编解码。
//!
//! 不依赖 WebRTC，便于单测：调用方把任意采样率的 PCM 帧喂进来，`Vad` 负责
//! 按「有人声 / 静音」切出句子，`pcm16_to_wav` 负责编码成 ASR 接口要的 WAV。

/// 线性插值重采样（单声道）。`from_rate == 0` 时返回空。
pub fn resample_mono(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == 0 || to_rate == 0 || input.is_empty() {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = pos - idx as f64;
        let a = input[idx.min(input.len() - 1)] as f64;
        let b = input[(idx + 1).min(input.len() - 1)] as f64;
        out.push((a + (b - a) * frac).round().clamp(-32768.0, 32767.0) as i16);
    }
    out
}

/// 多声道交错 PCM 下混为单声道。
pub fn downmix(interleaved: &[i16], channels: u32) -> Vec<i16> {
    match channels {
        0 | 1 => interleaved.to_vec(),
        n => interleaved
            .chunks_exact(n as usize)
            .map(|frame| {
                let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                (sum / n as i32).clamp(-32768, 32767) as i16
            })
            .collect(),
    }
}

/// 16-bit PCM 单声道 WAV 编码。
pub fn pcm16_to_wav(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// 解析 WAV（PCM / IEEE float）为 `(samples, sample_rate, channels)`。
///
/// 只支持 8/16/24/32-bit PCM 与 32-bit float；ffmpeg 转出来的 16-bit PCM 是常见路径。
pub fn wav_to_pcm16(bytes: &[u8]) -> Result<(Vec<i16>, u32, u32), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("不是 WAV 数据".into());
    }
    let mut pos = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 0u32;
    let mut bits = 0u16;
    let mut format = 0u16;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"fmt " && size >= 16 && body + 16 <= bytes.len() {
            format = u16::from_le_bytes(bytes[body..body + 2].try_into().unwrap());
            channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().unwrap()) as u32;
            sample_rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
            bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap());
        } else if id == b"data" {
            let end = (body + size).min(bytes.len());
            let data = &bytes[body..end];
            let samples = decode_pcm(data, format, bits)?;
            return Ok((samples, sample_rate, channels.max(1)));
        }
        pos = body + size + (size & 1);
    }
    Err("WAV 缺少 data 块".into())
}

fn decode_pcm(data: &[u8], format: u16, bits: u16) -> Result<Vec<i16>, String> {
    match (format, bits) {
        (1, 16) => Ok(data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()),
        (1, 8) => Ok(data.iter().map(|&b| ((b as i16) - 128) << 8).collect()),
        (1, 24) => Ok(data
            .chunks_exact(3)
            .map(|b| i16::from_le_bytes([b[1], b[2]]))
            .collect()),
        (1, 32) => Ok(data
            .chunks_exact(4)
            .map(|b| (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 16) as i16)
            .collect()),
        (3, 32) => Ok(data
            .chunks_exact(4)
            .map(|b| {
                let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                (f.clamp(-1.0, 1.0) * 32767.0) as i16
            })
            .collect()),
        _ => Err(format!("不支持的 WAV 格式：format={format} bits={bits}")),
    }
}

/// 能量 VAD：按固定帧长计算 RMS，连续有声 → 组句；静音超过 `silence_ms` 收句。
pub struct Vad {
    sample_rate: u32,
    frame_ms: u32,
    energy_threshold: f64,
    min_speech_ms: u32,
    min_silence_ms: u32,
    max_utterance_ms: u32,
    /// 当前句子的 PCM（16k mono）。
    current: Vec<i16>,
    /// 当前帧内部缓冲（不足一帧时累积）。
    pending: Vec<i16>,
    /// 当前句中已经连续静音的帧数。
    silent_frames: u32,
    /// 当前句中有声帧数（用于过滤咳嗽/碰撞等短噪声）。
    speech_frames: u32,
    /// 当前句中总共的帧数（用于超长截断）。
    total_frames: u32,
    in_speech: bool,
}

impl Vad {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            frame_ms: 20,
            energy_threshold: 380.0,
            min_speech_ms: 200,
            min_silence_ms: 600,
            max_utterance_ms: 30_000,
            current: Vec::new(),
            pending: Vec::new(),
            silent_frames: 0,
            speech_frames: 0,
            total_frames: 0,
            in_speech: false,
        }
    }

    pub fn with_silence_ms(mut self, ms: u32) -> Self {
        self.min_silence_ms = ms.max(100);
        self
    }

    fn frame_samples(&self) -> usize {
        (self.sample_rate as usize * self.frame_ms as usize / 1000).max(1)
    }

    fn frames_for(&self, ms: u32) -> u32 {
        (ms / self.frame_ms).max(1)
    }

    #[cfg(test)]
    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// 喂入 16k（或构造时的采样率）单声道 PCM，返回本次收下来的完整句子。
    pub fn push(&mut self, pcm: &[i16]) -> Vec<Vec<i16>> {
        self.pending.extend_from_slice(pcm);
        let frame_len = self.frame_samples();
        let mut done = Vec::new();
        while self.pending.len() >= frame_len {
            let frame: Vec<i16> = self.pending.drain(..frame_len).collect();
            if let Some(sentence) = self.process_frame(&frame) {
                done.push(sentence);
            }
        }
        done
    }

    /// 外部强制收句（挂断 / 对方离房）。
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        self.pending.clear();
        let sentence = if self.speech_frames >= self.frames_for(self.min_speech_ms) {
            Some(std::mem::take(&mut self.current))
        } else {
            self.current.clear();
            None
        };
        self.reset();
        sentence
    }

    fn reset(&mut self) {
        self.current.clear();
        self.silent_frames = 0;
        self.speech_frames = 0;
        self.total_frames = 0;
        self.in_speech = false;
    }

    fn process_frame(&mut self, frame: &[i16]) -> Option<Vec<i16>> {
        let energy = rms(frame);
        let voiced = energy >= self.energy_threshold;
        if !self.in_speech && !voiced {
            return None;
        }
        self.in_speech = true;
        self.total_frames += 1;
        if voiced {
            self.speech_frames += 1;
            self.silent_frames = 0;
        } else {
            self.silent_frames += 1;
        }
        self.current.extend_from_slice(frame);

        let too_long = self.total_frames >= self.frames_for(self.max_utterance_ms);
        let ended = self.silent_frames >= self.frames_for(self.min_silence_ms);
        if !too_long && !ended {
            return None;
        }

        let enough_speech = self.speech_frames >= self.frames_for(self.min_speech_ms);
        let sentence = if enough_speech {
            // 去掉结尾静音（保留 100ms 尾巴，避免截掉字尾）。
            let trim = (self.silent_frames.saturating_sub(self.frames_for(100)) as usize)
                * self.frame_samples();
            let keep = self.current.len().saturating_sub(trim);
            Some(self.current[..keep].to_vec())
        } else {
            None
        };
        self.reset();
        sentence
    }
}

fn rms(frame: &[i16]) -> f64 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / frame.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, amp: i16) -> Vec<i16> {
        (0..len)
            .map(|i| if i % 2 == 0 { amp } else { -amp })
            .collect()
    }

    const RATE: u32 = 16_000;

    #[test]
    fn resample_changes_length() {
        let input = tone(480, 1000); // 48k 下 10ms
        let out = resample_mono(&input, 48_000, 16_000);
        assert_eq!(out.len(), 160);
        assert!(resample_mono(&[], 48_000, 16_000).is_empty());
        assert_eq!(resample_mono(&input, 16_000, 16_000).len(), 480);
    }

    #[test]
    fn downmix_stereo_to_mono() {
        let stereo = vec![100, 300, -100, 100];
        assert_eq!(downmix(&stereo, 2), vec![200, 0]);
        assert_eq!(downmix(&stereo, 1), stereo);
    }

    #[test]
    fn wav_roundtrip() {
        let pcm = tone(1600, 2000);
        let wav = pcm16_to_wav(&pcm, RATE);
        let (decoded, rate, channels) = wav_to_pcm16(&wav).unwrap();
        assert_eq!(rate, RATE);
        assert_eq!(channels, 1);
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn vad_cuts_on_silence() {
        let mut vad = Vad::new(RATE);
        let mut sentences = Vec::new();
        // 500ms 有声
        sentences.extend(vad.push(&tone(RATE as usize / 2, 4000)));
        assert!(sentences.is_empty());
        assert!(vad.is_in_speech());
        // 800ms 静音 → 收句
        sentences.extend(vad.push(&vec![0i16; RATE as usize * 8 / 10]));
        assert_eq!(sentences.len(), 1);
        assert!(sentences[0].len() > RATE as usize / 3);
        assert!(!vad.is_in_speech());
    }

    #[test]
    fn vad_ignores_short_noise() {
        let mut vad = Vad::new(RATE);
        let mut sentences = Vec::new();
        sentences.extend(vad.push(&tone(1600, 4000))); // 100ms
        sentences.extend(vad.push(&vec![0i16; RATE as usize]));
        assert!(sentences.is_empty());
    }

    #[test]
    fn vad_flush_returns_pending_speech() {
        let mut vad = Vad::new(RATE);
        vad.push(&tone(RATE as usize / 2, 4000));
        let sentence = vad.flush().expect("flush");
        assert!(!sentence.is_empty());
        assert!(vad.flush().is_none());
    }

    #[test]
    fn vad_splits_long_speech() {
        let mut vad = Vad::new(RATE);
        let mut sentences = Vec::new();
        for _ in 0..40 {
            sentences.extend(vad.push(&tone(RATE as usize, 4000)));
        }
        assert!(!sentences.is_empty());
    }
}
