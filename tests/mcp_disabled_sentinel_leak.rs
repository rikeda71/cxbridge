//! Regression tests for gap 39/42: internal '__disabled' sentinel field must
//! not leak into the user-facing conversion report.
//!
//! The handler previously inserted an IRField with id "__disabled" as a
//! bookkeeping sentinel when a server had enabled=false.  build_report
//! iterates all IRFields, so the sentinel appeared in the user-facing
//! dropped list as "✕ __disabled dropped: __disabled has no Codex equivalent".
//!
//! The fix:
//!   - The handler must NOT insert any IRField with id "__disabled" into
//!     child.fields for a disabled server.
//!   - Disabled servers are detected via a DiagLevel::Drop diagnostic with
//!     id "mcp.enabled" (which is the user-meaningful field id).
//!   - The report must show exactly one dropped entry with id "mcp.enabled"
//!     and must not show any entry with id "__disabled".

use std::path::Path;

use ccx::core::{ir::Kind, mappings::load_mappings, report::build_report, transforms::ConvDir};
use ccx::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";
const FIXTURE: &str = "tests/fixtures/codex/mcp_disabled_server/config.toml";

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

/// After lift, the child IRNode for a disabled server must have NO field
/// with id "__disabled".  The only record of the disabled state must be
/// a Drop diagnostic with id "mcp.enabled".
#[test]
fn test_disabled_server_ir_has_no_disabled_sentinel_field() {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

    let parsed = handler
        .parse(Path::new(FIXTURE))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // The fixture has exactly one server (s) with enabled=false.
    assert_eq!(ir.children.len(), 1, "fixture must have exactly one server");
    let child = &ir.children[0];

    // No IRField with id "__disabled" must be present.
    assert!(
        !child.fields.contains_key("__disabled"),
        "child.fields must NOT contain a '__disabled' sentinel field; \
         found fields: {:?}",
        child.fields.keys().collect::<Vec<_>>()
    );

    // The disabled state must be recorded as a Drop diagnostic with id "mcp.enabled".
    let has_enabled_diag = child.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("mcp.enabled") && d.level == ccx::core::ir::DiagLevel::Drop
    });
    assert!(
        has_enabled_diag,
        "child.diagnostics must contain a Drop diagnostic with id 'mcp.enabled'; \
         found diagnostics: {:?}",
        child
            .diagnostics
            .iter()
            .map(|d| (d.level.clone(), d.id.as_deref().unwrap_or("<none>")))
            .collect::<Vec<_>>()
    );
}

/// The full pipeline report for an enabled=false server must contain exactly
/// one dropped entry with id "mcp.enabled" and zero entries with id "__disabled".
#[test]
fn test_disabled_server_report_shows_mcp_enabled_not_disabled_sentinel() {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

    let parsed = handler
        .parse(Path::new(FIXTURE))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // The "__disabled" sentinel must never appear in the user-facing dropped list.
    let disabled_sentinel_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("__disabled"))
        .count();
    assert_eq!(
        disabled_sentinel_count,
        0,
        "'__disabled' is an internal sentinel and must NOT appear in report.dropped; \
         found {} occurrence(s). Full dropped: {:?}",
        disabled_sentinel_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // "mcp.enabled" must appear exactly once.
    let enabled_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.enabled"))
        .count();
    assert_eq!(
        enabled_count,
        1,
        "'mcp.enabled' must appear exactly once in report.dropped; \
         found {} occurrence(s). Full dropped: {:?}",
        enabled_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// The total dropped count for a single disabled server must be exactly 1.
/// Before the fix it was 3: one from __disabled IRField, one from ir.diagnostics,
/// one from plan.diagnostics.
#[test]
fn test_disabled_server_total_dropped_exactly_one() {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Mcp, &maps);

    let parsed = handler
        .parse(Path::new(FIXTURE))
        .expect("parse should succeed");
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
        "A single disabled server must produce exactly 1 dropped entry total; \
         found {}. Full dropped: {:?}",
        report.dropped.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}
