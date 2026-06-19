//! image-gen-plugin — 本地 SD 生图插件
//!
//! 对接 ComfyUI API，支持 TXT2IMG。
//! 注册 Tool: `generate_image`，LLM 可直接调用。
//! 生成完成后发布 `image.gen.complete` 事件，IM 插件可订阅发送。
//! 生图完成后发布 image.gen.complete 事件，IM 插件可订阅发送。

use plugin_interface::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

const DEFAULT_SEED: u64 = u64::MAX;
const DEFAULT_STEPS: u64 = 25;
const DEFAULT_CFG: f64 = 7.0;
const DEFAULT_WIDTH: u64 = 768;
const DEFAULT_HEIGHT: u64 = 1024;

const DEFAULT_NEGATIVE: &str = "easynegative, worst quality, low quality, bad anatomy, bad hands, extra fingers, deformed, ugly, blurry, watermark, signature, multiple views, multiple girls, 2girls, 3girls, crowd, group, text, speech bubble";

const NSFW_NEGATIVE: &str = "worst quality, low quality, bad anatomy, bad hands, extra fingers, deformed, ugly, blurry, watermark, signature, multiple views, multiple girls, 2girls, 3girls, crowd, group, text, speech bubble";

const FACE_PROMPT: &str = "1girl, solo, extremely long twin tails, waist length twintails, very long white hair, silver hair, bangs, white sailor shirt, unbuttoned shirt, shirt open, visible cleavage, showing cleavage, bra visible, midriff exposed, bare waist, bare midriff, red ribbon, red pleated miniskirt, white thighhighs, black mary jane shoes, medium breasts, b cup, sitting on bed, soft bed, cozy bedroom, soft indoor lighting, looking at viewer, teasing, seductive smile, soft cute face, big eyes, fair skin, flawless skin, high quality, realistic, douyin style, loli face, innocent, perfect face";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn image_gen_output_dir() -> String {
    std::env::var("IMAGE_GEN_OUTPUT_DIR").unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|p| p.join("temp_images"))
            .unwrap_or_else(|_| PathBuf::from("temp_images"))
            .to_string_lossy()
            .into_owned()
    })
}

fn join_path(dir: &str, filename: &str) -> String {
    PathBuf::from(dir).join(filename).to_string_lossy().into_owned()
}

// ── 工作流模板 ──────────────────────────────────────────────────
const WORKFLOW_TEMPLATE: &str = r#"{
  "1": {
    "inputs": { "ckpt_name": "revAnimated_v121_vae2.safetensors" },
    "class_type": "CheckpointLoaderSimple"
  },
  "2": {
    "inputs": { "text": "PROMPT_HERE", "clip": ["1", 1] },
    "class_type": "CLIPTextEncode"
  },
  "3": {
    "inputs": { "text": "NEGATIVE_HERE", "clip": ["1", 1] },
    "class_type": "CLIPTextEncode"
  },
  "4": {
    "inputs": { "width": WIDTH_VAL, "height": HEIGHT_VAL, "batch_size": 1 },
    "class_type": "EmptyLatentImage"
  },
  "5": {
    "inputs": {
      "seed": SEED_VAL, "steps": STEPS_VAL, "cfg": CFG_VAL,
      "sampler_name": "euler_ancestral", "scheduler": "normal",
      "denoise": 1.0,
      "model": ["1", 0],
      "positive": ["2", 0], "negative": ["3", 0],
      "latent_image": ["4", 0]
    },
    "class_type": "KSampler"
  },
  "6": {
    "inputs": { "samples": ["5", 0], "vae": ["1", 2] },
    "class_type": "VAEDecode"
  },

  "9": {
    "inputs": { "filename_prefix": "ComfyUI", "images": ["6", 0] },
    "class_type": "SaveImage"
  }
}"#;

// ── API 类型 ────────────────────────────────────────────────────
#[derive(Serialize)]
struct ComfyPrompt {
    prompt: serde_json::Value,
    client_id: String,
}

#[derive(Deserialize)]
struct ComfyQueueResponse {
    prompt_id: String,
}

#[derive(Deserialize)]
struct ComfyHistoryEntry {
    outputs: HashMap<String, ComfyNodeOutput>,
    status: Option<ComfyStatus>,
}

#[derive(Deserialize)]
struct ComfyNodeOutput {
    images: Option<Vec<ComfyImage>>,
}

#[derive(Deserialize)]
struct ComfyImage {
    filename: String,
    #[allow(dead_code)]
    subfolder: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    file_type: String,
}

#[derive(Deserialize)]
struct ComfyStatus {
    completed: bool,
}

#[derive(Deserialize)]
struct ComfyHistory {
    #[serde(flatten)]
    entries: HashMap<String, ComfyHistoryEntry>,
}

