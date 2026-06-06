mod common;
use common::*;

use std::path::Path;

use cxbridge::core::{
    detect::detect, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use cxbridge::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

// ────────────────────────────────────────────────────────────────────────────
// Helpers local to this module (not hoisted to tests/common/mod.rs)
// ────────────────────────────────────────────────────────────────────────────

fn make_skill_file(dir: &tempfile::TempDir, skill_name: &str) -> std::path::PathBuf {
    let skill_dir = dir.path().join(".claude").join("skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    let path = skill_dir.join("SKILL.md");
    std::fs::write(
        &path,
        format!(
            "---\nname: {name}\ndescription: d\nallowed-tools:\n  - \"Bash(npm run *)\"\n---\nbody",
            name = skill_name
        ),
    )
    .unwrap();
    path
}

fn opts_with_scope(out_dir: &str, scope: Scope) -> LowerOpts {
    LowerOpts {
        out: Some(out_dir.to_string()),
        only: vec![],
        scope,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Skills tests (from roundtrip.rs and batch_flags_scope.rs)
// ────────────────────────────────────────────────────────────────────────────

/// Convert SKILL.md via c2x and verify the report matches expectations.
#[test]
fn test_skill_c2x_basic_roundtrip() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());
    assert!(
        !report.lossless.is_empty(),
        "Expected lossless fields (name, description)"
    );
    assert!(
        report.lossless.contains(&"skills.name".to_string()),
        "skills.name should be lossless"
    );
    assert!(
        report.lossless.contains(&"skills.description".to_string()),
        "skills.description should be lossless"
    );

    assert!(
        !report.dropped.is_empty(),
        "Expected dropped fields (user-invocable, paths, etc.)"
    );
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"skills.user-invocable"),
        "Expected skills.user-invocable in dropped. Got: {:?}",
        drop_ids
    );

    assert!(
        !report.degraded.is_empty() || !report.lossy.is_empty(),
        "Expected degraded or lossy entries (model, effort, allowed-tools)"
    );
}

/// Convert SKILL.md via c2x and verify that the dropped count is reported.
#[test]
fn test_skill_c2x_check_reports_dropped() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());
    assert!(
        report.dropped.len() >= 2,
        "Expected at least 2 dropped fields, got {}",
        report.dropped.len()
    );
}

/// Simulate the cxbridge check command: report the dropped count.
#[test]
fn test_check_skill_reports_dropped_count() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    println!(
        "check: {}\n  dropped: {}, degraded: {}, lossy: {}, lossless: {}",
        skill_path,
        report.dropped.len(),
        report.degraded.len(),
        report.lossy.len(),
        report.lossless.len()
    );

    assert!(report.lossless.contains(&"skills.name".to_string()));
    assert!(report.lossless.contains(&"skills.description".to_string()));

    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        dropped_ids.contains(&"skills.user-invocable"),
        "user-invocable should be dropped, got: {:?}",
        dropped_ids
    );

    // body warnings must be present (dynamic injection and variable references exist)
    assert!(
        !report.body_warnings.is_empty(),
        "Expected body warnings from skill body"
    );
}

/// Files are generated after c2x lower and the SKILL.md content is correct.
#[test]
fn test_skill_c2x_lower_generates_skill_md() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let skill_file = plan.files.iter().find(|f| f.path.ends_with("SKILL.md"));
    assert!(skill_file.is_some(), "Expected SKILL.md in output");

    let content = &skill_file.unwrap().content;
    assert!(content.contains("deploy"), "Expected 'deploy' in SKILL.md");
    assert!(
        content.contains("Deploy the application"),
        "Expected description in SKILL.md"
    );
    assert!(
        content.contains("Use this skill when"),
        "Expected when_to_use concatenated into description"
    );

    // A .rules file must be generated (Bash tool degrade)
    let rules_file = plan.files.iter().find(|f| f.path.ends_with(".rules"));
    assert!(
        rules_file.is_some(),
        "Expected .rules file for Bash tool degrade"
    );

    // A subagent TOML must be generated (model/effort degrade)
    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.contains(".codex/agents/") && f.path.ends_with(".toml"));
    assert!(
        agent_toml.is_some(),
        "Expected subagent .toml for model/effort degrade"
    );
}

