use serde_json::Value;

use crate::core::ir::{DiagLevel, Diagnostic, IRNode};
use crate::core::model_tiers::{claude_tier, tier_to_codex};
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};

use super::parse::extract_agent_name_from_path;

/// c2x: .claude/agents/<n>.md → .codex/agents/<n>.toml
pub(crate) fn lower_c2x(ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
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
        toml_lines.push(format!(
            "developer_instructions = {}",
            crate::handlers::toml_multiline_basic(instructions.trim())
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
    const VALID_SANDBOX_MODES: &[&str] = &["read-only", "workspace-write", "danger-full-access"];
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
            message: "mcpServers renamed to mcp_servers (lossy: inline/reference form differences)"
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
    use crate::core::ir::{new_node, BodySegment, IRField, Kind, Loss, Tool};
    use crate::handlers::{Scope, SkillTargetMode};

    fn make_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            only: vec![],
            scope: Scope::Project,
            dual_manifest: false,
            hooks_target: Scope::User,
            skill_target: SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    fn make_string_field(id: &str, val: &str, loss: Loss) -> IRField {
        IRField {
            id: id.to_string(),
            value: Value::String(val.to_string()),
            loss,
            transforms_applied: vec![],
            degrade: None,
            warning: None,
            dropped: None,
        }
    }

    fn base_node(name: &str) -> IRNode {
        let mut node = new_node(
            Kind::Subagent,
            Tool::Claude,
            &format!(".claude/agents/{}.md", name),
        );
        node.fields.insert(
            "subagents.name".to_string(),
            make_string_field("subagents.name", name, Loss::Lossless),
        );
        node.body = Some(BodySegment {
            raw: String::new(),
            findings: vec![],
        });
        node
    }

    // --- model tier mapping ---

    #[test]
    fn known_opus_model_maps_to_gpt55() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.model".to_string(),
            make_string_field("subagents.model", "claude-opus-4-8", Loss::Lossy),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let opts = make_opts(dir.path().to_str().unwrap());
        let plan = lower_c2x(&ir, &opts).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("gpt-5.5"),
            "opus tier must map to gpt-5.5; got:\n{}",
            toml.content
        );
        // No warn diagnostic for known model
        assert!(
            !plan
                .diagnostics
                .iter()
                .any(|d| d.id.as_deref() == Some("subagents.model")),
            "Known model must not produce a diagnostic; got: {:?}",
            plan.diagnostics
        );
    }

    #[test]
    fn known_sonnet_model_maps_to_gpt54() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.model".to_string(),
            make_string_field("subagents.model", "claude-sonnet-4-6", Loss::Lossy),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("gpt-5.4"),
            "sonnet tier must map to gpt-5.4; got:\n{}",
            toml.content
        );
    }

    #[test]
    fn known_haiku_model_maps_to_gpt54_mini() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.model".to_string(),
            make_string_field("subagents.model", "claude-haiku-4-5", Loss::Lossy),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("gpt-5.4-mini"),
            "haiku tier must map to gpt-5.4-mini; got:\n{}",
            toml.content
        );
    }

    #[test]
    fn unknown_model_passes_through_with_warn_diagnostic() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.model".to_string(),
            make_string_field("subagents.model", "my-custom-model-v9", Loss::Lossy),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        // Value preserved verbatim
        assert!(
            toml.content.contains("my-custom-model-v9"),
            "Unknown model must be emitted as-is; got:\n{}",
            toml.content
        );
        // Warn diagnostic must be emitted
        let warn = plan
            .diagnostics
            .iter()
            .find(|d| d.id.as_deref() == Some("subagents.model") && d.level == DiagLevel::Warn);
        assert!(
            warn.is_some(),
            "Expected Warn diagnostic for unknown model; got: {:?}",
            plan.diagnostics
        );
        assert!(
            warn.unwrap().message.contains("my-custom-model-v9"),
            "Warn message must name the unknown model; got: {}",
            warn.unwrap().message
        );
    }

    #[test]
    fn model_inherit_omits_model_field() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.model".to_string(),
            make_string_field("subagents.model", "inherit", Loss::Lossless),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            !toml.content.contains("model ="),
            "model=inherit must not emit a model field; got:\n{}",
            toml.content
        );
    }

    // --- effort mapping ---

    #[test]
    fn effort_field_emitted_as_model_reasoning_effort() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.effort".to_string(),
            make_string_field("subagents.effort", "xhigh", Loss::Lossy),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("model_reasoning_effort = \"xhigh\""),
            "effort must be emitted as model_reasoning_effort; got:\n{}",
            toml.content
        );
    }

    #[test]
    fn empty_effort_field_omitted() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.effort".to_string(),
            make_string_field("subagents.effort", "", Loss::Lossless),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            !toml.content.contains("model_reasoning_effort"),
            "Empty effort must not emit model_reasoning_effort; got:\n{}",
            toml.content
        );
    }

    // --- initialPrompt appended to developer_instructions ---

    #[test]
    fn initial_prompt_appended_when_body_present() {
        let mut ir = base_node("agent");
        ir.body = Some(BodySegment {
            raw: "System instructions here.".to_string(),
            findings: vec![],
        });
        ir.fields.insert(
            "subagents.initialPrompt".to_string(),
            make_string_field(
                "subagents.initialPrompt",
                "Initial user prompt.",
                Loss::Lossy,
            ),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        // Both body and initialPrompt must appear in developer_instructions
        assert!(
            toml.content.contains("System instructions here."),
            "body must appear in developer_instructions; got:\n{}",
            toml.content
        );
        assert!(
            toml.content.contains("Initial user prompt."),
            "initialPrompt must be appended to developer_instructions; got:\n{}",
            toml.content
        );
        // Body comes first, then initial prompt (separated by blank line)
        let body_pos = toml.content.find("System instructions").unwrap();
        let prompt_pos = toml.content.find("Initial user prompt").unwrap();
        assert!(
            body_pos < prompt_pos,
            "body must precede initialPrompt in developer_instructions"
        );
    }

    #[test]
    fn initial_prompt_only_when_no_body() {
        let mut ir = base_node("agent");
        ir.body = Some(BodySegment {
            raw: String::new(),
            findings: vec![],
        });
        ir.fields.insert(
            "subagents.initialPrompt".to_string(),
            make_string_field("subagents.initialPrompt", "Only prompt.", Loss::Lossy),
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("Only prompt."),
            "initialPrompt must appear as developer_instructions when body is empty; got:\n{}",
            toml.content
        );
    }

    // --- sandbox_mode approximation from tools list ---

    #[test]
    fn tools_with_write_prefix_produces_workspace_write() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.tools".to_string(),
            IRField {
                id: "subagents.tools".to_string(),
                value: Value::Array(vec![
                    Value::String("ReadFile".to_string()),
                    Value::String("WriteFile".to_string()),
                ]),
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("sandbox_mode = \"workspace-write\""),
            "Write tool must approximate to workspace-write; got:\n{}",
            toml.content
        );
        let has_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("subagents.tools") && d.level == DiagLevel::Warn);
        assert!(
            has_warn,
            "tools → sandbox_mode approximation must emit a Warn; got: {:?}",
            plan.diagnostics
        );
    }

    #[test]
    fn tools_with_only_read_prefix_produces_read_only() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.tools".to_string(),
            IRField {
                id: "subagents.tools".to_string(),
                value: Value::Array(vec![Value::String("ReadFile".to_string())]),
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("sandbox_mode = \"read-only\""),
            "Read-only tool must approximate to read-only; got:\n{}",
            toml.content
        );
    }

    #[test]
    fn tools_with_no_read_or_write_prefix_omits_sandbox_mode() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.tools".to_string(),
            IRField {
                id: "subagents.tools".to_string(),
                value: Value::Array(vec![Value::String("ListDirectory".to_string())]),
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            !toml.content.contains("sandbox_mode"),
            "Unclassified tools must not emit sandbox_mode; got:\n{}",
            toml.content
        );
    }

    #[test]
    fn bash_prefix_tool_produces_workspace_write() {
        let mut ir = base_node("agent");
        ir.fields.insert(
            "subagents.tools".to_string(),
            IRField {
                id: "subagents.tools".to_string(),
                value: Value::Array(vec![Value::String("Bash".to_string())]),
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agent.toml"))
            .unwrap();
        assert!(
            toml.content.contains("sandbox_mode = \"workspace-write\""),
            "Bash tool must produce workspace-write; got:\n{}",
            toml.content
        );
    }

    // --- output shape ---

    #[test]
    fn output_includes_agent_toml_and_config_toml() {
        let ir = base_node("myagent");
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        assert!(
            plan.files.iter().any(|f| f.path.ends_with("myagent.toml")),
            "EmitPlan must include agent TOML; got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert!(
            plan.files.iter().any(|f| f.path.ends_with("config.toml")),
            "EmitPlan must include config.toml; got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn config_toml_contains_agents_section_and_multi_agent() {
        let ir = base_node("myagent");
        let dir = tempfile::TempDir::new().unwrap();
        let plan = lower_c2x(&ir, &make_opts(dir.path().to_str().unwrap())).unwrap();

        let config = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            config.content.contains("[agents.myagent]"),
            "config.toml must contain [agents.myagent]; got:\n{}",
            config.content
        );
        assert!(
            config.content.contains("config_file"),
            "config.toml must contain config_file pointer; got:\n{}",
            config.content
        );
        assert!(
            config.content.contains("multi_agent = true"),
            "config.toml must contain multi_agent = true; got:\n{}",
            config.content
        );
    }

    // --- approximate_sandbox_mode unit tests ---

    #[test]
    fn approximate_sandbox_mode_empty_returns_none() {
        assert_eq!(approximate_sandbox_mode(&[]), None);
    }

    #[test]
    fn approximate_sandbox_mode_edit_tool_is_workspace_write() {
        let tools = vec!["EditFile".to_string()];
        assert_eq!(approximate_sandbox_mode(&tools), Some("workspace-write"));
    }

    #[test]
    fn approximate_sandbox_mode_unrecognized_tool_returns_none() {
        let tools = vec!["ListDir".to_string(), "GlobSearch".to_string()];
        assert_eq!(approximate_sandbox_mode(&tools), None);
    }
}
