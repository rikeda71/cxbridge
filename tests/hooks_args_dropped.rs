//! Integration test for gap 29/42: hooks.command.args must appear in the
//! `dropped` section of the report, not `lossy`.
//!
//! mappings/hooks.yaml entry `id: hooks.command.args` declares `loss: dropped`.
//! Spec §7 invariant #1: dropped entries must always be listed as dropped.
//! The args field is synthesized into the command string before being dropped,
//! so the diagnostic level must be DiagLevel::Drop, not Warn.

use std::path::Path;

use ccx::core::ir::{DiagLevel, Kind};
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

/// End-to-end: converting a hooks.json with `args` must place `hooks.command.args`
/// in the `dropped` section of the report, not in `lossy`.
///
/// mappings/hooks.yaml: `id: hooks.command.args` with `loss: dropped`.
/// The args are synthesized into `command` (shell-escaped), then dropped.
/// DiagLevel must be Drop so build_report routes it to `dropped`, not `lossy`.
#[test]
fn test_hooks_args_report_section_is_dropped() {
    let fixture = Path::new("tests/fixtures/claude/hooks_args_drop/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Hooks, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // The diagnostic for hooks.command.args must have DiagLevel::Drop
    let args_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.command.args"))
        .collect();
    assert!(
        !args_diags.is_empty(),
        "Expected a diagnostic with id 'hooks.command.args'"
    );
    for diag in &args_diags {
        assert_eq!(
            diag.level,
            DiagLevel::Drop,
            "hooks.command.args diagnostic must be DiagLevel::Drop, got {:?}: {}",
            diag.level,
            diag.message
        );
    }

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // Must appear in dropped
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.command.args"));
    assert!(
        in_dropped,
        "hooks.command.args must be in report.dropped; dropped={:?}",
        report
            .dropped
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );

    // Must NOT appear in lossy
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.command.args"));
    assert!(
        !in_lossy,
        "hooks.command.args must NOT be in report.lossy; lossy={:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
