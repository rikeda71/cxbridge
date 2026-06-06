mod common;
use common::*;

use std::path::Path;

use cxbridge::core::{
    detect::detect, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use cxbridge::handlers::pick_handler;

/// Claude agents/<n>.md c2x: .codex/agents/<n>.toml is generated.
#[test]
fn test_subagent_c2x_generates_codex_toml() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    assert_eq!(
        kind,
        cxbridge::core::ir::Kind::Subagent,
        "agents/<n>.md should be Kind::Subagent"
    );

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, cxbridge::core::ir::Kind::Subagent);
    assert!(ir.fields.contains_key("subagents.name"));
    assert!(ir.fields.contains_key("subagents.description"));

    // name should be lossless
    let name_f = &ir.fields["subagents.name"];
    assert_eq!(
        name_f.loss,
        cxbridge::core::ir::Loss::Lossless,
        "name should be lossless"
    );
    assert_eq!(
        name_f.value,
        serde_json::Value::String("researcher".to_string())
    );

    // model is lossy (different providers)
    let model_f = &ir.fields["subagents.model"];
    assert_eq!(
        model_f.loss,
        cxbridge::core::ir::Loss::Lossy,
        "model should be lossy"
    );

    // effort: high → high (1:1, lossless in terms of transform)
    let effort_f = &ir.fields["subagents.effort"];
    assert_eq!(
        effort_f.value,
        serde_json::Value::String("high".to_string())
    );

    // maxTurns → dropped
    let max_turns = &ir.fields["subagents.maxTurns"];
    assert_eq!(
        max_turns.loss,
        cxbridge::core::ir::Loss::Dropped,
        "maxTurns should be dropped"
    );

    // background → dropped
    let bg = &ir.fields["subagents.background"];
    assert_eq!(
        bg.loss,
        cxbridge::core::ir::Loss::Dropped,
        "background should be dropped"
    );

    // color → dropped
    let color = &ir.fields["subagents.color"];
    assert_eq!(
        color.loss,
        cxbridge::core::ir::Loss::Dropped,
        "color should be dropped"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

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
        content.contains("Conduct research"),
        "description should be in TOML"
    );
    assert!(
        content.contains("developer_instructions"),
        "body should be in developer_instructions"
    );
    assert!(
        content.contains("research specialist"),
        "body text should be present"
    );
    assert!(
        content.contains("model_reasoning_effort"),
        "effort should be in TOML"
    );
    assert!(content.contains("high"), "effort value should be in TOML");
}

/// .codex/agents/<n>.toml x2c: .claude/agents/<n>.md is generated.
#[test]
fn test_subagent_x2c_generates_claude_md() {
    let agent_path = "tests/fixtures/codex/agents/coder.toml";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Subagent);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    assert_eq!(ir.kind, cxbridge::core::ir::Kind::Subagent);

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_skill(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let agent_md = plan.files.iter().find(|f| f.path.ends_with("coder.md"));
    assert!(
        agent_md.is_some(),
        "Expected coder.md in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &agent_md.unwrap().content;
    assert!(content.contains("coder"), "name should be in frontmatter");
    assert!(
        content.contains("Write, review"),
        "description should be in frontmatter"
    );
    assert!(
        content.contains("expert software engineer"),
        "developer_instructions should be in body"
    );
}

/// Subagent c2x roundtrip: report enumerates dropped fields.
#[test]
fn test_subagent_c2x_report_dropped_fields() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    assert!(
        !report.dropped.is_empty(),
        "Expected dropped fields in subagent report"
    );
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"subagents.maxTurns"),
        "Expected subagents.maxTurns in dropped, got: {:?}",
        drop_ids
    );
    assert!(
        drop_ids.contains(&"subagents.background"),
        "Expected subagents.background in dropped, got: {:?}",
        drop_ids
    );
    assert!(
        drop_ids.contains(&"subagents.color"),
        "Expected subagents.color in dropped, got: {:?}",
        drop_ids
    );

    // A spawn-model warn must be emitted
    let has_spawn_warn = ir
        .diagnostics
        .iter()
        .any(|d| d.message.contains("spawn_agent"));
    assert!(
        has_spawn_warn,
        "Expected spawn_agent warning about auto-delegation difference"
    );
}

