//! ComfyUI API 工作流模板解析与绑定。
//!
//! 工作流必须是 ComfyUI **API 格式**（顶层是 `{ "<node_id>": { class_type, inputs, _meta } }`），
//! 通过节点 `_meta.hub` 声明用途绑定，例如：
//!
//! ```json
//! "5": {
//!   "class_type": "TextEncodeQwenImage21",
//!   "inputs": { "prompt": "", "negative_prompt": "" },
//!   "_meta": { "hub": { "positive": "prompt", "negative": "negative_prompt" } }
//! }
//! ```
//!
//! 必需绑定：`positive`、`negative`、`width`、`height`、`latent`、`output_image`。
//! 可选绑定：`steps`、`cfg`（绑定后才应用插件配置里的默认步数/CFG）。

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use uuid::Uuid;

pub const MAX_REFERENCE_IMAGES: usize = 16;
pub const MAX_REFERENCE_BYTES: usize = 20 * 1024 * 1024;
pub const MIN_DIMENSION: u32 = 64;
pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_OUTPUT_PIXELS: u64 = 2048 * 2048;
pub const DIMENSION_ALIGN: u64 = 32;

pub const QWEN_ENCODE_CLASS: &str = "TextEncodeQwenImage21";
pub const QWEN_REQUIRED_BINDINGS: &[&str] =
    &["positive", "negative", "width", "height", "latent", "output_image"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub node_id: String,
    pub field: String,
}

#[derive(Clone, Debug)]
pub struct WorkflowTemplate {
    pub source: String,
    pub workflow: Value,
    pub bindings: HashMap<String, Binding>,
}