/// insta snapshot test: report JSON output must be stable.
#[test]
fn test_skill_c2x_report_snapshot() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");
    let report = build_report(&ir, &empty_plan());

    // Only snapshot stable output fields
    let snapshot = serde_json::json!({
        "lossless_count": report.lossless.len(),
        "dropped_count": report.dropped.len(),
        "body_warnings_count": report.body_warnings.len(),
        "lossless_includes_name": report.lossless.contains(&"skills.name".to_string()),
        "lossless_includes_description": report.lossless.contains(&"skills.description".to_string()),
    });

    insta::assert_json_snapshot!("skill_c2x_report_summary", snapshot);
}

/// c2x lower for a skill with Write/Read allowed-tools must emit a config.toml
/// file containing [permissions.<skill>].filesystem entries ("write" and "read").
#[test]
fn test_skill_c2x_write_read_tools_produce_config_toml() {
    let skill_path = "tests/fixtures/claude/skills/ed/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // A config.toml file must be emitted for the Write/Read tool permissions.
    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml SideArtifact for Write/Read tool degrade. Got files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &config_toml.unwrap().content;
    assert!(
        content.contains("[permissions.ed]"),
        "Expected [permissions.ed] table in config.toml, got:\n{}",
        content
    );
    assert!(
        content.contains("write"),
        "Expected 'write' value for Write(**/*.py) glob, got:\n{}",
        content
    );
    assert!(
        content.contains("read"),
        "Expected 'read' value for Read(~/.ssh/*) glob, got:\n{}",
        content
    );
}

/// c2x lift of a SKILL.md with disable-model-invocation=true must produce
/// an IR field with loss=Lossy and a warning (not silently dropped).
#[test]
fn test_skill_c2x_disable_model_invocation_in_report() {
    let skill_path = "tests/fixtures/claude/skills/s/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    let lossy_ids: Vec<_> = report
        .lossy
        .iter()
        .filter_map(|e| e.id.as_deref())
        .collect();
    assert!(
        lossy_ids.contains(&"skills.disable-model-invocation"),
        "Expected skills.disable-model-invocation in lossy report entries, got: {:?}",
        lossy_ids
    );
}

/// c2x lower of a SKILL.md with disable-model-invocation=true must emit
/// .agents/skills/s/agents/openai.yaml with allow_implicit_invocation: false.
#[test]
fn test_skill_c2x_disable_model_invocation_lower_emits_openai_yaml() {
    let skill_path = "tests/fixtures/claude/skills/s/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let openai_yaml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("agents/openai.yaml"))
        .expect("Expected .agents/skills/s/agents/openai.yaml in emit plan");

    assert!(
        openai_yaml
            .content
            .contains("allow_implicit_invocation: false"),
        "openai.yaml must contain 'allow_implicit_invocation: false', got:\n{}",
        openai_yaml.content
    );
}

/// c2x with keep_claude_frontmatter=true must emit a SKILL.md that retains
/// Claude-specific frontmatter keys (when_to_use, allowed-tools) in addition
/// to the standard Codex fields (name, description).
#[test]
fn test_skill_c2x_keep_claude_frontmatter_retains_claude_keys() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: true,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let skill_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("SKILL.md"))
        .expect("Expected SKILL.md in emit plan");

    // Claude-specific keys must be present in the output frontmatter
    assert!(
        skill_file.content.contains("when_to_use"),
        "Expected 'when_to_use' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("allowed-tools"),
        "Expected 'allowed-tools' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    // Standard Codex fields must also be present
    assert!(
        skill_file.content.contains("name"),
        "Expected 'name' in frontmatter, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("description"),
        "Expected 'description' in frontmatter, got:\n{}",
        skill_file.content
    );
}

