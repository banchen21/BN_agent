//! 插件配置管理服务层。
//!
//! HTTP handler 在 `main.rs`；这里放与 actix 无关的纯逻辑，便于单测：
//! 列表响应、详情（secret 打码）、保存（校验 + 原子写 + 旧文件快照）、失败回滚。
//!
//! 存储约定见 `plugin_interface::config`：
//! `data/plugin_configs/<plugin-name>.json`（`PLUGIN_CONFIG_DIR` 可覆盖）。

use plugin_interface::config as pi_config;
use plugin_interface::PluginConfigSchema;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// 列表/详情所需的一个已加载插件条目。
#[derive(Clone, Debug)]
pub struct PluginConfigEntry {
    pub name: String,
    pub title: String,
    pub description: String,
    pub schema: Option<PluginConfigSchema>,
}

impl PluginConfigEntry {
    /// 构造条目（测试与未来扩展使用）。
    #[allow(dead_code)]
    pub fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        schema: Option<PluginConfigSchema>,
    ) -> Self {
        let name = name.into();
        let title = title.into();
        Self {
            title: if title.is_empty() {
                name.clone()
            } else {
                title
            },
            name,
            description: description.into(),
            schema,
        }
    }
}

/// `GET /api/plugin-config` 响应体。
pub fn list_response(entries: &[PluginConfigEntry], dir: &Path) -> Value {
    let plugins: Vec<Value> = entries
        .iter()
        .map(|entry| {
            let path = pi_config::config_path(&entry.name, dir).ok();
            let has_file = path.map(|p| p.is_file()).unwrap_or(false);
            json!({
                "name": entry.name,
                "title": entry.title,
                "description": entry.description,
                "editable": entry.schema.is_some(),
                "field_count": entry.schema.as_ref().map(|s| s.fields.len()).unwrap_or(0),
                "has_config_file": has_file,
            })
        })
        .collect();
    json!({
        "config_dir": dir.to_string_lossy(),
        "plugins": plugins,
    })
}

/// `GET /api/plugin-config/{name}` 响应体。
///
/// 无 schema 的插件不暴露原始存储值（其中可能有未声明为 secret 的敏感信息），
/// 只标记 `editable: false`。文件损坏不报错，返回默认值 + warnings。
pub fn detail_response(entry: &PluginConfigEntry, dir: &Path) -> Result<Value, String> {
    let path = pi_config::config_path(&entry.name, dir)?;
    let stored = path.is_file();

    let Some(schema) = entry.schema.as_ref() else {
        return Ok(json!({
            "name": entry.name,
            "title": entry.title,
            "description": entry.description,
            "editable": false,
            "schema": Value::Null,
            "values": Value::Null,
            "path": path.to_string_lossy(),
            "stored": stored,
            "note": "该插件未提供 config schema，不能在 WebUI 中编辑（配置文件仍可被插件自行读取）",
        }));
    };

    let mut warnings = Vec::new();
    let stored_value = match pi_config::read_stored(&path) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(e);
            Value::Object(Map::new())
        }
    };
    let (merged, merge_warnings) = pi_config::merge_with_warnings(schema, &stored_value);
    warnings.extend(merge_warnings);
    let masked = pi_config::mask_secrets(schema, &merged);

    Ok(json!({
        "name": entry.name,
        "title": entry.title,
        "description": entry.description,
        "editable": true,
        "schema": schema,
        "values": masked,
        "path": path.to_string_lossy(),
        "stored": stored,
        "warnings": warnings,
    }))
}

// ── Save ────────────────────────────────────────────────────────────────────

/// 保存前的快照，用于 reload 失败时回滚。
#[derive(Debug)]
pub struct PreparedSave {
    pub path: PathBuf,
    pub old_bytes: Option<Vec<u8>>,
    pub old_existed: bool,
    /// 已打码的副本，可直接返回给 HTTP（明文只写盘、不回传）。
    pub masked: Value,
}

#[derive(Debug)]
pub enum SaveError {
    /// 插件没有 schema / 不存在。
    BadRequest(String),
    /// 字段校验失败（未知字段、类型不符、超范围等）。
    Validation(Vec<String>),
    /// 磁盘写入失败。
    Io(String),
}

