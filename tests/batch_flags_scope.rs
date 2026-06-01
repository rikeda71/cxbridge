//! Integration tests for gap 18/42: --scope user effect on .rules file placement.
//!
//! Spec §13: '--scope <project|user>' controls the degrade target scope
//! (.rules / agents placement).
//! Spec §10.1: user-scope path is '~/.codex/rules/default.rules'.
//!
//! With '--scope user', the generated .rules file must go to a user-scope path.
//! With '--scope project' (default), it must go to '.codex/rules/<skill>.rules'.

use std::path::Path;

use ccx::core::ir::Kind;
use ccx::core::{mappings::load_mappings, transforms::ConvDir};
use ccx::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

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

/// With --scope project (default), the .rules file must land at
/// '.codex/rules/<skill>.rules' (project-relative path).
#[test]
fn test_scope_project_rules_path_is_project_relative() {
    let dir = tempfile::TempDir::new().unwrap();
    let skill_path = make_skill_file(&dir, "t");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = opts_with_scope(out_dir.path().to_str().unwrap(), Scope::Project);

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Skill, &maps);
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

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Skill, &maps);
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