#[derive(Clone, Debug, Default)]
pub struct QwenBuildRequest {
    pub prompt: String,
    pub negative: String,
    /// 允许负数，由 `clamp_dimensions` 统一钳制。
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub scale: Option<f64>,
    pub steps: Option<u32>,
    pub cfg: Option<f64>,
    pub seed: Option<u64>,
    /// 已上传到 ComfyUI 的参考图文件名（保持顺序，对应 <imageN>）。
    pub reference_filenames: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BuiltWorkflow {
    pub workflow: Value,
    pub seed: u64,
    /// 编辑时未指定尺寸，latent 跟随第一张参考图。
    pub uses_reference_latent: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl WorkflowTemplate {
    pub fn parse(source: &str, text: &str, required: &[&str]) -> Result<Self, String> {
        let workflow: Value = serde_json::from_str(text)
            .map_err(|e| format!("{} 不是有效 JSON：{}", source, e))?;
        Self::from_value(source, workflow, required)
    }

    pub fn from_value(
        source: &str,
        workflow: Value,
        required: &[&str],
    ) -> Result<Self, String> {
        let root = workflow
            .as_object()
            .ok_or_else(|| format!("{} 顶层必须是 JSON 对象", source))?;
        if root.is_empty() {
            return Err(format!("{} 不包含任何节点", source));
        }
        if root.contains_key("nodes") {
            return Err(format!(
                "{} 必须是 ComfyUI API 格式工作流（不能是 UI 导出的 nodes 结构）",
                source
            ));
        }

        let mut bindings: HashMap<String, Binding> = HashMap::new();
        for (node_id, node) in root {
            let node = node
                .as_object()
                .ok_or_else(|| format!("{} 的节点 {} 必须是对象", source, node_id))?;
            if !node.get("class_type").map(Value::is_string).unwrap_or(false) {
                return Err(format!("{} 的节点 {} 缺少 class_type", source, node_id));
            }
            let inputs = node
                .get("inputs")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("{} 的节点 {} 缺少 inputs 对象", source, node_id))?;

            let Some(meta) = node.get("_meta") else {
                continue;
            };
            let meta = meta
                .as_object()
                .ok_or_else(|| format!("{} 的节点 {} 的 _meta 必须是对象", source, node_id))?;
            let Some(hub) = meta.get("hub") else {
                continue;
            };
            let hub = hub.as_object().ok_or_else(|| {
                format!("{} 的节点 {} 的 _meta.hub 必须是对象", source, node_id)
            })?;

            for (role, field_value) in hub {
                let field = match field_value {
                    Value::String(field) => Some(field.clone()),
                    // 与参考实现一致：仅 output 允许 null（表示整个输出节点）。
                    Value::Null if role == "output" => None,
                    _ => {
                        return Err(format!(
                            "{} 的节点 {} 的绑定 {} 必须指定输入字段名",
                            source, node_id, role
                        ))
                    }
                };
                if bindings.contains_key(role) {
                    return Err(format!("{} 的绑定 {} 重复", source, role));
                }
                if let Some(name) = field.as_ref() {
                    if !inputs.contains_key(name) {
                        return Err(format!(
                            "{} 的绑定 {} 指向不存在的字段 {}（节点 {}）",
                            source, role, name, node_id
                        ));
                    }
                }
                bindings.insert(
                    role.clone(),
                    Binding {
                        node_id: node_id.clone(),
                        field: field.unwrap_or_default(),
                    },
                );
            }
        }

        let mut missing = Vec::new();
        for role in required {
            if !bindings.contains_key(*role) {
                missing.push(*role);
            }
        }
        if !missing.is_empty() {
            missing.sort_unstable();
            return Err(format!(
                "{} 缺少绑定：{}",
                source,
                missing.join(", ")
            ));
        }

        Ok(Self {
            source: source.to_string(),
            workflow,
            bindings,
        })
    }

    /// Qwen 工作流额外校验：正负提示词必须绑定同一个 `TextEncodeQwenImage21` 节点。
    pub fn validate_qwen(&self) -> Result<(), String> {
        let positive = self.binding("positive")?;
        let negative = self.binding("negative")?;
        let class = self
            .workflow
            .get(&positive.node_id)
            .and_then(|node| node.get("class_type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if class != QWEN_ENCODE_CLASS {
            return Err(format!(
                "{} 的 positive 绑定节点 {} 必须是 {}（当前为 {}）",
                self.source, positive.node_id, QWEN_ENCODE_CLASS, class
            ));
        }
        if positive.node_id != negative.node_id {
            return Err(format!(
                "{} 的正负提示词必须绑定同一个 {} 节点",
                self.source, QWEN_ENCODE_CLASS
            ));
        }
        Ok(())
    }

    pub fn binding(&self, role: &str) -> Result<&Binding, String> {
        self.bindings
            .get(role)
            .ok_or_else(|| format!("{} 缺少绑定 {}", self.source, role))
    }

    pub fn has_binding(&self, role: &str) -> bool {
        self.bindings.contains_key(role)
    }

    pub fn node_id(&self, role: &str) -> Option<&str> {
        self.bindings.get(role).map(|b| b.node_id.as_str())
    }

    pub fn get<'a>(&self, workflow: &'a Value, role: &str) -> Option<&'a Value> {
        let binding = self.bindings.get(role)?;
        workflow
            .get(&binding.node_id)?
            .get("inputs")?
            .get(&binding.field)
    }

