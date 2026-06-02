use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{
    new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::{
    applies_direction, index_by_claude_field, index_by_codex_field, DomainMap,
};
use crate::core::transforms::{
    apply_transforms, claude_tier, tier_to_codex, ConvDir, TransformCtx,
};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// Handler for the subagents domain.
pub struct SubagentHandler {
    pub map: DomainMap,
}

impl Handler for SubagentHandler {
    fn kind(&self) -> Kind {
        Kind::Subagent
    }

    fn detect(&self, path: &Path) -> bool {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let path_str = path.to_str().unwrap_or("");

        // c2x: .claude/agents/<n>.md
        if file_name.ends_with(".md") {
            let parent = path.parent().and_then(|p| p.to_str()).unwrap_or("");
            if parent.ends_with("agents")
                || parent.contains("/agents")
                || parent.contains("\\agents")
            {
                return true;
            }
        }

        // x2c: .codex/agents/<n>.toml (not config.toml)
        if file_name.ends_with(".toml")
            && file_name != "config.toml"
            && (path_str.contains(".codex/agents/") || path_str.contains(".codex\\agents\\"))
        {
            return true;
        }

        false
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let path_str = path.to_str().unwrap_or("");

        if file_name.ends_with(".toml") && file_name != "config.toml" {
            // x2c: Codex TOML agent file
            parse_codex_agent_toml(path)
        } else if file_name.ends_with(".md")
            && (path_str.contains("/agents/") || path_str.contains("\\agents\\"))
        {
            // c2x: Claude agent Markdown file
            crate::core::serialize::frontmatter::parse_frontmatter_file(path)
        } else {
            anyhow::bail!(
                "SubagentHandler: unrecognized file format for {}",
                path.display()
            )
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Subagent, source_tool, &source_path);

        let idx = match dir {
            ConvDir::C2x => index_by_claude_field(&self.map),
            ConvDir::X2c => index_by_codex_field(&self.map),
        };

        let frontmatter = match parsed["frontmatter"].as_object() {
            Some(fm) => fm,
            None => {
                // no frontmatter — still lift the body
                let body_raw = parsed["body"].as_str().unwrap_or("").to_string();
                node.body = Some(crate::core::ir::BodySegment {
                    raw: body_raw,
                    findings: vec![],
                });
                return Ok(node);
            }
        };

        for (key, value) in frontmatter {
            let Some(entry) = idx.get(key.as_str()) else {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: None,
                    message: format!("unknown frontmatter key: {key}"),
                });
                continue;
            };

            if !applies_direction(entry, dir) {
                continue;
            }

            let ctx = TransformCtx {
                direction: dir,
                args: None,
                field: entry,
            };
            let (v, applied) = apply_transforms(value, entry.transform.as_deref(), &ctx);

            let loss = Loss::from(&entry.loss);

            let degrade_info = entry.degrade.as_ref().map(|d| DegradeInfo {
                to: d.to.clone(),
                target: d.target.clone(),
            });

            let dropped_info = if matches!(loss, Loss::Dropped) {
                Some(DroppedInfo {
                    reason: entry
                        .notes
                        .clone()
                        .unwrap_or_else(|| format!("{key} has no equivalent")),
                })
            } else {
                None
            };

            let warning = if entry.warn == Some(true) {
                Some(format!(
                    "{}: {}",
                    entry.id,
                    entry.notes.as_deref().unwrap_or("warn")
                ))
            } else {
                None
            };

            node.fields.insert(
                entry.id.clone(),
                IRField {
                    id: entry.id.clone(),
                    value: v,
                    loss,
                    transforms_applied: applied,
                    degrade: degrade_info,
                    warning: warning.clone(),
                    dropped: dropped_info,
                },
            );
        }

        // body: for c2x, the Markdown body is the system prompt content
        let body_raw = parsed["body"].as_str().unwrap_or("").to_string();
        node.body = Some(crate::core::ir::BodySegment {
            raw: body_raw,
            findings: vec![],
        });

        // Only relevant when converting Claude → Codex: Claude auto-delegates via
        // description match, but Codex requires explicit spawn_agent calls.
        if matches!(dir, ConvDir::C2x) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("subagents.spawn-model".to_string()),
                message: "Claude auto-delegates via description match. \
                          Codex requires explicit spawn_agent call (multi_agent=true). \
                          Add spawn instructions to developer_instructions."
                    .to_string(),
            });
        }

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

