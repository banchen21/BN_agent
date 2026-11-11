//! comfyui-qwen-plugin — 本地 ComfyUI 图片生成与参考图编辑插件。
//!
//! 能力（只做这两项，不包含 SD / 图生视频 / Tagger）：
//! - 文生图：`comfyui_generate_image`
//! - 1~16 张参考图编辑：`comfyui_edit_image`（上传到 `/upload/image`，注入 `LoadImage` 节点）
//!
//! 工作流通过节点 `_meta.hub` 绑定 `positive` / `negative` / `width` / `height` /
//! `latent` / `output_image`（可选 `steps` / `cfg`），内置工作流见 `workflows/`。
//!
//! 完成后发布 `image.gen.complete` 事件（与 image-gen-plugin 兼容），
//! 并把 `file_path`、尺寸、耗时、seed 与 base64 返回给调用方。

mod comfy_api;
mod config;
mod engine;
mod workflow;

use config::{QwenConfig, PLUGIN_NAME};
use engine::{decode_base64_image, Mode, QwenEngine, QwenRequest};
use plugin_interface::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

const EVENT_TOPIC: &str = "image.gen.complete";
const RECENT_IMAGE_LIMIT: usize = 16;
const TOOL_GENERATE: &str = "comfyui_generate_image";
const TOOL_EDIT: &str = "comfyui_edit_image";

// ── 共享状态 ────────────────────────────────────────────────────

struct SharedState {
    runtime: Runtime,
    engine: Arc<QwenEngine>,
    event_bus: Addr<EventBus>,
    recent_images: Arc<Mutex<VecDeque<CachedImage>>>,
    publish_event: bool,
    logger: PluginLogger,
}

#[derive(Clone, Debug)]
struct CachedImage {
    base64: String,
    #[allow(dead_code)]
    mime: String,
    #[allow(dead_code)]
    received_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── 工具参数 ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GenerateArgs {
    prompt: String,
    #[serde(default)]
    negative: Option<String>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
    #[serde(default)]
    scale: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct EditArgs {
    prompt: String,
    #[serde(default)]
    negative: Option<String>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
    #[serde(default)]
    scale: Option<f64>,
    #[serde(default)]
    image_paths: Option<Vec<String>>,
    #[serde(default)]
    image_indices: Option<Vec<usize>>,
    #[serde(default)]
    image_base64s: Option<Vec<String>>,
}

// ── 工具执行 ────────────────────────────────────────────────────

struct GenerateImageTool {
    state: Arc<SharedState>,
}

impl ToolExecutor for GenerateImageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: TOOL_GENERATE.into(),
            description: "精细文生图，支持中文/英文。优先用于非典型动作、明确肢体姿势、物体接触、空间关系、多主体互动、复杂构图或指定文字，也用于一般非动漫生成。仅根据文字生成，不使用输入图片；需要依据现有图片修改或参考多图时用 comfyui_edit_image。除非用户明确要求，省略 width/height/scale，不要随意指定尺寸。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "minLength": 1,
                        "description": "使用中文或英文自然语言，保留用户指定的主体种类、数量、动作、相对位置和文字原文。补足必要表达即可，不擅自增加主体、情节或限制；不堆砌通用画质词。"
                    },
                    "negative": {
                        "type": "string",
                        "description": "可选的不希望出现的内容。未明确需要时省略，不机械套用通用负面词，也不要否定用户要求的主体或效果。"
                    },
                    "width": {
                        "type": "integer",
                        "minimum": 64,
                        "maximum": 4096,
                        "description": "可选输出宽度（像素）。仅在用户指定尺寸或需要特定画幅时填写；否则省略，不随意设置为 512。"
                    },
                    "height": {
                        "type": "integer",
                        "minimum": 64,
                        "maximum": 4096,
                        "description": "可选输出高度（像素）。仅在用户指定尺寸或需要特定画幅时填写；否则省略。"
                    },
                    "scale": {
                        "type": "number",
                        "minimum": 1,
                        "maximum": 8,
                        "description": "仅当用户要求放大时填写，例如 2。否则省略，不默认追加放大操作。"
                    }
                },
                "required": ["prompt"],
                "additionalProperties": false
            }),
            internal: false,
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let parsed: GenerateArgs = match serde_json::from_value(args.clone()) {
            Ok(value) => value,
            Err(e) => return ToolResult::err(&format!("参数解析失败：{}", e)),
        };
        let request = QwenRequest {
            prompt: parsed.prompt,
            negative: parsed.negative,
            width: parsed.width,
            height: parsed.height,
            scale: parsed.scale,
            ..Default::default()
        };
        run_generation(&self.state, Mode::Generate, request)
    }
}

