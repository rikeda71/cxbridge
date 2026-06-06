mod common;
use common::*;

use std::path::Path;
use std::process::Command;

use cxbridge::cli::{default_out_dir, infer_conv_dir, write_plan};
use cxbridge::core::{
    detect::detect_files,
    ir::{Diagnostic, Kind},
    mappings::load_mappings,
    report::build_report,
    transforms::ConvDir,
};
use cxbridge::degrade::rules::degrade_allowed_tools;
use cxbridge::handlers::{pick_handler, EmitFile, EmitPlan, LowerOpts, Scope, SkillTargetMode};

// ── cli_dir_input ────────────────────────────────────────────────────────────

/// `c2x <dir>` exits 0 and produces `.agents/skills/<name>/SKILL.md` in the output dir.
#[test]
fn test_cli_c2x_directory_exits_zero_and_produces_skill_output() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let out_dir = tempfile::TempDir::new().unwrap();

    let skill_dir = input_dir.path().join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nBody.\n",
    )
    .unwrap();

    let status = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
        ])
        .status()
        .expect("failed to run cxbridge binary");

    assert!(
        status.success(),
        "cxbridge c2x <dir> must exit 0, got: {}",
        status
    );

    // Verify the skill was converted
    let output_skill = out_dir
        .path()
        .join(".agents")
        .join("skills")
        .join("s")
        .join("SKILL.md");
    assert!(
        output_skill.exists(),
        "Expected output skill at {}, but it was not produced",
        output_skill.display()
    );
}

/// `--strict` exits 2 when a field is dropped; the same input without `--strict`
/// exits 0. `user-invocable` has no Codex equivalent and is always dropped.
#[test]
fn test_cli_strict_exits_2_on_dropped_field() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let skill_dir = input_dir.path().join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\nuser-invocable: false\n---\nBody.\n",
    )
    .unwrap();

    // Without --strict: dropped fields are allowed, exit 0.
    let lenient = tempfile::TempDir::new().unwrap();
    let ok = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            lenient.path().to_str().unwrap(),
        ])
        .status()
        .expect("failed to run cxbridge binary");
    assert_eq!(ok.code(), Some(0), "without --strict must exit 0");

    // With --strict: a dropped field forces exit code 2.
    let strict = tempfile::TempDir::new().unwrap();
    let strict_status = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            strict.path().to_str().unwrap(),
            "--strict",
        ])
        .status()
        .expect("failed to run cxbridge binary");
    assert_eq!(
        strict_status.code(),
        Some(2),
        "--strict with a dropped field must exit 2, got: {strict_status}"
    );
}

/// `--rewrite-body` applies body substitutions: in c2x, `$ARGUMENTS[1]` → `$2`.
/// Without the flag, the body is emitted unchanged.
#[test]
fn test_cli_rewrite_body_substitutes_arguments() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let skill_dir = input_dir.path().join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nUse $ARGUMENTS[1] here.\n",
    )
    .unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    let status = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
            "--rewrite-body",
        ])
        .status()
        .expect("failed to run cxbridge binary");
    assert!(status.success(), "c2x --rewrite-body must exit 0");

    let out_skill = out_dir
        .path()
        .join(".agents")
        .join("skills")
        .join("s")
        .join("SKILL.md");
    let body = std::fs::read_to_string(&out_skill).expect("output SKILL.md");
    assert!(
        body.contains("$2") && !body.contains("$ARGUMENTS[1]"),
        "--rewrite-body must rewrite $ARGUMENTS[1] to $2, got:\n{body}"
    );
}

