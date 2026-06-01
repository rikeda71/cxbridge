//! Integration test for gap 41/42: when all hooks within a common event entry
//! are dropped (e.g. only `type:http` hooks present), the event field must be
//! classified as Dropped (or at minimum not appear in `lossless`).
//!
//! Spec §12 invariant: the report must accurately reflect information loss.
//! An event whose entire hook content is dropped carries no semantic content
//! and must NOT be listed as lossless.

use std::path::Path;

use ccx::core::ir::{Kind, Loss};
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

/// End-to-end: a PostToolUse event containing only `type:http` hooks must NOT
/// be listed in `report.lossless` after c2x conversion.
///
/// When all hook items are dropped (http has no Codex equivalent), the event's
/// semantic content is entirely lost. The field must be classified as
/// `Loss::Dropped` so `build_report` routes it to `dropped`, not `lossless`.
#[test]
fn test_event_with_all_hooks_dropped_not_in_lossless() {
    let fixture = Path::new("tests/fixtures/claude/hooks_all_http_dropped/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Hooks, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // The event field must be classified as Dropped (not Lossless)
    let field = ir
        .fields
        .get("hooks.event.PostToolUse")
        .expect("hooks.event.PostToolUse must exist in IR");
    assert_eq!(
        field.loss,
        Loss::Dropped,
        "hooks.event.PostToolUse must be Loss::Dropped when all hooks are dropped; got {:?}",
        field.loss
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // Must NOT appear in lossless
    let in_lossless = report
        .lossless
        .iter()
        .any(|id| id == "hooks.event.PostToolUse");
    assert!(
        !in_lossless,
        "hooks.event.PostToolUse must NOT be in report.lossless when all hooks are dropped; \
         lossless={:?}",
        report.lossless
    );

    // Must appear in dropped
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.event.PostToolUse"));
    assert!(
        in_dropped,
        "hooks.event.PostToolUse must appear in report.dropped; dropped={:?}",
        report
            .dropped
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// Verify that when at least one hook survives (command type), the event remains
/// lossless — only the all-dropped case triggers the dropped classification.
#[test]
fn test_event_with_surviving_hook_remains_lossless() {
    let fixture = Path::new("tests/fixtures/claude/hooks_wildcard/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Hooks, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // Stop has a command hook → must remain lossless
    let field = ir
        .fields
        .get("hooks.event.Stop")
        .expect("hooks.event.Stop must exist in IR");
    assert_eq!(
        field.loss,
        Loss::Lossless,
        "hooks.event.Stop must remain Loss::Lossless when command hooks survive"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // The surviving event must appear in report.lossless
    let in_lossless = report.lossless.iter().any(|id| id == "hooks.event.Stop");
    assert!(
        in_lossless,
        "hooks.event.Stop must appear in report.lossless when command hooks survive; \
         lossless={:?}",
        report.lossless
    );
}
