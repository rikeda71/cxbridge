//! Integration test for gap 31/42: marketplace.json c2x npm source type must
//! emit a DiagLevel::Drop diagnostic and not silently set source to null.
//!
//! mappings/plugins.yaml entry:
//!   plugins.marketplace.plugins.source  notes: 'npm → no Codex equivalent (dropped + warn)'
//!
//! Spec invariant §7: dropped entries must ALWAYS be listed in the report;
//! silent discard is prohibited.

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

/// An npm-source plugin entry must produce a DiagLevel::Drop diagnostic
/// and must NOT leave a null source in the output.
#[test]
fn test_marketplace_c2x_npm_source_emits_drop_diagnostic() {
    let fixture =
        Path::new("tests/fixtures/claude/marketplace_npm_source/.claude-plugin/plugin.json");
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

    // (1) A DiagLevel::Drop diagnostic with id "plugins.marketplace.plugins.source" must be present.
    let drop_diags: Vec<_> = plan
        .diagnostics
        .iter()
        .filter(|d| {
            d.level == DiagLevel::Drop
                && d.id.as_deref() == Some("plugins.marketplace.plugins.source")
        })
        .collect();

    assert!(
        !drop_diags.is_empty(),
        "Expected a DiagLevel::Drop diagnostic with id \
         'plugins.marketplace.plugins.source' for the npm source entry, \
         but none was found. diagnostics: {:?}",
        plan.diagnostics
    );

    // (2) The diagnostic message must mention 'npm' and the plugin name.
    let msg = &drop_diags[0].message;
    assert!(
        msg.to_lowercase().contains("npm"),
        "Drop diagnostic message must mention 'npm', got: {}",
        msg
    );
    assert!(
        msg.contains("plugin-c"),
        "Drop diagnostic message must contain the plugin name 'plugin-c', got: {}",
        msg
    );

    // (3) The generated marketplace.json must not contain a null source for plugin-c.
    let marketplace_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("marketplace.json"))
        .expect("Expected marketplace.json in c2x output");

    let content: serde_json::Value = serde_json::from_str(&marketplace_file.content)
        .expect("marketplace.json must be valid JSON");

    let plugins = content["plugins"]
        .as_array()
        .expect("plugins must be array");
    let plugin_c = plugins
        .iter()
        .find(|p| p["name"].as_str() == Some("plugin-c"))
        .expect("plugin-c must be present in output");

    assert!(
        plugin_c.get("source").is_none_or(|s| !s.is_null()),
        "plugin-c source must not be null; found: {}",
        plugin_c
    );

    // (4) The drop diagnostic must appear in report.dropped.
    let report = build_report(&ir, &plan);
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("plugins.marketplace.plugins.source"));
    assert!(
        in_dropped,
        "'plugins.marketplace.plugins.source' must appear in report.dropped; \
         dropped={:?}",
        report
            .dropped
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
