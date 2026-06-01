//! Integration tests for gap 38/42: MCP dropped+warn fields must appear in
//! report.dropped exactly once and must NOT appear in report.lossy.
//!
//! Fields under test (c2x):
//!   mcp.alwaysLoad, mcp.headersHelper — both have loss:dropped + warn:true
//!   in mappings/mcp.yaml.
//!
//! Fields under test (x2c):
//!   mcp.enabled — a disabled server (enabled=false) must produce exactly one
//!   dropped entry for mcp.enabled; the internal __disabled bookkeeping field
//!   must not surface in the user-facing report.
//!
//! The full pipeline (lift → lower → build_report) must not produce any entry
//! in lossy[] for these fields and must produce exactly one entry each in
//! dropped[].  Before the fix, three sources each pushed an entry:
//!   (1) IRField.loss==Dropped scanned by build_report
//!   (2) DiagLevel::Drop pushed by lift (the spurious warn:true path)
//!   (3) DiagLevel::Drop pushed by lower (the _ => arm fallthrough)
//! yielding a summary of "3 dropped" for a single field.

use std::path::Path;

use ccx::core::{ir::Kind, mappings::load_mappings, report::build_report, transforms::ConvDir};
use ccx::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";
const FIXTURE: &str = "tests/fixtures/claude/mcp_dropped_warn_fields/.mcp.json";

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

/// alwaysLoad (loss:dropped + warn:true) must appear exactly once in
/// report.dropped and must not appear in report.lossy.
#[test]
fn test_mcp_c2x_always_load_dropped_once_not_in_lossy() {
    let fixture = Path::new(FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

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

    // alwaysLoad must appear exactly once in dropped.
    let always_load_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.alwaysLoad"))
        .count();
    assert_eq!(
        always_load_count,
        1,
        "mcp.alwaysLoad must appear exactly once in report.dropped, found {} times. \
         Full dropped: {:?}",
        always_load_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // alwaysLoad must not appear in lossy.
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("mcp.alwaysLoad"));
    assert!(
        !in_lossy,
        "mcp.alwaysLoad must NOT appear in report.lossy; lossy: {:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// headersHelper (loss:dropped + warn:true) must appear exactly once in
/// report.dropped and must not appear in report.lossy.
#[test]
fn test_mcp_c2x_headers_helper_dropped_once_not_in_lossy() {
    let fixture = Path::new(FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

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

    // headersHelper must appear exactly once in dropped.
    let headers_helper_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.headersHelper"))
        .count();
    assert_eq!(
        headers_helper_count,
        1,
        "mcp.headersHelper must appear exactly once in report.dropped, found {} times. \
         Full dropped: {:?}",
        headers_helper_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // headersHelper must not appear in lossy.
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("mcp.headersHelper"));
    assert!(
        !in_lossy,
        "mcp.headersHelper must NOT appear in report.lossy; lossy: {:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// The full report summary for the minimal fixture (one server with
/// alwaysLoad + headersHelper) must show exactly 2 dropped entries —
/// one per field, each appearing once.
#[test]
fn test_mcp_c2x_summary_counts_two_dropped_not_six() {
    let fixture = Path::new(FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

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

    // The fixture has exactly 2 dropped fields: mcp.alwaysLoad and mcp.headersHelper.
    // The known dropped field IDs (ignoring any transport-type or mcp.format entries).
    let known_dropped_ids = ["mcp.alwaysLoad", "mcp.headersHelper"];
    let known_dropped_count = report
        .dropped
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| known_dropped_ids.contains(&id))
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        known_dropped_count,
        2,
        "Expected exactly 2 dropped entries for the 2 known dropped fields, \
         found {}. Full dropped: {:?}",
        known_dropped_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

const X2C_FIXTURE: &str = "tests/fixtures/codex/mcp_disabled_server/config.toml";

/// x2c: a disabled server (enabled=false) must produce exactly one
/// dropped entry for mcp.enabled in the report.  Before the fix, it appeared
/// twice — once from ir.diagnostics (pushed by lift) and once from
/// plan.diagnostics (pushed by lower_x2c) — plus a third __disabled entry
/// from ir.fields, yielding "3 dropped".
#[test]
fn test_mcp_x2c_enabled_false_dropped_once() {
    let fixture = Path::new(X2C_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // mcp.enabled must appear exactly once in dropped.
    let enabled_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.enabled"))
        .count();
    assert_eq!(
        enabled_count,
        1,
        "mcp.enabled must appear exactly once in report.dropped, found {} times. \
         Full dropped: {:?}",
        enabled_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // __disabled is an internal bookkeeping key and must not surface in the report.
    let disabled_in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("__disabled"));
    assert!(
        !disabled_in_dropped,
        "__disabled must not appear in report.dropped (it is an internal field); \
         Full dropped: {:?}",
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// x2c: the total dropped count for a single disabled server must be exactly 1,
/// not 3 (the pre-fix value).
#[test]
fn test_mcp_x2c_enabled_false_total_dropped_is_one() {
    let fixture = Path::new(X2C_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    assert_eq!(
        report.dropped.len(),
        1,
        "A single disabled server must produce exactly 1 dropped entry, \
         found {}. Full dropped: {:?}",
        report.dropped.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}
