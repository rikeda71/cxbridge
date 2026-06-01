//! Integration tests for gap 34/42: WebFetch deny domains in permissions.deny
//! must appear in config.toml under [permissions.default.network.domains] with
//! value "deny", and a Warn diagnostic must be emitted with id
//! "settings.permissions.deny.webfetch".
//!
//! Spec invariant #1: loss:dropped (and lossy) entries must always appear in
//! the report — silent discard is forbidden.

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

/// End-to-end: permissions.deny with WebFetch(domain:...) entries must
/// produce config.toml with those domains set to "deny" under
/// [permissions.default.network.domains], and emit a Warn diagnostic.
#[test]
fn test_webfetch_deny_domains_appear_in_config_toml() {
    let fixture = Path::new("tests/fixtures/claude/webfetch_deny/settings.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Settings, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // config.toml must be generated
    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml to be generated; files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &config_toml.unwrap().content;

    // bad.com must appear with value "deny"
    assert!(
        content.contains("\"bad.com\" = \"deny\"") || content.contains("bad.com = \"deny\""),
        "Expected bad.com = \"deny\" in config.toml; got:\n{}",
        content
    );

    // evil.net must appear with value "deny"
    assert!(
        content.contains("\"evil.net\" = \"deny\"") || content.contains("evil.net = \"deny\""),
        "Expected evil.net = \"deny\" in config.toml; got:\n{}",
        content
    );

    // The [permissions.default.network.domains] section must be present
    assert!(
        content.contains("[permissions"),
        "Expected [permissions...] section in config.toml; got:\n{}",
        content
    );
}

/// End-to-end: a Warn diagnostic with id "settings.permissions.deny.webfetch"
/// must be emitted when WebFetch deny domains are present.
#[test]
fn test_webfetch_deny_domains_emit_warn_diagnostic() {
    let fixture = Path::new("tests/fixtures/claude/webfetch_deny/settings.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Settings, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // A Warn diagnostic with id "settings.permissions.deny.webfetch" must exist
    let webfetch_deny_diags: Vec<_> = plan
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("settings.permissions.deny.webfetch"))
        .collect();

    assert!(
        !webfetch_deny_diags.is_empty(),
        "Expected a diagnostic with id 'settings.permissions.deny.webfetch'; \
         diagnostics: {:?}",
        plan.diagnostics
            .iter()
            .map(|d| (d.id.as_deref().unwrap_or("<none>"), &d.message))
            .collect::<Vec<_>>()
    );

    for diag in &webfetch_deny_diags {
        assert_eq!(
            diag.level,
            DiagLevel::Warn,
            "settings.permissions.deny.webfetch diagnostic must be DiagLevel::Warn, got {:?}",
            diag.level
        );
    }
}

/// End-to-end: the report must include the deny.webfetch diagnostic so users
/// can see what happened (spec invariant — no silent discard).
#[test]
fn test_webfetch_deny_domains_visible_in_report() {
    let fixture = Path::new("tests/fixtures/claude/webfetch_deny/settings.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Settings, &maps);

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

    // The warn diagnostic must be surfaced via the plan diagnostics that
    // build_report aggregates. Check that the deny webfetch diagnostic is
    // NOT silently discarded — it should appear in lossy or warnings.
    let all_diag_ids: Vec<_> = plan
        .diagnostics
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    assert!(
        all_diag_ids.contains(&"settings.permissions.deny.webfetch"),
        "settings.permissions.deny.webfetch must appear in plan diagnostics (not silently dropped); \
         ids: {:?}",
        all_diag_ids
    );

    // It should also appear in report.lossy (Warn level → lossy bucket)
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("settings.permissions.deny.webfetch"));
    assert!(
        in_lossy,
        "settings.permissions.deny.webfetch must appear in report.lossy; \
         lossy={:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
