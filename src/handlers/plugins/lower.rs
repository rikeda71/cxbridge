use serde_json::{Map, Value};

use anyhow::Context;

use crate::core::ir::{DiagLevel, Diagnostic, IRNode, Kind, Loss};
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

use super::marketplace::{complete_semver, transform_marketplace_c2x, transform_marketplace_x2c};
use super::PluginsHandler;

impl PluginsHandler {
    /// c2x: Claude plugin → Codex plugin conversion
    pub(super) fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build the manifest JSON
        let codex_manifest = self.build_codex_manifest(ir, &mut diagnostics);

        // --dual-manifest: retain .claude-plugin/ while also generating .codex-plugin/
        if opts.dual_manifest {
            // Retain the Claude-side manifest by re-reading and emitting the original file
            if let Ok(content) = std::fs::read_to_string(&ir.source_path) {
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/plugin.json", out_root),
                    content,
                });
            }
        }

        // Generate the Codex-side manifest
        let codex_json = serde_json::to_string_pretty(&codex_manifest)
            .with_context(|| "Failed to serialize Codex plugin.json")?;
        files.push(EmitFile {
            path: format!("{}/.codex-plugin/plugin.json", out_root),
            content: codex_json,
        });

        // Merge EmitPlans from child nodes
        lower_children(self, ir, ConvDir::C2x, opts, &mut files, &mut diagnostics);

        // Convert marketplace.json
        for artifact in &ir.side_artifacts {
            if artifact.note == "marketplace.json" {
                let transformed = transform_marketplace_c2x(&artifact.content, &mut diagnostics);
                files.push(EmitFile {
                    path: format!("{}/.agents/plugins/marketplace.json", out_root),
                    content: transformed,
                });
            }
        }

        // Emit commands/ and agents/ files (path-remap to output plugin root)
        for artifact in &ir.side_artifacts {
            if artifact.note == "commands" || artifact.note == "agents" {
                files.push(EmitFile {
                    path: format!("{}/.codex-plugin/{}", out_root, artifact.path),
                    content: artifact.content.clone(),
                });
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: Codex plugin → Claude plugin conversion
    pub(super) fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build the manifest JSON
        let claude_manifest = self.build_claude_manifest(ir, &mut diagnostics);

        // Generate the Claude-side manifest
        let claude_json = serde_json::to_string_pretty(&claude_manifest)
            .with_context(|| "Failed to serialize Claude plugin.json")?;
        files.push(EmitFile {
            path: format!("{}/.claude-plugin/plugin.json", out_root),
            content: claude_json,
        });

        // Merge EmitPlans from child nodes
        lower_children(self, ir, ConvDir::X2c, opts, &mut files, &mut diagnostics);

        // Convert marketplace.json
        for artifact in &ir.side_artifacts {
            if artifact.note == "marketplace.json" {
                let transformed = transform_marketplace_x2c(&artifact.content, &mut diagnostics);
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/marketplace.json", out_root),
                    content: transformed,
                });
            }
        }

        // Emit commands/ and agents/ files (path-remap to output plugin root)
        for artifact in &ir.side_artifacts {
            if artifact.note == "commands" || artifact.note == "agents" {
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/{}", out_root, artifact.path),
                    content: artifact.content.clone(),
                });
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// Builds a Codex-target plugin.json from the IR (c2x).
    pub(super) fn build_codex_manifest(
        &self,
        ir: &IRNode,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Value {
        let mut manifest = Map::new();

        // Convert IR fields to Codex fields
        for (id, field) in &ir.fields {
            // Dropped fields are already represented by IRField.loss == Dropped and
            // recorded via IRField.dropped.  build_report reads those directly, so
            // pushing an additional Diagnostic here would cause each dropped field
            // to appear multiple times in the report summary.
            if matches!(field.loss, Loss::Dropped) {
                continue;
            }

            // Retrieve the Codex field name from the entry
            let codex_field = self
                .map
                .entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.codex.as_ref())
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());

            let Some(cf) = codex_field else {
                continue;
            };

            insert_possibly_nested(&mut manifest, cf, field.value.clone());
        }

        // Fill in semver "0.0.0" if version is missing
        if !manifest.contains_key("version") {
            manifest.insert("version".to_string(), Value::String("0.0.0".to_string()));
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.version".to_string()),
                message: "version field missing: auto-completed as '0.0.0' (Codex requires strict semver)".to_string(),
            });
        } else if let Some(ver) = manifest.get("version").and_then(|v| v.as_str()) {
            // semver completion: complete to major.minor.patch if not already in that form
            let completed = complete_semver(ver);
            if completed != ver {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.version".to_string()),
                    message: format!(
                        "version '{}' completed to semver '{}' (Codex requires strict semver)",
                        ver, completed
                    ),
                });
                manifest.insert("version".to_string(), Value::String(completed));
            }
        }

        // Fill in description from name if missing
        if !manifest.contains_key("description") {
            if let Some(name) = manifest.get("name").and_then(|v| v.as_str()) {
                manifest.insert(
                    "description".to_string(),
                    Value::String(format!("Plugin: {}", name)),
                );
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.description".to_string()),
                    message: "description field missing: auto-filled from name (Codex requires description)".to_string(),
                });
            }
        }

        Value::Object(manifest)
    }

    /// Builds a Claude-target plugin.json from the IR (x2c).
    pub(super) fn build_claude_manifest(
        &self,
        ir: &IRNode,
        _diagnostics: &mut Vec<Diagnostic>,
    ) -> Value {
        let mut manifest = Map::new();

        for (id, field) in &ir.fields {
            // Dropped fields are already captured by IRField.loss == Dropped;
            // no additional Diagnostic needed here.
            if matches!(field.loss, Loss::Dropped) {
                continue;
            }

            // Retrieve the Claude field name from the entry
            let claude_field = self
                .map
                .entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.claude.as_ref())
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());

            let Some(cf) = claude_field else {
                continue;
            };

            insert_possibly_nested(&mut manifest, cf, field.value.clone());
        }

        Value::Object(manifest)
    }
}

