//! Tests for `write_plan`'s append-target merge: converting several skills into
//! one output tree, or into an existing Codex project, must accumulate
//! `[agents.*]`/`[features]` in `config.toml` instead of clobbering them.

use ccx::cli::write_plan;
use ccx::handlers::{EmitFile, EmitPlan};

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

fn agent_snippet(name: &str) -> String {
    format!(
        "[agents.{name}]\nconfig_file = \".codex/agents/{name}.toml\"\n\n[features]\nmulti_agent = true\n"
    )
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