    pub fn set(&self, workflow: &mut Value, role: &str, value: Value) -> Result<(), String> {
        let binding = self.binding(role)?.clone();
        let inputs = workflow
            .get_mut(&binding.node_id)
            .and_then(|node| node.get_mut("inputs"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| format!("{} 的节点 {} 缺少 inputs", self.source, binding.node_id))?;
        if !inputs.contains_key(&binding.field) {
            return Err(format!(
                "{} 的节点 {} 缺少字段 {}",
                self.source, binding.node_id, binding.field
            ));
        }
        inputs.insert(binding.field, value);
        Ok(())
    }

    /// 构建一次 Qwen 生成/编辑工作流。
    pub fn build_qwen_workflow(
        &self,
        req: &QwenBuildRequest,
    ) -> Result<BuiltWorkflow, String> {
        if req.reference_filenames.len() > MAX_REFERENCE_IMAGES {
            return Err(format!(
                "最多支持 {} 张参考图（收到 {} 张）",
                MAX_REFERENCE_IMAGES,
                req.reference_filenames.len()
            ));
        }

        let mut workflow = self.workflow.clone();
        self.set(&mut workflow, "positive", json!(req.prompt))?;
        self.set(&mut workflow, "negative", json!(req.negative))?;

        let encode_node = self.binding("positive")?.node_id.clone();

        // 参考图编号：工作流自带的 images.image_N 始终保留在最前，
        // 用户图片从最大编号 + 1 开始追加（对应 <imageN>）。
        let start_index = {
            let inputs = node_inputs(&workflow, &encode_node)?;
            preset_image_indexes(inputs)
                .into_iter()
                .max()
                .unwrap_or(0)
                + 1
        };
        let total_images = req.reference_filenames.len() + start_index as usize - 1;
        if total_images > MAX_REFERENCE_IMAGES {
            return Err(format!(
                "当前工作流已有 {} 张预置参考图，最多还能接收 {} 张（本次 {} 张）",
                start_index - 1,
                MAX_REFERENCE_IMAGES - (start_index as usize - 1),
                req.reference_filenames.len()
            ));
        }

        let mut pending: Vec<(String, String)> = Vec::new();
        for (offset, filename) in req.reference_filenames.iter().enumerate() {
            let index = start_index + offset as u32;
            let mut node_id = format!("hub_image_{}", index);
            while workflow.get(&node_id).is_some() {
                node_id.push('_');
            }
            pending.push((format!("images.image_{}", index), node_id.clone()));
            if let Some(root) = workflow.as_object_mut() {
                root.insert(
                    node_id,
                    json!({
                        "class_type": "LoadImage",
                        "inputs": { "image": filename }
                    }),
                );
            }
        }

        {
            let inputs = node_inputs_mut(&mut workflow, &encode_node)?;
            for (key, node_id) in pending {
                inputs.insert(key, json!([node_id, 0]));
            }
        }

        let (width, height, uses_reference_latent) = if !req.reference_filenames.is_empty()
            && req.width.is_none()
            && req.height.is_none()
        {
            // 编辑且未指定尺寸：输出跟随第一张参考图，避免构图被拉伸。
            let old_latent = self.get(&workflow, "latent").cloned().unwrap_or(Value::Null);
            self.set(&mut workflow, "latent", json!([encode_node, 2]))?;
            if let Some(old_node) = old_latent
                .as_array()
                .and_then(|items| items.first())
                .and_then(Value::as_str)
            {
                if old_node != encode_node && !references_node(&workflow, old_node) {
                    if let Some(root) = workflow.as_object_mut() {
                        root.remove(old_node);
                    }
                }
            }
            (None, None, true)
        } else {
            let current_width = self.get(&workflow, "width").and_then(Value::as_i64);
            let current_height = self.get(&workflow, "height").and_then(Value::as_i64);
            let w = req.width.or(current_width);
            let h = req.height.or(current_height);
            let (w, h) = clamp_dimensions(w.unwrap_or(1024), h.unwrap_or(1024));
            self.set(&mut workflow, "width", json!(w))?;
            self.set(&mut workflow, "height", json!(h))?;
            (Some(w), Some(h), false)
        };

        if let Some(steps) = req.steps {
            if self.has_binding("steps") {
                self.set(&mut workflow, "steps", json!(steps))?;
            }
        }
        if let Some(cfg) = req.cfg {
            if self.has_binding("cfg") {
                self.set(&mut workflow, "cfg", json!(cfg))?;
            }
        }

        if let Some(scale) = req.scale {
            if !(1.0..=8.0).contains(&scale) {
                return Err(format!("放大倍数必须在 1~8 之间（收到 {}）", scale));
            }
            if (scale - 1.0).abs() > f64::EPSILON {
                let source = self
                    .get(&workflow, "output_image")
                    .cloned()
                    .ok_or_else(|| format!("{} 缺少 output_image 绑定值", self.source))?;
                let mut node_id = "hub_upscale".to_string();
                while workflow.get(&node_id).is_some() {
                    node_id.push('_');
                }
                if let Some(root) = workflow.as_object_mut() {
                    root.insert(
                        node_id.clone(),
                        json!({
                            "class_type": "ImageScaleBy",
                            "inputs": {
                                "image": source,
                                "upscale_method": "lanczos",
                                "scale_by": scale
                            }
                        }),
                    );
                }
                self.set(&mut workflow, "output_image", json!([node_id, 0]))?;
            }
        }

        let seed = apply_seed(&mut workflow, req.seed);
        Ok(BuiltWorkflow {
            workflow,
            seed,
            uses_reference_latent,
            width,
            height,
        })
    }
}

/// 将用户/配置尺寸钳制到合法范围：
/// - 单边 64..4096
/// - 总像素不超过 2048×2048（超出按面积等比缩小）
/// - 向下对齐到 32 的倍数（同时满足 8 对齐）
pub fn clamp_dimensions(width: i64, height: i64) -> (u32, u32) {
    let mut w = width.clamp(MIN_DIMENSION as i64, MAX_DIMENSION as i64) as u64;
    let mut h = height.clamp(MIN_DIMENSION as i64, MAX_DIMENSION as i64) as u64;

    let pixels = w.saturating_mul(h);
    if pixels > MAX_OUTPUT_PIXELS {
        let scale = (MAX_OUTPUT_PIXELS as f64 / pixels as f64).sqrt();
        w = ((w as f64) * scale).floor().max(1.0) as u64;
        h = ((h as f64) * scale).floor().max(1.0) as u64;
    }

    (align_down(w), align_down(h))
}

fn align_down(value: u64) -> u32 {
    let aligned = value / DIMENSION_ALIGN * DIMENSION_ALIGN;
    aligned.max(MIN_DIMENSION as u64) as u32
}

/// 扫描工作流里全部 `seed` / `noise_seed` 数字输入并统一写入。
/// 返回使用的首个种子；`seed=None` 时随机生成。
pub fn apply_seed(workflow: &mut Value, seed: Option<u64>) -> u64 {
    let base = seed.unwrap_or_else(random_seed);
    let mut next = base;
    if let Some(root) = workflow.as_object_mut() {
        for node in root.values_mut() {
            let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) else {
                continue;
            };
            for field in ["seed", "noise_seed"] {
                if inputs.get(field).map(Value::is_number).unwrap_or(false) {
                    inputs.insert(field.to_string(), json!(next));
                    next = next.wrapping_add(1).max(1);
                }
            }
        }
    }
    base
}

