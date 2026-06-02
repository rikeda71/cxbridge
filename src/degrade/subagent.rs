use crate::core::ir::{DiagLevel, Diagnostic, IRNode, SideArtifact};
use crate::core::model_tiers::{claude_tier, tier_to_codex};
use crate::handlers::{LowerOpts, SkillTargetMode};

/// Whether a skill is emitted as a Codex skill file or as a subagent.
///
/// Avoids a direct dependency on `cli.rs`; accessed through `LowerOpts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTarget {
    Skill,
    Subagent,
}

/// Determines whether an IR node should be emitted as a skill or subagent.
///
/// Decision priority:
/// 1. Explicit option — if `LowerOpts.skill_target` is not `Auto`, obey it.
/// 2. Deterministic cases:
///    - `skill.model` / `skill.effort` / `skill.context==fork` present → `Subagent`
///    - No permissions (pure instruction) → `Skill`
/// 3. Ambiguous case (permissions present, unclear if session scope is acceptable):
///    - Interactive mode → prompt the user via TTY
///    - Non-interactive → conservative default (`Subagent`); reason is recorded in the report
pub fn decide_skill_target(ir: &IRNode, opts: &LowerOpts) -> SkillTarget {
    // Explicit option takes precedence over all heuristics.
    match opts.skill_target {
        SkillTargetMode::Skill => return SkillTarget::Skill,
        SkillTargetMode::Subagent => return SkillTarget::Subagent,
        SkillTargetMode::Auto => {}
    }

    let has_model = ir.fields.contains_key("skills.model");
    let has_effort = ir.fields.contains_key("skills.effort");
    let has_fork = ir
        .fields
        .get("skills.context-fork")
        .is_some_and(|f| f.value == serde_json::Value::String("fork".into()));

    if has_model || has_effort || has_fork {
        return SkillTarget::Subagent;
    }

    let has_perms = ir.fields.contains_key("skills.allowed-tools")
        || ir.fields.contains_key("skills.disallowed-tools");
    if !has_perms {
        // Pure instruction with no tool constraints fits natively as a skill.
        return SkillTarget::Skill;
    }

    // Ambiguous: has permissions but no model/effort/fork signal.
    if opts.interactive {
        ask_user_skill_target(ir)
    } else {
        // Conservative default: keep permissions by bundling into a subagent.
        SkillTarget::Subagent
    }
}

/// Prompts the user via TTY to choose a conversion target (uses dialoguer).
fn ask_user_skill_target(ir: &IRNode) -> SkillTarget {
    use dialoguer::Select;

    let skill_name = ir.source_path.as_str();
    let items = &[
        "skill (permissions degrade to session scope; auto-trigger preserved)",
        "subagent (permissions bundled into subagent; explicit invocation required)",
    ];

    let selection = Select::new()
        .with_prompt(format!(
            "skill '{}' has allowed-tools. Choose a conversion target",
            skill_name
        ))
        .items(items)
        .default(1) // conservative default: subagent
        .interact();

    match selection {
        Ok(0) => SkillTarget::Skill,
        _ => SkillTarget::Subagent,
    }
}

