//! End-to-end tests for `ccx check` direction inference (gap 26/42).
//!
//! `check` must infer the conversion direction from the source filename.
//! For Codex-origin files (`config.toml`, `AGENTS.md`, etc.) the relevant
//! direction is x2c, so Codex-only dropped fields must appear in the report.

use std::process::Command;

fn ccx_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("debug");
    p.push("ccx");
    p
}

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

    let output = Command::new(ccx_bin())
        .args(["check", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run ccx binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "ccx check must exit 0\nstdout: {}\nstderr: {}",
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

    let output = Command::new(ccx_bin())
        .args(["check", agents_path.to_str().unwrap()])
        .output()
        .expect("failed to run ccx binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "ccx check AGENTS.md must exit 0\nstdout: {}\nstderr: {}",
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
