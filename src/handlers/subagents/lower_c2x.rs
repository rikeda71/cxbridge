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
