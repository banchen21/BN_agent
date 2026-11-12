//! 插件配置。
//!
//! 优先级（高 → 低）：
//! 1. 宿主通过 `PluginContext.config` 下发的值（WebUI 保存后重载）——见 `load(Some(&cfg))`
//! 2. 插件专属环境变量 `COMFYUI_QWEN_*`（仅在宿主未下发配置时作为旧部署回退）
//! 3. `data/plugin_configs/comfyui-qwen-plugin.json`（宿主未提供 config 时的自读兜底）
//! 4. 通用环境变量 `COMFYUI_URL` / `IMAGE_GEN_OUTPUT_DIR`
//! 5. 内置默认值

use serde_json::Value;
use std::path::PathBuf;

pub const PLUGIN_NAME: &str = "comfyui-qwen-plugin";

pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8188";
pub const DEFAULT_TIMEOUT_SECS: u64 = 900;
pub const DEFAULT_WIDTH: u32 = 1024;
pub const DEFAULT_HEIGHT: u32 = 1024;
pub const DEFAULT_STEPS: u32 = 25;
pub const DEFAULT_CFG: f64 = 1.0;
pub const DEFAULT_PUBLISH_EVENT: bool = true;
/// 8GB 显存下 8 张参考图会触发驱动 TDR；默认按安全值 4 张，配置上限保留 1–16。
pub const DEFAULT_MAX_REFERENCE_IMAGES: u32 = 4;

const MIN_TIMEOUT_SECS: u64 = 10;
const MAX_TIMEOUT_SECS: u64 = 3600;
const MIN_STEPS: u32 = 1;
const MAX_STEPS: u32 = 150;
const MIN_CFG: f64 = 0.0;
const MAX_CFG: f64 = 30.0;

#[derive(Clone, Debug, PartialEq)]
pub struct QwenConfig {
    pub server_url: String,
    pub timeout_secs: u64,
    pub workflow_file: String,
    pub default_width: u32,
    pub default_height: u32,
    pub default_steps: u32,
    pub default_cfg: f64,
    pub default_negative: String,
    pub output_dir: String,
    pub publish_event: bool,
    /// 单次编辑允许的参考图数量上限（1–16，默认 4）。
    pub max_reference_images: u32,
}

impl Default for QwenConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            workflow_file: String::new(),
            default_width: DEFAULT_WIDTH,
            default_height: DEFAULT_HEIGHT,
            default_steps: DEFAULT_STEPS,
            default_cfg: DEFAULT_CFG,
            default_negative: String::new(),
            output_dir: String::new(),
            publish_event: DEFAULT_PUBLISH_EVENT,
            max_reference_images: DEFAULT_MAX_REFERENCE_IMAGES,
        }
    }
}

impl QwenConfig {
    /// 从宿主下发的 JSON（已合并 schema 默认值）构造配置。
    pub fn from_json(value: &Value) -> Self {
        let mut cfg = Self::default();
        if let Some(map) = value.as_object() {
            if let Some(v) = map.get("server_url").and_then(Value::as_str) {
                cfg.server_url = v.trim().to_string();
            }
            if let Some(v) = map.get("timeout_secs").and_then(Value::as_u64) {
                cfg.timeout_secs = v.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
            }
            if let Some(v) = map.get("workflow_file").and_then(Value::as_str) {
                cfg.workflow_file = v.trim().to_string();
            }
            if let Some(v) = map.get("default_width").and_then(Value::as_i64) {
                cfg.default_width = normalize_dimension(v);
            }
            if let Some(v) = map.get("default_height").and_then(Value::as_i64) {
                cfg.default_height = normalize_dimension(v);
            }
            if let Some(v) = map.get("default_steps").and_then(Value::as_u64) {
                cfg.default_steps = (v as u32).clamp(MIN_STEPS, MAX_STEPS);
            }
            if let Some(v) = map.get("default_cfg").and_then(Value::as_f64) {
                cfg.default_cfg = v.clamp(MIN_CFG, MAX_CFG);
            }
            if let Some(v) = map.get("default_negative").and_then(Value::as_str) {
                cfg.default_negative = v.to_string();
            }
            if let Some(v) = map.get("output_dir").and_then(Value::as_str) {
                cfg.output_dir = v.trim().to_string();
            }
            if let Some(v) = map.get("publish_event").and_then(Value::as_bool) {
                cfg.publish_event = v;
            }
            // 不做钳制：越界值保留原样，由 `validate()` 明确拒绝。
            if let Some(v) = map.get("max_reference_images").and_then(Value::as_u64) {
                cfg.max_reference_images = v.min(u64::from(u32::MAX)) as u32;
            }
        }
        cfg
    }

