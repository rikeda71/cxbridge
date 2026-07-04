use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{DiagLevel, Diagnostic};
use crate::core::model_tiers::{codex_tier, tier_to_claude, Tier};
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};

use super::SettingsHandler;

impl SettingsHandler {
    /// x2c lower: produce Claude settings.json from Codex config.toml IR.
    pub(super) fn lower_x2c(
        &self,
        ir: &crate::core::ir::IRNode,
        opts: &LowerOpts,
    ) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        let out_root = opts.out.as_deref().unwrap_or(".");

        let mut settings = serde_json::Map::new();

        // model: tier mapping (lossy); the emitted High-tier name is Opus, never Fable/Mythos.
        // codex_tier is total — unknown names fall back to Mid — so there is no pass-through path.
        if let Some(f) = ir.fields.get("settings.model") {
            if let Some(s) = f.value.as_str() {
                let claude_model = tier_to_claude(codex_tier(s).unwrap_or(Tier::Mid));
                settings.insert("model".to_string(), Value::String(claude_model.to_string()));
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.model".to_string()),
                    message: format!(
                        "model '{}' mapped to '{}' via tier mapping (lossy; different provider)",
                        s, claude_model
                    ),
                });
            }
        }

        // effortLevel (already reverse-mapped via lift)
        if let Some(f) = ir.fields.get("settings.effortLevel") {
            if let Some(s) = f.value.as_str() {
                settings.insert("effortLevel".to_string(), Value::String(s.to_string()));
            }
        }

        // editorMode
        if let Some(f) = ir.fields.get("settings.editorMode") {
            settings.insert("editorMode".to_string(), f.value.clone());
        }

        // env
        if let Some(f) = ir.fields.get("settings.env") {
            settings.insert("env".to_string(), f.value.clone());
        }

        // attribution.commit
        if let Some(f) = ir.fields.get("settings.attribution.commit") {
            if let Some(s) = f.value.as_str() {
                let mut attr = serde_json::Map::new();
                attr.insert("commit".to_string(), Value::String(s.to_string()));
                settings.insert("attribution".to_string(), Value::Object(attr));
            }
        }

        // sandbox.network.allowAllUnixSockets
        if let Some(f) = ir
            .fields
            .get("settings.sandbox.network.allowAllUnixSockets")
        {
            if let Some(b) = f.value.as_bool() {
                let sandbox = settings
                    .entry("sandbox".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Value::Object(s_obj) = sandbox {
                    let network = s_obj
                        .entry("network".to_string())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                    if let Value::Object(n_obj) = network {
                        n_obj.insert("allowAllUnixSockets".to_string(), Value::Bool(b));
                    }
                }
            }
        }

        // autoMemoryEnabled
        if let Some(f) = ir.fields.get("settings.autoMemoryEnabled") {
            settings.insert("autoMemoryEnabled".to_string(), f.value.clone());
        }

        // cleanupPeriodDays
        if let Some(f) = ir.fields.get("settings.cleanupPeriodDays") {
            settings.insert("cleanupPeriodDays".to_string(), f.value.clone());
        }

        // permissions.defaultMode: written from the internal IR field produced by lift_x2c
        // when approval_policy / sandbox_mode are present in the Codex config.
        if let Some(f) = ir.fields.get("__permissions.defaultMode") {
            if let Some(mode_str) = f.value.as_str() {
                let perms = settings
                    .entry("permissions".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Value::Object(perms_obj) = perms {
                    perms_obj.insert(
                        "defaultMode".to_string(),
                        Value::String(mode_str.to_string()),
                    );
                }
            }
        }

        let json_content = serde_json::to_string_pretty(&Value::Object(settings))
            .with_context(|| "Failed to serialize settings.json")?;

        if json_content.trim() != "{}" {
            files.push(EmitFile {
                path: format!("{}/.claude/settings.json", out_root),
                content: json_content,
            });
        }

        // developer_instructions → CLAUDE.md (degrade: scope fixed to project)
        if let Some(f) = ir.fields.get("settings.codex.developer_instructions") {
            if let Some(text) = f.value.as_str() {
                let claude_md_content = format!(
                    "# Developer Instructions\n\n\
                     <!-- Converted from Codex developer_instructions (lossy: scope fixed to project) -->\n\n\
                     {}\n",
                    text
                );
                files.push(EmitFile {
                    path: format!("{}/.claude/CLAUDE.md", out_root),
                    content: claude_md_content,
                });
                // The warning diagnostic is already emitted during lift_x2c.
                // Emitting it again here would cause duplicate entries in the report.
            }
        }

        // Warn about remainder
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: None,
            message: "config.toml → settings.json: partial conversion only. \
                      hooks, mcp_servers, plugins, and many Codex-specific fields require manual conversion."
                .to_string(),
        });

        Ok(EmitPlan { files, diagnostics })
    }
}
