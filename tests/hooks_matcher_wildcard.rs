//! Integration tests for gap 27/42: wildcard matchers ("*" and "") must emit
//! diagnostic id "hooks.matcher.wildcard", not "hooks.matcher.exact".
//!
//! Spec entry `hooks.matcher.wildcard` in mappings/hooks.yaml: loss=lossy, warn=true.
//! The report entry for a wildcard conversion must carry id "hooks.matcher.wildcard".

use std::path::Path;

use ccx::core::ir::Kind;
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
        skill_target: SkillTargetMode::Auto,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    }
}

/// End-to-end: converting a hooks.json with wildcard matchers ("*" and "")
/// must produce IR diagnostics with id "hooks.matcher.wildcard" and must NOT
/// produce any diagnostic with id "hooks.matcher.exact".
#[test]
fn test_hooks_wildcard_matcher_id_e2e() {
    let fixture = Path::new("tests/fixtures/claude/hooks_wildcard/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Hooks, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // There must be at least one "hooks.matcher.wildcard" diagnostic
    let wildcard_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.matcher.wildcard"))
        .collect();
    assert!(
        !wildcard_diags.is_empty(),
        "Expected at least one diagnostic with id 'hooks.matcher.wildcard', got: {:?}",
        ir.diagnostics
            .iter()
            .map(|d| d.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );

    // Must NOT have any "hooks.matcher.exact" diagnostics (both matchers are wildcards)
    let exact_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.matcher.exact"))
        .collect();
    assert!(
        exact_diags.is_empty(),
        "Expected NO 'hooks.matcher.exact' diagnostics for wildcard-only fixture, got: {:?}",
        exact_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Both wildcard matchers ("*" and "") should each produce a wildcard diagnostic
    assert_eq!(
        wildcard_diags.len(),
        2,
        "Expected 2 wildcard diagnostics (one for '*', one for ''), got: {:?}",
        wildcard_diags
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // Smoke-check: hooks.json must be emitted
    assert!(
        plan.files.iter().any(|f| f.path.ends_with("hooks.json")),
        "Expected hooks.json in output files"
    );
}