    /// 严格校验配置。宿主 schema 校验之外，插件自身也拒绝越界值，
    /// 避免手写配置文件绕过 WebUI 后带病启动。
    pub fn validate(&self) -> Result<(), String> {
        let max_allowed = super::workflow::MAX_REFERENCE_IMAGES as u32;
        if !(1..=max_allowed).contains(&self.max_reference_images) {
            return Err(format!(
                "max_reference_images 必须在 1~{} 之间（当前 {}）",
                max_allowed, self.max_reference_images
            ));
        }
        Ok(())
    }

    /// 宿主加载插件时调用。`host` 为 `ctx.config`（旧宿主没有该字段时传 `None`）。
    ///
    /// 环境变量只在配置文件里**没有显式设置**该字段时生效，
    /// 这样 WebUI 保存的值优先，而未配置的旧部署仍可用环境变量启动。
    pub fn load(host: Option<&Value>) -> Self {
        let explicit = explicit_stored_config();
        let host_has_config = matches!(host, Some(Value::Object(map)) if !map.is_empty());
        let mut cfg = if host_has_config {
            Self::from_json(host.expect("checked above"))
        } else {
            Self::from_json(&explicit)
        };
        let is_explicit = |key: &str| explicit.get(key).is_some();

        if !is_explicit("server_url") {
            if let Some(v) = env_non_empty("COMFYUI_QWEN_URL")
                .or_else(|| env_non_empty("COMFYUI_URL"))
            {
                cfg.server_url = v;
            }
        }
        if !is_explicit("timeout_secs") {
            if let Some(v) =
                env_non_empty("COMFYUI_QWEN_TIMEOUT").and_then(|v| v.parse::<u64>().ok())
            {
                cfg.timeout_secs = v.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
            }
        }
        if !is_explicit("workflow_file") {
            if let Some(v) = env_non_empty("COMFYUI_QWEN_WORKFLOW") {
                cfg.workflow_file = v;
            }
        }
        if !is_explicit("default_width") {
            if let Some(v) =
                env_non_empty("COMFYUI_QWEN_WIDTH").and_then(|v| v.parse::<i64>().ok())
            {
                cfg.default_width = normalize_dimension(v);
            }
        }
        if !is_explicit("default_height") {
            if let Some(v) =
                env_non_empty("COMFYUI_QWEN_HEIGHT").and_then(|v| v.parse::<i64>().ok())
            {
                cfg.default_height = normalize_dimension(v);
            }
        }
        if !is_explicit("default_steps") {
            if let Some(v) =
                env_non_empty("COMFYUI_QWEN_STEPS").and_then(|v| v.parse::<u64>().ok())
            {
                cfg.default_steps = (v as u32).clamp(MIN_STEPS, MAX_STEPS);
            }
        }
        if !is_explicit("default_cfg") {
            if let Some(v) =
                env_non_empty("COMFYUI_QWEN_CFG").and_then(|v| v.parse::<f64>().ok())
            {
                cfg.default_cfg = v.clamp(MIN_CFG, MAX_CFG);
            }
        }
        if !is_explicit("default_negative") {
            if let Some(v) = env_non_empty("COMFYUI_QWEN_NEGATIVE") {
                cfg.default_negative = v;
            }
        }
        if !is_explicit("output_dir") || cfg.output_dir.trim().is_empty() {
            if let Some(v) = env_non_empty("COMFYUI_QWEN_OUTPUT_DIR")
                .or_else(|| env_non_empty("IMAGE_GEN_OUTPUT_DIR"))
            {
                cfg.output_dir = v;
            }
        }
        if !is_explicit("publish_event") {
            if let Some(v) = env_non_empty("COMFYUI_QWEN_PUBLISH_EVENT") {
                cfg.publish_event =
                    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
            }
        }
        if !is_explicit("max_reference_images") {
            if let Some(v) = env_non_empty("COMFYUI_QWEN_MAX_REFERENCES")
                .and_then(|v| v.parse::<u64>().ok())
            {
                cfg.max_reference_images = v.min(u64::from(u32::MAX)) as u32;
            }
        }

        if cfg.server_url.trim().is_empty() {
            cfg.server_url = DEFAULT_SERVER_URL.to_string();
        }
        cfg
    }

