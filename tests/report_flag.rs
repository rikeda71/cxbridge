//! Integration tests for gap 24/42: --report flag controls whether the report is printed.
//!
//! Spec §13 Options: --report default is 'none' (no output without the flag).
//! --report (no value) prints text format to stdout.
//! --report=json prints JSON to stdout.
//! Without --report, no report is printed.

use std::process::Command;

fn ccx_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("debug");
    p.push("ccx");
    p
}

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

    let output = Command::new(ccx_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("failed to run ccx binary");

    assert!(
        output.status.success(),
        "ccx c2x must exit 0, got: {}\nstderr: {}",
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

    let output = Command::new(ccx_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
            "--report",
        ])
        .output()
        .expect("failed to run ccx binary");

    assert!(
        output.status.success(),
        "ccx c2x --report must exit 0, got: {}\nstderr: {}",
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

    let output = Command::new(ccx_bin())
        .args([
            "c2x",
            input_dir.path().to_str().unwrap(),
            "--out",
            out_dir.path().to_str().unwrap(),
            "--report=json",
        ])
        .output()
        .expect("failed to run ccx binary");

    assert!(
        output.status.success(),
        "ccx c2x --report=json must exit 0, got: {}\nstderr: {}",
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