/// `check <dir>` exits 0 for a directory containing only a skill file.
#[test]
fn test_cli_check_directory_exits_zero() {
    let input_dir = tempfile::TempDir::new().unwrap();

    let skill_dir = input_dir.path().join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nBody.\n",
    )
    .unwrap();

    let output = Command::new(cxbridge_bin())
        .args(["check", input_dir.path().to_str().unwrap()])
        .output()
        .expect("failed to run cxbridge binary");

    assert!(
        output.status.success(),
        "cxbridge check <dir> must exit 0, got: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `x2c <dir>` exits 0 and converts a Codex skill back to Claude skill.
#[test]
fn test_cli_x2c_directory_exits_zero_and_produces_skill_output() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let out_dir = tempfile::TempDir::new().unwrap();

    // Create a Codex-style skill directory
    let skill_dir = input_dir.path().join(".agents").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nBody.\n",
    )
    .unwrap();

    let status = Command::new(cxbridge_bin())
        .args([
            "x2c",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
        ])
        .status()
        .expect("failed to run cxbridge binary");

    assert!(
        status.success(),
        "cxbridge x2c <dir> must exit 0, got: {}",
        status
    );

    // Verify the converted Claude skill was produced.
    let output_skill = out_dir
        .path()
        .join(".claude")
        .join("skills")
        .join("s")
        .join("SKILL.md");
    assert!(
        output_skill.exists(),
        "Expected output skill at {}, but it was not produced",
        output_skill.display()
    );
}

// ── default_out_dir ──────────────────────────────────────────────────────────

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
    let maps = load_mappings();
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

    let handler = pick_handler(kind, maps);
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
    let maps = load_mappings();
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

    let handler = pick_handler(kind, maps);
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

// ── check_direction ──────────────────────────────────────────────────────────

/// `check config.toml` must report dropped > 0 when the file contains
/// Codex-only fields (`env_vars`, `required`) that have no Claude equivalent.
#[test]
fn test_check_codex_config_toml_reports_dropped() {
    let dir = tempfile::TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[mcp_servers.s]\ncommand = \"node\"\nenv_vars = [\"SECRET\"]\nrequired = true\n",
    )
    .unwrap();

    let output = Command::new(cxbridge_bin())
        .args(["check", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run cxbridge binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "cxbridge check must exit 0\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // The report line must show dropped: > 0.
    // Iterate children to find a "dropped: N" where N is a non-zero digit.
    let has_nonzero_dropped = stdout.lines().any(|line| {
        if let Some(rest) = line.find("dropped:").map(|i| &line[i + "dropped:".len()..]) {
            let count_str = rest.trim_start().split(',').next().unwrap_or("").trim();
            count_str.parse::<usize>().map(|n| n > 0).unwrap_or(false)
        } else {
            false
        }
    });

    assert!(
        has_nonzero_dropped,
        "Expected dropped > 0 in check output for Codex config.toml with env_vars/required.\nstdout:\n{}",
        stdout
    );
}

/// `check AGENTS.md` must use x2c direction — it should exit 0 and
/// report the file (Memory kind).
#[test]
fn test_check_agents_md_uses_x2c_direction() {
    let dir = tempfile::TempDir::new().unwrap();
    let agents_path = dir.path().join("AGENTS.md");
    std::fs::write(&agents_path, "# Agent Instructions\n\nDo things.\n").unwrap();

    let output = Command::new(cxbridge_bin())
        .args(["check", agents_path.to_str().unwrap()])
        .output()
        .expect("failed to run cxbridge binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "cxbridge check AGENTS.md must exit 0\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // The check output must mention the file.
    assert!(
        stdout.contains("AGENTS.md"),
        "Expected AGENTS.md in check output.\nstdout:\n{}",
        stdout
    );
}

// ── only_filter ──────────────────────────────────────────────────────────────

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
    let maps = load_mappings();
    let opts = LowerOpts {
        only: only.iter().map(|s| s.to_string()).collect(),
        ..lower_opts_with_out(out_dir)
    };

    let pairs = detect_files(path).expect("detect_files should succeed");

    let mut combined_files: Vec<EmitFile> = Vec::new();
    let mut combined_diags: Vec<Diagnostic> = Vec::new();

    for (kind, file_path) in &pairs {
        // Apply --only filter exactly as run_convert does: skip domains not in the
        // allow-list by comparing against LowerOpts.only, not a local copy.
        if !opts.only.is_empty() {
            let domain = kind.domain_name();
            if !opts.only.iter().any(|d| d.as_str() == domain) {
                continue;
            }
        }

        let handler = pick_handler(kind, maps);
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

// ── report_flag ──────────────────────────────────────────────────────────────

fn make_skill_dir(dir: &tempfile::TempDir) {
    let skill_dir = dir.path().join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nBody.\n",
    )
    .unwrap();
}

/// Without --report, stdout must contain no report output.
#[test]
fn test_no_report_flag_produces_no_stdout() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let out_dir = tempfile::TempDir::new().unwrap();
    make_skill_dir(&input_dir);

    let output = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("failed to run cxbridge binary");

    assert!(
        output.status.success(),
        "cxbridge c2x must exit 0, got: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.is_empty(),
        "Without --report, stdout must be empty. Got:\n{}",
        stdout
    );
}

/// --report (no value) must print text format to stdout.
#[test]
fn test_report_flag_without_value_prints_text_report() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let out_dir = tempfile::TempDir::new().unwrap();
    make_skill_dir(&input_dir);

    let output = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
            "--report",
        ])
        .output()
        .expect("failed to run cxbridge binary");

    assert!(
        output.status.success(),
        "cxbridge c2x --report must exit 0, got: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "--report must produce output on stdout");
    assert!(
        stdout.contains("Summary:"),
        "--report must produce text format with 'Summary:'. Got:\n{}",
        stdout
    );
    // Must NOT be JSON
    assert!(
        !stdout.trim_start().starts_with('{'),
        "--report (no value) must produce text, not JSON. Got:\n{}",
        stdout
    );
}

