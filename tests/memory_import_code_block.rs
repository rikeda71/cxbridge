//! Integration test for gap 42/42: @import detection in lift() must not emit
//! false-positive lossy warnings for @-lines that appear inside code fences.
//!
//! Spec/mappings memory.import-syntax notes: "@-lines inside code fences are excluded".
//! lower() already skips expanding code-fence imports via stateful fence
//! tracking. lift() must apply the same logic so the conversion report does
//! not falsely list "memory.import-syntax" as a lossy entry when every @-line
//! is inside a code block.

use std::path::Path;

use ccx::core::{mappings::load_mappings, report::build_report, transforms::ConvDir};
use ccx::handlers::memory::MemoryHandler;
use ccx::handlers::{Handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

fn make_handler() -> MemoryHandler {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    MemoryHandler {
        map: maps["memory"].clone(),
    }
}

fn default_opts(out_dir: &str) -> LowerOpts {
    LowerOpts {
        out: Some(out_dir.to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    }
}

/// lift() must not add "memory.import-syntax" to the IR when every @-line is
/// inside a code fence.  The conversion report must therefore have zero lossy
/// entries for this input.
#[test]
fn test_lift_no_false_positive_lossy_for_code_block_import() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude_md = dir.path().join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        "# Instructions\n\n```\n@some/file.md is not an import\n```\n\nContent.\n",
    )
    .unwrap();

    let h = make_handler();
    let parsed = h.parse(&claude_md).unwrap();
    let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

    // The IR must NOT contain "memory.import-syntax" — no real @import exists.
    assert!(
        !ir.fields.contains_key("memory.import-syntax"),
        "lift() falsely flagged a code-block @-line as an import; \
         IR fields: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );

    // The report must therefore have zero lossy entries.
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_opts(out_dir.path().to_str().unwrap());
    let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();
    let report = build_report(&ir, &plan);

    assert!(
        report.lossy.is_empty(),
        "Expected zero lossy entries in report when @-lines are only inside \
         code fences; got {} lossy: {:?}",
        report.lossy.len(),
        report.lossy
    );

    // The output file must still contain the @-line verbatim (not expanded).
    let agents_md = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("AGENTS.md"))
        .expect("Expected AGENTS.md in output");
    assert!(
        agents_md.content.contains("@some/file.md"),
        "Code-block @-line must be preserved verbatim in output; \
         got:\n{}",
        agents_md.content
    );
}

/// Verify the case of multiple code fences each containing @-lines: no false
/// positives across any of them.
#[test]
fn test_lift_no_false_positive_multiple_code_blocks() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude_md = dir.path().join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        concat!(
            "# Header\n\n",
            "```bash\n@rules/foo.md\n```\n\n",
            "Some text.\n\n",
            "```\n@another/bar.md\n```\n\n",
            "End.\n"
        ),
    )
    .unwrap();

    let h = make_handler();
    let parsed = h.parse(&claude_md).unwrap();
    let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

    assert!(
        !ir.fields.contains_key("memory.import-syntax"),
        "lift() must not flag @-lines inside any code fence; \
         fields: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );
}

/// A real @import OUTSIDE a code block must still be detected by lift().
/// This ensures the fix doesn't suppress legitimate import detection.
#[test]
fn test_lift_still_detects_real_import_outside_code_block() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude_md = dir.path().join("CLAUDE.md");
    // Mix: one @-line inside a fence (should be excluded), one outside (real import).
    std::fs::write(
        &claude_md,
        "# Header\n\n```\n@not-an-import.md\n```\n\n@real/import.md\n\nEnd.\n",
    )
    .unwrap();

    let h = make_handler();
    let parsed = h.parse(&claude_md).unwrap();
    let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

    assert!(
        ir.fields.contains_key("memory.import-syntax"),
        "lift() must detect a real @import that is outside a code fence; \
         fields: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );
}