impl SubagentHandler {
    /// c2x: .claude/agents/<n>.md → .codex/agents/<n>.toml
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");
        let agent_name = extract_agent_name_from_path(&ir.source_path);

        // Build TOML content
        let mut toml_lines: Vec<String> = Vec::new();

        // name
        let name_val = ir
            .fields
            .get("subagents.name")
            .and_then(|f| f.value.as_str())
            .unwrap_or(&agent_name);
        toml_lines.push(format!(r#"name = "{}""#, escape_toml_string(name_val)));

        // description
        if let Some(f) = ir.fields.get("subagents.description") {
            if let Some(desc) = f.value.as_str() {
                if !desc.is_empty() {
                    toml_lines.push(format!(r#"description = "{}""#, escape_toml_string(desc)));
                }
            }
        }

        // developer_instructions: from body (system prompt / Markdown body)
        let body = ir.body.as_ref().map(|b| b.raw.as_str()).unwrap_or("");

        // Also check initialPrompt field (subagents.initialPrompt → append to instructions)
        let initial_prompt = ir
            .fields
            .get("subagents.initialPrompt")
            .and_then(|f| f.value.as_str())
            .unwrap_or("");

        let instructions = if !initial_prompt.is_empty() && !body.is_empty() {
            format!("{}\n\n{}", body.trim(), initial_prompt.trim())
        } else if !initial_prompt.is_empty() {
            initial_prompt.to_string()
        } else {
            body.to_string()
        };

        if !instructions.trim().is_empty() {
            // Use TOML multi-line basic string (triple-quoted)
            toml_lines.push(format!(
                "developer_instructions = '''\n{}\n'''",
                instructions.trim()
            ));
        }

        // model: tier mapping (lossy)
        if let Some(f) = ir.fields.get("subagents.model") {
            if let Some(model_str) = f.value.as_str() {
                if !model_str.is_empty() && model_str != "inherit" {
                    let codex_model = if let Some(tier) = claude_tier(model_str) {
                        tier_to_codex(tier).to_string()
                    } else {
                        // unknown model: use as-is with warn
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("subagents.model".to_string()),
                            message: format!(
                                "Unknown model '{}': using as-is (lossy; no tier mapping)",
                                model_str
                            ),
                        });
                        model_str.to_string()
                    };
                    toml_lines.push(format!(r#"model = "{}""#, escape_toml_string(&codex_model)));
                }
                // inherit → omit field
            }
        }

        // effort → model_reasoning_effort (enum_map already applied in lift)
        if let Some(f) = ir.fields.get("subagents.effort") {
            if let Some(effort_str) = f.value.as_str() {
                if !effort_str.is_empty() {
                    toml_lines.push(format!(
                        r#"model_reasoning_effort = "{}""#,
                        escape_toml_string(effort_str)
                    ));
                }
            }
        }

        // tools → sandbox_mode (approximate; lossy)
        if let Some(f) = ir.fields.get("subagents.tools") {
            let tools = crate::handlers::json_to_string_list(&f.value);
            let sandbox_mode = approximate_sandbox_mode(&tools);
            if let Some(mode) = sandbox_mode {
                toml_lines.push(format!(r#"sandbox_mode = "{}""#, mode));
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("subagents.tools".to_string()),
                    message: format!(
                        "tools list approximated as sandbox_mode=\"{}\" (lossy; individual tool permissions not supported in Codex)",
                        mode
                    ),
                });
            }
        }

        // permissionMode → sandbox_mode (enum_map applied in lift → value already mapped)
        // Only values that survive the enum_map to a valid Codex sandbox_mode are emitted.
        // acceptEdits/auto/dontAsk have no Codex equivalent and must be dropped.
        const VALID_SANDBOX_MODES: &[&str] =
            &["read-only", "workspace-write", "danger-full-access"];
        if !ir.fields.contains_key("subagents.tools") {
            if let Some(f) = ir.fields.get("subagents.permissionMode") {
                if let Some(mode_str) = f.value.as_str() {
                    if !mode_str.is_empty() {
                        if VALID_SANDBOX_MODES.contains(&mode_str) {
                            toml_lines.push(format!(
                                r#"sandbox_mode = "{}""#,
                                escape_toml_string(mode_str)
                            ));
                        } else {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Drop,
                                id: Some("subagents.permissionMode".to_string()),
                                message: format!(
                                    "permissionMode=\"{}\" has no Codex sandbox_mode equivalent and was dropped",
                                    mode_str
                                ),
                            });
                        }
                    }
                }
            }
        }

        // skills → skills.config (lossy)
        if let Some(f) = ir.fields.get("subagents.skills") {
            let skills = crate::handlers::json_to_string_list(&f.value);
            if !skills.is_empty() {
                // Codex skills.config is an array of objects with {enabled, path}
                // We approximate by just writing enabled=true entries
                let entries: Vec<String> = skills
                    .iter()
                    .map(|s| {
                        format!(
                            r#"{{ enabled = true, path = "{}" }}"#,
                            escape_toml_string(s)
                        )
                    })
                    .collect();
                toml_lines.push(format!("skills = [{}]", entries.join(", ")));
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("subagents.skills".to_string()),
                    message: "skills list converted to Codex skills.config format (lossy; content injection differs from Claude)".to_string(),
                });
            }
        }

