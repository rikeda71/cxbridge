use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{
    new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::{
    applies_direction, index_by_claude_field, index_by_codex_field, DomainMap, LossSpec,
};
use crate::core::transforms::{
    apply_transforms, claude_tier, tier_to_codex, ConvDir, TransformCtx,
};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// subagents ドメインのハンドラ。
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

        if file_name.ends_with(".toml") && !file_name.ends_with("config.toml") {
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

            let loss = match entry.loss {
                LossSpec::Lossless => Loss::Lossless,
                LossSpec::Lossy => Loss::Lossy,
                LossSpec::Dropped => Loss::Dropped,
            };

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

            let is_dropped = matches!(loss, Loss::Dropped);

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

            if entry.warn == Some(true) {
                if let Some(msg) = &warning {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some(entry.id.clone()),
                        message: msg.clone(),
                    });
                }
            }

            if is_dropped {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(entry.id.clone()),
                    message: format!("{} dropped", entry.id),
                });
            }
        }

        // body: for c2x, the Markdown body is the system prompt content
        let body_raw = parsed["body"].as_str().unwrap_or("").to_string();
        node.body = Some(crate::core::ir::BodySegment {
            raw: body_raw,
            findings: vec![],
        });

        // Note about auto-delegation vs spawn_agent difference
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("subagents.spawn-model".to_string()),
            message: "Claude auto-delegates via description match. \
                      Codex requires explicit spawn_agent call (multi_agent=true). \
                      Add spawn instructions to developer_instructions."
                .to_string(),
        });

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
        let mut diagnostics = ir.diagnostics.clone();

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
            let tools = json_to_string_list(&f.value);
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
        // Only emit if tools wasn't already set (avoid duplication)
        if !ir.fields.contains_key("subagents.tools") {
            if let Some(f) = ir.fields.get("subagents.permissionMode") {
                if let Some(mode_str) = f.value.as_str() {
                    if !mode_str.is_empty() {
                        toml_lines.push(format!(
                            r#"sandbox_mode = "{}""#,
                            escape_toml_string(mode_str)
                        ));
                    }
                }
            }
        }

        // skills → skills.config (lossy)
        if let Some(f) = ir.fields.get("subagents.skills") {
            let skills = json_to_string_list(&f.value);
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

        files.push(EmitFile {
            path: agent_toml_path,
            content: toml_content,
        });

        // Note about spawn_agent requirement
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("subagents.spawn-model".to_string()),
            message: format!(
                "Subagent '{}' requires explicit spawn_agent call in Codex (auto-delegation not available). \
                 Add spawn instructions to AGENTS.md or calling agent's developer_instructions.",
                agent_name
            ),
        });

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: .codex/agents/<n>.toml → .claude/agents/<n>.md
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = ir.diagnostics.clone();

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

    let toml_val: toml::Value = content
        .parse()
        .with_context(|| format!("Failed to parse agent TOML: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Convert toml::Value to serde_json::Value for the frontmatter
    let json_val = toml_value_to_json(&toml_val)?;

    // The TOML agent file has a flat structure; all top-level keys go into "frontmatter"
    // and developer_instructions becomes the "body"
    let mut frontmatter = serde_json::Map::new();
    let mut body = String::new();

    if let Value::Object(map) = &json_val {
        for (k, v) in map {
            if k == "developer_instructions" {
                body = v.as_str().unwrap_or("").to_string();
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

/// Convert toml::Value to serde_json::Value.
fn toml_value_to_json(v: &toml::Value) -> anyhow::Result<Value> {
    match v {
        toml::Value::String(s) => Ok(Value::String(s.clone())),
        toml::Value::Integer(i) => Ok(Value::Number(serde_json::Number::from(*i))),
        toml::Value::Float(f) => Ok(Value::Number(
            serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)),
        )),
        toml::Value::Boolean(b) => Ok(Value::Bool(*b)),
        toml::Value::Array(arr) => {
            let items: anyhow::Result<Vec<Value>> = arr.iter().map(toml_value_to_json).collect();
            Ok(Value::Array(items?))
        }
        toml::Value::Table(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, val) in tbl {
                map.insert(k.clone(), toml_value_to_json(val)?);
            }
            Ok(Value::Object(map))
        }
        toml::Value::Datetime(dt) => Ok(Value::String(dt.to_string())),
    }
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

/// Convert a JSON Value to a list of strings.
fn json_to_string_list(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
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
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
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

        // dropped フィールドが report に列挙されていること
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
}
