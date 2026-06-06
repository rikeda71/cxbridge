mod common;
use common::*;

use std::path::Path;

use cxbridge::core::ir::{DiagLevel, Kind};
use cxbridge::core::{
    detect::detect, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use cxbridge::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

// ────────────────────────────────────────────────────────────────────────────
// Settings tests (from roundtrip.rs)
// ────────────────────────────────────────────────────────────────────────────

/// settings.json c2x: config.toml is generated and the converted subset is correct.
#[test]
fn test_settings_c2x_generates_config_toml() {
    let settings_path = "tests/fixtures/claude/settings.json";
    assert!(
        Path::new(settings_path).exists(),
        "Fixture {} must exist",
        settings_path
    );

    let maps = load_mappings();
    let kind = detect(settings_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Settings);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(settings_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, cxbridge::core::ir::Kind::Settings);

    assert!(ir.fields.contains_key("settings.model"));
    assert!(ir.fields.contains_key("settings.effortLevel"));

    // effortLevel high → high is a lossless 1:1 mapping
    let effort = &ir.fields["settings.effortLevel"];
    assert_eq!(effort.value, serde_json::Value::String("high".to_string()));

    assert!(ir.fields.contains_key("settings.editorMode"));

    let has_viewmode_dropped = ir
        .fields
        .get("settings.viewMode")
        .map(|f| matches!(f.loss, cxbridge::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(
        has_viewmode_dropped,
        "Expected settings.viewMode to be dropped"
    );

    let has_worktree_dropped = ir
        .fields
        .get("settings.worktree")
        .map(|f| matches!(f.loss, cxbridge::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(
        has_worktree_dropped,
        "Expected settings.worktree to be dropped"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &config_toml.unwrap().content;
    // effortLevel → model_reasoning_effort
    assert!(
        content.contains("model_reasoning_effort"),
        "Expected model_reasoning_effort in config.toml"
    );
    // editorMode=vim → tui.vim_mode_default=true
    assert!(
        content.contains("vim_mode_default"),
        "Expected vim_mode_default in config.toml"
    );
    // env → shell_environment_policy.set
    assert!(
        content.contains("shell_environment_policy"),
        "Expected shell_environment_policy in config.toml"
    );
    assert!(
        content.contains("RUST_LOG"),
        "Expected env vars in shell_environment_policy"
    );
    // memories
    assert!(
        content.contains("use_memories"),
        "Expected memories settings in config.toml"
    );

    // .rules file should be generated for Bash permissions
    let rules_file = plan.files.iter().find(|f| f.path.ends_with(".rules"));
    assert!(
        rules_file.is_some(),
        "Expected .rules file for Bash permissions"
    );

    let report = build_report(&ir, &plan);
    assert!(
        !report.dropped.is_empty(),
        "Expected dropped fields in settings report (viewMode, worktree, etc.)"
    );

    let has_partial_warn = plan
        .diagnostics
        .iter()
        .any(|d| d.message.contains("partial conversion"));
    assert!(
        has_partial_warn,
        "Expected partial conversion warning in diagnostics"
    );
}

/// settings.json c2x report: un-converted remainder is enumerated.
#[test]
fn test_settings_c2x_report_enumerates_remainder() {
    let settings_path = "tests/fixtures/claude/settings.json";

    let maps = load_mappings();
    let kind = detect(settings_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(settings_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    assert!(
        !report.dropped.is_empty(),
        "Expected dropped fields in settings report"
    );

    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"settings.viewMode"),
        "Expected settings.viewMode in dropped"
    );
    assert!(
        drop_ids.contains(&"settings.worktree"),
        "Expected settings.worktree in dropped"
    );
    assert!(
        drop_ids.contains(&"settings.autoUpdatesChannel"),
        "Expected settings.autoUpdatesChannel in dropped"
    );

    assert!(
        !report.lossy.is_empty(),
        "Expected lossy fields in settings report (model, effortLevel, etc.)"
    );

    assert!(
        !report.lossless.is_empty(),
        "Expected lossless fields (editorMode → vim_mode_default is lossless)"
    );
    assert!(
        report
            .lossless
            .contains(&"settings.sandbox.network.allowAllUnixSockets".to_string()),
        "Expected allowAllUnixSockets to be lossless"
    );
}

/// Codex settings.toml x2c: settings.json is generated.
#[test]
fn test_settings_x2c_generates_claude_settings() {
    let settings_path = "tests/fixtures/codex/settings.toml";
    assert!(
        Path::new(settings_path).exists(),
        "Fixture {} must exist",
        settings_path
    );

    let maps = load_mappings();

    // Test SettingsHandler directly (detect targets config.toml, so call it directly)
    use cxbridge::handlers::settings::SettingsHandler;
    use cxbridge::handlers::Handler;

    let handler = SettingsHandler {
        map: maps["settings-config"].clone(),
    };

    let parsed = handler
        .parse(Path::new(settings_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    assert_eq!(ir.kind, cxbridge::core::ir::Kind::Settings);
    assert!(ir.fields.contains_key("settings.model"));
    assert!(ir.fields.contains_key("settings.effortLevel"));
    assert!(ir.fields.contains_key("settings.editorMode"));

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let settings_json = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("settings.json"));
    assert!(settings_json.is_some(), "Expected settings.json in output");

    let content: serde_json::Value = serde_json::from_str(&settings_json.unwrap().content).unwrap();
    assert!(content.get("model").is_some(), "Expected model field");
    assert!(
        content.get("effortLevel").is_some(),
        "Expected effortLevel field"
    );
    assert!(
        content.get("editorMode").is_some(),
        "Expected editorMode field"
    );
    assert_eq!(
        content["editorMode"],
        serde_json::Value::String("vim".to_string()),
        "Expected editorMode=vim"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// developer_instructions degrade tests (from developer_instructions_degrade.rs)
// ────────────────────────────────────────────────────────────────────────────

/// developer_instructions in config.toml must produce a CLAUDE.md file during x2c.
#[test]
fn test_developer_instructions_produces_claude_md() {
    let fixture_path = "tests/fixtures/codex/developer_instructions/config.toml";
    assert!(
        Path::new(fixture_path).exists(),
        "Fixture {} must exist",
        fixture_path
    );

    let maps = load_mappings();
    let kind = cxbridge::core::detect::detect(fixture_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(fixture_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_skill(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // CLAUDE.md must be generated at the exact relative path .claude/CLAUDE.md.
    let expected_suffix = std::path::Path::new(".claude").join("CLAUDE.md");
    let claude_md = plan
        .files
        .iter()
        .find(|f| std::path::Path::new(&f.path).ends_with(&expected_suffix));
    assert!(
        claude_md.is_some(),
        "Expected a file ending with .claude/CLAUDE.md in output when developer_instructions is set; got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &claude_md.unwrap().content;
    assert!(
        content.contains("Always respond in English"),
        "CLAUDE.md must contain the original developer_instructions text; got:\n{}",
        content
    );
    assert!(
        content.contains("Focus on clear answers"),
        "CLAUDE.md must contain full developer_instructions text; got:\n{}",
        content
    );
}

/// The degrade diagnostic for developer_instructions must still be emitted.
#[test]
fn test_developer_instructions_degrade_diagnostic_present() {
    let fixture_path = "tests/fixtures/codex/developer_instructions/config.toml";

    let maps = load_mappings();
    let kind = cxbridge::core::detect::detect(fixture_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(fixture_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // The IR must have the developer_instructions field with degrade metadata
    let field = ir
        .fields
        .get("settings.codex.developer_instructions")
        .expect("IR must contain settings.codex.developer_instructions");
    assert!(
        field.degrade.is_some(),
        "developer_instructions must have degrade info"
    );
    assert_eq!(
        field.degrade.as_ref().unwrap().target,
        "CLAUDE.md",
        "degrade target must be CLAUDE.md"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_skill(out_dir.path().to_str().unwrap());
    let _plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // The degrade diagnostic is emitted during lift (ir.diagnostics), not lower,
    // to avoid duplicating it in the report.
    let has_degrade_diag = ir
        .diagnostics
        .iter()
        .any(|d| d.id.as_deref() == Some("settings.codex.developer_instructions"));
    assert!(
        has_degrade_diag,
        "Expected degrade diagnostic for developer_instructions in ir.diagnostics; got: {:?}",
        ir.diagnostics
            .iter()
            .map(|d| (d.id.as_deref().unwrap_or("<none>"), &d.message))
            .collect::<Vec<_>>()
    );
}

/// When developer_instructions is absent, no CLAUDE.md should be emitted from settings.
#[test]
fn test_no_developer_instructions_no_claude_md_from_settings() {
    let fixture_path = "tests/fixtures/codex/config.toml";

    let maps = load_mappings();
    let kind = cxbridge::core::detect::detect(fixture_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(fixture_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_skill(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // No CLAUDE.md from the settings handler when no developer_instructions
    let claude_md_from_settings = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
    assert!(
        claude_md_from_settings.is_none(),
        "No CLAUDE.md should be emitted when developer_instructions is absent; got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// WebFetch deny domains tests (from webfetch_deny_domains.rs)
// ────────────────────────────────────────────────────────────────────────────

/// End-to-end: permissions.deny with WebFetch(domain:...) entries must
/// produce config.toml with those domains set to "deny" under
/// [permissions.default.network.domains], and emit a Warn diagnostic.
#[test]
fn test_webfetch_deny_domains_appear_in_config_toml() {
    let fixture = Path::new("tests/fixtures/claude/webfetch_deny/settings.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Settings, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // config.toml must be generated
    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml to be generated; files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &config_toml.unwrap().content;

    // bad.com must appear with value "deny"
    assert!(
        content.contains("\"bad.com\" = \"deny\"") || content.contains("bad.com = \"deny\""),
        "Expected bad.com = \"deny\" in config.toml; got:\n{}",
        content
    );

    // evil.net must appear with value "deny"
    assert!(
        content.contains("\"evil.net\" = \"deny\"") || content.contains("evil.net = \"deny\""),
        "Expected evil.net = \"deny\" in config.toml; got:\n{}",
        content
    );

    // The [permissions.default.network.domains] section must be present
    assert!(
        content.contains("[permissions"),
        "Expected [permissions...] section in config.toml; got:\n{}",
        content
    );
}

/// End-to-end: a Warn diagnostic with id "settings.permissions.deny.webfetch"
/// must be emitted when WebFetch deny domains are present.
#[test]
fn test_webfetch_deny_domains_emit_warn_diagnostic() {
    let fixture = Path::new("tests/fixtures/claude/webfetch_deny/settings.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Settings, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // A Warn diagnostic with id "settings.permissions.deny.webfetch" must exist
    let webfetch_deny_diags: Vec<_> = plan
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("settings.permissions.deny.webfetch"))
        .collect();

    assert!(
        !webfetch_deny_diags.is_empty(),
        "Expected a diagnostic with id 'settings.permissions.deny.webfetch'; \
         diagnostics: {:?}",
        plan.diagnostics
            .iter()
            .map(|d| (d.id.as_deref().unwrap_or("<none>"), &d.message))
            .collect::<Vec<_>>()
    );

    for diag in &webfetch_deny_diags {
        assert_eq!(
            diag.level,
            DiagLevel::Warn,
            "settings.permissions.deny.webfetch diagnostic must be DiagLevel::Warn, got {:?}",
            diag.level
        );
    }
}

/// End-to-end: the report must include the deny.webfetch diagnostic so users
/// can see what happened (spec invariant — no silent discard).
#[test]
fn test_webfetch_deny_domains_visible_in_report() {
    let fixture = Path::new("tests/fixtures/claude/webfetch_deny/settings.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Settings, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // The warn diagnostic must be surfaced via the plan diagnostics that
    // build_report aggregates. Check that the deny webfetch diagnostic is
    // NOT silently discarded — it should appear in lossy or warnings.
    let all_diag_ids: Vec<_> = plan
        .diagnostics
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    assert!(
        all_diag_ids.contains(&"settings.permissions.deny.webfetch"),
        "settings.permissions.deny.webfetch must appear in plan diagnostics (not silently dropped); \
         ids: {:?}",
        all_diag_ids
    );

    // It should also appear in report.lossy (Warn level → lossy bucket)
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("settings.permissions.deny.webfetch"));
    assert!(
        in_lossy,
        "settings.permissions.deny.webfetch must appear in report.lossy; \
         lossy={:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
