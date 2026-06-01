//! Integration tests for gap 28/42: regex passthrough matchers must NOT emit
//! a warning and must appear in the lossless section.
//!
//! Spec entry `hooks.matcher.regex` in mappings/hooks.yaml:
//!   loss: lossless, warn: false (no warn field = false by default)
//! An already-regex matcher passed through to Codex unchanged should produce
//! no "hooks.matcher.regex" diagnostic and must appear in the lossless section
//! of the report.

use std::path::Path;

use ccx::core::ir::DiagLevel;
use ccx::core::ir::Kind;
use ccx::core::{mappings::load_mappings, report::build_report, transforms::ConvDir};
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

/// End-to-end: converting a hooks.json with a regex matcher ("^Bash.*") must
/// NOT emit any diagnostic with id "hooks.matcher.regex" (loss:lossless, warn:false).
/// The event must appear in the lossless section of the report, not the lossy section.
#[test]
fn test_hooks_regex_matcher_no_warn_e2e() {
    let fixture = Path::new("tests/fixtures/claude/hooks_regex_matcher/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Hooks, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // Must NOT have any "hooks.matcher.regex" Warn diagnostic
    let regex_warn_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.matcher.regex") && d.level == DiagLevel::Warn)
        .collect();
    assert!(
        regex_warn_diags.is_empty(),
        "Expected NO 'hooks.matcher.regex' Warn diagnostics for regex passthrough, got: {:?}",
        regex_warn_diags
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // hooks.event.PreToolUse must appear in lossless, not in lossy
    let in_lossless = report
        .lossless
        .iter()
        .any(|s| s == "hooks.event.PreToolUse");
    assert!(
        in_lossless,
        "hooks.event.PreToolUse must be in lossless section; lossless={:?}, lossy={:?}",
        report.lossless,
        report.lossy.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    // hooks.matcher.regex must NOT appear in the lossy section of the report
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.matcher.regex"));
    assert!(
        !in_lossy,
        "hooks.matcher.regex must NOT be in lossy section; lossy={:?}",
        report.lossy.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