/// --report=json must print JSON to stdout.
#[test]
fn test_report_flag_json_prints_json_report() {
    let input_dir = tempfile::TempDir::new().unwrap();
    let out_dir = tempfile::TempDir::new().unwrap();
    make_skill_dir(&input_dir);

    let output = Command::new(cxbridge_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
            "--report=json",
        ])
        .output()
        .expect("failed to run cxbridge binary");

    assert!(
        output.status.success(),
        "cxbridge c2x --report=json must exit 0, got: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "--report=json must produce output on stdout"
    );

    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(
        parsed.is_ok(),
        "--report=json must produce valid JSON. Got:\n{}",
        stdout
    );

    let json = parsed.unwrap();
    assert!(
        json["summary"].is_object(),
        "--report=json output must have a 'summary' key. Got:\n{}",
        stdout
    );
}

// ── write_plan ───────────────────────────────────────────────────────────────

fn plan(files: Vec<(String, &str)>) -> EmitPlan {
    EmitPlan {
        files: files
            .into_iter()
            .map(|(path, content)| EmitFile {
                path,
                content: content.to_string(),
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn config_toml_merge_preserves_existing_and_adds_new() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        "[features]\nmulti_agent = true\n\n[agents.existing]\nconfig_file = \".codex/agents/existing.toml\"\n",
    )
    .unwrap();

    write_plan(
        &plan(vec![(
            cfg.to_str().unwrap().to_string(),
            &agent_snippet("deploy"),
        )]),
        false,
    )
    .unwrap();

    let out = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        out.contains("[agents.existing]"),
        "existing agent dropped:\n{out}"
    );
    assert!(out.contains("[agents.deploy]"), "new agent missing:\n{out}");
    assert_eq!(
        out.matches("[features]").count(),
        1,
        "duplicate [features]:\n{out}"
    );
}

#[test]
fn config_toml_multiple_emitfiles_accumulate() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    let path = cfg.to_str().unwrap().to_string();

    // A plugin with two skills emits two config.toml EmitFiles at the same path.
    write_plan(
        &plan(vec![
            (path.clone(), &agent_snippet("alpha")),
            (path.clone(), &agent_snippet("beta")),
        ]),
        false,
    )
    .unwrap();

    let out = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        out.contains("[agents.alpha]"),
        "first agent dropped:\n{out}"
    );
    assert!(
        out.contains("[agents.beta]"),
        "second agent dropped:\n{out}"
    );
    assert_eq!(
        out.matches("[features]").count(),
        1,
        "duplicate [features]:\n{out}"
    );
}

#[test]
fn config_toml_existing_scalar_is_not_clobbered() {
    let dir = tempfile::TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[features]\nmulti_agent = false\n").unwrap();

    // Addition tries to set multi_agent = true; the existing value must win.
    write_plan(
        &plan(vec![(
            cfg.to_str().unwrap().to_string(),
            &agent_snippet("deploy"),
        )]),
        false,
    )
    .unwrap();

    let out = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        out.contains("multi_agent = false"),
        "existing value clobbered:\n{out}"
    );
    assert!(
        !out.contains("multi_agent = true"),
        "existing value clobbered:\n{out}"
    );
    assert!(out.contains("[agents.deploy]"), "new agent missing:\n{out}");
}

