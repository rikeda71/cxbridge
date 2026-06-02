use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{DiagLevel, Diagnostic, IRNode, Loss};
use crate::handlers::{EmitFile, EmitPlan, LowerOpts, Scope};

use super::toml_convert::build_hooks_toml;
use super::{HooksHandler, COMMON_EVENTS};

impl HooksHandler {
    /// c2x: IR → Codex TOML hooks.
    pub(super) fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut diagnostics = Vec::new();
        let out_root = opts.out.as_deref().unwrap_or(".");

        // #16430 only affects plugin-bundled hooks (Codex does not load them).
        // A standalone hooks.json / settings.json conversion is unaffected, so the
        // warning must not fire there.
        if ir.source_path.contains(".claude-plugin") {
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("hooks.plugin_bundled".to_string()),
                message: "Warning (#16430): Plugin-bundled hooks are not loaded by Codex. \
                          Use --hooks-target=user|project to write hooks to ~/.codex/hooks.json \
                          or .codex/config.toml [hooks] instead."
                    .to_string(),
            });
        }

        // Collect common event hooks
        let mut hooks_entries: Vec<(String, Value)> = Vec::new();
        for (id, field) in &ir.fields {
            if field.loss == Loss::Dropped {
                continue;
            }
            if let Some(event_name) = id.strip_prefix("hooks.event.") {
                if COMMON_EVENTS.contains(&event_name) {
                    hooks_entries.push((event_name.to_string(), field.value.clone()));
                }
            }
        }

        let files = match opts.hooks_target {
            Scope::User => {
                // Write to ~/.codex/hooks.json (JSON format)
                let mut hooks_json = serde_json::Map::new();
                for (event_name, entries) in &hooks_entries {
                    hooks_json.insert(event_name.clone(), entries.clone());
                }
                let content = serde_json::to_string_pretty(&Value::Object(hooks_json))
                    .with_context(|| "failed to serialize hooks")?;
                vec![EmitFile {
                    path: format!("{}/hooks.json", out_root),
                    content,
                }]
            }
            Scope::Project => {
                // Write to .codex/config.toml [hooks] section (TOML format)
                let toml_str = build_hooks_toml(&hooks_entries)?;
                vec![EmitFile {
                    path: format!("{}/.codex/config.toml", out_root),
                    content: toml_str,
                }]
            }
        };

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: IR → Claude JSON hooks.
    pub(super) fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let diagnostics = Vec::new();
        let out_root = opts.out.as_deref().unwrap_or(".");

        let mut hooks_obj = serde_json::Map::new();
        for (id, field) in &ir.fields {
            if field.loss == Loss::Dropped {
                continue;
            }
            if let Some(event_name) = id.strip_prefix("hooks.event.") {
                if COMMON_EVENTS.contains(&event_name) {
                    hooks_obj.insert(event_name.to_string(), field.value.clone());
                }
            }
        }

        let hooks_wrapper = serde_json::json!({ "hooks": hooks_obj });
        let content = serde_json::to_string_pretty(&hooks_wrapper)
            .with_context(|| "failed to serialize hooks")?;

        let files = vec![EmitFile {
            path: format!("{}/hooks.json", out_root),
            content,
        }];

        Ok(EmitPlan { files, diagnostics })
    }
}