/// Subagent c2x: lower emits config.toml with [agents.<name>] pointer and
/// [features] multi_agent = true alongside the agent TOML file.
#[test]
fn test_subagent_c2x_emits_config_toml_with_agents_and_features() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // The agent TOML must be present.
    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("researcher.toml"));
    assert!(
        agent_toml.is_some(),
        "Expected researcher.toml in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // A config.toml must also be emitted with [agents.researcher] and multi_agent.
    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml in output (spec §10.2), got files: {:?}",
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
        "Expected config_file pointer in config.toml, got:\n{}",
        content
    );
    assert!(
        content.contains("multi_agent"),
        "Expected multi_agent in config.toml [features], got:\n{}",
        content
    );
    assert!(
        content.contains("true"),
        "Expected multi_agent = true in config.toml, got:\n{}",
        content
    );
}

/// x2c: Codex TOML with [skills] table is lifted into subagents.skills and
/// lowered to `skills:` list in the Claude agent frontmatter.
#[test]
fn test_subagent_x2c_skills_lifted_integration() {
    let agent_path = "tests/fixtures/codex/agents/skills-agent.toml";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Subagent);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // skills.config must be lifted into IR as subagents.skills — not dropped
    assert!(
        ir.fields.contains_key("subagents.skills"),
        "IR must contain subagents.skills from Codex [skills] table; got fields: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );

    // Must not appear as unknown drop diagnostic
    let has_unknown_drop = ir
        .diagnostics
        .iter()
        .any(|d| d.level == cxbridge::core::ir::DiagLevel::Drop && d.message.contains("skills"));
    assert!(
        !has_unknown_drop,
        "Must not drop 'skills' as unknown key; diagnostics: {:?}",
        ir.diagnostics
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_skill(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let agent_md = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("skills-agent.md"))
        .unwrap_or_else(|| {
            panic!(
                "Expected skills-agent.md in output, got: {:?}",
                plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        });

    let content = &agent_md.content;
    assert!(
        content.contains("python"),
        "Output .md must contain 'python' in skills; got:\n{}",
        content
    );
    assert!(
        content.contains("javascript"),
        "Output .md must contain 'javascript' in skills; got:\n{}",
        content
    );
    assert!(
        content.contains("skills"),
        "Output .md must contain 'skills' frontmatter key; got:\n{}",
        content
    );

    // A Warn diagnostic for the lossy skills mapping must be emitted
    let has_skills_warn = plan.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("subagents.skills")
            && d.level == cxbridge::core::ir::DiagLevel::Warn
    });
    assert!(
        has_skills_warn,
        "Expected subagents.skills Warn diagnostic; got: {:?}",
        plan.diagnostics
    );
}

/// Each dropped field id must appear exactly once in report.dropped.
/// Fields like maxTurns (warn:true, loss:dropped) must not be duplicated.
#[test]
fn test_subagent_c2x_no_duplicate_dropped_entries() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");
    let report = build_report(&ir, &plan);

    // Every dropped field id must appear exactly once.
    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    for target_id in &[
        "subagents.maxTurns",
        "subagents.background",
        "subagents.color",
    ] {
        let count = dropped_ids.iter().filter(|id| *id == target_id).count();
        assert_eq!(
            count, 1,
            "Expected {} to appear exactly once in dropped, found {} times. dropped ids: {:?}",
            target_id, count, dropped_ids
        );
    }
}

/// A field that is in report.dropped must not also appear in report.lossy.
#[test]
fn test_subagent_c2x_dropped_and_lossy_are_disjoint() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");
    let report = build_report(&ir, &plan);

    let dropped_ids: std::collections::HashSet<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    let lossy_ids: Vec<_> = report
        .lossy
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    for lossy_id in &lossy_ids {
        assert!(
            !dropped_ids.contains(lossy_id),
            "Field '{}' appears in both dropped and lossy — fields must be in exactly one category",
            lossy_id
        );
    }
}

/// The spawn-model note must appear exactly once in report.lossy.
#[test]
fn test_subagent_c2x_spawn_model_appears_exactly_once_in_lossy() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");
    let report = build_report(&ir, &plan);

    let spawn_model_id = "subagents.spawn-model";
    let count_in_lossy = report
        .lossy
        .iter()
        .filter(|e| e.id.as_deref() == Some(spawn_model_id))
        .count();
    assert_eq!(
        count_in_lossy, 1,
        "Expected spawn-model to appear exactly once in report.lossy, found {} times",
        count_in_lossy
    );

    // spawn-model must not be in dropped
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some(spawn_model_id));
    assert!(!in_dropped, "spawn-model must not appear in report.dropped");
}