/// Inserts `value` at `field_name` in `manifest`, expanding a single dot into a
/// nested object (e.g. `"interface.displayName"` → `manifest["interface"]["displayName"]`).
fn insert_possibly_nested(manifest: &mut Map<String, Value>, field_name: &str, value: Value) {
    if let Some(dot_pos) = field_name.find('.') {
        let parent = &field_name[..dot_pos];
        let child_key = &field_name[dot_pos + 1..];
        let parent_obj = manifest
            .entry(parent.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(obj) = parent_obj.as_object_mut() {
            obj.insert(child_key.to_string(), value);
        }
    } else {
        manifest.insert(field_name.to_string(), value);
    }
}

/// Dispatches lower() for each child IR node (skills, hooks, mcp) and collects results.
fn lower_children(
    handler: &PluginsHandler,
    ir: &IRNode,
    dir: ConvDir,
    opts: &LowerOpts,
    files: &mut Vec<EmitFile>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for child_ir in &ir.children {
        match child_ir.kind {
            Kind::Skill => {
                let skill_handler = crate::handlers::skills::SkillsHandler {
                    map: handler.maps["skills"].clone(),
                };
                match skill_handler.lower(child_ir, dir, opts) {
                    Ok(plan) => {
                        files.extend(plan.files);
                        diagnostics.extend(plan.diagnostics);
                    }
                    Err(e) => {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: None,
                            message: format!("Failed to lower skill child: {}", e),
                        });
                    }
                }
            }
            Kind::Hooks => {
                let hooks_handler = crate::handlers::hooks::HooksHandler {
                    map: handler.maps["hooks"].clone(),
                };
                match hooks_handler.lower(child_ir, dir, opts) {
                    Ok(plan) => {
                        files.extend(plan.files);
                        diagnostics.extend(plan.diagnostics);
                    }
                    Err(e) => {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: None,
                            message: format!("Failed to lower hooks child: {}", e),
                        });
                    }
                }
            }
            Kind::Mcp => {
                let mcp_handler = crate::handlers::mcp::McpHandler {
                    map: handler.maps["mcp"].clone(),
                };
                match mcp_handler.lower(child_ir, dir, opts) {
                    Ok(plan) => {
                        files.extend(plan.files);
                        diagnostics.extend(plan.diagnostics);
                    }
                    Err(e) => {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: None,
                            message: format!("Failed to lower MCP child: {}", e),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}