    /// 实际输出目录（不存在则由调用方创建）。
    pub fn resolved_output_dir(&self) -> PathBuf {
        if !self.output_dir.trim().is_empty() {
            PathBuf::from(self.output_dir.trim())
        } else if let Some(dir) = env_non_empty("IMAGE_GEN_OUTPUT_DIR") {
            PathBuf::from(dir)
        } else {
            PathBuf::from("temp_images")
        }
    }
}

fn explicit_stored_config() -> Value {
    let dir = plugin_interface::config::config_dir();
    let Ok(path) = plugin_interface::config::config_path(PLUGIN_NAME, &dir) else {
        return Value::Object(serde_json::Map::new());
    };
    plugin_interface::config::read_stored(&path)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_dimension(value: i64) -> u32 {
    value.clamp(super::workflow::MIN_DIMENSION as i64, super::workflow::MAX_DIMENSION as i64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_requirements() {
        let cfg = QwenConfig::default();
        assert_eq!(cfg.server_url, "http://127.0.0.1:8188");
        assert_eq!(cfg.timeout_secs, 900);
        assert_eq!(cfg.default_width, 1024);
        assert_eq!(cfg.default_height, 1024);
        assert_eq!(cfg.default_steps, 25);
        assert_eq!(cfg.default_cfg, 1.0);
        assert!(cfg.default_negative.is_empty());
        assert!(cfg.publish_event);
        assert_eq!(cfg.max_reference_images, 4);
        cfg.validate().expect("defaults valid");
    }

    #[test]
    fn from_json_applies_overrides_and_clamps() {
        let cfg = QwenConfig::from_json(&json!({
            "server_url": "  http://10.0.0.2:8188  ",
            "timeout_secs": 99999,
            "workflow_file": "custom.json",
            "default_width": 12,
            "default_height": 9000,
            "default_steps": 999,
            "default_cfg": 99.0,
            "default_negative": "blur",
            "output_dir": " D:/out ",
            "publish_event": false,
            "max_reference_images": 8
        }));
        assert_eq!(cfg.server_url, "http://10.0.0.2:8188");
        assert_eq!(cfg.timeout_secs, 3600);
        assert_eq!(cfg.workflow_file, "custom.json");
        assert_eq!(cfg.default_width, super::super::workflow::MIN_DIMENSION);
        assert_eq!(cfg.default_height, super::super::workflow::MAX_DIMENSION);
        assert_eq!(cfg.default_steps, 150);
        assert_eq!(cfg.default_cfg, 30.0);
        assert_eq!(cfg.default_negative, "blur");
        assert_eq!(cfg.output_dir, "D:/out");
        assert!(!cfg.publish_event);
        assert_eq!(cfg.max_reference_images, 8);
        cfg.validate().expect("8 is within 1..=16");
    }

    #[test]
    fn validate_rejects_out_of_range_max_reference_images() {
        for value in [0u64, 17, 100] {
            let cfg = QwenConfig::from_json(&json!({ "max_reference_images": value }));
            let err = cfg.validate().expect_err("must reject");
            assert!(err.contains("max_reference_images"), "{err}");
            assert!(err.contains("1~16"), "{err}");
        }
        for value in [1u64, 4, 8, 16] {
            let cfg = QwenConfig::from_json(&json!({ "max_reference_images": value }));
            cfg.validate().expect("within range");
            assert_eq!(cfg.max_reference_images, value as u32);
        }
    }

    #[test]
    fn from_json_ignores_wrong_types() {
        let cfg = QwenConfig::from_json(&json!({
            "server_url": 123,
            "timeout_secs": "abc",
            "default_steps": 7.5,
            "max_reference_images": "four"
        }));
        assert_eq!(cfg, QwenConfig::default());
    }
}