fn random_seed() -> u64 {
    (Uuid::new_v4().as_u128() as u64) % 999_999_999_999_999 + 1
}

/// 从节点 inputs 中提取 `images.image_N` 的编号。
pub fn preset_image_indexes(inputs: &Map<String, Value>) -> Vec<u32> {
    inputs
        .keys()
        .filter_map(|key| {
            key.strip_prefix("images.image_")
                .and_then(|suffix| suffix.parse::<u32>().ok())
        })
        .collect()
}

fn node_inputs<'a>(workflow: &'a Value, node_id: &str) -> Result<&'a Map<String, Value>, String> {
    workflow
        .get(node_id)
        .and_then(|node| node.get("inputs"))
        .and_then(Value::as_object)
        .ok_or_else(|| format!("工作流节点 {} 缺少 inputs", node_id))
}

fn node_inputs_mut<'a>(
    workflow: &'a mut Value,
    node_id: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    workflow
        .get_mut(node_id)
        .and_then(|node| node.get_mut("inputs"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("工作流节点 {} 缺少 inputs", node_id))
}

fn references_node(workflow: &Value, node_id: &str) -> bool {
    workflow
        .as_object()
        .map(|root| {
            root.values().any(|node| {
                node.get("inputs")
                    .and_then(Value::as_object)
                    .map(|inputs| {
                        inputs.values().any(|value| {
                            value
                                .as_array()
                                .and_then(|items| items.first())
                                .and_then(Value::as_str)
                                == Some(node_id)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLED: &str = include_str!("../workflows/qwen_image_21.json");

    fn template() -> WorkflowTemplate {
        WorkflowTemplate::parse("qwen_image_21.json", BUNDLED, QWEN_REQUIRED_BINDINGS)
            .expect("bundled workflow valid")
    }

    fn build(filenames: &[&str]) -> QwenBuildRequest {
        QwenBuildRequest {
            prompt: "一只蓝色陶瓷小鸟".into(),
            negative: String::new(),
            reference_filenames: filenames.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    // ── 工作流绑定 ───────────────────────────────────────────────

    #[test]
    fn bundled_workflow_has_required_bindings() {
        let tpl = template();
        for role in QWEN_REQUIRED_BINDINGS {
            assert!(tpl.has_binding(role), "missing role {role}");
        }
        assert!(tpl.has_binding("steps"));
        assert!(tpl.has_binding("cfg"));
        tpl.validate_qwen().expect("qwen node valid");
        assert_eq!(tpl.node_id("positive"), Some("5"));
        assert_eq!(tpl.node_id("latent"), Some("6"));
        assert_eq!(tpl.node_id("output_image"), Some("8"));
    }

    #[test]
    fn parse_rejects_missing_required_binding() {
        let err = WorkflowTemplate::parse(
            "x.json",
            r#"{"1":{"class_type":"KSampler","inputs":{"latent_image":["2",0]},"_meta":{"hub":{"latent":"latent_image"}}}}"#,
            QWEN_REQUIRED_BINDINGS,
        )
        .expect_err("should fail");
        assert!(err.contains("缺少绑定"), "{err}");
    }

    #[test]
    fn parse_rejects_duplicate_binding() {
        let err = WorkflowTemplate::parse(
            "x.json",
            r#"{
              "1":{"class_type":"A","inputs":{"text":""},"_meta":{"hub":{"positive":"text"}}},
              "2":{"class_type":"B","inputs":{"text":""},"_meta":{"hub":{"positive":"text"}}}
            }"#,
            &[],
        )
        .expect_err("should fail");
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn parse_rejects_binding_to_missing_input() {
        let err = WorkflowTemplate::parse(
            "x.json",
            r#"{"1":{"class_type":"A","inputs":{},"_meta":{"hub":{"positive":"prompt"}}}}"#,
            &[],
        )
        .expect_err("should fail");
        assert!(err.contains("不存在的字段"), "{err}");
    }

    #[test]
    fn parse_rejects_ui_format_workflow() {
        let err = WorkflowTemplate::parse("x.json", r#"{"nodes":[]}"#, &[]).expect_err("fail");
        assert!(err.contains("API 格式"), "{err}");
    }

    #[test]
    fn parse_rejects_wrong_encode_class() {
        let mut value: Value = serde_json::from_str(BUNDLED).unwrap();
        value["5"]["class_type"] = json!("CLIPTextEncode");
        let tpl = WorkflowTemplate::from_value("x.json", value, QWEN_REQUIRED_BINDINGS).unwrap();
        let err = tpl.validate_qwen().expect_err("fail");
        assert!(err.contains(QWEN_ENCODE_CLASS), "{err}");
    }

    #[test]
    fn parse_rejects_negative_binding_on_other_node() {
        let mut value: Value = serde_json::from_str(BUNDLED).unwrap();
        value["5"]["_meta"]["hub"] = json!({"positive": "prompt"});
        value["7"]["_meta"] = json!({"hub": {"negative": "samples"}});
        let tpl = WorkflowTemplate::from_value("x.json", value, QWEN_REQUIRED_BINDINGS).unwrap();
        let err = tpl.validate_qwen().expect_err("fail");
        assert!(err.contains("同一个"), "{err}");
    }

    #[test]
    fn output_null_is_allowed_only_for_output_role() {
        let tpl = WorkflowTemplate::parse(
            "x.json",
            r#"{"1":{"class_type":"X","inputs":{},"_meta":{"hub":{"output":null}}}}"#,
            &[],
        )
        .expect("output null allowed");
        assert!(tpl.has_binding("output"));

        let err = WorkflowTemplate::parse(
            "x.json",
            r#"{"1":{"class_type":"X","inputs":{},"_meta":{"hub":{"positive":null}}}}"#,
            &[],
        )
        .expect_err("positive null rejected");
        assert!(err.contains("必须指定输入字段名"), "{err}");
    }

    // ── 尺寸钳制 ─────────────────────────────────────────────────

    #[test]
    fn clamp_keeps_default_1024() {
        assert_eq!(clamp_dimensions(1024, 1024), (1024, 1024));
    }

    #[test]
    fn clamp_rounds_down_to_32() {
        assert_eq!(clamp_dimensions(1000, 1000), (992, 992));
        assert_eq!(clamp_dimensions(100, 100), (96, 96));
        assert_eq!(clamp_dimensions(65, 65), (64, 64));
    }

    #[test]
    fn clamp_enforces_minimum_and_maximum() {
        assert_eq!(clamp_dimensions(1, -5), (64, 64));
        assert_eq!(clamp_dimensions(4095, 100), (4064, 96));
    }

    #[test]
    fn clamp_enforces_pixel_budget() {
        let (w, h) = clamp_dimensions(4096, 4096);
        assert!(w as u64 * h as u64 <= MAX_OUTPUT_PIXELS, "{w}x{h}");
        assert_eq!(w % 32, 0);
        assert_eq!(h % 32, 0);
        assert_eq!((w, h), (2048, 2048));
    }

    #[test]
    fn clamp_extreme_aspect_ratio_still_valid() {
        let (w, h) = clamp_dimensions(4096, 64);
        assert_eq!((w, h), (4096, 64));
        assert!(w <= MAX_DIMENSION && h >= MIN_DIMENSION);
    }

    // ── 参考图编号 ───────────────────────────────────────────────

    #[test]
    fn reference_images_start_at_one_without_presets() {
        let built = template()
            .build_qwen_workflow(&build(&["a.png", "b.png"]))
            .unwrap();
        let inputs = &built.workflow["5"]["inputs"];
        assert_eq!(inputs["images.image_1"], json!(["hub_image_1", 0]));
        assert_eq!(inputs["images.image_2"], json!(["hub_image_2", 0]));
        assert_eq!(built.workflow["hub_image_1"]["class_type"], "LoadImage");
        assert_eq!(built.workflow["hub_image_1"]["inputs"]["image"], "a.png");
        assert_eq!(built.workflow["hub_image_2"]["inputs"]["image"], "b.png");
    }

    #[test]
    fn reference_images_append_after_presets() {
        let mut value: Value = serde_json::from_str(BUNDLED).unwrap();
        value["ref1"] = json!({"class_type":"LoadImage","inputs":{"image":"preset1.png"}});
        value["ref2"] = json!({"class_type":"LoadImage","inputs":{"image":"preset2.png"}});
        value["5"]["inputs"]["images.image_1"] = json!(["ref1", 0]);
        value["5"]["inputs"]["images.image_2"] = json!(["ref2", 0]);
        let tpl =
            WorkflowTemplate::from_value("x.json", value, QWEN_REQUIRED_BINDINGS).unwrap();

        let built = tpl.build_qwen_workflow(&build(&["user.png"])).unwrap();
        let inputs = &built.workflow["5"]["inputs"];
        assert_eq!(inputs["images.image_3"], json!(["hub_image_3", 0]));
        assert_eq!(built.workflow["hub_image_3"]["inputs"]["image"], "user.png");
    }

    #[test]
    fn reference_count_limit_accounts_for_presets() {
        let mut value: Value = serde_json::from_str(BUNDLED).unwrap();
        value["5"]["inputs"]["images.image_1"] = json!(["ref1", 0]);
        value["5"]["inputs"]["images.image_2"] = json!(["ref2", 0]);
        let tpl =
            WorkflowTemplate::from_value("x.json", value, QWEN_REQUIRED_BINDINGS).unwrap();

        let fifteen: Vec<String> = (1..=15).map(|i| format!("u{i}.png")).collect();
        let req = QwenBuildRequest {
            reference_filenames: fifteen,
            ..build(&[])
        };
        let err = tpl.build_qwen_workflow(&req).expect_err("too many");
        assert!(err.contains("最多"), "{err}");

        let fourteen: Vec<String> = (1..=14).map(|i| format!("u{i}.png")).collect();
        let req = QwenBuildRequest {
            reference_filenames: fourteen,
            ..build(&[])
        };
        tpl.build_qwen_workflow(&req).expect("2 preset + 14 = 16");
    }

    #[test]
    fn reference_node_id_collision_is_avoided() {
        let mut value: Value = serde_json::from_str(BUNDLED).unwrap();
        value["hub_image_1"] = json!({"class_type":"LoadImage","inputs":{"image":"occupied.png"}});
        let tpl =
            WorkflowTemplate::from_value("x.json", value, QWEN_REQUIRED_BINDINGS).unwrap();
        let built = tpl.build_qwen_workflow(&build(&["a.png"])).unwrap();
        assert_eq!(built.workflow["5"]["inputs"]["images.image_1"], json!(["hub_image_1_", 0]));
        assert_eq!(built.workflow["hub_image_1"]["inputs"]["image"], "occupied.png");
        assert_eq!(built.workflow["hub_image_1_"]["inputs"]["image"], "a.png");
    }

    #[test]
    fn more_than_sixteen_images_rejected() {
        let names: Vec<String> = (0..17).map(|i| format!("{i}.png")).collect();
        let req = QwenBuildRequest {
            reference_filenames: names,
            ..build(&[])
        };
        let err = template().build_qwen_workflow(&req).expect_err("fail");
        assert!(err.contains("最多支持 16"), "{err}");
    }

    // ── 构建行为 ─────────────────────────────────────────────────

    #[test]
    fn prompt_and_negative_are_written() {
        let mut req = build(&[]);
        req.prompt = "一只蓝色陶瓷小鸟".into();
        req.negative = "blurry".into();
        let built = template().build_qwen_workflow(&req).unwrap();
        assert_eq!(built.workflow["5"]["inputs"]["prompt"], "一只蓝色陶瓷小鸟");
        assert_eq!(built.workflow["5"]["inputs"]["negative_prompt"], "blurry");
    }

    #[test]
    fn edit_without_size_uses_reference_latent() {
        let built = template().build_qwen_workflow(&build(&["a.png"])).unwrap();
        assert!(built.uses_reference_latent);
        assert_eq!(built.workflow["6"]["inputs"]["latent_image"], json!(["5", 2]));
        assert!(built.workflow.get("4").is_none(), "EmptyLatentImage removed");
        assert!(built.width.is_none());
    }

    #[test]
    fn edit_with_size_keeps_empty_latent_and_clamps() {
        let req = QwenBuildRequest {
            width: Some(1000),
            height: Some(700),
            ..build(&["a.png"])
        };
        let built = template().build_qwen_workflow(&req).unwrap();
        assert!(!built.uses_reference_latent);
        assert_eq!(built.workflow["4"]["inputs"]["width"], 992);
        assert_eq!(built.workflow["4"]["inputs"]["height"], 672);
        assert_eq!(built.workflow["6"]["inputs"]["latent_image"], json!(["4", 0]));
        assert_eq!((built.width, built.height), (Some(992), Some(672)));
    }

    #[test]
    fn generation_applies_default_size_clamp() {
        let built = template()
            .build_qwen_workflow(&QwenBuildRequest {
                width: Some(64),
                height: Some(999),
                ..build(&[])
            })
            .unwrap();
        assert_eq!(built.workflow["4"]["inputs"]["width"], 64);
        assert_eq!(built.workflow["4"]["inputs"]["height"], 992);
    }

    #[test]
    fn steps_and_cfg_applied_when_bound() {
        let req = QwenBuildRequest {
            steps: Some(8),
            cfg: Some(2.5),
            ..build(&[])
        };
        let built = template().build_qwen_workflow(&req).unwrap();
        assert_eq!(built.workflow["6"]["inputs"]["steps"], 8);
        assert_eq!(built.workflow["6"]["inputs"]["cfg"], 2.5);
    }

    #[test]
    fn scale_inserts_upscale_and_rewires_output() {
        let req = QwenBuildRequest {
            scale: Some(2.0),
            ..build(&[])
        };
        let built = template().build_qwen_workflow(&req).unwrap();
        assert_eq!(built.workflow["8"]["inputs"]["images"], json!(["hub_upscale", 0]));
        assert_eq!(built.workflow["hub_upscale"]["class_type"], "ImageScaleBy");
        assert_eq!(built.workflow["hub_upscale"]["inputs"]["scale_by"], 2.0);
        assert_eq!(built.workflow["hub_upscale"]["inputs"]["image"], json!(["7", 0]));
    }

    #[test]
    fn invalid_scale_rejected() {
        let req = QwenBuildRequest {
            scale: Some(0.5),
            ..build(&[])
        };
        let err = template().build_qwen_workflow(&req).expect_err("fail");
        assert!(err.contains("1~8"), "{err}");
    }

    #[test]
    fn explicit_seed_is_reproducible() {
        let req = QwenBuildRequest {
            seed: Some(123456),
            ..build(&[])
        };
        let a = template().build_qwen_workflow(&req).unwrap();
        let b = template().build_qwen_workflow(&req).unwrap();
        assert_eq!(a.seed, 123456);
        assert_eq!(a.workflow["6"]["inputs"]["seed"], 123456);
        assert_eq!(a.workflow["6"]["inputs"]["seed"], b.workflow["6"]["inputs"]["seed"]);
    }

    #[test]
    fn random_seed_changes_between_runs() {
        let a = template().build_qwen_workflow(&build(&[])).unwrap();
        let b = template().build_qwen_workflow(&build(&[])).unwrap();
        assert_ne!(a.seed, b.seed);
    }
}
