use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{DiagLevel, Diagnostic, IRNode, SideArtifact};
use crate::core::mappings::applies_direction;
use crate::core::transforms::ConvDir;
use crate::degrade::rules::degrade_allowed_tools;
use crate::degrade::subagent::{decide_skill_target, degrade_to_subagent, SkillTarget};
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};
use crate::scanner::body_rewrite::rewrite_body;

use super::aux_files::{collect_aux_files, extract_skill_name};
use super::SkillsHandler;

impl SkillsHandler {
    pub(super) fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        let mut side_artifacts: Vec<SideArtifact> = Vec::new();

        // Extract skill name from source_path
        let skill_name = extract_skill_name(&ir.source_path);
        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build frontmatter
        let mut fm = serde_json::Map::new();

        // name
        if let Some(f) = ir.fields.get("skills.name") {
            fm.insert("name".to_string(), f.value.clone());
        }

        // description: concatenate skills.description + skills.when_to_use
        let desc = ir
            .fields
            .get("skills.description")
            .and_then(|f| f.value.as_str())
            .unwrap_or("");
        let when_to_use = ir
            .fields
            .get("skills.when_to_use")
            .and_then(|f| f.value.as_str())
            .unwrap_or("");
        let combined_desc = if when_to_use.is_empty() {
            desc.to_string()
        } else if desc.is_empty() {
            when_to_use.to_string()
        } else {
            format!("{}\n\n{}", desc, when_to_use)
        };
        if !combined_desc.is_empty() {
            fm.insert("description".to_string(), Value::String(combined_desc));
            if !when_to_use.is_empty() {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("skills.when_to_use".to_string()),
                    message: "when_to_use concatenated into description (lossy)".to_string(),
                });
            }
        }

        // determine skill target
        let target = decide_skill_target(ir, opts);

        // allowed-tools → degrade
        if let Some(f) = ir.fields.get("skills.allowed-tools") {
            let tools = crate::handlers::json_to_string_list(&f.value);
            let (arts, diags) = degrade_allowed_tools(&skill_name, &tools, true, opts.scope);
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // disallowed-tools → degrade
        if let Some(f) = ir.fields.get("skills.disallowed-tools") {
            let tools = crate::handlers::json_to_string_list(&f.value);
            let (arts, diags) = degrade_allowed_tools(&skill_name, &tools, false, opts.scope);
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // hooks → degrade
        if let Some(f) = ir.fields.get("skills.hooks") {
            let (arts, diags) = crate::degrade::hooks_scope::degrade_skill_hooks(
                &skill_name,
                &f.value,
                &opts.hooks_target,
            );
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // disable-model-invocation → SideArtifact: agents/openai.yaml
        // polarity:invert was applied in lift:
        //   disable-model-invocation=true  (Claude) → IR holds false → allow_implicit_invocation: false
        //   disable-model-invocation=false (Claude) → IR holds true  → allow_implicit_invocation: true
        if let Some(f) = ir.fields.get("skills.disable-model-invocation") {
            let (allow_val, source_val) = if f.value == Value::Bool(false) {
                ("false", "true")
            } else {
                ("true", "false")
            };
            let content = format!("policy:\n  allow_implicit_invocation: {}\n", allow_val);
            side_artifacts.push(SideArtifact {
                path: format!(".agents/skills/{}/agents/openai.yaml", skill_name),
                content,
                note: format!(
                    "disable-model-invocation={} → policy.allow_implicit_invocation: {}",
                    source_val, allow_val
                ),
            });
        }

        // model/effort/context:fork → subagent degrade
        let has_model = ir.fields.contains_key("skills.model");
        let has_effort = ir.fields.contains_key("skills.effort");
        let has_fork = ir.fields.contains_key("skills.context-fork");

        if matches!(target, SkillTarget::Subagent) && (has_model || has_effort || has_fork) {
            let trigger_id = if has_model {
                "skills.model"
            } else if has_effort {
                "skills.effort"
            } else {
                "skills.context-fork"
            };
            let (arts, diags) = degrade_to_subagent(&skill_name, ir, trigger_id);
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // paths/user-invocable/arguments/argument-hint → dropped (already handled in lift)
        // shell: powershell → propose only (warn)
        if let Some(f) = ir.fields.get("skills.shell") {
            if f.value.as_str() == Some("powershell") {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("skills.shell".to_string()),
                    message: "shell: powershell – propose mapping to hooks.commandWindows (manual action required)".to_string(),
                });
            }
        }

        // Body
        let body_out = compute_body_out(ir, opts);

        // When requested, retain the original Claude-specific frontmatter keys so
        // that Codex can ignore them via fail-open while they remain readable.
        // Values are taken from raw_frontmatter (pre-transform) to avoid writing
        // semantically wrong data — e.g. a polarity-inverted boolean for
        // disable-model-invocation.
        if opts.keep_claude_frontmatter {
            for entry in &self.map.entries {
                // Only entries whose Claude side has a real (non-pseudo) field name
                let claude_field = match entry
                    .claude
                    .as_ref()
                    .and_then(|c| c.field.as_deref())
                    .filter(|f| !f.starts_with('\u{FF08}'))
                {
                    Some(f) => f,
                    None => continue,
                };

                // Skip the two standard Codex fields already inserted above
                if claude_field == "name" || claude_field == "description" {
                    continue;
                }

                // Only insert if the IR carries this field (field was present in source)
                if ir.fields.contains_key(&entry.id) {
                    // Use the original pre-transform value from raw_frontmatter so
                    // transformed fields (e.g. polarity-inverted booleans) are not
                    // written back with the wrong polarity.
                    if let Some(raw_val) = ir
                        .raw_frontmatter
                        .as_ref()
                        .and_then(|fm_raw| fm_raw.get(claude_field))
                    {
                        fm.entry(claude_field.to_string())
                            .or_insert_with(|| raw_val.clone());
                    }
                }
            }
        }

        // Output SKILL.md
        let skill_md_path = format!("{}/.agents/skills/{}/SKILL.md", out_root, skill_name);

        // frontmatter → YAML string
        let fm_yaml = if fm.is_empty() {
            String::new()
        } else {
            let yaml_val = Value::Object(fm);
            serde_saphyr::to_string(&yaml_val)
                .with_context(|| "Failed to serialize frontmatter as YAML")?
        };

        let skill_md_content = if fm_yaml.is_empty() {
            body_out.clone()
        } else {
            format!("---\n{}---\n{}", fm_yaml, body_out)
        };

        files.push(EmitFile {
            path: skill_md_path,
            content: skill_md_content,
        });

        // Non-.md auxiliary files → path-remap only, content unchanged.
        let skill_dir = Path::new(&ir.source_path).parent();
        if let Some(dir) = skill_dir {
            if dir.is_dir() {
                let out_skill_dir = format!("{}/.agents/skills/{}", out_root, skill_name);
                let aux = collect_aux_files(dir, &out_skill_dir).with_context(|| {
                    format!("Failed to collect aux files from {}", dir.display())
                })?;
                files.extend(aux);
            }
        }

        // SideArtifacts → EmitFiles.
        // Absolute artifact paths (user-scope) are used as-is; relative paths
        // are resolved under the output root.
        for art in &side_artifacts {
            let emit_path = if art.path.starts_with('/') {
                art.path.clone()
            } else {
                format!("{}/{}", out_root, art.path)
            };
            files.push(EmitFile {
                path: emit_path,
                content: art.content.clone(),
            });
        }

        Ok(EmitPlan { files, diagnostics })
    }

    pub(super) fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let diagnostics = Vec::new();

        let skill_name = extract_skill_name(&ir.source_path);
        let out_root = opts.out.as_deref().unwrap_or(".");

        let mut fm = serde_json::Map::new();

        // Convert Codex fields to Claude fields
        for (key, value) in &ir.fields {
            // key is entry.id; find the Codex field name
            let Some(entry) = self.map.entries.iter().find(|e| e.id == *key) else {
                continue;
            };
            if !applies_direction(entry, ConvDir::X2c) {
                continue;
            }
            // Retrieve the Claude field name
            let claude_field = entry
                .claude
                .as_ref()
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());
            let Some(cf) = claude_field else {
                continue;
            };
            // pseudo field skips
            if cf.starts_with('\u{FF08}') {
                continue;
            }
            fm.insert(cf.to_string(), value.value.clone());
        }

        // Body
        let body_out = compute_body_out(ir, opts);

        // interface.default_prompt → prepend to body (lossy approximate)
        let body_out = if let Some(dp_field) = ir.fields.get("skills.openai-yaml.default_prompt") {
            if let Some(prompt) = dp_field.value.as_str() {
                if !prompt.is_empty() {
                    format!("{}\n\n{}", prompt, body_out)
                } else {
                    body_out
                }
            } else {
                body_out
            }
        } else {
            body_out
        };

        let skill_md_path = format!("{}/.claude/skills/{}/SKILL.md", out_root, skill_name);

        let fm_yaml = if fm.is_empty() {
            String::new()
        } else {
            let yaml_val = Value::Object(fm);
            serde_saphyr::to_string(&yaml_val)
                .with_context(|| "Failed to serialize frontmatter as YAML")?
        };

        let skill_md_content = if fm_yaml.is_empty() {
            body_out
        } else {
            format!("---\n{}---\n{}", fm_yaml, body_out)
        };

        files.push(EmitFile {
            path: skill_md_path,
            content: skill_md_content,
        });

        // Non-.md auxiliary files → path-remap only, content unchanged.
        // agents/openai.yaml is excluded (already lifted separately).
        let skill_dir = Path::new(&ir.source_path).parent();
        if let Some(dir) = skill_dir {
            if dir.is_dir() {
                let out_skill_dir = format!("{}/.claude/skills/{}", out_root, skill_name);
                let aux = collect_aux_files(dir, &out_skill_dir).with_context(|| {
                    format!("Failed to collect aux files from {}", dir.display())
                })?;
                files.extend(aux);
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }
}

/// Compute the output body text for a skill, optionally rewriting syntax.
pub(super) fn compute_body_out(ir: &IRNode, opts: &LowerOpts) -> String {
    let body_raw = ir.body.as_ref().map(|b| b.raw.as_str()).unwrap_or("");
    if opts.rewrite_body {
        if let Some(body_seg) = &ir.body {
            rewrite_body(body_raw, &body_seg.findings)
        } else {
            body_raw.to_string()
        }
    } else {
        body_raw.to_string()
    }
}
