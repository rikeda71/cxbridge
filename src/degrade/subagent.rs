use crate::core::ir::{DiagLevel, Diagnostic, IRNode, SideArtifact};
use crate::core::transforms::{claude_tier, tier_to_codex};
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

/// TTY 対話で変換先を確認する（dialoguer を使用）。
fn ask_user_skill_target(ir: &IRNode) -> SkillTarget {
    use dialoguer::Select;

    let skill_name = ir.source_path.as_str();
    let items = &[
        "skill (権限は session 降格・自動発火を維持)",
        "subagent (権限を subagent に束ねる・明示起動)",
    ];

    let selection = Select::new()
        .with_prompt(format!(
            "skill '{}' は allowed-tools を持ちます。変換先を選択してください",
            skill_name
        ))
        .items(items)
        .default(1) // 保守的デフォルト: subagent
        .interact();

    match selection {
        Ok(0) => SkillTarget::Skill,
        _ => SkillTarget::Subagent,
    }
}

/// Generates `.codex/agents/<skill>.toml` and appends the required `[agents.*]` /
/// `[features].multi_agent` entries to `config.toml`.
///
/// A Diagnostic is always emitted to record that the skill's auto-fork behaviour
/// changes to an explicit `spawn_agent` call in Codex.
pub fn degrade_to_subagent(skill_name: &str, ir: &IRNode) -> (Vec<SideArtifact>, Vec<Diagnostic>) {
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
        tier_to_codex(crate::core::transforms::Tier::Mid).to_string()
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
        toml_lines.push(format!("developer_instructions = '''\n{}\n'''", body));
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
        id: Some("skills.context-fork".to_string()),
        message: format!(
            "skill '{}' degraded to subagent (.codex/agents/{}.toml). \
             自動 fork ではなく spawn_agent の明示起動になります。\
             features.multi_agent=true の設定も必要です。",
            skill_name, skill_name
        ),
    });

    (artifacts, diagnostics)
}
