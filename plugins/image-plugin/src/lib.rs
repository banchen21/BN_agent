//! image-plugin — 图片处理插件。
//!
//! 提供图片缩放、格式转换、信息查询等工具，供 LLM 调用。
//! 所有图片输入输出均使用 base64 编码。

use base64::Engine;
use image::{DynamicImage, ImageFormat, ImageReader};
use plugin_interface::*;
use std::io::Cursor;

// ── 工具：image_info ─────────────────────────────────────────────────────────

struct ImageInfoTool;

impl ToolExecutor for ImageInfoTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "image_info".into(),
            description: "获取图片信息：尺寸、格式、文件大小。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64": {"type": "string", "description": "Base64 编码的图片数据"}
                },
                "required": ["image_base64"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let b64 = match args.get("image_base64").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: image_base64"),
        };

        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 decode: {}", e)),
        };

        let reader = match ImageReader::new(Cursor::new(&bytes)).with_guessed_format() {
            Ok(r) => r,
            Err(e) => return ToolResult::err(&format!("image reader: {}", e)),
        };

        let fmt = reader.format().map(|f| format!("{:?}", f)).unwrap_or_else(|| "unknown".into());
        let img = match reader.decode() {
            Ok(i) => i,
            Err(e) => return ToolResult::err(&format!("image decode: {}", e)),
        };

        ToolResult::ok(&format!(
            "width={} height={} format={} bytes={}",
            img.width(), img.height(), fmt, bytes.len()
        ))
    }
}

// ── 工具：image_resize ───────────────────────────────────────────────────────

struct ImageResizeTool;

impl ToolExecutor for ImageResizeTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "image_resize".into(),
            description: "缩放图片到指定尺寸，返回 base64 编码的 JPEG。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64": {"type": "string", "description": "Base64 编码的图片数据"},
                    "width": {"type": "integer", "description": "目标宽度（px）"},
                    "height": {"type": "integer", "description": "目标高度（px），0=按比例"},
                    "format": {"type": "string", "description": "输出格式：jpeg（默认）| png | webp"}
                },
                "required": ["image_base64", "width"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let b64 = match args.get("image_base64").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: image_base64"),
        };
        let w = match args.get("width").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::err("missing: width"),
        };
        let h = args.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let out_fmt = args.get("format").and_then(|v| v.as_str()).unwrap_or("jpeg");

        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 decode: {}", e)),
        };

        let img = match load_image(&bytes) {
            Ok(i) => i,
            Err(e) => return ToolResult::err(&e),
        };

        let resized = if h > 0 {
            img.resize_exact(w, h, image::imageops::FilterType::Lanczos3)
        } else {
            let ratio = w as f64 / img.width() as f64;
            let nh = (img.height() as f64 * ratio).round() as u32;
            img.resize_to_fill(w, nh, image::imageops::FilterType::Lanczos3)
        };

        match encode_image(&resized, out_fmt) {
            Ok(out) => ToolResult::ok(&out),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ── 工具：image_convert ──────────────────────────────────────────────────────

struct ImageConvertTool;

impl ToolExecutor for ImageConvertTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "image_convert".into(),
            description: "转换图片格式：jpeg / png / webp / bmp，返回 base64 编码。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64": {"type": "string", "description": "Base64 编码的图片数据"},
                    "format": {"type": "string", "description": "目标格式：jpeg | png | webp | bmp"}
                },
                "required": ["image_base64", "format"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let b64 = match args.get("image_base64").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: image_base64"),
        };
        let out_fmt = match args.get("format").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: format"),
        };

        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 decode: {}", e)),
        };

        let img = match load_image(&bytes) {
            Ok(i) => i,
            Err(e) => return ToolResult::err(&e),
        };

        match encode_image(&img, out_fmt) {
            Ok(out) => ToolResult::ok(&out),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ── 工具：image_grayscale ────────────────────────────────────────────────────

struct ImageGrayscaleTool;

impl ToolExecutor for ImageGrayscaleTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "image_grayscale".into(),
            description: "将图片转为黑白/灰度图，返回 base64 编码的 JPEG。".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "image_base64": {"type": "string", "description": "Base64 编码的图片数据"}
                },
                "required": ["image_base64"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let b64 = match args.get("image_base64").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err("missing: image_base64"),
        };

        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(d) => d,
            Err(e) => return ToolResult::err(&format!("base64 decode: {}", e)),
        };

        let img = match load_image(&bytes) {
            Ok(i) => i,
            Err(e) => return ToolResult::err(&e),
        };

        let gray = img.grayscale();
        match encode_image(&gray, "jpeg") {
            Ok(out) => ToolResult::ok(&out),
            Err(e) => ToolResult::err(&e),
        }
    }
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

fn load_image(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(image::ImageFormat::Png);
    // 先尝试 PNG，如果失败则自动检测格式
    match reader.decode() {
        Ok(img) => Ok(img),
        Err(_) => {
            let mut reader2 = ImageReader::new(Cursor::new(bytes));
            reader2.no_limits();
            match reader2.with_guessed_format() {
                Ok(r) => r.decode().map_err(|e| format!("image decode: {}", e)),
                Err(e) => Err(format!("image format: {}", e)),
            }
        }
    }
}

fn encode_image(img: &DynamicImage, fmt: &str) -> Result<String, String> {
    let format = match fmt.to_lowercase().as_str() {
        "jpeg" | "jpg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        "webp" => ImageFormat::WebP,
        "bmp" => ImageFormat::Bmp,
        "gif" => ImageFormat::Gif,
        _ => return Err(format!("unsupported format: {}", fmt)),
    };

    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, format).map_err(|e| format!("encode: {}", e))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

// ── Plugin trait ─────────────────────────────────────────────────────────────

struct ImagePlugin {
    info: PluginInfo,
}

impl ImagePlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "image-plugin".into(),
                version: "0.1.0".into(),
                description: "图片处理插件 — 缩放、格式转换、灰度等".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
        }
    }
}

impl Plugin for ImagePlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(ref reg) = ctx.tool_registry {
            let mut reg = reg.lock().map_err(|e| format!("lock: {}", e))?;
            reg.register(std::sync::Arc::new(ImageInfoTool));
            reg.register(std::sync::Arc::new(ImageResizeTool));
            reg.register(std::sync::Arc::new(ImageConvertTool));
            reg.register(std::sync::Arc::new(ImageGrayscaleTool));
            eprintln!("[image-plugin] registered 4 tools");
        }
        eprintln!("[image-plugin] started");
        Ok(())
    }

    fn stop(&mut self) {
        eprintln!("[image-plugin] stopped");
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