struct EditImageTool {
    state: Arc<SharedState>,
}

impl ToolExecutor for EditImageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: TOOL_EDIT.into(),
            description: "参考图编辑、合成及参考图驱动的生成。需要修改已有图、保留主体或参考多图时使用。参考图按给定顺序对应 <image1>、<image2> 等；可用 image_paths 指定本地图片路径，或用 image_indices 选择最近收到的图片（1 = 最近一张）；都省略时使用最近收到的一张。没有可用图片时请用户提供，不要编造输入，也不要改成纯文生图。最多 16 张参考图。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "minLength": 1,
                        "description": "使用中文或英文自然语言，说明要改什么、哪些内容保持不变；多图时用 <image1>、<image2> 等指明各图用途，编号按最终输入顺序。只修改用户要求的范围。"
                    },
                    "negative": {
                        "type": "string",
                        "description": "可选的不希望出现的内容。未明确需要时省略，不机械套用通用负面词，也不要否定用户要求的主体或效果。"
                    },
                    "image_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": 16,
                        "description": "已知存在的本地图片文件路径列表，不猜测路径、不接受 URL。与 image_indices 同时提供时，选中的最近图片在前，文件图片在后。"
                    },
                    "image_indices": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "description": "最近收到图片的序号列表，1 = 最近一张，2 = 上一张。省略或留空时不按序号选择。"
                    },
                    "image_base64s": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": 16,
                        "description": "Base64 编码的图片列表（可带 data:image/... 前缀）。一般由程序调用时使用，聊天场景优先用 image_paths 或 image_indices。"
                    },
                    "width": {
                        "type": "integer",
                        "minimum": 64,
                        "maximum": 4096,
                        "description": "可选输出宽度（像素）。省略时跟随第一张参考图的宽高比；仅在用户指定画布尺寸时填写。"
                    },
                    "height": {
                        "type": "integer",
                        "minimum": 64,
                        "maximum": 4096,
                        "description": "可选输出高度（像素）。省略时跟随第一张参考图的宽高比。"
                    },
                    "scale": {
                        "type": "number",
                        "minimum": 1,
                        "maximum": 8,
                        "description": "仅当用户要求放大时填写，例如 2。否则省略。"
                    }
                },
                "required": ["prompt"],
                "additionalProperties": false
            }),
            internal: false,
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let parsed: EditArgs = match serde_json::from_value(args.clone()) {
            Ok(value) => value,
            Err(e) => return ToolResult::err(&format!("参数解析失败：{}", e)),
        };

        let reference_images = match collect_edit_images(&self.state.recent_images, &parsed) {
            Ok(images) => images,
            Err(e) => return ToolResult::err(&e),
        };

        let request = QwenRequest {
            prompt: parsed.prompt,
            negative: parsed.negative,
            width: parsed.width,
            height: parsed.height,
            scale: parsed.scale,
            reference_images,
            ..Default::default()
        };
        run_generation(&self.state, Mode::Edit, request)
    }
}

