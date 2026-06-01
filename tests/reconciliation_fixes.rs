//! Regression tests for bugs found by reviewing the comprehensive-coverage sweep:
//! WebFetch/WebSearch denials must not grant access, `infer_conv_dir` must
//! recognise relative Codex paths, and `--dry-run` must print the report.

use ccx::cli::infer_conv_dir;
use ccx::core::transforms::ConvDir;
use ccx::degrade::rules::degrade_allowed_tools;
use ccx::handlers::Scope;

// --- infer_conv_dir: relative Codex paths are X2c -------------------------

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

// --- WebFetch / WebSearch denials must deny, not grant --------------------

fn config_toml(artifacts: &[ccx::core::ir::SideArtifact]) -> String {
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
        config_toml(&allow_arts).contains("network = true"),
        "allowed WebFetch should grant network"
    );

    let (deny_arts, _) =
        degrade_allowed_tools("s", &["WebFetch".to_string()], false, Scope::Project);
    let cfg = config_toml(&deny_arts);
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
    let cfg = config_toml(&deny_arts);
    assert!(
        cfg.contains("web_search = false"),
        "disallowed WebSearch must disable the feature, got:\n{cfg}"
    );
    assert!(
        !cfg.contains("web_search = true"),
        "disallowed WebSearch must never enable the feature, got:\n{cfg}"
    );
}

// --- --dry-run prints the report; default stays quiet ---------------------

#[test]
fn dry_run_prints_report_default_is_quiet() {
    let skill = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude/skills/deploy/SKILL.md"
    );
    let out = tempfile::TempDir::new().unwrap();

    let dry = std::process::Command::new(env!("CARGO_BIN_EXE_ccx"))
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
    let quiet = std::process::Command::new(env!("CARGO_BIN_EXE_ccx"))
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