/// c2x with keep_claude_frontmatter=true must retain allowed-tools, model, and
/// effort in the output SKILL.md (not just when_to_use and name/description).
/// Uses the deploy fixture which has all three Claude-specific fields.
#[test]
fn test_skill_c2x_keep_claude_frontmatter_model_effort_allowed_tools() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: true,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let skill_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("SKILL.md"))
        .expect("Expected SKILL.md in emit plan");

    assert!(
        skill_file.content.contains("allowed-tools"),
        "Expected 'allowed-tools' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("model"),
        "Expected 'model' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("effort"),
        "Expected 'effort' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("name"),
        "Expected 'name' in frontmatter, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("description"),
        "Expected 'description' in frontmatter, got:\n{}",
        skill_file.content
    );
}

/// Integration test: the four warn:true + loss:dropped skill fields
/// (user-invocable, paths, argument-hint, arguments) must appear only in the
/// `dropped` section of the report and must NOT appear in `lossy`.
/// `summary.lossy` must be 0 for those entries.
#[test]
fn test_skill_c2x_dropped_warn_fields_not_in_lossy() {
    let skill_path = "tests/fixtures/claude/skills/dup-warn-dropped/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    let lossy_ids: Vec<_> = report
        .lossy
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    for field_id in &[
        "skills.user-invocable",
        "skills.paths",
        "skills.argument-hint",
        "skills.arguments",
    ] {
        assert!(
            dropped_ids.contains(field_id),
            "{} must appear in dropped, dropped: {:?}",
            field_id,
            dropped_ids
        );
        assert!(
            !lossy_ids.contains(field_id),
            "{} must NOT appear in lossy (was promoted from dropped), lossy: {:?}",
            field_id,
            lossy_ids
        );
    }

    // summary.lossy should count only genuinely lossy entries, not dropped ones
    // For this fixture (name+description lossless, four fields dropped), lossy == 0.
    assert_eq!(
        report.lossy.len(),
        0,
        "Expected 0 lossy entries for dup-warn-dropped fixture, got: {:?}",
        report
            .lossy
            .iter()
            .map(|d| d.id.as_deref())
            .collect::<Vec<_>>()
    );
}

/// c2x: Non-.md auxiliary files in skill dir are copied to the output with path remap.
///
/// The fixture has `tests/fixtures/claude/skills/aux-skill/scripts/run.sh` and
/// `tests/fixtures/claude/skills/aux-skill/README.txt` alongside SKILL.md.
/// After lower(c2x), both must appear at `.agents/skills/aux-skill/scripts/run.sh`
/// and `.agents/skills/aux-skill/README.txt` respectively, content unchanged.
#[test]
fn test_skill_c2x_aux_files_are_path_remapped() {
    let skill_path = "tests/fixtures/claude/skills/aux-skill/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // SKILL.md must be present
    let has_skill_md = plan.files.iter().any(|f| f.path.ends_with("SKILL.md"));
    assert!(has_skill_md, "Expected SKILL.md in emit plan");

    // scripts/run.sh must be remapped to .agents/skills/aux-skill/scripts/run.sh
    let run_sh = plan
        .files
        .iter()
        .find(|f| f.path.contains(".agents/skills/aux-skill/scripts/run.sh"));
    assert!(
        run_sh.is_some(),
        "Expected .agents/skills/aux-skill/scripts/run.sh in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        run_sh.unwrap().content.trim(),
        "#!/bin/bash\necho hi",
        "run.sh content must be unchanged"
    );

    // README.txt must be remapped to .agents/skills/aux-skill/README.txt
    let readme = plan
        .files
        .iter()
        .find(|f| f.path.contains(".agents/skills/aux-skill/README.txt"));
    assert!(
        readme.is_some(),
        "Expected .agents/skills/aux-skill/README.txt in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        readme.unwrap().content.trim(),
        "readme",
        "README.txt content must be unchanged"
    );
}