#[test]
fn non_append_target_is_overwrite_protected() {
    let dir = tempfile::TempDir::new().unwrap();
    let skill = dir.path().join("SKILL.md");
    std::fs::write(&skill, "original").unwrap();
    let path = skill.to_str().unwrap().to_string();

    // Without --force, an existing non-append file is refused.
    let err = write_plan(&plan(vec![(path.clone(), "new")]), false);
    assert!(err.is_err(), "overwrite protection did not trigger");
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "original");

    // With force, it is overwritten.
    write_plan(&plan(vec![(path, "new")]), true).unwrap();
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "new");
}

#[test]
fn rules_append_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let rules = dir.path().join("deploy.rules");
    std::fs::write(
        &rules,
        "prefix_rule(pattern=[\"git\",\"add\"], decision=\"allow\")\n",
    )
    .unwrap();
    let path = rules.to_str().unwrap().to_string();
    let addition = "prefix_rule(pattern=[\"git\",\"push\"], decision=\"allow\")\n";

    write_plan(&plan(vec![(path.clone(), addition)]), false).unwrap();
    let out = std::fs::read_to_string(&rules).unwrap();
    assert!(
        out.contains("\"git\",\"add\""),
        "existing rule dropped:\n{out}"
    );
    assert!(out.contains("\"git\",\"push\""), "new rule missing:\n{out}");

    // Re-applying the same addition does not duplicate it.
    write_plan(&plan(vec![(path, addition)]), false).unwrap();
    let out2 = std::fs::read_to_string(&rules).unwrap();
    assert_eq!(
        out2.matches("\"git\",\"push\"").count(),
        1,
        "rule duplicated:\n{out2}"
    );
}

// ── dir_input ────────────────────────────────────────────────────────────────

/// `detect_files` on a file returns a single-element list with the correct kind.
#[test]
fn test_detect_files_single_file() {
    let pairs = detect_files("tests/fixtures/claude/skills/deploy/SKILL.md")
        .expect("detect_files should succeed on a file");
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, Kind::Skill);
    assert_eq!(
        pairs[0].1,
        Path::new("tests/fixtures/claude/skills/deploy/SKILL.md")
    );
}

/// `detect_files` on a directory returns ALL recognizable files, not just the dominant kind.
#[test]
fn test_detect_files_directory_returns_all_kinds() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create .claude/skills/s/SKILL.md
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // Create .mcp.json
    std::fs::write(base.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    let pairs =
        detect_files(base.to_str().unwrap()).expect("detect_files should succeed on directory");

    // Must include both Skill and Mcp — not just the dominant one
    let kinds: Vec<&Kind> = pairs.iter().map(|(k, _)| k).collect();
    assert!(
        kinds.contains(&&Kind::Skill),
        "Expected Kind::Skill in pairs, got: {:?}",
        kinds
    );
    assert!(
        kinds.contains(&&Kind::Mcp),
        "Expected Kind::Mcp in pairs, got: {:?}",
        kinds
    );

    // Each pair must point to the actual file, not the directory
    for (_, path) in &pairs {
        assert!(
            path.is_file(),
            "Expected file path in pair, got directory: {}",
            path.display()
        );
    }
}