/// 按 AstrBot 的语义收集参考图：序号图在前、文件图在后；
/// 三者都为空时回退到最近收到的一张图片。
fn collect_edit_images(
    recent_images: &Mutex<VecDeque<CachedImage>>,
    args: &EditArgs,
) -> Result<Vec<Vec<u8>>, String> {
    let mut images: Vec<Vec<u8>> = Vec::new();

    if let Some(indices) = args.image_indices.as_ref().filter(|list| !list.is_empty()) {
        let cache = recent_images.lock();
        for index in indices {
            if *index == 0 || *index > cache.len() {
                return Err(format!(
                    "图片序号无效：最近只收到 {} 张图片，序号从 1（最近一张）开始",
                    cache.len()
                ));
            }
            let cached = &cache[*index - 1];
            images.push(decode_base64_image(&cached.base64).map_err(|e| {
                format!("第 {} 张最近图片无法解码：{}", index, e)
            })?);
        }
    }

    if let Some(paths) = args.image_paths.as_ref() {
        for path in paths {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("读取参考图失败 {}：{}", path, e))?;
            images.push(bytes);
        }
    }

    if let Some(base64s) = args.image_base64s.as_ref() {
        for (offset, base64) in base64s.iter().enumerate() {
            images.push(
                decode_base64_image(base64)
                    .map_err(|e| format!("第 {} 张 Base64 图片无法解码：{}", offset + 1, e))?,
            );
        }
    }

    if images.is_empty() {
        if let Some(latest) = recent_images.lock().front() {
            images.push(decode_base64_image(&latest.base64)?);
        }
    }

    if images.is_empty() {
        return Err(
            "编辑需要至少一张参考图片：请在消息中附图，或提供 image_paths / image_base64s".into(),
        );
    }
    if images.len() > workflow::MAX_REFERENCE_IMAGES {
        return Err(format!(
            "最多支持 {} 张参考图（收到 {} 张）",
            workflow::MAX_REFERENCE_IMAGES,
            images.len()
        ));
    }
    Ok(images)
}

fn run_generation(state: &SharedState, mode: Mode, request: QwenRequest) -> ToolResult {
    let prompt = request.prompt.clone();
    let outcome = state.runtime.block_on(state.engine.generate(mode, &request));
    match outcome {
        Ok(artifact) => {
            if state.publish_event {
                if let Some(event) = build_completion_event(&artifact, &prompt) {
                    state.event_bus.do_send(event);
                }
            }
            state.logger.info(format!(
                "{} 完成: {} ({}x{}, {} bytes, {} ms, seed={})",
                mode.as_str(),
                artifact.file_path,
                artifact.width,
                artifact.height,
                artifact.bytes_len,
                artifact.elapsed_ms,
                artifact.seed
            ));
            let content = json!({
                "status": "success",
                "mode": artifact.mode.as_str(),
                "file_path": artifact.file_path,
                "filename": artifact.filename,
                "width": artifact.width,
                "height": artifact.height,
                "bytes": artifact.bytes_len,
                "elapsed_ms": artifact.elapsed_ms,
                "seed": artifact.seed,
                "reference_count": artifact.reference_count,
                "message": "图片已保存到 file_path。请调用当前平台的发送图片工具（Telegram: tg_send_photo；微信: wechat_send_image）把 file_path 发送给用户；不要用文字描述代替发图。"
            });
            let metadata = json!({
                "file_path": artifact.file_path,
                "filename": artifact.filename,
                "width": artifact.width,
                "height": artifact.height,
                "bytes": artifact.bytes_len,
                "elapsed_ms": artifact.elapsed_ms,
                "seed": artifact.seed,
                "mode": artifact.mode.as_str(),
                "reference_count": artifact.reference_count,
                "prompt_id": artifact.prompt_id,
                "queue_extra_secs": artifact.queue_extra_secs,
                "uses_reference_latent": artifact.uses_reference_latent,
                "base64": artifact.base64,
                "mime_type": artifact.mime_type
            });
            ToolResult::ok_with_metadata(&content.to_string(), metadata)
        }
        Err(e) => {
            state.logger.error(format!("{} 失败: {}", mode.as_str(), e));
            ToolResult::err(&format!("图片{}失败：{}", if mode == Mode::Edit { "编辑" } else { "生成" }, e))
        }
    }
}

