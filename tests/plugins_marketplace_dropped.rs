//! Integration test for gap 30/42: marketplace.json c2x must drop top-level
//! Claude-only fields (owner, allowCrossMarketplaceDependenciesOn,
//! forceRemoveDeletedPlugins) and report them as DiagLevel::Drop.
//!
//! mappings/plugins.yaml entries:
//!   - plugins.marketplace.owner                        direction:claude_to_codex loss:dropped
//!   - plugins.marketplace.allowCrossMarketplaceDependenciesOn  direction:claude_to_codex loss:dropped
//!   - plugins.marketplace.forceRemoveDeletedPlugins    direction:claude_to_codex loss:dropped
//!
//! Spec §7 invariant #1: dropped entries must be absent from output and listed
//! in the report.

use std::path::Path;

use ccx::core::{
    ir::{DiagLevel, Kind},
    mappings::load_mappings,
    report::build_report,
    transforms::ConvDir,
};
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

/// The three Claude-only top-level fields must be absent from the generated
/// marketplace.json and must appear in the report.dropped section.
#[test]
fn test_marketplace_c2x_dropped_top_level_fields_absent_from_output() {
    let fixture =
        Path::new("tests/fixtures/claude/marketplace_dropped_fields/.claude-plugin/plugin.json");
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

    // Find the generated marketplace.json content
    let marketplace_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("marketplace.json"))
        .expect("Expected marketplace.json in c2x output");

    let content: serde_json::Value = serde_json::from_str(&marketplace_file.content)
        .expect("marketplace.json must be valid JSON");

    // (1) The three Claude-only fields must NOT be present in the output
    assert!(
        content.get("owner").is_none(),
        "owner must be absent from c2x marketplace.json output, got: {}",
        content
    );
    assert!(
        content.get("allowCrossMarketplaceDependenciesOn").is_none(),
        "allowCrossMarketplaceDependenciesOn must be absent from c2x marketplace.json output"
    );
    assert!(
        content.get("forceRemoveDeletedPlugins").is_none(),
        "forceRemoveDeletedPlugins must be absent from c2x marketplace.json output"
    );

    // (2) The plugins[] array must still be present (other content preserved)
    assert!(
        content.get("plugins").is_some(),
        "plugins array must remain in c2x marketplace.json output"
    );

    // (3) Three Drop diagnostics must be emitted for the dropped fields
    let drop_ids: Vec<Option<&str>> = plan
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagLevel::Drop)
        .map(|d| d.id.as_deref())
        .collect();

    let expected_ids = [
        "plugins.marketplace.owner",
        "plugins.marketplace.allowCrossMarketplaceDependenciesOn",
        "plugins.marketplace.forceRemoveDeletedPlugins",
    ];

    for id in &expected_ids {
        assert!(
            drop_ids.contains(&Some(id)),
            "Expected DiagLevel::Drop diagnostic with id '{}'; found drop ids: {:?}",
            id,
            drop_ids
        );
    }

    // (4) All three must appear in report.dropped
    let report = build_report(&ir, &plan);

    for id in &expected_ids {
        let in_dropped = report.dropped.iter().any(|e| e.id.as_deref() == Some(id));
        assert!(
            in_dropped,
            "'{}' must appear in report.dropped; dropped={:?}",
            id,
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }
}
