//! 插件配置 schema 约定 v1 + 配置文件读写。
//!
//! ## 约定
//!
//! - 插件在 `Plugin::config_schema()` 中返回一个 [`PluginConfigSchema`]（JSON 形态见文档）。
//! - 配置落盘于 `data/plugin_configs/<plugin-name>.json`（可用 `PLUGIN_CONFIG_DIR` 覆盖），
//!   文件内容是一个扁平 JSON 对象：`{"field": value, ...}`。
//! - 宿主在加载/重载插件时读取文件，按 schema 补齐默认值，然后通过
//!   `PluginContext::config` 注入给插件。
//! - `secret: true` 的字段：HTTP GET 返回 [`SECRET_MASK`]；PUT 时回传 `SECRET_MASK`
//!   或省略该字段表示保持原值，传空字符串表示清空。
//!
//! 本模块所有函数都不 panic；文件损坏/缺字段/类型非法时回退到默认值并返回警告，
//! 保证没有 schema 或配置损坏的插件也能正常启动。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 覆盖配置目录的环境变量。
pub const CONFIG_DIR_ENV: &str = "PLUGIN_CONFIG_DIR";
/// 默认配置目录（相对进程工作目录）。
pub const DEFAULT_CONFIG_DIR: &str = "data/plugin_configs";
/// secret 字段在 HTTP 响应中的打码值 / PUT 保持原值的哨兵。
pub const SECRET_MASK: &str = "******";

// ── Schema types ────────────────────────────────────────────────────────────

/// 字段类型。与 WebUI 表单控件一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFieldType {
    /// 单行文本
    String,
    /// 多行文本
    Text,
    /// 布尔开关
    Bool,
    /// 整数
    Int,
    /// 浮点数
    Float,
    /// 枚举下拉（`options` 必填）
    Select,
    /// 字符串数组（WebUI 按行编辑）
    List,
}

impl Default for ConfigFieldType {
    fn default() -> Self {
        Self::String
    }
}

impl ConfigFieldType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Text => "text",
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Select => "select",
            Self::List => "list",
        }
    }

    /// 未显式给出 `default` 时，按类型生成的默认值。
    pub fn type_default(&self) -> Value {
        match self {
            Self::String | Self::Text => Value::String(String::new()),
            Self::Bool => Value::Bool(false),
            Self::Int => Value::from(0),
            Self::Float => Value::from(0.0),
            Self::Select => Value::Null,
            Self::List => Value::Array(Vec::new()),
        }
    }
}

/// 单个配置字段的描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    #[serde(rename = "type", default)]
    pub field_type: ConfigFieldType,
    /// 默认值；缺省时按 `type` 生成（select 取第一个 option）。
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub description: String,
    /// `select` 的候选值。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Value>,
    /// 是否敏感字段（GET 打码，PUT 支持保持原值）。
    #[serde(default)]
    pub secret: bool,
    /// int/float 最小值（含）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// int/float 最大值（含）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl Default for ConfigField {
    fn default() -> Self {
        Self {
            field_type: ConfigFieldType::String,
            default: Value::Null,
            description: String::new(),
            options: Vec::new(),
            secret: false,
            min: None,
            max: None,
            placeholder: None,
        }
    }
}

impl ConfigField {
    pub fn new(field_type: ConfigFieldType) -> Self {
        Self {
            field_type,
            ..Self::default()
        }
    }

    pub fn with_default(mut self, value: Value) -> Self {
        self.default = value;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    pub fn with_options(mut self, options: Vec<Value>) -> Self {
        self.options = options;
        self
    }

    /// 生效默认值：显式 `default` 优先；否则按类型生成（select 取第一个 option）。
    pub fn default_value(&self) -> Value {
        if !self.default.is_null() {
            return self.default.clone();
        }
        match self.field_type {
            ConfigFieldType::Select => self.options.first().cloned().unwrap_or(Value::Null),
            other => other.type_default(),
        }
    }
}

/// 一个插件的完整配置 schema。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigSchema {
    #[serde(default = "default_schema_version")]
    pub version: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// 字段名 → 字段描述。BTreeMap 保证输出顺序稳定（按名字排序）。
    #[serde(default)]
    pub fields: BTreeMap<String, ConfigField>,
}