        // mcpServers → mcp_servers (rename + format)
        if let Some(f) = ir.fields.get("subagents.mcpServers") {
            // Write inline TOML table for mcp_servers
            if let Value::Object(servers) = &f.value {
                for (server_name, server_config) in servers {
                    // Each server is a TOML table
                    toml_lines.push(format!("\n[mcp_servers.{}]", server_name));
                    if let Value::Object(cfg) = server_config {
                        for (k, v) in cfg {
                            toml_lines.push(format!("{} = {}", k, toml_value_string(v)));
                        }
                    }
                }
            }
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("subagents.mcpServers".to_string()),
                message:
                    "mcpServers renamed to mcp_servers (lossy: inline/reference form differences)"
                        .to_string(),
            });
        }

        let toml_content = toml_lines.join("\n") + "\n";
        let agent_toml_path = format!("{}/.codex/agents/{}.toml", out_root, agent_name);
        // Relative path used in config_file pointer (spec §10.2).
        let agent_toml_rel = format!(".codex/agents/{}.toml", agent_name);

        files.push(EmitFile {
            path: agent_toml_path,
            content: toml_content,
        });

        // config.toml patch: [agents.<name>] config_file pointer + [features] multi_agent=true.
        // write_plan performs a non-destructive toml_edit merge so existing keys are preserved.
        let config_toml_path = format!("{}/config.toml", out_root);
        let config_toml_content = format!(
            "[agents.{}]\nconfig_file = \"{}\"\n\n[features]\nmulti_agent = true\n",
            agent_name, agent_toml_rel
        );
        files.push(EmitFile {
            path: config_toml_path,
            content: config_toml_content,
        });

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: .codex/agents/<n>.toml → .claude/agents/<n>.md
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");
        let agent_name = extract_agent_name_from_path(&ir.source_path);

        let mut fm = serde_json::Map::new();

        // name
        if let Some(f) = ir.fields.get("subagents.name") {
            if let Some(s) = f.value.as_str() {
                fm.insert("name".to_string(), Value::String(s.to_string()));
            }
        }

        // description
        if let Some(f) = ir.fields.get("subagents.description") {
            if let Some(s) = f.value.as_str() {
                if !s.is_empty() {
                    fm.insert("description".to_string(), Value::String(s.to_string()));
                }
            }
        }

        // model: tier mapping (lossy)
        if let Some(f) = ir.fields.get("subagents.model") {
            if let Some(model_str) = f.value.as_str() {
                if !model_str.is_empty() {
                    let claude_model = if let Some(tier) =
                        crate::core::transforms::codex_tier(model_str)
                    {
                        crate::core::transforms::tier_to_claude(tier).to_string()
                    } else {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("subagents.model".to_string()),
                            message: format!("Unknown Codex model '{}': using as-is", model_str),
                        });
                        model_str.to_string()
                    };
                    fm.insert("model".to_string(), Value::String(claude_model));
                }
            }
        }

        // effort / model_reasoning_effort
        if let Some(f) = ir.fields.get("subagents.effort") {
            if let Some(s) = f.value.as_str() {
                if !s.is_empty() {
                    fm.insert("effort".to_string(), Value::String(s.to_string()));
                }
            }
        }

        // skills (subagents.skills): Codex skills.config → Claude skills list
        if let Some(f) = ir.fields.get("subagents.skills") {
            let skills: Vec<Value> = if let Value::Array(arr) = &f.value {
                arr.iter()
                    .filter_map(|item| {
                        item.get("path")
                            .and_then(|p| p.as_str())
                            .map(|s| Value::String(s.to_string()))
                    })
                    .collect()
            } else {
                vec![]
            };
            if !skills.is_empty() {
                fm.insert("skills".to_string(), Value::Array(skills));
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("subagents.skills".to_string()),
                    message: "Codex skills.config lifted to Claude skills list (lossy: content injection semantics differ)".to_string(),
                });
            }
        }

        // body: developer_instructions → Markdown body
        let body = ir.body.as_ref().map(|b| b.raw.as_str()).unwrap_or("");
        // Also check developer_instructions from fields (in case lifted differently)
        let dev_instructions = ir
            .fields
            .get("subagents.body")
            .and_then(|f| f.value.as_str())
            .unwrap_or("");
        let effective_body = if !dev_instructions.is_empty() {
            dev_instructions.to_string()
        } else {
            body.to_string()
        };

        // Serialize frontmatter as YAML
        let fm_yaml = if fm.is_empty() {
            String::new()
        } else {
            let yaml_val = Value::Object(fm);
            serde_saphyr::to_string(&yaml_val)
                .with_context(|| "Failed to serialize frontmatter as YAML")?
        };

        let agent_md_content = if fm_yaml.is_empty() {
            effective_body.clone()
        } else {
            format!("---\n{}---\n{}", fm_yaml, effective_body)
        };

        let agent_md_path = format!("{}/.claude/agents/{}.md", out_root, agent_name);

        files.push(EmitFile {
            path: agent_md_path,
            content: agent_md_content,
        });

        Ok(EmitPlan { files, diagnostics })
    }
}

