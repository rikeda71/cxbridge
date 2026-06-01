//! Integration tests for gap 32/42: dropped and lossy fields must appear exactly
//! once each in the report — not duplicated across IR fields, IR diagnostics, and
//! plan diagnostics.
//!
//! Repro: build_report() accumulated entries from three sources:
//!   (1) ir.fields loop for IRField.loss entries
//!   (2) ir.diagnostics pushed by lift() via node.diagnostics.push()
//!   (3) plan.diagnostics pushed by lower() via build_codex_manifest / build_claude_manifest
//!
//! Each dropped field appeared 3× in c2x, causing summary.dropped to report 18
//! instead of 6 for a plugin with 6 dropped fields.

use std::path::Path;

use ccx::core::{ir::Kind, mappings::load_mappings, report::build_report, transforms::ConvDir};
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

/// A Claude plugin.json with exactly 6 dropped fields
/// (lspServers, outputStyles, channels, settings, dependencies, userConfig) must
/// produce report.dropped.len() == 6 and summary.dropped == 6.
///
/// Before the fix each dropped field appeared 3× (from ir.fields, from the
/// DiagLevel::Drop diagnostic pushed by lift_single_field, and from the
/// DiagLevel::Drop diagnostic pushed by build_codex_manifest), yielding 18.
#[test]
fn test_plugin_c2x_six_dropped_fields_no_duplicates() {
    let fixture =
        Path::new("tests/fixtures/claude/plugin_six_dropped_fields/.claude-plugin/plugin.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Plugin, &maps);

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

    // The 6 dropped fields by their mapping IDs.
    let expected_dropped_ids = [
        "plugins.lspServers",
        "plugins.outputStyles",
        "plugins.channels",
        "plugins.settings",
        "plugins.dependencies",
        "plugins.userConfig",
    ];

    // Each dropped field ID must appear exactly once.
    for id in &expected_dropped_ids {
        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some(id))
            .count();
        assert_eq!(
            count,
            1,
            "Dropped field '{}' must appear exactly once in report.dropped, found {} times. \
             Full dropped list: {:?}",
            id,
            count,
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }

    // Total dropped count must equal the number of distinct dropped fields (6).
    // In addition, there may be one entry for the userConfig warn diagnostic
    // (DiagLevel::Warn from node.diagnostics is NOT a dropped entry, but
    // DiagLevel::Drop diagnostics without a matching IRField ID may be present
    // for unknown-field drops). We check that IDs for the 6 known fields are
    // each unique.
    let dropped_ids_for_known: Vec<Option<&str>> = report
        .dropped
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| expected_dropped_ids.contains(&id))
                .unwrap_or(false)
        })
        .map(|e| e.id.as_deref())
        .collect();

    assert_eq!(
        dropped_ids_for_known.len(),
        6,
        "Expected exactly 6 entries in report.dropped for the 6 known dropped fields, \
         found {}. Full dropped: {:?}",
        dropped_ids_for_known.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // Summary dropped count must equal the total number of distinct dropped entries.
    assert_eq!(
        report.dropped.len(),
        6,
        "summary dropped count must be 6; got {}. Full dropped: {:?}",
        report.dropped.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// Dropped fields that do NOT have a secondary warn diagnostic (lspServers,
/// outputStyles, channels, settings, dependencies) must not appear in
/// report.lossy at all.
///
/// Note: plugins.userConfig also has a separate intentional Warn diagnostic
/// about unresolved ${user_config.KEY} references (not a dropped-field
/// duplicate), so it is excluded from this assertion.
#[test]
fn test_plugin_c2x_dropped_fields_without_secondary_warn_not_in_lossy() {
    let fixture =
        Path::new("tests/fixtures/claude/plugin_six_dropped_fields/.claude-plugin/plugin.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Plugin, &maps);

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

    // These 5 dropped fields have no secondary warn diagnostic.
    // They must not appear in report.lossy.
    let pure_dropped_ids = [
        "plugins.lspServers",
        "plugins.outputStyles",
        "plugins.channels",
        "plugins.settings",
        "plugins.dependencies",
    ];

    let spurious_in_lossy: Vec<_> = report
        .lossy
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| pure_dropped_ids.contains(&id))
                .unwrap_or(false)
        })
        .collect();

    assert!(
        spurious_in_lossy.is_empty(),
        "Pure-dropped field IDs must NOT appear in report.lossy; found: {:?}",
        spurious_in_lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
