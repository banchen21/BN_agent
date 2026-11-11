//! 生成引擎：参考图校验/上传 → 构建工作流 → 提交等待 → 下载保存。

use crate::comfy_api::ComfyClient;
use crate::config::QwenConfig;
use crate::workflow::{
    clamp_dimensions, QwenBuildRequest, WorkflowTemplate, MAX_REFERENCE_BYTES,
    MAX_REFERENCE_IMAGES, QWEN_REQUIRED_BINDINGS,
};
use base64::Engine as _;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const BUNDLED_WORKFLOW_NAME: &str = "qwen_image_21.json";
const BUNDLED_WORKFLOW: &str = include_str!("../workflows/qwen_image_21.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Generate,
    Edit,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Generate => "generate",
            Mode::Edit => "edit",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct QwenRequest {
    pub prompt: String,
    pub negative: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub scale: Option<f64>,
    pub steps: Option<u32>,
    pub cfg: Option<f64>,
    pub seed: Option<u64>,
    /// 参考图原始字节（最多 16 张，单张 ≤ 20MB）。
    pub reference_images: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct GeneratedArtifact {
    pub file_path: String,
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub bytes_len: usize,
    pub base64: String,
    pub mime_type: String,
    pub elapsed_ms: u64,
    pub seed: u64,
    pub prompt_id: String,
    pub mode: Mode,
    pub reference_count: usize,
    pub queue_extra_secs: u64,
    pub uses_reference_latent: bool,
}

#[derive(Debug)]
pub struct QwenEngine {
    client: ComfyClient,
    template: WorkflowTemplate,
    config: QwenConfig,
    workflow_source: String,
}

impl QwenEngine {
    pub fn new(config: QwenConfig) -> Result<Self, String> {
        let (workflow_source, text) = load_workflow_text(&config)?;
        let template = WorkflowTemplate::parse(
            &workflow_source,
            &text,
            QWEN_REQUIRED_BINDINGS,
        )?;
        template.validate_qwen()?;
        let client = ComfyClient::new(&config.server_url, config.timeout_secs)?;
        Ok(Self {
            client,
            template,
            config,
            workflow_source,
        })
    }

    pub fn workflow_source(&self) -> &str {
        &self.workflow_source
    }

    pub fn publish_event(&self) -> bool {
        self.config.publish_event
    }

    pub async fn generate(
        &self,
        mode: Mode,
        req: &QwenRequest,
    ) -> Result<GeneratedArtifact, String> {
        let started = Instant::now();

        let prompt = req.prompt.trim();
        if prompt.is_empty() {
            return Err("提示词不能为空".to_string());
        }
        match mode {
            Mode::Generate if !req.reference_images.is_empty() => {
                return Err("文生图不接受输入图片；请使用 comfyui_edit_image".to_string());
            }
            Mode::Edit if req.reference_images.is_empty() => {
                return Err("编辑需要至少一张参考图片".to_string());
            }
            _ => {}
        }
        if req.reference_images.len() > MAX_REFERENCE_IMAGES {
            return Err(format!(
                "最多支持 {} 张参考图（收到 {} 张）",
                MAX_REFERENCE_IMAGES,
                req.reference_images.len()
            ));
        }
        for (index, data) in req.reference_images.iter().enumerate() {
            validate_reference_image(index + 1, data)?;
        }

        // 逐张上传，任何一张失败立即终止，避免后续 <imageN> 编号错位。
        let mut filenames = Vec::with_capacity(req.reference_images.len());
        for (index, data) in req.reference_images.iter().enumerate() {
            let extension = image_format(data).unwrap_or("png");
            let upload_name = format!(
                "qwen_{}_{}.{}",
                Uuid::new_v4().simple(),
                index + 1,
                extension
            );
            let uploaded = self
                .client
                .upload_image(&upload_name, data.clone())
                .await
                .map_err(|e| format!("第 {} 张参考图上传失败：{}", index + 1, e))?;
            filenames.push(uploaded);
        }

        // 文生图始终使用配置里的默认尺寸；编辑未指定尺寸时跟随第一张参考图。
        let (width, height) = match mode {
            Mode::Generate => {
                let w = req.width.unwrap_or(i64::from(self.config.default_width));
                let h = req.height.unwrap_or(i64::from(self.config.default_height));
                let (w, h) = clamp_dimensions(w, h);
                (Some(w as i64), Some(h as i64))
            }
            Mode::Edit => (req.width, req.height),
        };

        let negative = req
            .negative
            .clone()
            .unwrap_or_else(|| self.config.default_negative.clone());
        let build_request = QwenBuildRequest {
            prompt: prompt.to_string(),
            negative,
            width,
            height,
            scale: req.scale,
            steps: Some(req.steps.unwrap_or(self.config.default_steps)),
            cfg: Some(req.cfg.unwrap_or(self.config.default_cfg)),
            seed: req.seed,
            reference_filenames: filenames,
        };
        let built = self.template.build_qwen_workflow(&build_request)?;
        let output_node = self.template.node_id("output_image").map(str::to_string);

        let fetched = self
            .client
            .queue_and_wait_image(&built.workflow, output_node.as_deref())
            .await?;

        let output_dir = self.config.resolved_output_dir();
        std::fs::create_dir_all(&output_dir).map_err(|e| {
            format!(
                "创建输出目录失败 {}：{}",
                output_dir.display(),
                e
            )
        })?;
        let extension = image_format(&fetched.bytes).unwrap_or("png");
        let filename = format!(
            "qwen_{}_{}.{}",
            unix_millis(),
            &Uuid::new_v4().simple().to_string()[..8],
            extension
        );
        let file_path = output_dir.join(&filename);
        std::fs::write(&file_path, &fetched.bytes)
            .map_err(|e| format!("保存生成图片失败 {}：{}", file_path.display(), e))?;

        let (width, height) = image_dimensions(&fetched.bytes)
            .unwrap_or((built.width.unwrap_or(0), built.height.unwrap_or(0)));
        let base64 =
            base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);

        Ok(GeneratedArtifact {
            file_path: file_path.to_string_lossy().into_owned(),
            filename,
            width,
            height,
            bytes_len: fetched.bytes.len(),
            base64,
            mime_type: mime_for_extension(extension).to_string(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            seed: built.seed,
            prompt_id: fetched.prompt_id,
            mode,
            reference_count: req.reference_images.len(),
            queue_extra_secs: fetched.queue_extra_secs,
            uses_reference_latent: built.uses_reference_latent,
        })
    }
}

/// 校验单张参考图：非空、≤20MB、格式可识别。
pub fn validate_reference_image(index: usize, data: &[u8]) -> Result<(), String> {
    if data.is_empty() {
        return Err(format!("第 {} 张图片为空", index));
    }
    if data.len() > MAX_REFERENCE_BYTES {
        return Err(format!(
            "第 {} 张图片超过 20MB 限制（{} 字节）",
            index,
            data.len()
        ));
    }
    if image_format(data).is_none() {
        return Err(format!(
            "第 {} 张图片格式无法识别（支持 PNG / JPEG / WebP / GIF / BMP）",
            index
        ));
    }
    Ok(())
}

/// 解码 base64（兼容 `data:image/png;base64,...` 前缀与空白）。
pub fn decode_base64_image(input: &str) -> Result<Vec<u8>, String> {
    let trimmed = input.trim();
    let payload = if let Some(rest) = trimmed.strip_prefix("data:") {
        rest.split_once(',').map(|(_, data)| data).unwrap_or(rest)
    } else {
        trimmed
    };
    let compact: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .map_err(|e| format!("Base64 图片解码失败：{}", e))
}

pub fn image_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some("jpg")
    } else if bytes.len() >= 6
        && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
    {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.len() >= 2 && bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

fn mime_for_extension(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// 读取 PNG（SaveImage 的默认输出）尺寸；其它格式返回 None，由调用方回退。
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        if width > 0 && height > 0 {
            return Some((width, height));
        }
    }
    jpeg_dimensions(bytes)
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if (0xD0..=0xD9).contains(&marker) {
            index += 2;
            continue;
        }
        let segment_len = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if segment_len < 2 || index + 2 + segment_len > bytes.len() {
            return None;
        }
        if (0xC0..=0xCF).contains(&marker)
            && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
        {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32;
            if width > 0 && height > 0 {
                return Some((width, height));
            }
            return None;
        }
        index += 2 + segment_len;
    }
    None
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn load_workflow_text(config: &QwenConfig) -> Result<(String, String), String> {
    let requested = config.workflow_file.trim();
    if requested.is_empty() {
        return Ok((
            BUNDLED_WORKFLOW_NAME.to_string(),
            BUNDLED_WORKFLOW.to_string(),
        ));
    }
    let path = resolve_workflow_path(requested)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取工作流文件失败 {}：{}", path.display(), e))?;
    Ok((path.display().to_string(), text))
}

/// 自定义工作流查找顺序：
/// 1. 绝对路径 / 当前目录相对路径
/// 2. `data/plugin_data/comfyui-qwen-plugin/workflows/`
/// 3. `plugins/comfyui-qwen-plugin/workflows/`
/// 4. 可执行文件目录下 `workflows/`
/// 5. 编译期插件目录 `workflows/`（开发运行）
pub fn resolve_workflow_path(requested: &str) -> Result<PathBuf, String> {
    let requested_path = PathBuf::from(requested);
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(requested_path.clone());
    if requested_path.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(
                cwd.join("data/plugin_data/comfyui-qwen-plugin/workflows")
                    .join(requested),
            );
            candidates.push(
                cwd.join("plugins/comfyui-qwen-plugin/workflows")
                    .join(requested),
            );
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("workflows").join(requested));
                candidates.push(
                    dir.join("plugin_data/comfyui-qwen-plugin/workflows")
                        .join(requested),
                );
            }
        }
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("workflows")
                .join(requested),
        );
    }

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(format!(
        "找不到工作流文件 {}（已尝试：{}）",
        requested,
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("、")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_stub(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth, color type, etc.
        bytes.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder
        bytes
    }

    #[test]
    fn detects_image_formats() {
        assert_eq!(image_format(&png_stub(1, 1)), Some("png"));
        assert_eq!(image_format(b"\xff\xd8\xff\xdbjunk"), Some("jpg"));
        assert_eq!(image_format(b"GIF89a...."), Some("gif"));
        assert_eq!(image_format(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_format(b"BMjunk"), Some("bmp"));
        assert_eq!(image_format(b"not an image"), None);
    }

    #[test]
    fn validates_reference_images() {
        assert!(validate_reference_image(1, &png_stub(8, 8)).is_ok());
        let err = validate_reference_image(2, b"").expect_err("empty");
        assert!(err.contains("第 2 张"), "{err}");
        let err = validate_reference_image(3, b"garbage").expect_err("format");
        assert!(err.contains("无法识别"), "{err}");
        let huge = vec![0u8; MAX_REFERENCE_BYTES + 1];
        let err = validate_reference_image(4, &huge).expect_err("size");
        assert!(err.contains("20MB"), "{err}");
    }

    #[test]
    fn decodes_data_url_and_plain_base64() {
        let bytes = png_stub(2, 3);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decode_base64_image(&b64).unwrap(), bytes);
        let data_url = format!("data:image/png;base64,{}", b64);
        assert_eq!(decode_base64_image(&data_url).unwrap(), bytes);
        assert!(decode_base64_image("!!!not base64!!!").is_err());
    }

    #[test]
    fn reads_png_dimensions() {
        assert_eq!(image_dimensions(&png_stub(1024, 768)), Some((1024, 768)));
        assert_eq!(image_dimensions(b"junk"), None);
    }

    #[test]
    fn resolves_bundled_workflow_from_plugin_dir() {
        let path = resolve_workflow_path("qwen_image_21.json").expect("bundled workflow found");
        assert!(path.is_file(), "{}", path.display());
    }

    #[test]
    fn engine_rejects_invalid_workflow_path() {
        let config = QwenConfig {
            workflow_file: "definitely-missing-workflow.json".into(),
            ..Default::default()
        };
        let err = QwenEngine::new(config).expect_err("should fail");
        assert!(err.contains("找不到工作流文件"), "{err}");
    }

    #[test]
    fn engine_builds_from_bundled_workflow() {
        let engine = QwenEngine::new(QwenConfig::default()).expect("engine");
        assert!(engine.workflow_source().ends_with("qwen_image_21.json"));
    }

    #[test]
    fn mime_mapping() {
        assert_eq!(mime_for_extension("png"), "image/png");
        assert_eq!(mime_for_extension("jpg"), "image/jpeg");
        assert_eq!(mime_for_extension("webp"), "image/webp");
    }

    // ── 真实 ComfyUI 联调（默认忽略） ───────────────────────────
    // 运行：cargo test -p comfyui-qwen-plugin live_qwen -- --ignored --nocapture
    #[test]
    #[ignore = "需要本机 ComfyUI 8188 + Qwen 工作流模型"]
    #[allow(clippy::field_reassign_with_default)]
    fn live_qwen_generate_and_edit() {
        let mut config = QwenConfig::default();
        config.server_url = std::env::var("COMFYUI_QWEN_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8188".into());
        config.workflow_file = std::env::var("COMFYUI_QWEN_LIVE_WORKFLOW")
            .unwrap_or_else(|_| "qwen_image_21_8step_lora.json".into());
        config.default_steps = 8;
        let live_size = std::env::var("COMFYUI_QWEN_LIVE_SIZE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(640);
        config.default_width = live_size;
        config.default_height = live_size;
        config.timeout_secs = std::env::var("COMFYUI_QWEN_LIVE_TIMEOUT")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1800);
        config.output_dir = std::env::temp_dir()
            .join("bn_qwen_live")
            .to_string_lossy()
            .into_owned();

        let engine = QwenEngine::new(config).expect("engine init");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");

        let started = Instant::now();
        let generated = runtime
            .block_on(engine.generate(
                Mode::Generate,
                &QwenRequest {
                    prompt: "一只蓝色陶瓷小鸟，白色背景，简洁构图，柔和光线".into(),
                    seed: Some(20261111),
                    ..Default::default()
                },
            ))
            .expect("live generate");
        eprintln!(
            "[live] generate ok: {} ({}x{}, {} bytes, {} ms, seed={}, total {} ms)",
            generated.file_path,
            generated.width,
            generated.height,
            generated.bytes_len,
            generated.elapsed_ms,
            generated.seed,
            started.elapsed().as_millis()
        );
        assert!(Path::new(&generated.file_path).is_file());
        assert!(generated.width > 0 && generated.height > 0);
        assert!(!generated.base64.is_empty());
        assert_eq!(generated.mode, Mode::Generate);

        let reference = std::fs::read(&generated.file_path).expect("read generated");
        let edit_started = Instant::now();
        let edited = runtime
            .block_on(engine.generate(
                Mode::Edit,
                &QwenRequest {
                    prompt: "将 <image1> 中的陶瓷小鸟改成红色，保持构图、背景和光线不变".into(),
                    reference_images: vec![reference],
                    seed: Some(20261112),
                    ..Default::default()
                },
            ))
            .expect("live edit");
        eprintln!(
            "[live] edit ok: {} ({}x{}, {} bytes, {} ms, seed={}, refs={}, total {} ms)",
            edited.file_path,
            edited.width,
            edited.height,
            edited.bytes_len,
            edited.elapsed_ms,
            edited.seed,
            edited.reference_count,
            edit_started.elapsed().as_millis()
        );
        assert!(Path::new(&edited.file_path).is_file());
        assert!(edited.width > 0 && edited.height > 0);
        assert_eq!(edited.reference_count, 1);
        assert_eq!(edited.mode, Mode::Edit);
        assert_ne!(generated.file_path, edited.file_path);
    }
}
