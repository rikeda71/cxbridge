//! Integration tests for gap 37/42: subagents fields with loss:dropped + warn:true
//! must appear in report.dropped exactly once and must NOT appear in report.lossy.
//!
//! The four fields are: disallowedTools, maxTurns, background, isolation.
//! All have loss:dropped + warn:true in mappings/subagents.yaml.
//!
//! The full pipeline (lift → lower → build_report) must not produce any entry
//! in lossy[] for these fields and must produce exactly one entry each in dropped[].

use std::path::Path;

use ccx::core::{ir::Kind, mappings::load_mappings, report::build_report, transforms::ConvDir};
use ccx::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";
const FIXTURE: &str = "tests/fixtures/claude/agents/dropped_warn_fields.md";

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

/// Each loss:dropped + warn:true subagents field must appear exactly once in
/// report.dropped when the full pipeline (lift → lower → build_report) is run.
#[test]
fn test_subagents_dropped_warn_fields_appear_once_in_dropped() {
    let fixture = Path::new(FIXTURE);
    assert!(fixture.exists(), "Fixture {FIXTURE} must exist");

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Subagent, &maps);

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

    let dropped_warn_ids = [
        "subagents.maxTurns",
        "subagents.background",
        "subagents.isolation",
        "subagents.disallowedTools",
    ];

    for field_id in &dropped_warn_ids {
        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some(field_id))
            .count();
        assert_eq!(
            count,
            1,
            "{field_id} must appear exactly once in report.dropped; found {count} times. \
             Full dropped: {:?}",
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }
}

/// Each loss:dropped + warn:true subagents field must NOT appear in report.lossy.
#[test]
fn test_subagents_dropped_warn_fields_not_in_lossy() {
    let fixture = Path::new(FIXTURE);
    assert!(fixture.exists(), "Fixture {FIXTURE} must exist");

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = pick_handler(&Kind::Subagent, &maps);

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

    let dropped_warn_ids = [
        "subagents.maxTurns",
        "subagents.background",
        "subagents.isolation",
        "subagents.disallowedTools",
    ];

    let spurious_in_lossy: Vec<_> = report
        .lossy
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| dropped_warn_ids.contains(&id))
                .unwrap_or(false)
        })
        .collect();

    assert!(
        spurious_in_lossy.is_empty(),
        "loss:dropped + warn:true subagents fields must NOT appear in report.lossy; found: {:?}",
        spurious_in_lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