// ── 工具参数 ────────────────────────────────────────────────────
#[derive(Deserialize, Serialize, Clone, Debug)]
struct GenerateImageArgs {
    prompt: String,
    #[serde(default = "default_negative")]
    negative: String,
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default = "default_steps")]
    steps: u64,
    #[serde(default = "default_cfg")]
    cfg: f64,
    #[serde(default = "default_width")]
    width: u64,
    #[serde(default = "default_height")]
    height: u64,
    /// 是否直接发送到 IM（默认 true，无需 LLM 手动发图）
    #[serde(default = "default_true")]
    auto_send: bool,
    /// NSFW 模式：启用无限制内容生成，去掉安全过滤词
    #[serde(default)]
    nsfw: bool,
}

fn default_true() -> bool { true }

fn default_negative() -> String { DEFAULT_NEGATIVE.to_string() }
fn default_seed() -> u64 { DEFAULT_SEED }
fn default_steps() -> u64 { DEFAULT_STEPS }
fn default_cfg() -> f64 { DEFAULT_CFG }
fn default_width() -> u64 { DEFAULT_WIDTH }
fn default_height() -> u64 { DEFAULT_HEIGHT }

// ── 共享状态 ────────────────────────────────────────────────────
struct PluginState {
    runtime: Runtime,
    client: Client,
    comfy_url: String,
    comfy_output_dir: String,
    output_dir: String,
    event_bus: Option<Addr<EventBus>>,
}

// ── 插件主体 ────────────────────────────────────────────────────

pub struct ImageGenPlugin {
    info: PluginInfo,
    state: Option<PluginState>,
}

impl ImageGenPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "image-gen-plugin".into(),
                version: "0.1.0".into(),
                description: "本地 SD 生图插件，通过 ComfyUI API 生成动漫风格图片。LLM 可通过 generate_image 工具调用。生成完成后自动发布 image.gen.complete 事件。".into(),
                author: "BN_agent".into(),
                min_host_version: "0.1.0".into(),
            },
            state: None,
        }
    }

    fn build_workflow(
        prompt: &str,
        negative: &str,
        seed: u64,
        steps: u64,
        cfg: f64,
        width: u64,
        height: u64,
        nsfw: bool,
    ) -> serde_json::Value {
        let full_prompt = format!("{}, {}", FACE_PROMPT, prompt);
        let neg = if nsfw { NSFW_NEGATIVE } else { negative };
        let wf_str = WORKFLOW_TEMPLATE
            .replace("PROMPT_HERE", &full_prompt)
            .replace("NEGATIVE_HERE", neg)
            .replace("SEED_VAL", &seed.to_string())
            .replace("STEPS_VAL", &steps.to_string())
            .replace("CFG_VAL", &cfg.to_string())
            .replace("WIDTH_VAL", &width.to_string())
            .replace("HEIGHT_VAL", &height.to_string());
        serde_json::from_str(&wf_str).unwrap()
    }

    async fn generate_image_inner(
        client: &Client,
        comfy_url: &str,
        args: &GenerateImageArgs,
    ) -> Result<String, String> {
        let workflow = Self::build_workflow(
            &args.prompt, &args.negative,
            args.seed, args.steps, args.cfg,
            args.width, args.height,
            args.nsfw,
        );

        let client_id = uuid::Uuid::new_v4().to_string();
        let body = ComfyPrompt { prompt: workflow, client_id: client_id.clone() };

        let queue_resp = client
            .post(format!("{}/prompt", comfy_url.trim_end_matches('/')))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("ComfyUI 请求失败: {}", e))?
            .json::<ComfyQueueResponse>()
            .await
            .map_err(|e| format!("解析响应失败: {}", e))?;

        let prompt_id = queue_resp.prompt_id;

        for _ in 0..240 {
            let history = client
                .get(format!("{}/history/{}", comfy_url.trim_end_matches('/'), prompt_id))
                .send()
                .await
                .map_err(|e| format!("查询历史失败: {}", e))?
                .json::<ComfyHistory>()
                .await
                .map_err(|e| format!("解析历史失败: {}", e))?;

            if let Some(entry) = history.entries.get(&prompt_id) {
                if let Some(status) = &entry.status {
                    if !status.completed {
                        return Err("生成失败: ComfyUI 报告执行错误".into());
                    }
                }
                for (_, output) in &entry.outputs {
                    if let Some(images) = &output.images {
                        if let Some(img) = images.first() {
                            return Ok(img.filename.clone());
                        }
                    }
                }
                return Err("生成完成但未找到输出图片".into());
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        Err("生成超时 (120s)".into())
    }

    fn make_plugin_state(event_bus: Addr<EventBus>) -> Result<PluginState, Box<dyn std::error::Error>> {
        Ok(PluginState {
            runtime: Runtime::new()?,
            client: Client::new(),
            comfy_url: env_or("COMFYUI_URL", "http://127.0.0.1:8188"),
            comfy_output_dir: env_or("COMFYUI_OUTPUT_DIR", "output"),
            output_dir: image_gen_output_dir(),
            event_bus: Some(event_bus),
        })
    }
}