fn build_completion_event(artifact: &engine::GeneratedArtifact, prompt: &str) -> Option<Event> {
    Some(Event::new(
        EVENT_TOPIC,
        json!({
            "status": "success",
            "mode": artifact.mode.as_str(),
            "path": artifact.file_path,
            "file_path": artifact.file_path,
            "filename": artifact.filename,
            "prompt": prompt,
            "width": artifact.width,
            "height": artifact.height,
            "bytes": artifact.bytes_len,
            "elapsed_ms": artifact.elapsed_ms,
            "seed": artifact.seed,
            "reference_count": artifact.reference_count,
            "prompt_id": artifact.prompt_id,
            "queue_extra_secs": artifact.queue_extra_secs,
            "uses_reference_latent": artifact.uses_reference_latent,
            "base64": artifact.base64,
            "mime_type": artifact.mime_type
        }),
        PLUGIN_NAME,
    ))
}

// ── 插件主体 ────────────────────────────────────────────────────

pub struct ComfyQwenPlugin {
    info: PluginInfo,
    state: Option<Arc<SharedState>>,
    recent_images: Arc<Mutex<VecDeque<CachedImage>>>,
}

impl ComfyQwenPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: PLUGIN_NAME.into(),
                version: "0.1.0".into(),
                description: "本地 ComfyUI 图片生成与参考图编辑插件。注册 comfyui_generate_image / comfyui_edit_image 工具；支持 1~16 张参考图编辑，生成完成后发布 image.gen.complete 事件。".into(),
                author: "BN_agent".into(),
                min_host_version: "0.1.0".into(),
            },
            state: None,
            recent_images: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

impl Plugin for ComfyQwenPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        // 重新 start 同一实例时先清掉旧状态，避免半初始化残留。
        self.state = None;
        ctx.logger.info("comfyui-qwen-plugin 启动中...");
        let config = QwenConfig::load(Some(&ctx.config));
        ctx.logger.info(format!(
            "ComfyUI: {} | workflow: {} | output: {} | timeout: {}s",
            config.server_url,
            if config.workflow_file.trim().is_empty() {
                "内置 qwen_image_21.json"
            } else {
                config.workflow_file.as_str()
            },
            config.resolved_output_dir().display(),
            config.timeout_secs
        ));

        let engine = match QwenEngine::new(config) {
            Ok(engine) => engine,
            Err(e) => {
                let message = format!(
                    "初始化失败（请检查 ComfyUI 地址/工作流配置）：{}",
                    e
                );
                ctx.logger.error(&message);
                // 必须向上返回 Err，宿主才能回滚配置并保持工具注册状态一致。
                return Err(message.into());
            }
        };
        ctx.logger.info(format!("工作流：{}", engine.workflow_source()));

        let runtime = match Runtime::new() {
            Ok(runtime) => runtime,
            Err(e) => {
                let message = format!("创建 tokio runtime 失败：{}", e);
                ctx.logger.error(&message);
                return Err(message.into());
            }
        };

        let state = Arc::new(SharedState {
            runtime,
            publish_event: engine.publish_event(),
            engine: Arc::new(engine),
            event_bus: ctx.event_bus.clone(),
            recent_images: self.recent_images.clone(),
            logger: ctx.logger.clone(),
        });

        if let Some(registry) = &ctx.tool_registry {
            registry
                .lock()
                .register(Arc::new(GenerateImageTool { state: state.clone() }));
            registry
                .lock()
                .register(Arc::new(EditImageTool { state: state.clone() }));
            ctx.logger.info(format!(
                "工具 {} / {} 已注册",
                TOOL_GENERATE, TOOL_EDIT
            ));
        }

        self.state = Some(state);
        ctx.logger.info("comfyui-qwen-plugin 启动完成");
        Ok(())
    }

    fn stop(&mut self) {
        self.state = None;
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "user.message" {
            if let Some(base64) = event.data.get("image_base64").and_then(Value::as_str) {
                if !base64.is_empty() {
                    let mime = event
                        .data
                        .get("mime_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/jpeg")
                        .to_string();
                    let mut cache = self.recent_images.lock();
                    cache.push_front(CachedImage {
                        base64: base64.to_string(),
                        mime,
                        received_ms: now_ms(),
                    });
                    cache.truncate(RECENT_IMAGE_LIMIT);
                }
            }
        }
        true
    }

    fn config_schema(&self) -> Option<PluginConfigSchema> {
        serde_json::from_str::<PluginConfigSchema>(include_str!("../config_schema.json")).ok()
    }
}