/// 校验并写入配置，返回旧文件快照供回滚。
///
/// 注意：写盘成功后调用方必须触发插件 reload；若 reload 失败，调用
/// [`rollback_save`] 恢复旧文件，并再次 reload。
pub fn prepare_save(
    entry: &PluginConfigEntry,
    dir: &Path,
    incoming: &Map<String, Value>,
) -> Result<PreparedSave, SaveError> {
    let Some(schema) = entry.schema.as_ref() else {
        return Err(SaveError::BadRequest(format!(
            "plugin '{}' has no config schema",
            entry.name
        )));
    };

    let path = pi_config::config_path(&entry.name, dir).map_err(SaveError::BadRequest)?;
    let old_bytes = std::fs::read(&path).ok();
    let old_existed = path.is_file();

    let stored = pi_config::read_stored(&path).unwrap_or_else(|_| Value::Object(Map::new()));
    let updated = pi_config::apply_update(schema, &stored, incoming).map_err(SaveError::Validation)?;
    pi_config::write_stored(&path, &updated).map_err(SaveError::Io)?;

    Ok(PreparedSave {
        path,
        old_bytes,
        old_existed,
        masked: pi_config::mask_secrets(schema, &updated),
    })
}

/// reload 失败时恢复保存前的文件内容（原来不存在则删除）。
pub fn rollback_save(prepared: &PreparedSave) -> Result<(), String> {
    log::warn!(
        "[plugin-config] rolling back config file: {}",
        prepared.path.display()
    );
    if prepared.old_existed {
        let bytes = prepared
            .old_bytes
            .as_ref()
            .ok_or_else(|| "old config bytes unavailable".to_string())?;
        write_bytes_atomic(&prepared.path, bytes)
    } else {
        match std::fs::remove_file(&prepared.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove {}: {}", prepared.path.display(), e)),
        }
    }
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("create dir {}: {}", parent.display(), e))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin_config.json");
    let tmp = parent.join(format!(".{}.{}.rollback.tmp", file_name, std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("restore {}: {}", path.display(), e));
    }
    Ok(())
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
            "bn-agent-main-config-{}-{}-{}",
            tag,
            std::process::id(),
            seq
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn schema() -> PluginConfigSchema {
        serde_json::from_value(json!({
            "version": 1,
            "title": "Demo 插件",
            "fields": {
                "url": {"type": "string", "default": "http://127.0.0.1:8188"},
                "token": {"type": "string", "default": "", "secret": true},
                "timeout": {"type": "int", "default": 30, "min": 1, "max": 600},
                "enabled": {"type": "bool", "default": true}
            }
        }))
        .unwrap()
    }

    fn entry() -> PluginConfigEntry {
        PluginConfigEntry::new("demo-plugin", "Demo 插件", "desc", Some(schema()))
    }

    #[test]
    fn list_response_marks_editable_and_counts_fields() {
        let dir = temp_dir("list");
        let entries = vec![
            entry(),
            PluginConfigEntry::new("legacy-plugin", "", "", None),
        ];
        let out = list_response(&entries, &dir);
        assert_eq!(out["config_dir"], dir.to_string_lossy().as_ref());
        let plugins = out["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0]["name"], "demo-plugin");
        assert_eq!(plugins[0]["title"], "Demo 插件");
        assert_eq!(plugins[0]["editable"], true);
        assert_eq!(plugins[0]["field_count"], 4);
        assert_eq!(plugins[1]["name"], "legacy-plugin");
        assert_eq!(plugins[1]["title"], "legacy-plugin");
        assert_eq!(plugins[1]["editable"], false);
        assert_eq!(plugins[1]["field_count"], 0);
    }

    #[test]
    fn detail_returns_defaults_and_masks_secret() {
        let dir = temp_dir("detail");
        let e = entry();
        let path = pi_config::config_path("demo-plugin", &dir).unwrap();
        pi_config::write_stored(&path, &json!({"token": "s3cret", "timeout": 10})).unwrap();

        let out = detail_response(&e, &dir).unwrap();
        assert_eq!(out["editable"], true);
        assert_eq!(out["values"]["token"], pi_config::SECRET_MASK);
        assert_eq!(out["values"]["timeout"], 10);
        assert_eq!(out["values"]["url"], "http://127.0.0.1:8188");
        assert_eq!(out["stored"], true);
        assert!(out["schema"]["fields"]["url"].is_object());
        assert!(out["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn detail_without_schema_does_not_leak_values() {
        let dir = temp_dir("detail-noschema");
        let e = PluginConfigEntry::new("legacy-plugin", "", "", None);
        let path = pi_config::config_path("legacy-plugin", &dir).unwrap();
        pi_config::write_stored(&path, &json!({"api_key": "should-not-leak"})).unwrap();

        let out = detail_response(&e, &dir).unwrap();
        assert_eq!(out["editable"], false);
        assert!(out["schema"].is_null());
        assert!(out["values"].is_null());
        assert!(!out.to_string().contains("should-not-leak"));
        assert_eq!(out["stored"], true);
    }

    #[test]
    fn prepare_save_writes_merged_values_and_masks_response() {
        let dir = temp_dir("save");
        let e = entry();
        let path = pi_config::config_path("demo-plugin", &dir).unwrap();
        pi_config::write_stored(&path, &json!({"token": "old-secret"})).unwrap();

        let incoming = json!({"url": "http://new:9000", "token": pi_config::SECRET_MASK})
            .as_object()
            .unwrap()
            .clone();
        let prepared = prepare_save(&e, &dir, &incoming).unwrap();

        assert_eq!(prepared.masked["token"], pi_config::SECRET_MASK);
        assert_eq!(prepared.masked["url"], "http://new:9000");

        let on_disk = pi_config::read_stored(&path).unwrap();
        assert_eq!(on_disk["token"], "old-secret");
        assert_eq!(on_disk["url"], "http://new:9000");
        assert_eq!(on_disk["timeout"], 30);
        assert_eq!(on_disk["enabled"], true);
        assert_eq!(prepared.old_bytes.as_deref(), Some(br#"{
  "token": "old-secret"
}"#.as_slice()));
    }

    #[test]
    fn prepare_save_rejects_invalid_values_without_writing() {
        let dir = temp_dir("save-invalid");
        let e = entry();
        let path = pi_config::config_path("demo-plugin", &dir).unwrap();

        let bad = json!({"timeout": 9999}).as_object().unwrap().clone();
        match prepare_save(&e, &dir, &bad) {
            Err(SaveError::Validation(errs)) => assert!(errs[0].contains("timeout")),
            other => panic!("expected validation error, got {other:?}"),
        }
        assert!(!path.exists());

        let unknown = json!({"nope": 1}).as_object().unwrap().clone();
        assert!(matches!(
            prepare_save(&e, &dir, &unknown),
            Err(SaveError::Validation(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn prepare_save_rejects_plugin_without_schema() {
        let dir = temp_dir("save-noschema");
        let e = PluginConfigEntry::new("legacy-plugin", "", "", None);
        let result = prepare_save(&e, &dir, &Map::new());
        assert!(matches!(result, Err(SaveError::BadRequest(_))));
    }

    #[test]
    fn rollback_restores_previous_file() {
        let dir = temp_dir("rollback-old");
        let e = entry();
        let path = pi_config::config_path("demo-plugin", &dir).unwrap();
        pi_config::write_stored(&path, &json!({"token": "keep-me", "timeout": 5})).unwrap();
        let old = std::fs::read(&path).unwrap();

        let incoming = json!({"timeout": 60}).as_object().unwrap().clone();
        let prepared = prepare_save(&e, &dir, &incoming).unwrap();
        assert_eq!(pi_config::read_stored(&path).unwrap()["timeout"], 60);
        assert_eq!(prepared.old_bytes.as_deref(), Some(old.as_slice()));

        rollback_save(&prepared).unwrap();
        let restored = pi_config::read_stored(&path).unwrap();
        assert_eq!(restored["timeout"], 5);
        assert_eq!(restored["token"], "keep-me");
    }

    #[test]
    fn rollback_removes_file_that_did_not_exist_before() {
        let dir = temp_dir("rollback-new");
        let e = entry();
        let path = pi_config::config_path("demo-plugin", &dir).unwrap();

        let incoming = json!({"timeout": 60}).as_object().unwrap().clone();
        let prepared = prepare_save(&e, &dir, &incoming).unwrap();
        assert!(path.is_file());

        rollback_save(&prepared).unwrap();
        assert!(!path.exists());
    }
}
