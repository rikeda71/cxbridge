//! End-to-end CLI tests for directory input (gap 21/42).
//!
//! Drives the real `ccx` binary via `std::process::Command` to verify that
//! `c2x <dir>`, `x2c <dir>`, and `check <dir>` all exit 0 and produce the
//! expected output files.

use std::process::Command;

fn ccx_bin() -> std::path::PathBuf {
    // Use the debug build produced by `cargo build`.
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("debug");
    p.push("ccx");
    p
}

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

    let status = Command::new(ccx_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
        ])
        .status()
        .expect("failed to run ccx binary");

    assert!(
        status.success(),
        "ccx c2x <dir> must exit 0, got: {}",
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

    let output = Command::new(ccx_bin())
        .args(["check", input_dir.path().to_str().unwrap()])
        .output()
        .expect("failed to run ccx binary");

    assert!(
        output.status.success(),
        "ccx check <dir> must exit 0, got: {}\nstdout: {}\nstderr: {}",
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

    let status = Command::new(ccx_bin())
        .args([
            "x2c",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
        ])
        .status()
        .expect("failed to run ccx binary");

    assert!(
        status.success(),
        "ccx x2c <dir> must exit 0, got: {}",
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
