//! Skill Plugin — loads Markdown skills as callable tools for the LLM.
//!
//! Reads `SKILL_DIR` env (default `data/skills/`), scans for `*.md` files,
//! parses YAML frontmatter (`---` delimited), and registers each skill as
//! a `skill__{name}` tool.
//!
//! When called, the skill's body text (with `{{param}}` placeholders replaced
//! by the tool arguments) is returned as the tool result — the LLM then follows
//! the instructions.
//!
//! # Skill file format
//! ```markdown
//! ---
//! name: summarize_text
//! description: "对文本进行摘要总结"
//! parameters:
//!   type: object
//!   properties:
//!     text:
//!       type: string
//!       description: "要总结的文本内容"
//!   required: [text]
//! ---
//! 请对以下文本进行简明扼要的总结，提取关键信息，控制在 200 字以内：
//!
//! {{text}}
//! ```
//!
//! # Env
//! - `SKILL_DIR` — directory to scan for `.md` skill files (default `data/skills/`)

use plugin_interface::*;
use std::sync::Arc;

// ── Skill file parsing ───────────────────────────────────────────────────────

/// Parsed YAML frontmatter of a skill file.
#[derive(serde::Deserialize)]
struct SkillMeta {
    name: String,
    description: String,
    #[serde(default = "default_parameters")]
    parameters: serde_json::Value,
}

fn default_parameters() -> serde_json::Value {
    serde_json::json!({"type":"object","properties":{}})
}

/// Parsed skill definition.
struct SkillDef {
    meta: SkillMeta,
    body: String,
}

/// Split a `.md` file into (frontmatter_yaml, body).
/// Expects the file to start with `---\n`, then YAML, then `\n---\n`, then body.
fn parse_skill_file(content: &str) -> Option<(String, String)> {
    let content = content.trim();
    if !content.starts_with("---") {
        return None;
    }
    let after_first = content.strip_prefix("---")?;
    let end = after_first.find("\n---")?;
    let yaml_part = &after_first[..end];
    // Skip past the closing ---
    let body_start = end + 4; // \n + ---
    let body_part = after_first[body_start..].trim();
    Some((yaml_part.to_string(), body_part.to_string()))
}

/// Load all `.md` files from a directory and parse them into `SkillDef`s.
fn load_skills(dir: &str) -> Vec<SkillDef> {
    let Ok(entries) = std::fs::read_dir(dir) else {
         log::info!("[skill] SKILL_DIR '{dir}' not found, skipping");
        return Vec::new();
    };

    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                 log::info!("[skill] failed to read '{}': {}", path.display(), e);
                continue;
            }
        };

        let (yaml_str, body) = match parse_skill_file(&content) {
            Some(p) => p,
            None => {
                 log::info!("[skill] '{}' has no valid frontmatter (needs `---` delimiters), skipping", path.display());
                continue;
            }
        };

        let meta: SkillMeta = match serde_yaml::from_str(&yaml_str) {
            Ok(m) => m,
            Err(e) => {
                 log::info!("[skill] '{}' frontmatter parse error: {}", path.display(), e);
                continue;
            }
        };

         log::info!("[skill] loaded skill '{}' from {}", meta.name, path.display());
        skills.push(SkillDef { meta, body });
    }

    skills
}

// ── Tool executor ────────────────────────────────────────────────────────────

struct SkillToolExecutor {
    def: ToolDef,
    body: String,
}

impl SkillToolExecutor {
    fn new(skill: SkillDef) -> Self {
        let parameters = skill.meta.parameters;
        let def = ToolDef {
            name: format!("skill__{}", skill.meta.name),
            description: skill.meta.description,
            parameters,
            internal: false,
        };
        Self { def, body: skill.body }
    }

    /// Replace `{{param_name}}` placeholders with values from `args`.
    fn render_body(&self, args: &serde_json::Value) -> String {
        let mut result = self.body.clone();
        if let serde_json::Value::Object(map) = args {
            for (key, value) in map {
                let placeholder = format!("{{{{{}}}}}", key);
                let replacement = match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                result = result.replace(&placeholder, &replacement);
            }
        }
        result
    }
}

impl ToolExecutor for SkillToolExecutor {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let rendered = self.render_body(args);
        ToolResult::ok(&rendered)
    }
}

// ── Plugin ───────────────────────────────────────────────────────────────────

pub struct SkillPlugin {
    info: PluginInfo,
}

impl SkillPlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "skill-plugin".into(),
                version: "0.1.0".into(),
                description: "Skill Plugin — loads Markdown skills as callable tools for the LLM".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
        }
    }
}

impl Plugin for SkillPlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let skill_dir = std::env::var("SKILL_DIR").unwrap_or_else(|_| "data/skills".into());
         log::info!("[skill] loading skills from '{skill_dir}'");

        let skills = load_skills(&skill_dir);
        if skills.is_empty() {
             log::info!("[skill] WARN: no skills loaded ({} dir exists but no valid .md files)", skill_dir);
            return Ok(());
        }

        let registry = match ctx.tool_registry {
            Some(ref r) => r,
            None => {
                 log::info!("[skill] WARN: no tool_registry available, skills will not be registered");
                return Ok(());
            }
        };

        let mut reg = registry.lock().map_err(|e| format!("lock: {}", e))?;
        let skill_count = skills.len();
        for skill in skills {
            let executor = Arc::new(SkillToolExecutor::new(skill));
            let name = executor.def().name.clone();
            reg.register(executor);
             log::info!("[skill] registered tool: {name}");
        }

         log::info!("[skill] started with {} skill(s)", skill_count);
        Ok(())
    }

    fn on_event(&self, _event: &Event) -> bool { true }

    fn stop(&mut self) {
         log::info!("[skill] stopped");
    }
}

// ── DLL exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(SkillPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_p: Box<dyn Plugin>) {}
