//! Integration tests for gap 36/42: developer_instructions → CLAUDE.md degrade.
//!
//! Spec §9.7: developer_instructions from config.toml must be written to
//! .claude/CLAUDE.md during x2c conversion, not silently dropped.

use std::path::Path;

use ccx::core::{mappings::load_mappings, transforms::ConvDir};
use ccx::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

fn default_lower_opts(out_dir: &str) -> LowerOpts {
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

/// developer_instructions in config.toml must produce a CLAUDE.md file during x2c.
#[test]
fn test_developer_instructions_produces_claude_md() {
    let fixture_path = "tests/fixtures/codex/developer_instructions/config.toml";
    assert!(
        Path::new(fixture_path).exists(),
        "Fixture {} must exist",
        fixture_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = ccx::core::detect::detect(fixture_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(fixture_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // CLAUDE.md must be generated at the exact relative path .claude/CLAUDE.md.
    let expected_suffix = std::path::Path::new(".claude").join("CLAUDE.md");
    let claude_md = plan
        .files
        .iter()
        .find(|f| std::path::Path::new(&f.path).ends_with(&expected_suffix));
    assert!(
        claude_md.is_some(),
        "Expected a file ending with .claude/CLAUDE.md in output when developer_instructions is set; got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &claude_md.unwrap().content;
    assert!(
        content.contains("Always respond in English"),
        "CLAUDE.md must contain the original developer_instructions text; got:\n{}",
        content
    );
    assert!(
        content.contains("Focus on clear answers"),
        "CLAUDE.md must contain full developer_instructions text; got:\n{}",
        content
    );
}

/// The degrade diagnostic for developer_instructions must still be emitted.
#[test]
fn test_developer_instructions_degrade_diagnostic_present() {
    let fixture_path = "tests/fixtures/codex/developer_instructions/config.toml";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = ccx::core::detect::detect(fixture_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(fixture_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // The IR must have the developer_instructions field with degrade metadata
    let field = ir
        .fields
        .get("settings.codex.developer_instructions")
        .expect("IR must contain settings.codex.developer_instructions");
    assert!(
        field.degrade.is_some(),
        "developer_instructions must have degrade info"
    );
    assert_eq!(
        field.degrade.as_ref().unwrap().target,
        "CLAUDE.md",
        "degrade target must be CLAUDE.md"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let _plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // The degrade diagnostic is emitted during lift (ir.diagnostics), not lower,
    // to avoid duplicating it in the report.
    let has_degrade_diag = ir
        .diagnostics
        .iter()
        .any(|d| d.id.as_deref() == Some("settings.codex.developer_instructions"));
    assert!(
        has_degrade_diag,
        "Expected degrade diagnostic for developer_instructions in ir.diagnostics; got: {:?}",
        ir.diagnostics
            .iter()
            .map(|d| (d.id.as_deref().unwrap_or("<none>"), &d.message))
            .collect::<Vec<_>>()
    );
}

/// When developer_instructions is absent, no CLAUDE.md should be emitted from settings.
#[test]
fn test_no_developer_instructions_no_claude_md_from_settings() {
    let fixture_path = "tests/fixtures/codex/config.toml";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = ccx::core::detect::detect(fixture_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(fixture_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // No CLAUDE.md from the settings handler when no developer_instructions
    let claude_md_from_settings = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
    assert!(
        claude_md_from_settings.is_none(),
        "No CLAUDE.md should be emitted when developer_instructions is absent; got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}