/// Parse a Codex agent TOML file (.codex/agents/<n>.toml) into the handler-internal Value format.
fn parse_codex_agent_toml(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read agent TOML: {}", path.display()))?;

    let toml_val: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse agent TOML: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Convert toml::Value to serde_json::Value for the frontmatter
    let json_val = crate::core::serialize::toml_to_json(&toml_val)?;

    // The TOML agent file has a flat structure; all top-level keys go into "frontmatter"
    // and developer_instructions becomes the "body"
    let mut frontmatter = serde_json::Map::new();
    let mut body = String::new();

    if let Value::Object(map) = &json_val {
        for (k, v) in map {
            if k == "developer_instructions" {
                body = v.as_str().unwrap_or("").to_string();
            } else if let Value::Object(nested) = v {
                // Flatten one level of nested tables using dot notation so that
                // codex.field paths like "skills.config" resolve correctly.
                for (sk, sv) in nested {
                    frontmatter.insert(format!("{}.{}", k, sk), sv.clone());
                }
            } else {
                frontmatter.insert(k.clone(), v.clone());
            }
        }
    }

    Ok(serde_json::json!({
        "frontmatter": Value::Object(frontmatter),
        "body": body,
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// Extract agent name from path.
/// .claude/agents/<name>.md → <name>
/// .codex/agents/<name>.toml → <name>
fn extract_agent_name_from_path(source_path: &str) -> String {
    let path = Path::new(source_path);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem != "config" && !stem.is_empty() {
            return stem.to_string();
        }
    }
    "agent".to_string()
}

/// Escape a string for TOML double-quoted string.
fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Convert a serde_json::Value to an inline TOML value string.
fn toml_value_string(v: &Value) -> String {
    match v {
        Value::String(s) => format!(r#""{}""#, escape_toml_string(s)),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(toml_value_string).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("{} = {}", k, toml_value_string(val)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// Approximate tools list as a Codex sandbox_mode.
/// Read-only tools → "read-only"
/// Write tools → "workspace-write"
/// All/none → None (inherit from parent)
fn approximate_sandbox_mode(tools: &[String]) -> Option<&'static str> {
    if tools.is_empty() {
        return None;
    }
    let has_write = tools.iter().any(|t| {
        let t = t.to_lowercase();
        t.starts_with("write") || t.starts_with("edit") || t.starts_with("bash")
    });
    let has_read = tools.iter().any(|t| {
        let t = t.to_lowercase();
        t.starts_with("read")
    });
    if has_write {
        Some("workspace-write")
    } else if has_read {
        Some("read-only")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use tempfile::TempDir;

    fn make_handler() -> SubagentHandler {
        let maps = load_mappings(Path::new("mappings"));
        SubagentHandler {
            map: maps["subagents"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    #[test]
    fn test_subagent_detect_claude_md() {
        let h = make_handler();
        // .claude/agents/my-agent.md
        assert!(h.detect(Path::new(".claude/agents/my-agent.md")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new("CLAUDE.md")));
    }

    #[test]
    fn test_subagent_detect_codex_toml() {
        let h = make_handler();
        assert!(h.detect(Path::new(".codex/agents/my-agent.toml")));
        assert!(!h.detect(Path::new("config.toml")));
        assert!(!h.detect(Path::new(".codex/config.toml")));
    }

    #[test]
    fn test_subagent_c2x_basic_roundtrip() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("researcher.md");
        fs::write(
            &agent_path,
            "---\nname: researcher\ndescription: Research tasks\n---\n\nYou are a research agent.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Subagent);
        assert!(ir.fields.contains_key("subagents.name"));
        assert!(ir.fields.contains_key("subagents.description"));
        let name_f = &ir.fields["subagents.name"];
        assert_eq!(name_f.value, Value::String("researcher".to_string()));
        assert_eq!(name_f.loss, Loss::Lossless);

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // .codex/agents/researcher.toml should be generated
        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("researcher.toml"));
        assert!(
            agent_toml.is_some(),
            "Expected researcher.toml in output, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        let content = &agent_toml.unwrap().content;
        assert!(content.contains("researcher"), "name should be in TOML");
        assert!(
            content.contains("Research tasks"),
            "description should be in TOML"
        );
        assert!(
            content.contains("research agent"),
            "body should be in developer_instructions"
        );
    }

    #[test]
    fn test_subagent_c2x_model_effort() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("heavy.md");
        fs::write(
            &agent_path,
            "---\nname: heavy\ndescription: Heavy processing\nmodel: claude-opus-4-8\neffort: max\n---\n\nDo heavy work.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(ir.fields.contains_key("subagents.model"));
        assert!(ir.fields.contains_key("subagents.effort"));

        // model should be lossy (different providers)
        let model_f = &ir.fields["subagents.model"];
        assert_eq!(model_f.loss, Loss::Lossy);

        // effort: max → xhigh via enum_map
        let effort_f = &ir.fields["subagents.effort"];
        assert_eq!(effort_f.value, Value::String("xhigh".to_string()));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("heavy.toml"))
            .unwrap();
        assert!(
            agent_toml.content.contains("model_reasoning_effort"),
            "Expected model_reasoning_effort in TOML"
        );
        assert!(
            agent_toml.content.contains("xhigh"),
            "Expected xhigh in TOML"
        );
    }

    #[test]
    fn test_subagent_c2x_dropped_fields() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("bg.md");
        fs::write(
            &agent_path,
            "---\nname: bg\ndescription: Background agent\nmaxTurns: 10\nbackground: true\nisolation: worktree\ncolor: blue\n---\n\nBackground work.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // maxTurns, background, isolation, color → dropped
        let max_turns = ir.fields.get("subagents.maxTurns").unwrap();
        assert_eq!(max_turns.loss, Loss::Dropped);

        let background = ir.fields.get("subagents.background").unwrap();
        assert_eq!(background.loss, Loss::Dropped);

        let isolation = ir.fields.get("subagents.isolation").unwrap();
        assert_eq!(isolation.loss, Loss::Dropped);

        let color = ir.fields.get("subagents.color").unwrap();
        assert_eq!(color.loss, Loss::Dropped);
    }

    #[test]
    fn test_subagent_x2c_basic_roundtrip() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".codex").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("coder.toml");
        fs::write(
            &agent_path,
            r#"name = "coder"
description = "Code writing agent"
developer_instructions = '''
You are a coding assistant.
'''
"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        assert_eq!(ir.kind, Kind::Subagent);
        assert!(ir.fields.contains_key("subagents.name"));
        assert_eq!(
            ir.fields["subagents.name"].value,
            Value::String("coder".to_string())
        );

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let agent_md = plan.files.iter().find(|f| f.path.ends_with("coder.md"));
        assert!(
            agent_md.is_some(),
            "Expected coder.md in output, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        let content = &agent_md.unwrap().content;
        assert!(content.contains("coder"), "name should be in frontmatter");
        assert!(
            content.contains("Code writing agent"),
            "description should be in frontmatter"
        );
        assert!(
            content.contains("coding assistant"),
            "developer_instructions should be in body"
        );
    }

    #[test]
    fn test_subagent_c2x_emits_config_toml_agents_and_features() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("researcher.md");
        fs::write(
            &agent_path,
            "---\nname: researcher\ndescription: Research tasks\nmodel: claude-opus-4-8\neffort: max\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // agent TOML must be present
        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("researcher.toml"));
        assert!(
            agent_toml.is_some(),
            "Expected researcher.toml, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        // config.toml must be present with [agents.researcher] and multi_agent
        let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
        assert!(
            config_toml.is_some(),
            "Expected config.toml, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        let content = &config_toml.unwrap().content;
        assert!(
            content.contains("[agents.researcher]"),
            "Expected [agents.researcher] in config.toml, got:\n{}",
            content
        );
        assert!(
            content.contains("config_file"),
            "Expected config_file in config.toml, got:\n{}",
            content
        );
        assert!(
            content.contains("multi_agent"),
            "Expected multi_agent in config.toml, got:\n{}",
            content
        );
        assert!(
            content.contains("true"),
            "Expected multi_agent = true in config.toml, got:\n{}",
            content
        );
    }

    #[test]
    fn test_subagent_c2x_report_enumerates_dropped() {
        use crate::core::report::build_report;
        use crate::handlers::EmitPlan;

        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("complex.md");
        fs::write(
            &agent_path,
            "---\nname: complex\ndescription: Complex agent\nmaxTurns: 5\nbackground: true\nisolation: worktree\ncolor: red\n---\n\nDo complex tasks.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let empty_plan = EmitPlan {
            files: vec![],
            diagnostics: vec![],
        };
        let report = build_report(&ir, &empty_plan);

        // Dropped fields should be enumerated in the report
        assert!(
            !report.dropped.is_empty(),
            "Expected dropped entries in report"
        );
        let dropped_ids: Vec<_> = report
            .dropped
            .iter()
            .filter_map(|d| d.id.as_deref())
            .collect();
        assert!(
            dropped_ids.contains(&"subagents.maxTurns"),
            "Expected subagents.maxTurns in dropped, got: {:?}",
            dropped_ids
        );
        assert!(
            dropped_ids.contains(&"subagents.background"),
            "Expected subagents.background in dropped, got: {:?}",
            dropped_ids
        );
    }

    /// permissionMode values with no Codex equivalent (acceptEdits, auto, dontAsk)
    /// must not produce sandbox_mode in the output TOML, and a Drop diagnostic
    /// must appear in plan.diagnostics.
    #[test]
    fn test_c2x_permission_mode_unmapped_values_dropped() {
        let h = make_handler();

        for (perm_mode, label) in [
            ("acceptEdits", "acceptEdits"),
            ("auto", "auto"),
            ("dontAsk", "dontAsk"),
        ] {
            let dir = TempDir::new().unwrap();
            let agents_dir = dir.path().join(".claude").join("agents");
            fs::create_dir_all(&agents_dir).unwrap();

            let agent_path = agents_dir.join("t.md");
            fs::write(
                &agent_path,
                format!(
                    "---\nname: t\ndescription: D\npermissionMode: {}\n---\nBody.\n",
                    perm_mode
                ),
            )
            .unwrap();

            let out_dir = TempDir::new().unwrap();
            let parsed = h.parse(&agent_path).unwrap();
            let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
            let opts = default_opts(out_dir.path().to_str().unwrap());
            let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

            let agent_toml = plan
                .files
                .iter()
                .find(|f| f.path.ends_with("t.toml"))
                .unwrap_or_else(|| {
                    panic!(
                        "Expected t.toml in output for permissionMode={}, got: {:?}",
                        label,
                        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                    )
                });

            assert!(
                !agent_toml.content.contains("sandbox_mode"),
                "sandbox_mode must not appear in TOML for permissionMode={}, got:\n{}",
                label,
                agent_toml.content
            );

            let has_drop = plan.diagnostics.iter().any(|d| {
                d.id.as_deref() == Some("subagents.permissionMode") && d.level == DiagLevel::Drop
            });
            assert!(
                has_drop,
                "Expected Drop diagnostic for subagents.permissionMode (permissionMode={}), got: {:?}",
                label,
                plan.diagnostics
                    .iter()
                    .map(|d| (d.id.as_deref(), &d.level))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Valid permissionMode values (default, bypassPermissions, plan) that map
    /// to a Codex sandbox_mode must still produce sandbox_mode in the TOML output.
    #[test]
    fn test_c2x_permission_mode_valid_values_emitted() {
        let h = make_handler();

        for (perm_mode, expected_sandbox, label) in [
            (
                "bypassPermissions",
                "danger-full-access",
                "bypassPermissions",
            ),
            ("plan", "read-only", "plan"),
            ("default", "workspace-write", "default"),
        ] {
            let dir = TempDir::new().unwrap();
            let agents_dir = dir.path().join(".claude").join("agents");
            fs::create_dir_all(&agents_dir).unwrap();

            let agent_path = agents_dir.join("t.md");
            fs::write(
                &agent_path,
                format!(
                    "---\nname: t\ndescription: D\npermissionMode: {}\n---\nBody.\n",
                    perm_mode
                ),
            )
            .unwrap();

            let out_dir = TempDir::new().unwrap();
            let parsed = h.parse(&agent_path).unwrap();
            let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
            let opts = default_opts(out_dir.path().to_str().unwrap());
            let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

            let agent_toml = plan
                .files
                .iter()
                .find(|f| f.path.ends_with("t.toml"))
                .unwrap_or_else(|| {
                    panic!(
                        "Expected t.toml in output for permissionMode={}, got: {:?}",
                        label,
                        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                    )
                });

            assert!(
                agent_toml
                    .content
                    .contains(&format!("sandbox_mode = \"{}\"", expected_sandbox)),
                "Expected sandbox_mode=\"{}\" for permissionMode={}, got:\n{}",
                expected_sandbox,
                label,
                agent_toml.content
            );

            let has_drop = plan.diagnostics.iter().any(|d| {
                d.id.as_deref() == Some("subagents.permissionMode") && d.level == DiagLevel::Drop
            });
            assert!(
                !has_drop,
                "Must not have Drop diagnostic for valid permissionMode={}, got: {:?}",
                label,
                plan.diagnostics
                    .iter()
                    .map(|d| (d.id.as_deref(), &d.level))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// x2c: Codex TOML with [skills]\nconfig = [{enabled=true, path="python"}] must lift
    /// subagents.skills into the IR (not dropped as unknown key) and lower it to
    /// a `skills:` list in the Claude agent frontmatter.
    #[test]
    fn test_subagent_x2c_skills_lifted() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".codex").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("coder.toml");
        fs::write(
            &agent_path,
            "name = \"coder\"\ndescription = \"D\"\ndeveloper_instructions = \"Body\"\n\n[skills]\nconfig = [{enabled = true, path = \"python\"}]\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        // The IR must have subagents.skills — it must NOT be dropped as unknown
        assert!(
            ir.fields.contains_key("subagents.skills"),
            "IR must contain subagents.skills; got fields: {:?}",
            ir.fields.keys().collect::<Vec<_>>()
        );

        // No drop diagnostic for "skills"
        let has_unknown_skills_drop = ir
            .diagnostics
            .iter()
            .any(|d| d.level == DiagLevel::Drop && d.message.contains("skills"));
        assert!(
            !has_unknown_skills_drop,
            "Must not have Drop diagnostic for skills; diagnostics: {:?}",
            ir.diagnostics
        );

        // lower → Claude .md should contain skills: [python]
        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let agent_md = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("coder.md"))
            .unwrap();
        assert!(
            agent_md.content.contains("python"),
            "Output .md must contain 'python' in skills list; got:\n{}",
            agent_md.content
        );
        assert!(
            agent_md.content.contains("skills"),
            "Output .md must contain 'skills' frontmatter key; got:\n{}",
            agent_md.content
        );

        // A Warn diagnostic for the lossy mapping must be emitted
        let has_skills_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("subagents.skills") && d.level == DiagLevel::Warn);
        assert!(
            has_skills_warn,
            "Expected subagents.skills Warn diagnostic; got: {:?}",
            plan.diagnostics
        );
    }

    /// gap 37/42: fields with loss:dropped + warn:true must appear in report.dropped
    /// exactly once and must NOT appear in report.lossy at all.
    ///
    /// The four subagents fields disallowedTools, maxTurns, background, and
    /// isolation are all loss:dropped + warn:true. Each must be counted once in
    /// dropped[] only — never in lossy[] and never duplicated.
    ///
    /// This is a full-pipeline test: lift → lower (obtaining a real plan with its
    /// diagnostics) → build_report. That ensures no duplication from any of the
    /// three diagnostic sources (IRField loop, ir.diagnostics loop,
    /// plan.diagnostics loop).
    #[test]
    fn test_subagent_c2x_dropped_warn_fields_not_in_lossy_not_duplicated() {
        use crate::core::report::build_report;

        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("full.md");
        fs::write(
            &agent_path,
            "---\nname: full\ndescription: Full agent\nmaxTurns: 5\nbackground: true\nisolation: worktree\ndisallowedTools:\n  - Bash\n---\n\nFull agent body.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let report = build_report(&ir, &plan);

        // Each loss:dropped + warn:true field must appear exactly once in dropped[].
        for field_id in &[
            "subagents.maxTurns",
            "subagents.background",
            "subagents.isolation",
            "subagents.disallowedTools",
        ] {
            let dropped_count = report
                .dropped
                .iter()
                .filter(|e| e.id.as_deref() == Some(field_id))
                .count();
            assert_eq!(
                dropped_count, 1,
                "{field_id} must appear exactly once in report.dropped, found {dropped_count} times. \
                 Full dropped: {:?}",
                report
                    .dropped
                    .iter()
                    .map(|e| e.id.as_deref().unwrap_or("<none>"))
                    .collect::<Vec<_>>()
            );

            // Must NOT appear in lossy[].
            let in_lossy = report
                .lossy
                .iter()
                .any(|e| e.id.as_deref() == Some(field_id));
            assert!(
                !in_lossy,
                "{field_id} must NOT appear in report.lossy. \
                 Full lossy: {:?}",
                report
                    .lossy
                    .iter()
                    .map(|e| e.id.as_deref().unwrap_or("<none>"))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// c2x regression: skills: [python] in Claude .md must still convert to
    /// skills = [...] in Codex TOML (regression guard for the c2x direction).
    #[test]
    fn test_subagent_c2x_skills_roundtrip() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("dev.md");
        fs::write(
            &agent_path,
            "---\nname: dev\ndescription: D\nskills:\n  - python\n  - javascript\n---\nBody.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(
            ir.fields.contains_key("subagents.skills"),
            "IR must contain subagents.skills"
        );
        assert_eq!(
            ir.fields["subagents.skills"].value,
            Value::Array(vec![
                Value::String("python".to_string()),
                Value::String("javascript".to_string()),
            ])
        );

        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("dev.toml"))
            .unwrap();
        assert!(
            agent_toml.content.contains("python"),
            "Codex TOML must contain python skill; got:\n{}",
            agent_toml.content
        );
        assert!(
            agent_toml.content.contains("javascript"),
            "Codex TOML must contain javascript skill; got:\n{}",
            agent_toml.content
        );
        assert!(
            agent_toml.content.contains("enabled"),
            "Codex TOML skills must have enabled field; got:\n{}",
            agent_toml.content
        );
    }
}