// ── Tool Executor ────────────────────────────────────────────────

struct GenerateImageTool {
    state: Arc<Mutex<Option<PluginState>>>,
}

impl ToolExecutor for GenerateImageTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "generate_image".into(),
            description: "【生成齐悦（双马尾JK少女）照片】当用户要求看照片、发自拍、换姿势、换衣服、看脸、看身体部位，或任何需要视觉呈现的请求时，必须调用此工具。这是唯一的生图方式。描述用英文Danbooru标签依次写：上衣、下装、鞋袜、配饰、场景、动作、表情。生成15-30秒，调用后不要回复确认文字，图片会自动送达。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "英文Danbooru标签格式。拆解每件衣物: 上衣+下装+鞋袜+配饰，再写场景动作表情。人物外貌(双马尾、黑发、体型)已固定不用写。例: 'white shirt, red pleated skirt, black kneehighs, loafers, red ribbon, standing in classroom, looking at viewer, sweet smile' 或 'loose hoodie, denim shorts, sneakers, walking on street, happy'。必须英文逗号分隔，从衣物到场景依次写。"
                    },
                    "negative": {
                        "type": "string",
                        "description": "反向提示词，一般不需要填，系统已内置"
                    },
                    "seed": {
                        "type": "integer",
                        "description": "随机种子，默认最大值。换种子换构图，不改人物脸型"
                    },
                    "nsfw": {
                        "type": "boolean",
                        "description": "NSFW无限制模式。当用户要求生成成人/色情/裸露内容时必须设为true，否则正常图片设为false",
                        "default": false
                    }
                },
                "required": ["prompt"]
            }),
            internal: false,
        })
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let gen_args: GenerateImageArgs = match serde_json::from_value(args.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::err(&format!("参数解析失败: {}", e)),
        };

        let state_guard = self.state.lock().unwrap();
        let state = match state_guard.as_ref() {
            Some(s) => s,
            None => return ToolResult::err("插件未初始化"),
        };

        let filename = match state.runtime.block_on(
            ImageGenPlugin::generate_image_inner(&state.client, &state.comfy_url, &gen_args)
        ) {
            Ok(f) => f,
            Err(e) => return ToolResult::err(&format!("生成失败: {}", e)),
        };

        let src = join_path(&state.comfy_output_dir, &filename);
        let dest_dir = state.output_dir.clone();
        let full_path = join_path(&dest_dir, &filename);
        fs::create_dir_all(&dest_dir).ok();
        if let Err(e) = fs::copy(&src, &full_path) {
            return ToolResult::err(&format!(
                "文件拷贝失败 [{}] -> [{}]: {}（请检查 COMFYUI_OUTPUT_DIR 环境变量）",
                src, full_path, e
            ));
        }

        let b64 = match fs::read(&full_path) {
            Ok(bytes) => base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes),
            Err(e) => return ToolResult::err(&format!("读取生成图片失败: {}", e)),
        };

        if let Some(ref eb) = state.event_bus {
            eb.do_send(Event::new(
                "image.gen.complete",
                serde_json::json!({
                    "path": full_path,
                    "filename": filename,
                    "prompt": gen_args.prompt,
                    "seed": gen_args.seed,
                    "base64": &b64,
                    "mime_type": "image/png",
                }),
                "image-gen-plugin",
            ));
        }

        ToolResult::ok(&format!(
            "图片生成成功！\n文件: {}\n尺寸: {}x{}\n种子: {}\n提示词: {}",
            full_path, gen_args.width, gen_args.height, gen_args.seed, gen_args.prompt
        ))
    }
}

// ── Plugin trait ─────────────────────────────────────────────────

impl Plugin for ImageGenPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        ctx.logger.info("image-gen-plugin 启动中...");

        let shared_state = ImageGenPlugin::make_plugin_state(ctx.event_bus.clone())?;
        ctx.logger.info(&format!(
            "ComfyUI: {} | output: {} | local: {}",
            shared_state.comfy_url, shared_state.comfy_output_dir, shared_state.output_dir
        ));

        if let Some(tool_registry) = &ctx.tool_registry {
            let tool_state = Arc::new(Mutex::new(Some(
                ImageGenPlugin::make_plugin_state(ctx.event_bus.clone())?
            )));

            tool_registry
                .lock()
                .unwrap()
                .register(Arc::new(GenerateImageTool { state: tool_state }));
            ctx.logger.info("工具 generate_image 已注册");
        }

        self.state = Some(shared_state);
        ctx.logger.info("image-gen-plugin 启动完成");
        Ok(())
    }

    fn stop(&mut self) {
        self.state = None;
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "image.gen.request" {
            // 可通过事件触发生成（未来扩展）
        }
        true
    }
}

// ── FFI ──────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ImageGenPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