/// c2x of an agent with permissionMode=acceptEdits must NOT write sandbox_mode
/// to the output TOML, and the report must contain a drop diagnostic for
/// subagents.permissionMode.
#[test]
fn test_subagent_c2x_permission_mode_accept_edits_dropped() {
    let agent_path = "tests/fixtures/claude/agents/perm_accept_edits.md";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // The agent TOML must NOT contain sandbox_mode.
    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("perm_accept_edits.toml"))
        .expect("Expected perm_accept_edits.toml in output");

    assert!(
        !agent_toml.content.contains("sandbox_mode"),
        "sandbox_mode must not appear in TOML for permissionMode=acceptEdits, got:\n{}",
        agent_toml.content
    );

    // A drop diagnostic for subagents.permissionMode must be in plan.diagnostics.
    let has_drop_diag = plan.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("subagents.permissionMode")
            && d.level == cxbridge::core::ir::DiagLevel::Drop
    });
    assert!(
        has_drop_diag,
        "Expected a Drop diagnostic for subagents.permissionMode in plan.diagnostics, got: {:?}",
        plan.diagnostics
            .iter()
            .map(|d| (d.id.as_deref(), &d.level))
            .collect::<Vec<_>>()
    );
}

/// c2x of agents with permissionMode=dontAsk and permissionMode=auto must also
/// not produce sandbox_mode in the TOML output.
#[test]
fn test_subagent_c2x_permission_mode_dont_ask_and_auto_dropped() {
    let maps = load_mappings();

    for (agent_path, stem) in [
        (
            "tests/fixtures/claude/agents/perm_dont_ask.md",
            "perm_dont_ask.toml",
        ),
        (
            "tests/fixtures/claude/agents/perm_auto.md",
            "perm_auto.toml",
        ),
    ] {
        let out_dir = tempfile::TempDir::new().unwrap();
        let kind = detect(agent_path).expect("detect should succeed");
        let handler = pick_handler(&kind, maps);
        let parsed = handler
            .parse(Path::new(agent_path))
            .expect("parse should succeed");
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .expect("lift should succeed");

        let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
        let plan = handler
            .lower(&ir, ConvDir::C2x, &opts)
            .expect("lower should succeed");

        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with(stem))
            .unwrap_or_else(|| {
                panic!(
                    "Expected {} in output, got: {:?}",
                    stem,
                    plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                )
            });

        assert!(
            !agent_toml.content.contains("sandbox_mode"),
            "sandbox_mode must not appear in TOML for {} (permissionMode has no Codex equivalent), got:\n{}",
            stem,
            agent_toml.content
        );

        let has_drop_diag = plan.diagnostics.iter().any(|d| {
            d.id.as_deref() == Some("subagents.permissionMode")
                && d.level == cxbridge::core::ir::DiagLevel::Drop
        });
        assert!(
            has_drop_diag,
            "Expected Drop diagnostic for subagents.permissionMode in plan for {}, got: {:?}",
            stem,
            plan.diagnostics
                .iter()
                .map(|d| (d.id.as_deref(), &d.level))
                .collect::<Vec<_>>()
        );
    }
}

/// Each loss:dropped + warn:true subagents field must appear exactly once in
/// report.dropped when the full pipeline (lift → lower → build_report) is run.
#[test]
fn test_subagents_dropped_warn_fields_appear_once_in_dropped() {
    let fixture = Path::new("tests/fixtures/claude/agents/dropped_warn_fields.md");
    assert!(
        fixture.exists(),
        "Fixture tests/fixtures/claude/agents/dropped_warn_fields.md must exist"
    );

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Subagent, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    let dropped_warn_ids = [
        "subagents.maxTurns",
        "subagents.background",
        "subagents.isolation",
        "subagents.disallowedTools",
    ];

    for field_id in &dropped_warn_ids {
        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some(field_id))
            .count();
        assert_eq!(
            count,
            1,
            "{field_id} must appear exactly once in report.dropped; found {count} times. \
             Full dropped: {:?}",
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }
}

/// Each loss:dropped + warn:true subagents field must NOT appear in report.lossy.
#[test]
fn test_subagents_dropped_warn_fields_not_in_lossy() {
    let fixture = Path::new("tests/fixtures/claude/agents/dropped_warn_fields.md");
    assert!(
        fixture.exists(),
        "Fixture tests/fixtures/claude/agents/dropped_warn_fields.md must exist"
    );

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Subagent, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    let dropped_warn_ids = [
        "subagents.maxTurns",
        "subagents.background",
        "subagents.isolation",
        "subagents.disallowedTools",
    ];

    let spurious_in_lossy: Vec<_> = report
        .lossy
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| dropped_warn_ids.contains(&id))
                .unwrap_or(false)
        })
        .collect();

    assert!(
        spurious_in_lossy.is_empty(),
        "loss:dropped + warn:true subagents fields must NOT appear in report.lossy; found: {:?}",
        spurious_in_lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