fn default_schema_version() -> u32 {
    1
}

impl Default for PluginConfigSchema {
    fn default() -> Self {
        Self {
            version: 1,
            title: String::new(),
            description: String::new(),
            fields: BTreeMap::new(),
        }
    }
}

impl PluginConfigSchema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn field(mut self, name: impl Into<String>, field: ConfigField) -> Self {
        self.fields.insert(name.into(), field);
        self
    }
}

// ── Paths ───────────────────────────────────────────────────────────────────

/// 配置目录：`PLUGIN_CONFIG_DIR`（非空）优先，否则 `data/plugin_configs`。
pub fn config_dir() -> PathBuf {
    std::env::var(CONFIG_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_DIR))
}

/// 插件名是否可安全用于文件名（拒绝路径分隔符与 `..`）。
pub fn is_valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
}

/// `dir` 下某个插件的配置文件路径。
pub fn config_path(plugin: &str, dir: &Path) -> Result<PathBuf, String> {
    if !is_valid_plugin_name(plugin) {
        return Err(format!("invalid plugin name '{}'", plugin));
    }
    Ok(dir.join(format!("{}.json", plugin)))
}

// ── File IO ─────────────────────────────────────────────────────────────────

/// 读取已存储的配置对象。文件不存在返回空对象；JSON 非法或顶层不是对象返回 Err。
pub fn read_stored(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if text.trim().is_empty() {
                return Ok(Value::Object(Map::new()));
            }
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| format!("invalid JSON in {}: {}", path.display(), e))?;
            if value.is_object() {
                Ok(value)
            } else {
                Err(format!("config {} must be a JSON object", path.display()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(e) => Err(format!("read {}: {}", path.display(), e)),
    }
}

/// 原子写入配置对象（先写同目录临时文件再 rename），自动创建父目录。
pub fn write_stored(path: &Path, values: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("create dir {}: {}", parent.display(), e))?;

    let text = serde_json::to_string_pretty(values)
        .map_err(|e| format!("serialize config: {}", e))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin_config.json");
    let tmp = parent.join(format!(".{}.{}.tmp", file_name, std::process::id()));
    std::fs::write(&tmp, text.as_bytes())
        .map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("rename to {}: {}", path.display(), e));
    }
    Ok(())
}

// ── Validation ──────────────────────────────────────────────────────────────

fn validate_number_range(field: &ConfigField, value: f64) -> Result<(), String> {
    if let Some(min) = field.min {
        if value < min {
            return Err(format!("must be >= {}", min));
        }
    }
    if let Some(max) = field.max {
        if value > max {
            return Err(format!("must be <= {}", max));
        }
    }
    Ok(())
}

/// 按字段类型校验单个值，返回可读错误。
pub fn validate_field(field: &ConfigField, value: &Value) -> Result<(), String> {
    match field.field_type {
        ConfigFieldType::String | ConfigFieldType::Text => {
            if value.is_string() {
                Ok(())
            } else {
                Err("expected string".into())
            }
        }
        ConfigFieldType::Bool => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err("expected bool".into())
            }
        }
        ConfigFieldType::Int => {
            let Some(n) = value.as_f64() else {
                return Err("expected integer".into());
            };
            if n.fract() != 0.0 || !n.is_finite() {
                return Err("expected integer".into());
            }
            validate_number_range(field, n)
        }
        ConfigFieldType::Float => {
            let Some(n) = value.as_f64() else {
                return Err("expected number".into());
            };
            if !n.is_finite() {
                return Err("expected finite number".into());
            }
            validate_number_range(field, n)
        }
        ConfigFieldType::Select => {
            if field.options.contains(value) {
                Ok(())
            } else {
                let options: Vec<String> = field.options.iter().map(|v| v.to_string()).collect();
                Err(format!("must be one of [{}]", options.join(", ")))
            }
        }
        ConfigFieldType::List => match value.as_array() {
            Some(items) if items.iter().all(|v| v.is_string()) => Ok(()),
            Some(_) => Err("expected array of strings".into()),
            None => Err("expected array of strings".into()),
        },
    }
}

