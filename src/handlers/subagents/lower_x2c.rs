use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{DiagLevel, Diagnostic, IRNode};
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};

use super::parse::extract_agent_name_from_path;

/// x2c: .codex/agents/<n>.toml → .claude/agents/<n>.md
pub(crate) fn lower_x2c(ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
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
                let claude_model =
                    if let Some(tier) = crate::core::model_tiers::codex_tier(model_str) {
                        crate::core::model_tiers::tier_to_claude(tier).to_string()
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