// ── FFI ─────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ComfyQwenPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_interface::config::merge_with_defaults;

    #[test]
    fn config_schema_json_covers_all_fields() {
        let schema = ComfyQwenPlugin::new()
            .config_schema()
            .expect("schema should parse");
        assert_eq!(schema.version, 1);
        assert!(schema.title.contains("生图"));
        for name in [
            "server_url",
            "timeout_secs",
            "workflow_file",
            "default_width",
            "default_height",
            "default_steps",
            "default_cfg",
            "default_negative",
            "output_dir",
            "publish_event",
        ] {
            assert!(schema.fields.contains_key(name), "missing field {name}");
        }
        assert!(schema.fields.values().all(|field| !field.secret));
    }

    #[test]
    fn schema_defaults_match_runtime_defaults() {
        let schema = ComfyQwenPlugin::new().config_schema().expect("schema");
        let merged = merge_with_defaults(&schema, &json!({}));
        let config = QwenConfig::from_json(&merged);
        assert_eq!(config, QwenConfig::default());
    }

    #[test]
    fn edit_args_accept_null_optional_arrays() {
        let args: EditArgs = serde_json::from_value(json!({
            "prompt": "把背景改成白色",
            "image_paths": null,
            "image_indices": null,
            "image_base64s": null
        }))
        .expect("null arrays accepted");
        assert!(args.image_paths.is_none());
        assert!(args.image_indices.is_none());
        assert!(args.image_base64s.is_none());
    }

    #[test]
    fn generate_args_reject_unknown_fields_only_when_strict() {
        // serde 默认忽略未知字段；工具 schema 的 additionalProperties=false 由宿主/LLM 约束。
        let args: GenerateArgs = serde_json::from_value(json!({
            "prompt": "test",
            "unexpected": true
        }))
        .expect("unknown field tolerated at deserialize layer");
        assert_eq!(args.prompt, "test");
    }

    fn edit_args() -> EditArgs {
        EditArgs {
            prompt: "p".into(),
            negative: None,
            width: None,
            height: None,
            scale: None,
            image_paths: None,
            image_indices: None,
            image_base64s: None,
        }
    }

    #[test]
    fn collect_edit_images_reports_missing_images() {
        let cache = Mutex::new(VecDeque::new());
        let err = collect_edit_images(&cache, &edit_args()).expect_err("no images");
        assert!(err.contains("至少一张参考图片"), "{err}");
    }

    #[test]
    fn collect_edit_images_uses_recent_image_fallback() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&8u32.to_be_bytes());
        png.extend_from_slice(&8u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        let base64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
        let cache = Mutex::new(VecDeque::from([CachedImage {
            base64: base64.clone(),
            mime: "image/png".into(),
            received_ms: 0,
        }]));

        let images = collect_edit_images(&cache, &edit_args()).expect("fallback");
        assert_eq!(images, vec![png]);

        let mut args = edit_args();
        args.image_indices = Some(vec![2]);
        let err = collect_edit_images(&cache, &args).expect_err("index out of range");
        assert!(err.contains("序号无效"), "{err}");
    }

    #[test]
    fn collect_edit_images_keeps_index_order() {
        let newest = b"\x89PNG\r\n\x1a\nnewest".to_vec();
        let older = b"\x89PNG\r\n\x1a\nolder".to_vec();
        let encode = |bytes: &[u8]| {
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
        };
        let cache = Mutex::new(VecDeque::from([
            CachedImage {
                base64: encode(&newest),
                mime: "image/png".into(),
                received_ms: 2,
            },
            CachedImage {
                base64: encode(&older),
                mime: "image/png".into(),
                received_ms: 1,
            },
        ]));

        let mut args = edit_args();
        args.image_indices = Some(vec![2, 1]);
        let images = collect_edit_images(&cache, &args).expect("indices");
        assert_eq!(images, vec![older, newest], "index 2 在前，index 1 在后");
    }

    #[test]
    fn plugin_info_name_matches_config_file_name() {
        let plugin = ComfyQwenPlugin::new();
        assert_eq!(plugin.info().name, "comfyui-qwen-plugin");
        assert_eq!(plugin.info().name, PLUGIN_NAME);
    }

    // ── 启动失败必须向上返回 Err（QA #7 回归） ───────────────────

    fn start_config(workflow_file: &str) -> Value {
        json!({
            "server_url": "http://127.0.0.1:8188",
            "timeout_secs": 900,
            "workflow_file": workflow_file,
            "default_width": 1024,
            "default_height": 1024,
            "default_steps": 25,
            "default_cfg": 1.0,
            "default_negative": "",
            "output_dir": "",
            "publish_event": true
        })
    }

    #[actix_rt::test]
    async fn start_returns_err_and_registers_no_tools_when_workflow_missing() {
        let mut plugin = ComfyQwenPlugin::new();
        let mut ctx = PluginContext::for_test(PLUGIN_NAME);
        ctx.config = start_config("definitely-missing-workflow.json");
        let registry = ctx.tool_registry.clone().expect("test registry");

        let error = plugin.start(ctx).expect_err("missing workflow must fail start");
        let message = error.to_string();
        assert!(message.contains("初始化失败"), "{message}");
        assert!(message.contains("找不到工作流文件"), "{message}");
        assert!(plugin.state.is_none(), "must not keep half-initialized state");
        assert!(
            registry.lock().all_defs().is_empty(),
            "no tools may be registered on failed start"
        );
    }

    #[actix_rt::test]
    async fn start_returns_err_when_workflow_json_invalid() {
        let path = std::env::temp_dir().join(format!(
            "bn-qwen-invalid-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "{not valid json").expect("write temp workflow");

        let mut plugin = ComfyQwenPlugin::new();
        let mut ctx = PluginContext::for_test(PLUGIN_NAME);
        ctx.config = start_config(&path.to_string_lossy());
        let registry = ctx.tool_registry.clone().expect("test registry");

        let error = plugin.start(ctx).expect_err("invalid workflow must fail start");
        let message = error.to_string();
        assert!(message.contains("不是有效 JSON"), "{message}");
        assert!(plugin.state.is_none());
        assert!(registry.lock().all_defs().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[actix_rt::test]
    async fn start_returns_err_when_required_bindings_missing() {
        let path = std::env::temp_dir().join(format!(
            "bn-qwen-nobind-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            r#"{"1":{"class_type":"KSampler","inputs":{"steps":25}}}"#,
        )
        .expect("write temp workflow");

        let mut plugin = ComfyQwenPlugin::new();
        let mut ctx = PluginContext::for_test(PLUGIN_NAME);
        ctx.config = start_config(&path.to_string_lossy());
        let error = plugin.start(ctx).expect_err("missing bindings must fail start");
        let message = error.to_string();
        assert!(message.contains("缺少绑定"), "{message}");
        assert!(plugin.state.is_none());
        let _ = std::fs::remove_file(&path);
    }
}