/// Generates `.codex/agents/<skill>.toml` and appends the required `[agents.*]` /
/// `[features].multi_agent` entries to `config.toml`.
///
/// `trigger_id` is the mapping entry id of the field that caused the degrade
/// (e.g. `"skills.model"`, `"skills.effort"`, `"skills.context-fork"`).
/// A Diagnostic carrying that id is always emitted to record that the skill's
/// auto-fork behaviour changes to an explicit `spawn_agent` call in Codex.
pub fn degrade_to_subagent(
    skill_name: &str,
    ir: &IRNode,
    trigger_id: &str,
) -> (Vec<SideArtifact>, Vec<Diagnostic>) {
    let mut artifacts = Vec::new();
    let mut diagnostics = Vec::new();

    let body = ir.body.as_ref().map(|b| b.raw.as_str()).unwrap_or("");

    let description = ir
        .fields
        .get("skills.description")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");

    let model_str = ir
        .fields
        .get("skills.model")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");
    let codex_model = if model_str.is_empty() {
        tier_to_codex(crate::core::model_tiers::Tier::Mid).to_string()
    } else if let Some(tier) = claude_tier(model_str) {
        tier_to_codex(tier).to_string()
    } else {
        // Unknown model string: pass through as-is and warn so the user can verify.
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("skills.model".to_string()),
            message: format!(
                "Unknown model '{}': using as-is in subagent TOML",
                model_str
            ),
        });
        model_str.to_string()
    };

    let effort_str = ir
        .fields
        .get("skills.effort")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");
    let reasoning_effort = match effort_str {
        "max" => "xhigh",
        "xhigh" => "xhigh",
        "high" => "high",
        "medium" => "medium",
        "low" => "low",
        "" => "",
        _ => effort_str,
    };

    let agents_toml_path = format!(".codex/agents/{}.toml", skill_name);
    let mut toml_lines = vec![
        format!(r#"name = "{}""#, skill_name),
        format!(r#"description = "{}""#, description.replace('"', r#"\""#)),
    ];

    if !body.is_empty() {
        toml_lines.push(format!(
            "developer_instructions = {}",
            crate::handlers::toml_multiline_basic(body)
        ));
    }

    if !codex_model.is_empty() {
        toml_lines.push(format!(r#"model = "{}""#, codex_model));
    }

    if !reasoning_effort.is_empty() {
        toml_lines.push(format!(
            r#"model_reasoning_effort = "{}""#,
            reasoning_effort
        ));
    }

    artifacts.push(SideArtifact {
        path: agents_toml_path.clone(),
        content: toml_lines.join("\n") + "\n",
        note: format!("skill '{}' degraded to subagent", skill_name),
    });

    let config_update = format!(
        "[agents.{}]\nconfig_file = \"{}\"\n\n[features]\nmulti_agent = true\n",
        skill_name, agents_toml_path
    );
    artifacts.push(SideArtifact {
        path: "config.toml".to_string(),
        content: config_update,
        note: format!(
            "[agents.{}] and [features].multi_agent=true added",
            skill_name
        ),
    });

    diagnostics.push(Diagnostic {
        level: DiagLevel::Warn,
        id: Some(trigger_id.to_string()),
        message: format!(
            "skill '{}' degraded to subagent (.codex/agents/{}.toml). \
             Auto-fork is replaced by an explicit spawn_agent call. \
             features.multi_agent=true must also be set.",
            skill_name, skill_name
        ),
    });

    (artifacts, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{new_node, IRField, Kind, Loss, Tool};
    use crate::handlers::Scope;

    fn opts(mode: SkillTargetMode, interactive: bool) -> LowerOpts {
        LowerOpts {
            out: None,
            only: vec![],
            scope: Scope::Project,
            dual_manifest: false,
            hooks_target: Scope::User,
            skill_target: mode,
            interactive,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    fn node_with(id: &str, value: serde_json::Value) -> IRNode {
        let mut n = new_node(Kind::Skill, Tool::Claude, "demo");
        n.fields.insert(
            id.to_string(),
            IRField {
                id: id.to_string(),
                value,
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );
        n
    }

    #[test]
    fn test_decide_skill_target_perms_only_auto_is_subagent() {
        // Branch (c): permissions present, no model/effort/fork, non-interactive.
        let n = node_with("skills.allowed-tools", serde_json::json!(["Bash(git*)"]));
        assert_eq!(
            decide_skill_target(&n, &opts(SkillTargetMode::Auto, false)),
            SkillTarget::Subagent
        );
    }

    #[test]
    fn test_decide_skill_target_pure_instruction_is_skill() {
        let n = new_node(Kind::Skill, Tool::Claude, "demo");
        assert_eq!(
            decide_skill_target(&n, &opts(SkillTargetMode::Auto, false)),
            SkillTarget::Skill
        );
    }

    #[test]
    fn test_decide_skill_target_explicit_mode_overrides_heuristic() {
        let n = node_with("skills.model", serde_json::json!("opus"));
        assert_eq!(
            decide_skill_target(&n, &opts(SkillTargetMode::Skill, false)),
            SkillTarget::Skill
        );
    }

    #[test]
    fn test_degrade_to_subagent_unknown_model_passthrough_warns() {
        let n = node_with("skills.model", serde_json::json!("gpt-5-custom"));
        let (artifacts, diags) = degrade_to_subagent("demo", &n, "skills.model");
        assert!(diags.iter().any(
            |d| d.id.as_deref() == Some("skills.model") && d.message.contains("Unknown model")
        ));
        let agent_toml = artifacts
            .iter()
            .find(|a| a.path.ends_with("demo.toml"))
            .unwrap();
        assert!(agent_toml.content.contains("gpt-5-custom"));
    }

    #[test]
    fn test_degrade_to_subagent_trigger_id_in_diagnostic() {
        // Each trigger field id must appear in the emitted diagnostic id.
        for trigger in &["skills.model", "skills.effort", "skills.context-fork"] {
            let n = new_node(Kind::Skill, Tool::Claude, "demo");
            let (_artifacts, diags) = degrade_to_subagent("demo", &n, trigger);
            let degrade_diag = diags
                .iter()
                .find(|d| d.message.contains("degraded to subagent"))
                .expect("degrade diagnostic must be emitted");
            assert_eq!(
                degrade_diag.id.as_deref(),
                Some(*trigger),
                "diagnostic id must match trigger_id '{}', got {:?}",
                trigger,
                degrade_diag.id
            );
        }
    }
}
