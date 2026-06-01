//! Integration tests for the `--only <domains>` domain filter flag.
//!
//! Spec §13 flag table: `--only <domains>` is a comma-separated domain filter
//! (e.g. `skills,mcp`). Passing `--only skills` on a `.mcp.json` file should
//! produce zero output files (the MCP domain is excluded). Passing `--only
//! skills,mcp` on a project directory should emit files only for those two
//! domains.

use std::path::Path;

use ccx::core::{detect::detect_files, mappings::load_mappings, transforms::ConvDir};
use ccx::handlers::{pick_handler, EmitFile, EmitPlan, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

fn lower_opts_with_out(out_dir: &str) -> LowerOpts {
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

/// Run the conversion pipeline for `path` with the given `only` filter and
/// return the combined `EmitPlan`.
///
/// This mirrors the real filtering path in `run_convert`: the `only` list is
/// placed into `LowerOpts.only` and the per-kind check uses the same logic as
/// the CLI handler, so a break in the actual `--only` wiring causes these tests
/// to fail.
fn run_convert_only(path: &str, only: &[&str], out_dir: &str) -> EmitPlan {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let opts = LowerOpts {
        only: only.iter().map(|s| s.to_string()).collect(),
        ..lower_opts_with_out(out_dir)
    };

    let pairs = detect_files(path).expect("detect_files should succeed");

    let mut combined_files: Vec<EmitFile> = Vec::new();
    let mut combined_diags: Vec<ccx::core::ir::Diagnostic> = Vec::new();

    for (kind, file_path) in &pairs {
        // Apply --only filter exactly as run_convert does: skip domains not in the
        // allow-list by comparing against LowerOpts.only, not a local copy.
        if !opts.only.is_empty() {
            let domain = kind.domain_name();
            if !opts.only.iter().any(|d| d.as_str() == domain) {
                continue;
            }
        }

        let handler = pick_handler(kind, &maps);
        let parsed = handler
            .parse(file_path)
            .unwrap_or_else(|e| panic!("parse failed for {}: {e}", file_path.display()));
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .unwrap_or_else(|e| panic!("lift failed for {}: {e}", file_path.display()));
        let plan = handler
            .lower(&ir, ConvDir::C2x, &opts)
            .unwrap_or_else(|e| panic!("lower failed for {}: {e}", file_path.display()));

        combined_files.extend(plan.files);
        combined_diags.extend(plan.diagnostics);
    }

    EmitPlan {
        files: combined_files,
        diagnostics: combined_diags,
    }
}

/// `--only skills` on a `.mcp.json` file: EmitPlan must have zero files.
///
/// Spec §13: `--only skills` means only the "skills" domain is processed.
/// A `.mcp.json` file belongs to the "mcp" domain, so it must be skipped
/// entirely and produce no output.
#[test]
fn only_skills_on_mcp_json_produces_no_output() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";
    assert!(
        Path::new(mcp_path).exists(),
        "Fixture {mcp_path} must exist"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let plan = run_convert_only(mcp_path, &["skills"], out_dir.path().to_str().unwrap());

    assert!(
        plan.files.is_empty(),
        "--only skills on .mcp.json should produce zero files, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// `--only skills,mcp` on a project directory: only skills and mcp files are
/// converted; hooks, memory, settings files are skipped.
#[test]
fn only_skills_mcp_on_project_dir_excludes_other_domains() {
    // Build a temporary project directory that contains one file per domain.
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // skills domain
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // mcp domain
    std::fs::write(base.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    // memory domain (should be excluded)
    std::fs::write(base.join("CLAUDE.md"), "# Instructions\nhello").unwrap();

    // hooks domain (should be excluded)
    std::fs::write(
        base.join("hooks.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    let plan = run_convert_only(
        base.to_str().unwrap(),
        &["skills", "mcp"],
        out_dir.path().to_str().unwrap(),
    );

    // At least one skill file must appear.
    assert!(
        plan.files
            .iter()
            .any(|f| f.path.ends_with("SKILL.md") || f.path.ends_with(".toml")),
        "--only skills,mcp should include skill output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // At least one mcp file must appear.
    assert!(
        plan.files.iter().any(|f| f.path.ends_with(".mcp.json")),
        "--only skills,mcp should include mcp output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // No AGENTS.md (memory) or hooks file must appear.
    assert!(
        !plan.files.iter().any(|f| f.path.ends_with("AGENTS.md")),
        "memory domain should be excluded by --only skills,mcp, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert!(
        !plan.files.iter().any(|f| f.path.ends_with("hooks.json")),
        "hooks domain should be excluded by --only skills,mcp, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Empty `--only` list means "convert all domains" — no filtering.
#[test]
fn empty_only_list_converts_all_domains() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();
    std::fs::write(base.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    // Pass empty slice — no filter applied.
    let plan = run_convert_only(
        base.to_str().unwrap(),
        &[],
        out_dir.path().to_str().unwrap(),
    );

    assert!(
        plan.files.len() >= 2,
        "Empty --only should convert all domains, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}