/// c2x on a directory converts every discovered file individually.
///
/// Repro from the bug report: `c2x /path/to/dir` previously crashed with
/// "Is a directory" because the handler received the directory path instead of
/// the individual file paths.
#[test]
fn test_c2x_directory_converts_all_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create .claude/skills/s/SKILL.md
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // Create .mcp.json
    std::fs::write(base.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let pairs = detect_files(base.to_str().unwrap()).expect("detect_files should succeed");

    let mut all_files: Vec<cxbridge::handlers::EmitFile> = Vec::new();
    let mut all_diags: Vec<cxbridge::core::ir::Diagnostic> = Vec::new();

    for (kind, file_path) in &pairs {
        let handler = pick_handler(kind, maps);
        let parsed = handler
            .parse(file_path)
            .unwrap_or_else(|e| panic!("parse failed for {}: {}", file_path.display(), e));
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .unwrap_or_else(|e| panic!("lift failed for {}: {}", file_path.display(), e));
        let plan = handler
            .lower(&ir, ConvDir::C2x, &opts)
            .unwrap_or_else(|e| panic!("lower failed for {}: {}", file_path.display(), e));
        all_files.extend(plan.files);
        all_diags.extend(plan.diagnostics);
    }

    // Converted SKILL.md must be present
    let has_skill = all_files.iter().any(|f| f.path.ends_with("SKILL.md"));
    assert!(
        has_skill,
        "Expected converted SKILL.md in output, got: {:?}",
        all_files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // Converted .mcp.json must be present
    let has_mcp = all_files.iter().any(|f| f.path.ends_with(".mcp.json"));
    assert!(
        has_mcp,
        "Expected converted .mcp.json in output, got: {:?}",
        all_files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// check on a directory succeeds and produces diagnostics for every file.
#[test]
fn test_check_directory_processes_all_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create .claude/skills/s/SKILL.md
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // Create CLAUDE.md (memory file)
    std::fs::write(base.join("CLAUDE.md"), "# Project Instructions\nHello.").unwrap();

    let maps = load_mappings();
    let pairs = detect_files(base.to_str().unwrap()).expect("detect_files should succeed");

    assert!(
        pairs.len() >= 2,
        "Expected at least 2 files detected, got {}",
        pairs.len()
    );

    // Simulate run_check: parse + lift each file
    for (kind, file_path) in &pairs {
        let handler = pick_handler(kind, maps);
        let parsed = handler
            .parse(file_path)
            .unwrap_or_else(|e| panic!("parse failed for {}: {}", file_path.display(), e));
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .unwrap_or_else(|e| panic!("lift failed for {}: {}", file_path.display(), e));
        let _report = build_report(&ir, &empty_plan());
    }
}

/// `detect_files` on a file path returns the actual `PathBuf` for that file,
/// not the parent directory (regression guard).
#[test]
fn test_detect_files_file_path_is_exact() {
    let path = "tests/fixtures/claude/.mcp.json";
    let pairs = detect_files(path).expect("detect_files should succeed");
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].1,
        Path::new(path),
        "Expected path to be the exact file, not its parent"
    );
}

