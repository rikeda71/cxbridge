//! Tests for the spec-mandated default output directory logic (gap 16/42).
//!
//! Spec §13 Output Directory Structure:
//!   - Single skill directory → `<input>.converted/`
//!   - `.mcp.json` file       → `./<filename_stem>.converted/`  (written as a dir)
//!   - Project root           → `./.codex-converted/`
//!
//! When `--out` is omitted, `run_convert` must compute the default and pass it
//! to each handler so output lands under the `.converted` tree rather than CWD.

use ccx::cli::default_out_dir;
use ccx::core::ir::Kind;

// ── Unit tests for default_out_dir ───────────────────────────────────────────

/// Skill SKILL.md file input → parent-of-SKILL.md with `.converted` appended.
#[test]
fn test_default_out_dir_skill_file() {
    let path = "/tmp/project/.claude/skills/deploy/SKILL.md";
    let out = default_out_dir(path, &Kind::Skill);
    assert_eq!(
        out, "/tmp/project/.claude/skills/deploy.converted",
        "skill file: parent dir + .converted"
    );
}

/// Skill directory input → path with `.converted` appended.
#[test]
fn test_default_out_dir_skill_dir() {
    let path = "/tmp/project/.claude/skills/deploy";
    let out = default_out_dir(path, &Kind::Skill);
    assert_eq!(
        out, "/tmp/project/.claude/skills/deploy.converted",
        "skill directory: path + .converted"
    );
}

/// `.mcp.json` file input → `<parent_dir>/<stem>.converted`.
#[test]
fn test_default_out_dir_mcp_file() {
    let path = "/tmp/project/.mcp.json";
    let out = default_out_dir(path, &Kind::Mcp);
    // stem of ".mcp.json" is ".mcp" (drop the last extension)
    assert_eq!(
        out, "/tmp/project/.mcp.converted",
        "mcp file: parent + stem.converted"
    );
}

/// Project root directory input → `<path>/.codex-converted`.
#[test]
fn test_default_out_dir_project_root() {
    let path = "/tmp/project";
    let out = default_out_dir(path, &Kind::Memory);
    assert_eq!(
        out, "/tmp/project/.codex-converted",
        "project root dir: path + /.codex-converted"
    );
}

/// Project root with other kinds also gets `.codex-converted`.
#[test]
fn test_default_out_dir_project_root_hooks() {
    let path = "/tmp/project";
    let out = default_out_dir(path, &Kind::Hooks);
    assert_eq!(out, "/tmp/project/.codex-converted");
}

// ── Integration test: c2x without --out lands in .converted tree ─────────────

use std::path::Path;

use ccx::core::{detect::detect_files, mappings::load_mappings, transforms::ConvDir};
use ccx::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

fn skill_lower_opts_no_out() -> LowerOpts {
    LowerOpts {
        out: None,
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

/// c2x on a SKILL.md without --out must place output under `<skill_dir>.converted/`,
/// not under the process CWD.
///
/// This is the exact repro from gap 16/42:
/// the handler must receive a non-CWD out dir so artifacts land relative to the input.
#[test]
fn test_c2x_skill_without_out_uses_converted_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create a skill fixture
    let skill_dir = base.join(".claude").join("skills").join("deploy");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_path,
        "---\nname: deploy\ndescription: Deploy the app\n---\nRun deployment steps.\n",
    )
    .unwrap();

    let skill_path_str = skill_path.to_str().unwrap();

    // Compute the default out dir (as run_convert would)
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let pairs = detect_files(skill_path_str).expect("detect_files should succeed");
    assert_eq!(pairs.len(), 1);
    let (kind, file_path) = &pairs[0];

    let computed_out = default_out_dir(skill_path_str, kind);

    // Must NOT be "." (CWD)
    assert_ne!(
        computed_out, ".",
        "default_out_dir must not return '.' for a skill file"
    );

    // Must contain ".converted"
    assert!(
        computed_out.contains(".converted"),
        "default_out_dir must produce a .converted path, got: {}",
        computed_out
    );

    // Must be relative to the skill dir, not CWD
    let expected_prefix = skill_dir.to_str().unwrap();
    assert!(
        computed_out.starts_with(expected_prefix),
        "default out dir should start with skill_dir={}, got: {}",
        expected_prefix,
        computed_out
    );

    // Actually run lower with the computed out dir to verify files land there
    let opts = LowerOpts {
        out: Some(computed_out.clone()),
        ..skill_lower_opts_no_out()
    };

    let handler = pick_handler(kind, &maps);
    let parsed = handler.parse(file_path).unwrap();
    let ir = handler.lift(&parsed, ConvDir::C2x).unwrap();
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    // The plan must contain at least one file so the loop below is non-vacuous.
    assert!(
        !plan.files.is_empty(),
        "Expected at least one output file from lower, got none"
    );

    // All output paths must start with the computed out dir (not CWD)
    for emit_file in &plan.files {
        let emit_path = Path::new(&emit_file.path);
        // emit_file.path is absolute (starts with computed_out)
        assert!(
            emit_file.path.starts_with(&computed_out),
            "Output file '{}' must be under computed out dir '{}'",
            emit_file.path,
            computed_out
        );
        let _ = emit_path;
    }
}

/// c2x on a .mcp.json file without --out must place output under `<stem>.converted/`,
/// not under CWD.
#[test]
fn test_c2x_mcp_without_out_uses_converted_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let mcp_path = base.join(".mcp.json");
    std::fs::write(
        &mcp_path,
        r#"{"mcpServers":{"fs":{"command":"node","args":["server.js"]}}}"#,
    )
    .unwrap();

    let mcp_path_str = mcp_path.to_str().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let pairs = detect_files(mcp_path_str).expect("detect_files should succeed");
    assert_eq!(pairs.len(), 1);
    let (kind, file_path) = &pairs[0];

    let computed_out = default_out_dir(mcp_path_str, kind);

    assert_ne!(
        computed_out, ".",
        "default_out_dir must not return '.' for .mcp.json"
    );
    assert!(
        computed_out.contains(".converted"),
        "default_out_dir must produce a .converted path for .mcp.json, got: {}",
        computed_out
    );

    let opts = LowerOpts {
        out: Some(computed_out.clone()),
        ..skill_lower_opts_no_out()
    };

    let handler = pick_handler(kind, &maps);
    let parsed = handler.parse(file_path).unwrap();
    let ir = handler.lift(&parsed, ConvDir::C2x).unwrap();
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    // All output paths must be under the computed out dir
    assert!(!plan.files.is_empty(), "Expected at least one output file");
    for emit_file in &plan.files {
        assert!(
            emit_file.path.starts_with(&computed_out),
            "Output file '{}' must be under computed out dir '{}'",
            emit_file.path,
            computed_out
        );
    }
}
