//! Host-level tools for LLM plugin management: load, unload, reload.
//!
//! These are registered in `main.rs` (not by any plugin) so they can
//! hold a reference to `Addr<PluginManager>`.

use actix::prelude::*;
use plugin_interface::*;
use futures_executor::block_on;

// ── LoadPluginTool ───────────────────────────────────────────────────────────

pub struct LoadPluginTool {
    plugin_manager: Addr<super::PluginManager>,
}

impl LoadPluginTool {
    pub fn new(plugin_manager: Addr<super::PluginManager>) -> Self {
        Self { plugin_manager }
    }
}

impl ToolExecutor for LoadPluginTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "load_plugin".into(),
            description: "Load a plugin from a .dll / .so path. Returns plugin info on success.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the plugin .dll / .so file"
                    }
                },
                "required": ["path"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return ToolResult::err("missing: path"),
        };

        match block_on(self.plugin_manager.send(super::LoadPlugin { path })) {
            Ok(Ok(info)) => ToolResult::ok(&format!(
                "✅ 插件 '{}' v{} 已加载 — {}",
                info.name, info.version, info.description
            )),
            Ok(Err(e)) => ToolResult::err(&format!("加载失败：{}", e)),
            Err(e) => ToolResult::err(&format!("通信失败：{}", e)),
        }
    }
}

// ── UnloadPluginTool ─────────────────────────────────────────────────────────

pub struct UnloadPluginTool {
    plugin_manager: Addr<super::PluginManager>,
}

impl UnloadPluginTool {
    pub fn new(plugin_manager: Addr<super::PluginManager>) -> Self {
        Self { plugin_manager }
    }
}

impl ToolExecutor for UnloadPluginTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "unload_plugin".into(),
            description: "Unload a loaded plugin by name. The plugin stops receiving events.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Plugin name to unload (e.g. \"image-plugin\")"
                    }
                },
                "required": ["name"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return ToolResult::err("missing: name"),
        };

        match block_on(self.plugin_manager.send(super::UnloadPlugin { name })) {
            Ok(Ok(())) => ToolResult::ok("✅ 插件已卸载"),
            Ok(Err(e)) => ToolResult::err(&format!("卸载失败：{}", e)),
            Err(e) => ToolResult::err(&format!("通信失败：{}", e)),
        }
    }
}

// ── ReloadPluginTool ─────────────────────────────────────────────────────────

pub struct ReloadPluginTool {
    plugin_manager: Addr<super::PluginManager>,
}

impl ReloadPluginTool {
    pub fn new(plugin_manager: Addr<super::PluginManager>) -> Self {
        Self { plugin_manager }
    }
}

impl ToolExecutor for ReloadPluginTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "reload_plugin".into(),
            description: "Reload a plugin by name (unload + load again from same path). Returns new plugin info.".into(),
            internal: false,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Plugin name to reload (e.g. \"image-plugin\")"
                    }
                },
                "required": ["name"]
            }),
        });
        &DEF
    }

    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return ToolResult::err("missing: name"),
        };

        match block_on(self.plugin_manager.send(super::ReloadPlugin { name })) {
            Ok(Ok(info)) => ToolResult::ok(&format!(
                "✅ 插件 '{}' v{} 已重载 — {}",
                info.name, info.version, info.description
            )),
            Ok(Err(e)) => ToolResult::err(&format!("重载失败：{}", e)),
            Err(e) => ToolResult::err(&format!("通信失败：{}", e)),
        }
    }
}
