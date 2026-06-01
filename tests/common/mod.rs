#![allow(dead_code)]

use ccx::handlers::{EmitPlan, LowerOpts, Scope, SkillTargetMode};

/// Path to the mappings directory, relative to the workspace root.
/// Appears in 18 test files.
pub const MAPPINGS_DIR: &str = "mappings";

/// `LowerOpts` with `skill_target: SkillTargetMode::Auto`.
///
/// Used by hooks_*, mcp_*, plugins_*, subagents_*, webfetch_*, marketplace_*,
/// no_duplicate_diagnostics, and dir_input tests.
pub fn default_lower_opts(out_dir: &str) -> LowerOpts {
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

/// `LowerOpts` with `skill_target: SkillTargetMode::Subagent`.
///
/// Used by roundtrip.rs skill/memory/plugin/subagent/settings tests that need
/// degrade to trigger.
pub fn default_lower_opts_subagent(out_dir: &str) -> LowerOpts {
    LowerOpts {
        out: Some(out_dir.to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Subagent, // subagent to trigger degrade
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    }
}

/// `LowerOpts` with `skill_target: SkillTargetMode::Skill`.
///
/// Used by developer_instructions_degrade, batch_flags_scope, and roundtrip.rs
/// hook/mcp/x2c tests.
pub fn default_lower_opts_skill(out_dir: &str) -> LowerOpts {
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

/// Returns an empty `EmitPlan` with no files and no diagnostics.
///
/// Used by roundtrip.rs and dir_input.rs.
pub fn empty_plan() -> EmitPlan {
    EmitPlan {
        files: vec![],
        diagnostics: vec![],
    }
}

/// Returns the path to the debug build of the `ccx` binary.
///
/// Used by cli_dir_input.rs, check_direction.rs, and report_flag.rs.
pub fn ccx_bin() -> std::path::PathBuf {
    // Use the debug build produced by `cargo build`.
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("debug");
    p.push("ccx");
    p
}

/// Produces an `[agents.<name>]` + `[features] multi_agent=true` TOML fragment.
///
/// Used by write_plan.rs.
pub fn agent_snippet(name: &str) -> String {
    format!(
        "[agents.{name}]\nconfig_file = \".codex/agents/{name}.toml\"\n\n[features]\nmulti_agent = true\n"
    )
}