/// x2c: Non-.md auxiliary files in skill dir (excluding agents/openai.yaml) are
/// copied to the output with path remap (.agents/ → .claude/).
#[test]
fn test_skill_x2c_aux_files_are_path_remapped() {
    let skill_path = "tests/fixtures/codex/agents/aux-skill/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // SKILL.md must be present
    let has_skill_md = plan.files.iter().any(|f| f.path.ends_with("SKILL.md"));
    assert!(has_skill_md, "Expected SKILL.md in emit plan");

    // scripts/run.sh must be remapped to .claude/skills/aux-skill/scripts/run.sh
    let run_sh = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude/skills/aux-skill/scripts/run.sh"));
    assert!(
        run_sh.is_some(),
        "Expected .claude/skills/aux-skill/scripts/run.sh in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        run_sh.unwrap().content.trim(),
        "#!/bin/bash\necho hi",
        "run.sh content must be unchanged"
    );

    // README.txt must be remapped to .claude/skills/aux-skill/README.txt
    let readme = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude/skills/aux-skill/README.txt"));
    assert!(
        readme.is_some(),
        "Expected .claude/skills/aux-skill/README.txt in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        readme.unwrap().content.trim(),
        "readme",
        "README.txt content must be unchanged"
    );
}

/// With --scope project (default), the .rules file must land at
/// '.codex/rules/<skill>.rules' (project-relative path).
#[test]
fn test_scope_project_rules_path_is_project_relative() {
    let dir = tempfile::TempDir::new().unwrap();
    let skill_path = make_skill_file(&dir, "t");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = opts_with_scope(out_dir.path().to_str().unwrap(), Scope::Project);

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Skill, maps);
    let parsed = handler.parse(&skill_path).unwrap();
    let ir = handler.lift(&parsed, ConvDir::C2x).unwrap();
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    let rules_files: Vec<&str> = plan
        .files
        .iter()
        .filter(|f| f.path.ends_with(".rules"))
        .map(|f| f.path.as_str())
        .collect();

    assert!(
        !rules_files.is_empty(),
        "Expected at least one .rules file, got none"
    );

    for path in &rules_files {
        // Project-scope: must be under <out_dir>/.codex/rules/
        assert!(
            path.contains(".codex/rules/") && path.ends_with("t.rules"),
            "Project-scope .rules must be .codex/rules/t.rules, got: {}",
            path
        );
        // Must NOT look like a user home path
        let home = std::env::var("HOME").unwrap_or_default();
        assert!(
            !path.starts_with(&home),
            "Project-scope .rules must not be in HOME, got: {}",
            path
        );
    }

    // Diagnostic must say skill→project
    let scope_diag = plan
        .diagnostics
        .iter()
        .find(|d| d.message.contains("scope:"));
    if let Some(d) = scope_diag {
        assert!(
            d.message.contains("skill→project"),
            "Diagnostic must say 'skill→project', got: {}",
            d.message
        );
    }
}

/// With --scope user, the .rules file must land at the user-scope path
/// (~/.codex/rules/default.rules), NOT at the project-relative path.
#[test]
fn test_scope_user_rules_path_is_user_home() {
    let dir = tempfile::TempDir::new().unwrap();
    let skill_path = make_skill_file(&dir, "t");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = opts_with_scope(out_dir.path().to_str().unwrap(), Scope::User);

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Skill, maps);
    let parsed = handler.parse(&skill_path).unwrap();
    let ir = handler.lift(&parsed, ConvDir::C2x).unwrap();
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    let rules_files: Vec<&str> = plan
        .files
        .iter()
        .filter(|f| f.path.ends_with(".rules"))
        .map(|f| f.path.as_str())
        .collect();

    assert!(
        !rules_files.is_empty(),
        "Expected at least one .rules file, got none"
    );

    let home = std::env::var("HOME").unwrap_or_default();
    for path in &rules_files {
        // User-scope: must be under ~/.codex/rules/default.rules
        assert!(
            path.ends_with(".codex/rules/default.rules"),
            "User-scope .rules must end with '.codex/rules/default.rules', got: {}",
            path
        );
        assert!(
            !home.is_empty() && path.starts_with(&home),
            "User-scope .rules must be inside HOME ({}), got: {}",
            home,
            path
        );
    }

    // Diagnostic must say skill→user
    let scope_diag = plan
        .diagnostics
        .iter()
        .find(|d| d.message.contains("scope:"));
    assert!(
        scope_diag.is_some(),
        "Expected a diagnostic containing 'scope:', got none. All diags: {:?}",
        plan.diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
    let d = scope_diag.unwrap();
    assert!(
        d.message.contains("skill→user"),
        "Diagnostic must say 'skill→user' for Scope::User, got: {}",
        d.message
    );
}
