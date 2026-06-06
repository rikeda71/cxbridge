mod common;
use common::*;

use std::path::Path;

use cxbridge::core::{
    detect::detect, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use cxbridge::handlers::memory::MemoryHandler;
use cxbridge::handlers::{pick_handler, Handler};

fn make_handler() -> MemoryHandler {
    let maps = load_mappings();
    MemoryHandler {
        map: maps["memory"].clone(),
    }
}

/// CLAUDE.md c2x: AGENTS.md is generated and its content is preserved.
#[test]
fn test_memory_c2x_basic() {
    let memory_path = "tests/fixtures/claude/CLAUDE.md";
    assert!(
        Path::new(memory_path).exists(),
        "Fixture {} must exist",
        memory_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(memory_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Memory);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(memory_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, cxbridge::core::ir::Kind::Memory);

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let agents_md = plan.files.iter().find(|f| f.path.ends_with("AGENTS.md"));
    assert!(agents_md.is_some(), "Expected AGENTS.md in output");
    let content = &agents_md.unwrap().content;
    assert!(
        content.contains("Project Instructions"),
        "Expected content preserved in AGENTS.md"
    );
}

/// AGENTS.md x2c: CLAUDE.md is generated.
#[test]
fn test_memory_x2c_basic() {
    let memory_path = "tests/fixtures/codex/AGENTS.md";
    assert!(
        Path::new(memory_path).exists(),
        "Fixture {} must exist",
        memory_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(memory_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Memory);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(memory_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let claude_md = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
    assert!(claude_md.is_some(), "Expected CLAUDE.md in output");
    let content = &claude_md.unwrap().content;
    assert!(
        content.contains("Agent Instructions"),
        "Expected content preserved in CLAUDE.md"
    );
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
    let opts = default_lower_opts_skill(out_dir.path().to_str().unwrap());
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