/// 校验一组待写入值。`reject_unknown` 为 true 时拒绝 schema 之外的字段（PUT 用）。
pub fn validate_values(
    schema: &PluginConfigSchema,
    values: &Map<String, Value>,
    reject_unknown: bool,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for (name, value) in values {
        match schema.fields.get(name) {
            Some(field) => {
                if let Err(e) = validate_field(field, value) {
                    errors.push(format!("{}: {}", name, e));
                }
            }
            None if reject_unknown => errors.push(format!("{}: unknown field", name)),
            None => {}
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ── Merge / mask / update ───────────────────────────────────────────────────

/// 用 schema 默认值合并已存储值，并收集被忽略的非法值/未知字段警告。
pub fn merge_with_warnings(
    schema: &PluginConfigSchema,
    stored: &Value,
) -> (Value, Vec<String>) {
    let mut out = Map::new();
    let mut warnings = Vec::new();
    let stored_obj = stored.as_object();

    for (name, field) in &schema.fields {
        let mut value = field.default_value();
        if let Some(sv) = stored_obj.and_then(|o| o.get(name)) {
            match validate_field(field, sv) {
                Ok(()) => value = sv.clone(),
                Err(e) => warnings.push(format!("{}: stored value ignored ({})", name, e)),
            }
        }
        out.insert(name.clone(), value);
    }

    if let Some(obj) = stored_obj {
        for key in obj.keys() {
            if !schema.fields.contains_key(key) {
                warnings.push(format!("{}: unknown field ignored", key));
            }
        }
    }

    (Value::Object(out), warnings)
}

/// 同 [`merge_with_warnings`]，丢弃警告。
pub fn merge_with_defaults(schema: &PluginConfigSchema, stored: &Value) -> Value {
    merge_with_warnings(schema, stored).0
}

/// 生成用于 HTTP 响应的打码副本：secret 字段的非空值替换为 [`SECRET_MASK`]。
pub fn mask_secrets(schema: &PluginConfigSchema, values: &Value) -> Value {
    let mut out = values.clone();
    if let Some(obj) = out.as_object_mut() {
        for (name, field) in &schema.fields {
            if !field.secret {
                continue;
            }
            if let Some(value) = obj.get_mut(name) {
                let empty = match value {
                    Value::Null => true,
                    Value::String(s) => s.is_empty(),
                    _ => false,
                };
                if !empty {
                    *value = Value::String(SECRET_MASK.to_string());
                }
            }
        }
    }
    out
}

/// PUT 语义：校验 incoming（拒绝未知字段/非法类型），合并到 stored 之上。
///
/// - 未出现在 incoming 中的字段保持已存储值（或默认值）。
/// - secret 字段的 incoming 值等于 [`SECRET_MASK`] 时保留已存储明文。
/// - secret 字段传空字符串表示清空。
pub fn apply_update(
    schema: &PluginConfigSchema,
    stored: &Value,
    incoming: &Map<String, Value>,
) -> Result<Value, Vec<String>> {
    validate_values(schema, incoming, true)?;

    let mut merged = merge_with_defaults(schema, stored);
    let merged_obj = merged
        .as_object_mut()
        .expect("merge_with_defaults always returns an object");

    for (name, value) in incoming {
        let is_secret = schema
            .fields
            .get(name)
            .map(|f| f.secret)
            .unwrap_or(false);
        if is_secret && value.as_str() == Some(SECRET_MASK) {
            // 保持 merged 中来自 stored 的原值。
            continue;
        }
        merged_obj.insert(name.clone(), value.clone());
    }

    Ok(merged)
}

/// 宿主加载/重载插件时使用：读取 + 合并，绝不失败。
///
/// 返回 `(注入给插件的配置, 可选警告)`。无 schema 时原样透传已存储对象
/// （保证老插件仍可按需读取自己的配置文件）。
pub fn load_merged(
    plugin: &str,
    schema: Option<&PluginConfigSchema>,
    dir: &Path,
) -> (Value, Option<String>) {
    let empty = Value::Object(Map::new());
    let path = match config_path(plugin, dir) {
        Ok(p) => p,
        Err(e) => return (schema.map(|s| merge_with_defaults(s, &empty)).unwrap_or(empty), Some(e)),
    };

    let stored = match read_stored(&path) {
        Ok(v) => v,
        Err(e) => {
            let value = schema
                .map(|s| merge_with_defaults(s, &empty))
                .unwrap_or(empty);
            return (value, Some(e));
        }
    };

    match schema {
        Some(schema) => {
            let (value, warnings) = merge_with_warnings(schema, &stored);
            let warning = if warnings.is_empty() {
                None
            } else {
                Some(warnings.join("; "))
            };
            (value, warning)
        }
        None => (stored, None),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "bn-agent-plugin-config-{}-{}-{}",
            tag,
            std::process::id(),
            seq
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_schema() -> PluginConfigSchema {
        serde_json::from_value(json!({
            "version": 1,
            "title": "Demo",
            "description": "demo schema",
            "fields": {
                "url": {"type": "string", "default": "http://127.0.0.1:8188", "description": "url"},
                "token": {"type": "string", "default": "", "secret": true, "description": "token"},
                "timeout": {"type": "int", "default": 30, "min": 1, "max": 600},
                "ratio": {"type": "float", "default": 0.5},
                "enabled": {"type": "bool", "default": true},
                "model": {"type": "select", "options": ["a", "b"], "default": "a"},
                "tags": {"type": "list", "default": []},
                "prompt": {"type": "text", "default": ""}
            }
        }))
        .unwrap()
    }

    #[test]
    fn schema_parses_all_field_types() {
        let schema = sample_schema();
        assert_eq!(schema.version, 1);
        assert_eq!(schema.title, "Demo");
        assert_eq!(schema.fields.len(), 8);
        assert_eq!(schema.fields["url"].field_type, ConfigFieldType::String);
        assert_eq!(schema.fields["prompt"].field_type, ConfigFieldType::Text);
        assert_eq!(schema.fields["enabled"].field_type, ConfigFieldType::Bool);
        assert_eq!(schema.fields["timeout"].field_type, ConfigFieldType::Int);
        assert_eq!(schema.fields["ratio"].field_type, ConfigFieldType::Float);
        assert_eq!(schema.fields["model"].field_type, ConfigFieldType::Select);
        assert_eq!(schema.fields["tags"].field_type, ConfigFieldType::List);
        assert!(schema.fields["token"].secret);
        assert_eq!(schema.fields["timeout"].min, Some(1.0));
        assert_eq!(schema.fields["timeout"].max, Some(600.0));
    }

    #[test]
    fn schema_serializes_type_key() {
        let value = serde_json::to_value(ConfigField::new(ConfigFieldType::Int)).unwrap();
        assert_eq!(value["type"], "int");
        assert!(value.get("options").is_none());
        assert!(value.get("min").is_none());
    }

    #[test]
    fn default_value_falls_back_by_type_and_select_option() {
        let mut field = ConfigField::new(ConfigFieldType::Int);
        assert_eq!(field.default_value(), json!(0));
        field.field_type = ConfigFieldType::Bool;
        assert_eq!(field.default_value(), json!(false));
        field.field_type = ConfigFieldType::List;
        assert_eq!(field.default_value(), json!([]));
        field.field_type = ConfigFieldType::Select;
        field.options = vec![json!("x"), json!("y")];
        assert_eq!(field.default_value(), json!("x"));
    }

    #[test]
    fn merge_fills_defaults_and_overrides() {
        let schema = sample_schema();
        let stored = json!({"url": "http://x:1234", "timeout": 5});
        let merged = merge_with_defaults(&schema, &stored);
        assert_eq!(merged["url"], "http://x:1234");
        assert_eq!(merged["timeout"], 5);
        assert_eq!(merged["enabled"], true);
        assert_eq!(merged["model"], "a");
        assert_eq!(merged["tags"], json!([]));
    }

    #[test]
    fn merge_ignores_invalid_stored_value() {
        let schema = sample_schema();
        let stored = json!({"timeout": "not-a-number", "url": "http://ok"});
        let (merged, warnings) = merge_with_warnings(&schema, &stored);
        assert_eq!(merged["timeout"], 30);
        assert_eq!(merged["url"], "http://ok");
        assert!(warnings.iter().any(|w| w.contains("timeout")));
    }

    #[test]
    fn merge_warns_unknown_stored_field() {
        let schema = sample_schema();
        let stored = json!({"legacy": 1});
        let (merged, warnings) = merge_with_warnings(&schema, &stored);
        assert!(merged.get("legacy").is_none());
        assert!(warnings.iter().any(|w| w.contains("legacy")));
    }

    #[test]
    fn validate_rejects_unknown_field_when_strict() {
        let schema = sample_schema();
        let values = json!({"nope": 1}).as_object().unwrap().clone();
        let errs = validate_values(&schema, &values, true).unwrap_err();
        assert!(errs[0].contains("unknown field"));
    }

    #[test]
    fn validate_accepts_unknown_field_when_lenient() {
        let schema = sample_schema();
        let values = json!({"nope": 1}).as_object().unwrap().clone();
        assert!(validate_values(&schema, &values, false).is_ok());
    }

    #[test]
    fn validate_rejects_wrong_types() {
        let schema = sample_schema();
        let bad = json!({
            "url": 1,
            "enabled": "yes",
            "timeout": 1.5,
            "ratio": "0.5",
            "tags": ["ok", 3],
            "prompt": false
        })
        .as_object()
        .unwrap()
        .clone();
        let errs = validate_values(&schema, &bad, true).unwrap_err();
        assert_eq!(errs.len(), 6, "errors: {:?}", errs);
    }

    #[test]
    fn validate_select_must_be_an_option() {
        let schema = sample_schema();
        let bad = json!({"model": "c"}).as_object().unwrap().clone();
        let errs = validate_values(&schema, &bad, true).unwrap_err();
        assert!(errs[0].contains("must be one of"));

        let good = json!({"model": "b"}).as_object().unwrap().clone();
        assert!(validate_values(&schema, &good, true).is_ok());
    }

    #[test]
    fn validate_int_range_and_finite() {
        let schema = sample_schema();
        for value in [json!(0), json!(601), json!(1.5)] {
            let vals = json!({"timeout": value}).as_object().unwrap().clone();
            assert!(validate_values(&schema, &vals, true).is_err(), "{value}");
        }
        let ok = json!({"timeout": 600}).as_object().unwrap().clone();
        assert!(validate_values(&schema, &ok, true).is_ok());
    }

    #[test]
    fn mask_secrets_only_masks_non_empty_secret_values() {
        let schema = sample_schema();
        let stored = json!({"url": "http://x", "token": "s3cret"});
        let masked = mask_secrets(&schema, &stored);
        assert_eq!(masked["url"], "http://x");
        assert_eq!(masked["token"], SECRET_MASK);

        let empty = json!({"token": ""});
        let masked_empty = mask_secrets(&schema, &empty);
        assert_eq!(masked_empty["token"], "");
    }

    #[test]
    fn apply_update_preserves_masked_secret() {
        let schema = sample_schema();
        let stored = json!({"token": "s3cret", "url": "http://old"});
        let incoming = json!({"token": SECRET_MASK, "url": "http://new"})
            .as_object()
            .unwrap()
            .clone();
        let updated = apply_update(&schema, &stored, &incoming).unwrap();
        assert_eq!(updated["token"], "s3cret");
        assert_eq!(updated["url"], "http://new");
    }

    #[test]
    fn apply_update_omitted_secret_keeps_value_and_new_value_replaces_it() {
        let schema = sample_schema();
        let stored = json!({"token": "s3cret"});

        let omitted = json!({"url": "http://new"}).as_object().unwrap().clone();
        let updated = apply_update(&schema, &stored, &omitted).unwrap();
        assert_eq!(updated["token"], "s3cret");

        let replaced = json!({"token": "next"})
            .as_object()
            .unwrap()
            .clone();
        let updated = apply_update(&schema, &stored, &replaced).unwrap();
        assert_eq!(updated["token"], "next");
    }

    #[test]
    fn apply_update_empty_string_clears_secret() {
        let schema = sample_schema();
        let stored = json!({"token": "s3cret"});
        let incoming = json!({"token": ""}).as_object().unwrap().clone();
        let updated = apply_update(&schema, &stored, &incoming).unwrap();
        assert_eq!(updated["token"], "");
    }

    #[test]
    fn apply_update_includes_defaults_and_rejects_unknown() {
        let schema = sample_schema();
        let incoming = json!({"url": "http://x"}).as_object().unwrap().clone();
        let updated = apply_update(&schema, &json!({}), &incoming).unwrap();
        assert_eq!(updated["timeout"], 30);
        assert_eq!(updated["enabled"], true);

        let bad = json!({"nope": true}).as_object().unwrap().clone();
        assert!(apply_update(&schema, &json!({}), &bad).is_err());
    }

    #[test]
    fn file_roundtrip_and_missing_file() {
        let dir = temp_dir("roundtrip");
        let path = config_path("demo-plugin", &dir).unwrap();

        assert_eq!(read_stored(&path).unwrap(), json!({}));
        write_stored(&path, &json!({"url": "http://x", "n": 3})).unwrap();
        assert_eq!(
            read_stored(&path).unwrap(),
            json!({"url": "http://x", "n": 3})
        );

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"url\""));
    }

    #[test]
    fn read_stored_rejects_non_object_and_bad_json() {
        let dir = temp_dir("bad");
        let path = dir.join("bad.json");
        std::fs::write(&path, "[1,2,3]").unwrap();
        assert!(read_stored(&path).is_err());
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_stored(&path).is_err());
        std::fs::write(&path, "  ").unwrap();
        assert_eq!(read_stored(&path).unwrap(), json!({}));
    }

    #[test]
    fn invalid_plugin_names_rejected() {
        let dir = Path::new("data/plugin_configs");
        for name in ["", ".", "..", "../x", "a/b", "a\\b", "C:x"] {
            assert!(config_path(name, dir).is_err(), "{name}");
        }
        assert!(config_path("comfyui-qwen-plugin", dir).is_ok());
    }

    #[test]
    fn load_merged_without_schema_passes_stored_raw() {
        let dir = temp_dir("noschema");
        let path = config_path("legacy-plugin", &dir).unwrap();
        write_stored(&path, &json!({"anything": "ok", "n": 1})).unwrap();
        let (cfg, warning) = load_merged("legacy-plugin", None, &dir);
        assert_eq!(cfg["anything"], "ok");
        assert!(warning.is_none());
    }

    #[test]
    fn load_merged_corrupt_file_returns_defaults_with_warning() {
        let dir = temp_dir("corrupt");
        let path = config_path("demo-plugin", &dir).unwrap();
        std::fs::write(&path, "{broken").unwrap();
        let schema = sample_schema();
        let (cfg, warning) = load_merged("demo-plugin", Some(&schema), &dir);
        assert_eq!(cfg["timeout"], 30);
        assert!(warning.is_some());
    }

    #[test]
    fn load_merged_missing_file_returns_defaults() {
        let dir = temp_dir("missing");
        let schema = sample_schema();
        let (cfg, warning) = load_merged("demo-plugin", Some(&schema), &dir);
        assert_eq!(cfg["url"], "http://127.0.0.1:8188");
        assert!(warning.is_none());
    }
}