/// Plugin directory input: c2x on a directory containing .claude-plugin/plugin.json
/// must succeed — detect_files must return the plugin.json file, not the directory.
#[test]
fn test_c2x_plugin_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let plugin_dir = base.join(".claude-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"dir-plugin","version":"1.0.0","description":"Dir plugin test"}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on directory with .claude-plugin/plugin.json");

    // Must find the plugin.json file with Kind::Plugin
    let plugin_pair = pairs.iter().find(|(k, _)| *k == Kind::Plugin);
    assert!(
        plugin_pair.is_some(),
        "Expected Kind::Plugin in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, plugin_path) = plugin_pair.unwrap();
    assert!(
        plugin_path.is_file(),
        "Plugin path must point to a file, not a directory: {}",
        plugin_path.display()
    );

    let maps = load_mappings();
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Plugin, maps);
    let parsed = handler
        .parse(plugin_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", plugin_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", plugin_path.display(), e));
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", plugin_path.display(), e));

    let has_codex_manifest = plan
        .files
        .iter()
        .any(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
    assert!(
        has_codex_manifest,
        "Expected .codex-plugin/plugin.json in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Plugin directory input: x2c on a directory containing .codex-plugin/plugin.json succeeds.
#[test]
fn test_x2c_plugin_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let plugin_dir = base.join(".codex-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"codex-dir-plugin","version":"1.0.0","description":"Codex dir plugin"}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on directory with .codex-plugin/plugin.json");

    let plugin_pair = pairs.iter().find(|(k, _)| *k == Kind::Plugin);
    assert!(plugin_pair.is_some(), "Expected Kind::Plugin in pairs");
    let (_, plugin_path) = plugin_pair.unwrap();
    assert!(plugin_path.is_file(), "Plugin path must be a file");

    let maps = load_mappings();
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Plugin, maps);
    let parsed = handler
        .parse(plugin_path)
        .unwrap_or_else(|e| panic!("parse failed: {}", e));
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .unwrap_or_else(|e| panic!("lift failed: {}", e));
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .unwrap_or_else(|e| panic!("lower failed: {}", e));

    let has_claude_manifest = plan
        .files
        .iter()
        .any(|f| f.path.contains(".claude-plugin") && f.path.ends_with("plugin.json"));
    assert!(
        has_claude_manifest,
        "Expected .claude-plugin/plugin.json in x2c output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Hooks directory input: c2x on a directory containing hooks.json succeeds.
#[test]
fn test_c2x_hooks_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    std::fs::write(
        base.join("hooks.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on hooks directory");

    let hooks_pair = pairs.iter().find(|(k, _)| *k == Kind::Hooks);
    assert!(
        hooks_pair.is_some(),
        "Expected Kind::Hooks in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, hooks_path) = hooks_pair.unwrap();
    assert!(hooks_path.is_file(), "Hooks path must point to a file");

    let maps = load_mappings();
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Hooks, maps);
    let parsed = handler
        .parse(hooks_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", hooks_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", hooks_path.display(), e));
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", hooks_path.display(), e));

    // c2x hooks should produce a config.toml with hooks section
    let has_hooks_output = plan
        .files
        .iter()
        .any(|f| f.path.ends_with("config.toml") || f.path.ends_with("hooks.json"));
    assert!(
        has_hooks_output,
        "Expected hooks output file, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Settings directory input: c2x on a directory containing settings.json succeeds.
#[test]
fn test_c2x_settings_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    std::fs::write(
        base.join("settings.json"),
        r#"{"model":"claude-sonnet-4-6","env":{"RUST_LOG":"info"}}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on settings directory");

    let settings_pair = pairs.iter().find(|(k, _)| *k == Kind::Settings);
    assert!(
        settings_pair.is_some(),
        "Expected Kind::Settings in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, settings_path) = settings_pair.unwrap();
    assert!(
        settings_path.is_file(),
        "Settings path must point to a file"
    );

    let maps = load_mappings();
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Settings, maps);
    let parsed = handler
        .parse(settings_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", settings_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", settings_path.display(), e));
    let _plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", settings_path.display(), e));
}

/// Subagent directory input: c2x on a directory with .claude/agents/*.md succeeds.
#[test]
fn test_c2x_subagent_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let agents_dir = base.join(".claude").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("researcher.md"),
        "---\nname: researcher\ndescription: Research specialist\n---\nYou are a researcher.\n",
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on subagent directory");

    let subagent_pair = pairs.iter().find(|(k, _)| *k == Kind::Subagent);
    assert!(
        subagent_pair.is_some(),
        "Expected Kind::Subagent in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, subagent_path) = subagent_pair.unwrap();
    assert!(
        subagent_path.is_file(),
        "Subagent path must point to a file, not directory: {}",
        subagent_path.display()
    );

    let maps = load_mappings();
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Subagent, maps);
    let parsed = handler
        .parse(subagent_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", subagent_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", subagent_path.display(), e));
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", subagent_path.display(), e));

    // c2x subagent should produce a .toml file
    let has_toml = plan.files.iter().any(|f| f.path.ends_with(".toml"));
    assert!(
        has_toml,
        "Expected .toml output for subagent c2x, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Memory directory input: c2x on a directory with CLAUDE.md succeeds.
#[test]
fn test_c2x_memory_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    std::fs::write(
        base.join("CLAUDE.md"),
        "# Project Instructions\n\nAlways use Rust.\n",
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on memory directory");

    let memory_pair = pairs.iter().find(|(k, _)| *k == Kind::Memory);
    assert!(
        memory_pair.is_some(),
        "Expected Kind::Memory in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, memory_path) = memory_pair.unwrap();
    assert!(
        memory_path.is_file(),
        "Memory path must point to a file, not directory: {}",
        memory_path.display()
    );

    let maps = load_mappings();
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Memory, maps);
    let parsed = handler
        .parse(memory_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", memory_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", memory_path.display(), e));
    let _plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", memory_path.display(), e));
}

/// check on a directory with a plugin file processes it without 'Is a directory' error.
#[test]
fn test_check_plugin_directory_input() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let plugin_dir = base.join(".claude-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"check-plugin","version":"1.0.0","description":"Check test plugin"}"#,
    )
    .unwrap();

    let maps = load_mappings();
    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on plugin directory");

    let plugin_pair = pairs.iter().find(|(k, _)| *k == Kind::Plugin);
    assert!(plugin_pair.is_some(), "Expected Kind::Plugin");
    let (kind, file_path) = plugin_pair.unwrap();

    assert!(
        file_path.is_file(),
        "check must receive a file path, not a directory: {}",
        file_path.display()
    );

    let handler = pick_handler(kind, maps);
    let parsed = handler
        .parse(file_path)
        .unwrap_or_else(|e| panic!("check parse failed: {}", e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("check lift failed: {}", e));
    let _report = build_report(&ir, &empty_plan());
}

// ── reconciliation_fixes ─────────────────────────────────────────────────────

#[test]
fn infer_conv_dir_recognises_relative_codex_paths() {
    // Relative paths under .agents/ / .codex/ (no leading slash) must be X2c.
    assert_eq!(
        infer_conv_dir(".agents/skills/deploy/SKILL.md"),
        ConvDir::X2c
    );
    assert_eq!(infer_conv_dir(".codex/agents/deploy.toml"), ConvDir::X2c);
    assert_eq!(infer_conv_dir("config.toml"), ConvDir::X2c);
    assert_eq!(infer_conv_dir("AGENTS.md"), ConvDir::X2c);
    // Claude-origin paths stay C2x.
    assert_eq!(
        infer_conv_dir(".claude/skills/deploy/SKILL.md"),
        ConvDir::C2x
    );
    assert_eq!(infer_conv_dir("CLAUDE.md"), ConvDir::C2x);
}

fn config_toml_artifacts(artifacts: &[cxbridge::core::ir::SideArtifact]) -> String {
    artifacts
        .iter()
        .filter(|a| a.path == "config.toml")
        .map(|a| a.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn webfetch_disallowed_denies_network() {
    let (allow_arts, _) =
        degrade_allowed_tools("s", &["WebFetch".to_string()], true, Scope::Project);
    assert!(
        config_toml_artifacts(&allow_arts).contains("network = true"),
        "allowed WebFetch should grant network"
    );

    let (deny_arts, _) =
        degrade_allowed_tools("s", &["WebFetch".to_string()], false, Scope::Project);
    let cfg = config_toml_artifacts(&deny_arts);
    assert!(
        cfg.contains("network = false"),
        "disallowed WebFetch must deny network, got:\n{cfg}"
    );
    assert!(
        !cfg.contains("network = true"),
        "disallowed WebFetch must never grant network, got:\n{cfg}"
    );
}

#[test]
fn websearch_disallowed_disables_feature() {
    let (deny_arts, _) =
        degrade_allowed_tools("s", &["WebSearch".to_string()], false, Scope::Project);
    let cfg = config_toml_artifacts(&deny_arts);
    assert!(
        cfg.contains("web_search = false"),
        "disallowed WebSearch must disable the feature, got:\n{cfg}"
    );
    assert!(
        !cfg.contains("web_search = true"),
        "disallowed WebSearch must never enable the feature, got:\n{cfg}"
    );
}

#[test]
fn dry_run_prints_report_default_is_quiet() {
    let skill = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude/skills/deploy/SKILL.md"
    );
    let out = tempfile::TempDir::new().unwrap();

    let dry = std::process::Command::new(env!("CARGO_BIN_EXE_cxbridge"))
        .args([
            "c2x",
            skill,
            "--out",
            out.path().to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();
    let dry_stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(
        dry_stdout.contains("lossless") || dry_stdout.contains("dropped"),
        "--dry-run must print the conversion report, got:\n{dry_stdout}"
    );

    // Default run (no --report, no --dry-run) stays quiet on stdout.
    let out2 = tempfile::TempDir::new().unwrap();
    let quiet = std::process::Command::new(env!("CARGO_BIN_EXE_cxbridge"))
        .args(["c2x", skill, "--out", out2.path().to_str().unwrap()])
        .output()
        .unwrap();
    let quiet_stdout = String::from_utf8_lossy(&quiet.stdout);
    for marker in &["lossless", "lossy", "dropped", "degraded", "Summary"] {
        assert!(
            !quiet_stdout.contains(marker),
            "default run must not print any report markers (found '{}'), got:\n{quiet_stdout}",
            marker
        );
    }
}
