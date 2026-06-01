// Roundtrip tests: after c2x → x2c, IR diffs should contain only known lossy/dropped fields.
// Lossless fields must match exactly.

use std::path::Path;

use ccx::core::{
    detect::detect, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use ccx::handlers::{pick_handler, EmitPlan, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

fn default_lower_opts(out_dir: &str) -> LowerOpts {
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

fn empty_plan() -> EmitPlan {
    EmitPlan {
        files: vec![],
        diagnostics: vec![],
    }
}

/// Convert SKILL.md via c2x and verify the report matches expectations.
#[test]
fn test_skill_c2x_basic_roundtrip() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());
    assert!(
        !report.lossless.is_empty(),
        "Expected lossless fields (name, description)"
    );
    assert!(
        report.lossless.contains(&"skills.name".to_string()),
        "skills.name should be lossless"
    );
    assert!(
        report.lossless.contains(&"skills.description".to_string()),
        "skills.description should be lossless"
    );

    assert!(
        !report.dropped.is_empty(),
        "Expected dropped fields (user-invocable, paths, etc.)"
    );
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"skills.user-invocable"),
        "Expected skills.user-invocable in dropped. Got: {:?}",
        drop_ids
    );

    assert!(
        !report.degraded.is_empty() || !report.lossy.is_empty(),
        "Expected degraded or lossy entries (model, effort, allowed-tools)"
    );
}

/// Convert SKILL.md via c2x and verify that the dropped count is reported.
#[test]
fn test_skill_c2x_check_reports_dropped() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());
    assert!(
        report.dropped.len() >= 2,
        "Expected at least 2 dropped fields, got {}",
        report.dropped.len()
    );
}

/// Convert .mcp.json via c2x and verify that basic conversion works correctly.
#[test]
fn test_mcp_c2x_basic() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";
    assert!(
        Path::new(mcp_path).exists(),
        "Fixture {} must exist",
        mcp_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(mcp_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Mcp);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(mcp_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.children.len(), 4, "Expected 4 MCP server children");

    // Verify timeout conversion for the filesystem server
    let fs_server = ir.children.iter().find(|c| c.source_path == "filesystem");
    assert!(fs_server.is_some(), "Expected 'filesystem' server");
    let fs = fs_server.unwrap();
    let timeout = fs.fields.get("mcp.timeout");
    assert!(timeout.is_some(), "Expected timeout field");
    // 30000ms → 30.0 sec
    assert_eq!(
        timeout.unwrap().value.as_f64().unwrap(),
        30.0,
        "Expected timeout converted to 30.0 sec"
    );

    // Verify Bearer token extraction for api-server
    let api_server = ir.children.iter().find(|c| c.source_path == "api-server");
    assert!(api_server.is_some(), "Expected 'api-server'");
    let api = api_server.unwrap();
    let bearer = api.fields.get("mcp.bearer");
    assert!(bearer.is_some(), "Expected bearer field");
    assert_eq!(
        bearer.unwrap().value.as_str().unwrap(),
        "API_TOKEN",
        "Expected bearer_token_env_var=API_TOKEN"
    );
}

/// Dropped/lossy fields are enumerated in the report after .mcp.json c2x conversion.
#[test]
fn test_mcp_c2x_report_dropped() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(mcp_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    // alwaysLoad is Claude-specific and should be dropped (unknown field or dropped)
    // alwaysLoad on disabled-server produces a Drop diagnostic as an unknown field
    let total_drops = report.dropped.len();
    assert!(
        total_drops >= 1,
        "Expected at least 1 dropped entry, got {}",
        total_drops
    );
}

/// Files are generated after .mcp.json c2x lower.
#[test]
fn test_mcp_c2x_lower_generates_files() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(mcp_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    assert!(
        !plan.files.is_empty(),
        "Expected at least one generated file"
    );
    let mcp_file = plan.files.iter().find(|f| f.path.ends_with(".mcp.json"));
    assert!(mcp_file.is_some(), "Expected .mcp.json in output");
    let content: serde_json::Value = serde_json::from_str(&mcp_file.unwrap().content).unwrap();
    assert!(
        content["mcpServers"].is_object(),
        "Expected mcpServers object"
    );
}

/// x2c conversion test for Codex config.toml.
#[test]
fn test_mcp_x2c_from_codex_config() {
    let config_path = "tests/fixtures/codex/config.toml";
    assert!(
        Path::new(config_path).exists(),
        "Fixture {} must exist",
        config_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(config_path).expect("detect should succeed");
    assert_eq!(
        kind,
        ccx::core::ir::Kind::Mcp,
        "config.toml with mcp_servers should be Kind::Mcp"
    );

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(config_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // filesystem and api-server are converted (disabled-server has enabled=false)
    assert!(ir.children.len() >= 2, "Expected at least 2 children");

    // filesystem server: timeout is converted
    let fs = ir.children.iter().find(|c| c.source_path == "filesystem");
    assert!(fs.is_some(), "Expected filesystem server");
    let fs = fs.unwrap();
    // tool_timeout_sec=30.0 → timeout=30000
    if let Some(timeout) = fs.fields.get("mcp.timeout") {
        assert_eq!(
            timeout.value.as_i64().unwrap_or(0),
            30000,
            "Expected timeout=30000ms"
        );
    }

    // Check whether disabled-server has its disabled flag set
    let disabled = ir
        .children
        .iter()
        .find(|c| c.source_path == "disabled-server");
    if let Some(d) = disabled {
        let has_disabled_flag = d.fields.contains_key("__disabled")
            || d.diagnostics
                .iter()
                .any(|diag| diag.message.contains("enabled=false"));
        assert!(
            has_disabled_flag,
            "Expected disabled-server to be marked disabled"
        );
    }
}

/// .mcp.json is generated after x2c conversion.
#[test]
fn test_mcp_x2c_lower_generates_claude_mcp_json() {
    let config_path = "tests/fixtures/codex/config.toml";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(config_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(config_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

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

    let mcp_file = plan.files.iter().find(|f| f.path.ends_with(".mcp.json"));
    assert!(mcp_file.is_some(), "Expected .mcp.json in output");

    let content: serde_json::Value = serde_json::from_str(&mcp_file.unwrap().content).unwrap();
    let servers = content["mcpServers"]
        .as_object()
        .expect("mcpServers should be object");

    assert!(
        servers.contains_key("filesystem"),
        "Expected filesystem server in .mcp.json"
    );

    // Servers with enabled=false must not appear in output.
    assert!(
        !servers.contains_key("disabled-server"),
        "disabled-server should be excluded from .mcp.json"
    );
}

/// Simulate the ccx check command: report the dropped count.
#[test]
fn test_check_skill_reports_dropped_count() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    println!(
        "check: {}\n  dropped: {}, degraded: {}, lossy: {}, lossless: {}",
        skill_path,
        report.dropped.len(),
        report.degraded.len(),
        report.lossy.len(),
        report.lossless.len()
    );

    assert!(report.lossless.contains(&"skills.name".to_string()));
    assert!(report.lossless.contains(&"skills.description".to_string()));

    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        dropped_ids.contains(&"skills.user-invocable"),
        "user-invocable should be dropped, got: {:?}",
        dropped_ids
    );

    // body warnings must be present (dynamic injection and variable references exist)
    assert!(
        !report.body_warnings.is_empty(),
        "Expected body warnings from skill body"
    );
}

/// Files are generated after c2x lower and the SKILL.md content is correct.
#[test]
fn test_skill_c2x_lower_generates_skill_md() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let skill_file = plan.files.iter().find(|f| f.path.ends_with("SKILL.md"));
    assert!(skill_file.is_some(), "Expected SKILL.md in output");

    let content = &skill_file.unwrap().content;
    assert!(content.contains("deploy"), "Expected 'deploy' in SKILL.md");
    assert!(
        content.contains("Deploy the application"),
        "Expected description in SKILL.md"
    );
    assert!(
        content.contains("Use this skill when"),
        "Expected when_to_use concatenated into description"
    );

    // A .rules file must be generated (Bash tool degrade)
    let rules_file = plan.files.iter().find(|f| f.path.ends_with(".rules"));
    assert!(
        rules_file.is_some(),
        "Expected .rules file for Bash tool degrade"
    );

    // A subagent TOML must be generated (model/effort degrade)
    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.contains(".codex/agents/") && f.path.ends_with(".toml"));
    assert!(
        agent_toml.is_some(),
        "Expected subagent .toml for model/effort degrade"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// P2: Hooks tests
// ────────────────────────────────────────────────────────────────────────────

/// hooks.json c2x: common events are converted; Claude-only events are dropped.
#[test]
fn test_hooks_c2x_basic() {
    let hooks_path = "tests/fixtures/claude/hooks.json";
    assert!(
        Path::new(hooks_path).exists(),
        "Fixture {} must exist",
        hooks_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(hooks_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Hooks);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // Common events should be Lossless
    let pre_tool_use = ir.fields.get("hooks.event.PreToolUse");
    assert!(pre_tool_use.is_some(), "Expected PreToolUse field");
    assert_eq!(
        pre_tool_use.unwrap().loss,
        ccx::core::ir::Loss::Lossless,
        "PreToolUse should be lossless"
    );

    // Setup is Claude-only → Dropped
    let setup = ir.fields.get("hooks.event.Setup");
    assert!(setup.is_some(), "Expected Setup field");
    assert_eq!(
        setup.unwrap().loss,
        ccx::core::ir::Loss::Dropped,
        "Setup should be dropped"
    );

    // Report check
    let report = build_report(&ir, &empty_plan());
    assert!(
        !report.dropped.is_empty(),
        "Expected dropped events in report"
    );
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"hooks.event.Setup"),
        "Expected Setup in dropped, got: {:?}",
        drop_ids
    );
    assert!(
        drop_ids.contains(&"hooks.event.Notification"),
        "Expected Notification in dropped, got: {:?}",
        drop_ids
    );
}

/// hooks.json c2x lower (user scope): hooks.json is generated and the matcher is normalized.
#[test]
fn test_hooks_c2x_lower_user_scope() {
    let hooks_path = "tests/fixtures/claude/hooks.json";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(hooks_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

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
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let hooks_file = plan.files.iter().find(|f| f.path.ends_with("hooks.json"));
    assert!(hooks_file.is_some(), "Expected hooks.json output");

    let content: serde_json::Value = serde_json::from_str(&hooks_file.unwrap().content).unwrap();

    // PreToolUse should be present with normalized matcher
    let pre_tool_entries = content["PreToolUse"].as_array();
    assert!(
        pre_tool_entries.is_some(),
        "Expected PreToolUse in hooks.json"
    );
    let first_entry = &pre_tool_entries.unwrap()[0];
    let matcher = first_entry["matcher"].as_str().unwrap_or("");
    assert_eq!(
        matcher, "^Bash$",
        "Expected normalized matcher ^Bash$, got: {}",
        matcher
    );

    // Setup should NOT be present (dropped)
    assert!(
        content.get("Setup").is_none(),
        "Setup event should not be in output"
    );

    // #16430 warning should be in diagnostics
    let has_16430 = plan
        .diagnostics
        .iter()
        .any(|d| d.message.contains("#16430"));
    assert!(has_16430, "Expected #16430 warning in diagnostics");
}

/// hooks.json c2x lower (project scope): .codex/config.toml is generated.
#[test]
fn test_hooks_c2x_lower_project_scope() {
    let hooks_path = "tests/fixtures/claude/hooks.json";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(hooks_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::Project,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let config_file = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(config_file.is_some(), "Expected config.toml output");

    let content = &config_file.unwrap().content;
    assert!(
        content.contains("[[hooks.PreToolUse]]"),
        "Expected [[hooks.PreToolUse]] in config.toml, got: {}",
        content
    );
    assert!(
        !content.contains("[[hooks.Setup]]"),
        "Setup event should not be in config.toml"
    );
}

/// Hooks matcher normalization test (Edit|Write → ^(Edit|Write)$).
#[test]
fn test_hooks_c2x_matcher_alternation_normalized() {
    let hooks_json = serde_json::json!({
        "hooks": {
            "PostToolUse": [
                {
                    "matcher": "Edit|Write",
                    "hooks": [
                        { "type": "command", "command": "lint.sh" }
                    ]
                }
            ]
        }
    });

    // Use hooks handler directly (not via fixture)
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::hooks::HooksHandler {
        map: maps["hooks"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&hooks_json, ConvDir::C2x).unwrap();

    let field = ir.fields.get("hooks.event.PostToolUse").unwrap();
    let entries = field.value.as_array().unwrap();
    let matcher = entries[0]["matcher"].as_str().unwrap();
    assert_eq!(
        matcher, "^(Edit|Write)$",
        "Expected alternation matcher normalized"
    );
    let has_lossy_warn = ir
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Edit|Write"));
    assert!(
        has_lossy_warn,
        "Expected matcher normalization warn diagnostic"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// P2: Memory tests
// ────────────────────────────────────────────────────────────────────────────

/// CLAUDE.md c2x: AGENTS.md is generated and its content is preserved.
#[test]
fn test_memory_c2x_basic() {
    let memory_path = "tests/fixtures/claude/CLAUDE.md";
    assert!(
        Path::new(memory_path).exists(),
        "Fixture {} must exist",
        memory_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(memory_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Memory);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(memory_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, ccx::core::ir::Kind::Memory);

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let agents_md = plan.files.iter().find(|f| f.path.ends_with("AGENTS.md"));
    assert!(agents_md.is_some(), "Expected AGENTS.md in output");
    let content = &agents_md.unwrap().content;
    assert!(
        content.contains("Project Instructions"),
        "Expected content preserved in AGENTS.md"
    );
}

/// AGENTS.md x2c: CLAUDE.md is generated.
#[test]
fn test_memory_x2c_basic() {
    let memory_path = "tests/fixtures/codex/AGENTS.md";
    assert!(
        Path::new(memory_path).exists(),
        "Fixture {} must exist",
        memory_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(memory_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Memory);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(memory_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let claude_md = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
    assert!(claude_md.is_some(), "Expected CLAUDE.md in output");
    let content = &claude_md.unwrap().content;
    assert!(
        content.contains("Agent Instructions"),
        "Expected content preserved in CLAUDE.md"
    );
}

/// insta snapshot test: report JSON output must be stable.
#[test]
fn test_skill_c2x_report_snapshot() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");
    let report = build_report(&ir, &empty_plan());

    // Only snapshot stable output fields
    let snapshot = serde_json::json!({
        "lossless_count": report.lossless.len(),
        "dropped_count": report.dropped.len(),
        "body_warnings_count": report.body_warnings.len(),
        "lossless_includes_name": report.lossless.contains(&"skills.name".to_string()),
        "lossless_includes_description": report.lossless.contains(&"skills.description".to_string()),
    });

    insta::assert_json_snapshot!("skill_c2x_report_summary", snapshot);
}

// ────────────────────────────────────────────────────────────────────────────
// P3: Plugins tests
// ────────────────────────────────────────────────────────────────────────────

/// plugin.json c2x: .codex-plugin/plugin.json is generated.
#[test]
fn test_plugin_c2x_generates_codex_manifest() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";
    assert!(
        Path::new(plugin_path).exists(),
        "Fixture {} must exist",
        plugin_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Plugin);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, ccx::core::ir::Kind::Plugin);
    // name and description should be lossless
    assert!(
        ir.fields.contains_key("plugins.name"),
        "Expected plugins.name field"
    );
    assert_eq!(
        ir.fields["plugins.name"].loss,
        ccx::core::ir::Loss::Lossless
    );

    // lspServers and userConfig should be dropped
    let has_lsp_dropped = ir
        .fields
        .get("plugins.lspServers")
        .map(|f| matches!(f.loss, ccx::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(has_lsp_dropped, "lspServers should be dropped");

    let has_user_config_dropped = ir
        .fields
        .get("plugins.userConfig")
        .map(|f| matches!(f.loss, ccx::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(has_user_config_dropped, "userConfig should be dropped");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let codex_manifest = plan
        .files
        .iter()
        .find(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
    assert!(
        codex_manifest.is_some(),
        "Expected .codex-plugin/plugin.json in output"
    );

    let content: serde_json::Value =
        serde_json::from_str(&codex_manifest.unwrap().content).unwrap();
    assert_eq!(content["name"].as_str(), Some("demo-plugin"));
    assert_eq!(content["version"].as_str(), Some("1.0.0"));
    assert_eq!(content["license"].as_str(), Some("MIT"));
}

/// plugin.json c2x: skills and .mcp.json are processed via recursive conversion.
#[test]
fn test_plugin_c2x_recursion() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let skill_children: Vec<_> = ir
        .children
        .iter()
        .filter(|c| c.kind == ccx::core::ir::Kind::Skill)
        .collect();
    assert!(
        !skill_children.is_empty(),
        "Expected skill children from recursion"
    );

    let mcp_children: Vec<_> = ir
        .children
        .iter()
        .filter(|c| c.kind == ccx::core::ir::Kind::Mcp)
        .collect();
    assert!(
        !mcp_children.is_empty(),
        "Expected MCP children from recursion"
    );
}

/// Verify the dropped classification for plugin.json c2x.
#[test]
fn test_plugin_c2x_dropped_classification() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        dropped_ids.contains(&"plugins.lspServers"),
        "Expected plugins.lspServers in dropped, got: {:?}",
        dropped_ids
    );
    assert!(
        dropped_ids.contains(&"plugins.userConfig"),
        "Expected plugins.userConfig in dropped, got: {:?}",
        dropped_ids
    );

    // A userConfig warn must be emitted (unresolved-variable risk)
    let has_user_config_warn = ir.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("plugins.userConfig") && d.level == ccx::core::ir::DiagLevel::Warn
    });
    assert!(
        has_user_config_warn,
        "Expected userConfig unresolved-variable warn"
    );
}

/// plugin.json c2x --dual-manifest: both manifests are generated.
#[test]
fn test_plugin_c2x_dual_manifest() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: true,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let has_claude = plan
        .files
        .iter()
        .any(|f| f.path.contains(".claude-plugin") && f.path.ends_with("plugin.json"));
    let has_codex = plan
        .files
        .iter()
        .any(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
    assert!(
        has_claude,
        "Expected .claude-plugin/plugin.json with dual-manifest"
    );
    assert!(
        has_codex,
        "Expected .codex-plugin/plugin.json with dual-manifest"
    );
}

/// plugin.json c2x: marketplace.json is converted and policy defaults are filled in.
#[test]
fn test_plugin_c2x_marketplace_policy_defaults() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let marketplace_file = plan
        .files
        .iter()
        .find(|f| f.path.contains("marketplace.json"));
    assert!(
        marketplace_file.is_some(),
        "Expected marketplace.json in output"
    );

    let content: serde_json::Value =
        serde_json::from_str(&marketplace_file.unwrap().content).unwrap();
    let plugins = content["plugins"]
        .as_array()
        .expect("Expected plugins array");
    assert!(!plugins.is_empty(), "Expected at least one plugin entry");

    let policy = &plugins[0]["policy"];
    assert!(policy.is_object(), "Expected policy object");
    assert_eq!(
        policy["installation"].as_str(),
        Some("AVAILABLE"),
        "Expected installation=AVAILABLE"
    );
    assert_eq!(
        policy["authentication"].as_str(),
        Some("ON_INSTALL"),
        "Expected authentication=ON_INSTALL"
    );

    let has_policy_warn = plan
        .diagnostics
        .iter()
        .any(|d| d.message.contains("policy"));
    assert!(has_policy_warn, "Expected policy auto-fill warning");
}

// ────────────────────────────────────────────────────────────────────────────
// P4: Subagent tests
// ────────────────────────────────────────────────────────────────────────────

/// Claude agents/<n>.md c2x: .codex/agents/<n>.toml is generated.
#[test]
fn test_subagent_c2x_generates_codex_toml() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    assert_eq!(
        kind,
        ccx::core::ir::Kind::Subagent,
        "agents/<n>.md should be Kind::Subagent"
    );

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, ccx::core::ir::Kind::Subagent);
    assert!(ir.fields.contains_key("subagents.name"));
    assert!(ir.fields.contains_key("subagents.description"));

    // name should be lossless
    let name_f = &ir.fields["subagents.name"];
    assert_eq!(
        name_f.loss,
        ccx::core::ir::Loss::Lossless,
        "name should be lossless"
    );
    assert_eq!(
        name_f.value,
        serde_json::Value::String("researcher".to_string())
    );

    // model is lossy (different providers)
    let model_f = &ir.fields["subagents.model"];
    assert_eq!(
        model_f.loss,
        ccx::core::ir::Loss::Lossy,
        "model should be lossy"
    );

    // effort: high → high (1:1, lossless in terms of transform)
    let effort_f = &ir.fields["subagents.effort"];
    assert_eq!(
        effort_f.value,
        serde_json::Value::String("high".to_string())
    );

    // maxTurns → dropped
    let max_turns = &ir.fields["subagents.maxTurns"];
    assert_eq!(
        max_turns.loss,
        ccx::core::ir::Loss::Dropped,
        "maxTurns should be dropped"
    );

    // background → dropped
    let bg = &ir.fields["subagents.background"];
    assert_eq!(
        bg.loss,
        ccx::core::ir::Loss::Dropped,
        "background should be dropped"
    );

    // color → dropped
    let color = &ir.fields["subagents.color"];
    assert_eq!(
        color.loss,
        ccx::core::ir::Loss::Dropped,
        "color should be dropped"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("researcher.toml"));
    assert!(
        agent_toml.is_some(),
        "Expected researcher.toml in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &agent_toml.unwrap().content;
    assert!(content.contains("researcher"), "name should be in TOML");
    assert!(
        content.contains("Conduct research"),
        "description should be in TOML"
    );
    assert!(
        content.contains("developer_instructions"),
        "body should be in developer_instructions"
    );
    assert!(
        content.contains("research specialist"),
        "body text should be present"
    );
    assert!(
        content.contains("model_reasoning_effort"),
        "effort should be in TOML"
    );
    assert!(content.contains("high"), "effort value should be in TOML");
}

/// .codex/agents/<n>.toml x2c: .claude/agents/<n>.md is generated.
#[test]
fn test_subagent_x2c_generates_claude_md() {
    let agent_path = "tests/fixtures/codex/agents/coder.toml";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Subagent);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    assert_eq!(ir.kind, ccx::core::ir::Kind::Subagent);

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

    let agent_md = plan.files.iter().find(|f| f.path.ends_with("coder.md"));
    assert!(
        agent_md.is_some(),
        "Expected coder.md in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &agent_md.unwrap().content;
    assert!(content.contains("coder"), "name should be in frontmatter");
    assert!(
        content.contains("Write, review"),
        "description should be in frontmatter"
    );
    assert!(
        content.contains("expert software engineer"),
        "developer_instructions should be in body"
    );
}

/// Subagent c2x roundtrip: report enumerates dropped fields.
#[test]
fn test_subagent_c2x_report_dropped_fields() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    assert!(
        !report.dropped.is_empty(),
        "Expected dropped fields in subagent report"
    );
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"subagents.maxTurns"),
        "Expected subagents.maxTurns in dropped, got: {:?}",
        drop_ids
    );
    assert!(
        drop_ids.contains(&"subagents.background"),
        "Expected subagents.background in dropped, got: {:?}",
        drop_ids
    );
    assert!(
        drop_ids.contains(&"subagents.color"),
        "Expected subagents.color in dropped, got: {:?}",
        drop_ids
    );

    // A spawn-model warn must be emitted
    let has_spawn_warn = ir
        .diagnostics
        .iter()
        .any(|d| d.message.contains("spawn_agent"));
    assert!(
        has_spawn_warn,
        "Expected spawn_agent warning about auto-delegation difference"
    );
}

/// Subagent c2x: lower emits config.toml with [agents.<name>] pointer and
/// [features] multi_agent = true alongside the agent TOML file.
#[test]
fn test_subagent_c2x_emits_config_toml_with_agents_and_features() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // The agent TOML must be present.
    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("researcher.toml"));
    assert!(
        agent_toml.is_some(),
        "Expected researcher.toml in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // A config.toml must also be emitted with [agents.researcher] and multi_agent.
    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml in output (spec §10.2), got files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &config_toml.unwrap().content;
    assert!(
        content.contains("[agents.researcher]"),
        "Expected [agents.researcher] in config.toml, got:\n{}",
        content
    );
    assert!(
        content.contains("config_file"),
        "Expected config_file pointer in config.toml, got:\n{}",
        content
    );
    assert!(
        content.contains("multi_agent"),
        "Expected multi_agent in config.toml [features], got:\n{}",
        content
    );
    assert!(
        content.contains("true"),
        "Expected multi_agent = true in config.toml, got:\n{}",
        content
    );
}

/// x2c: Codex TOML with [skills] table is lifted into subagents.skills and
/// lowered to `skills:` list in the Claude agent frontmatter.
#[test]
fn test_subagent_x2c_skills_lifted_integration() {
    let agent_path = "tests/fixtures/codex/agents/skills-agent.toml";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Subagent);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // skills.config must be lifted into IR as subagents.skills — not dropped
    assert!(
        ir.fields.contains_key("subagents.skills"),
        "IR must contain subagents.skills from Codex [skills] table; got fields: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );

    // Must not appear as unknown drop diagnostic
    let has_unknown_drop = ir
        .diagnostics
        .iter()
        .any(|d| d.level == ccx::core::ir::DiagLevel::Drop && d.message.contains("skills"));
    assert!(
        !has_unknown_drop,
        "Must not drop 'skills' as unknown key; diagnostics: {:?}",
        ir.diagnostics
    );

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

    let agent_md = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("skills-agent.md"))
        .unwrap_or_else(|| {
            panic!(
                "Expected skills-agent.md in output, got: {:?}",
                plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        });

    let content = &agent_md.content;
    assert!(
        content.contains("python"),
        "Output .md must contain 'python' in skills; got:\n{}",
        content
    );
    assert!(
        content.contains("javascript"),
        "Output .md must contain 'javascript' in skills; got:\n{}",
        content
    );
    assert!(
        content.contains("skills"),
        "Output .md must contain 'skills' frontmatter key; got:\n{}",
        content
    );

    // A Warn diagnostic for the lossy skills mapping must be emitted
    let has_skills_warn = plan.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("subagents.skills") && d.level == ccx::core::ir::DiagLevel::Warn
    });
    assert!(
        has_skills_warn,
        "Expected subagents.skills Warn diagnostic; got: {:?}",
        plan.diagnostics
    );
}

// ────────────────────────────────────────────────────────────────────────────
// P4: Settings tests
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

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(settings_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Settings);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(settings_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, ccx::core::ir::Kind::Settings);

    assert!(ir.fields.contains_key("settings.model"));
    assert!(ir.fields.contains_key("settings.effortLevel"));

    // effortLevel high → high is a lossless 1:1 mapping
    let effort = &ir.fields["settings.effortLevel"];
    assert_eq!(effort.value, serde_json::Value::String("high".to_string()));

    assert!(ir.fields.contains_key("settings.editorMode"));

    let has_viewmode_dropped = ir
        .fields
        .get("settings.viewMode")
        .map(|f| matches!(f.loss, ccx::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(
        has_viewmode_dropped,
        "Expected settings.viewMode to be dropped"
    );

    let has_worktree_dropped = ir
        .fields
        .get("settings.worktree")
        .map(|f| matches!(f.loss, ccx::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(
        has_worktree_dropped,
        "Expected settings.worktree to be dropped"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
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

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(settings_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
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

    let maps = load_mappings(Path::new(MAPPINGS_DIR));

    // Test SettingsHandler directly (detect targets config.toml, so call it directly)
    use ccx::handlers::settings::SettingsHandler;
    use ccx::handlers::Handler;

    let handler = SettingsHandler {
        map: maps["settings-config"].clone(),
    };

    let parsed = handler
        .parse(Path::new(settings_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    assert_eq!(ir.kind, ccx::core::ir::Kind::Settings);
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
// batch-flags gap: Write/Read/Edit allowed-tools must produce config.toml SideArtifact
// ────────────────────────────────────────────────────────────────────────────

/// c2x lower for a skill with Write/Read allowed-tools must emit a config.toml
/// file containing [permissions.<skill>].filesystem entries ("write" and "read").
#[test]
fn test_skill_c2x_write_read_tools_produce_config_toml() {
    let skill_path = "tests/fixtures/claude/skills/ed/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // A config.toml file must be emitted for the Write/Read tool permissions.
    let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(
        config_toml.is_some(),
        "Expected config.toml SideArtifact for Write/Read tool degrade. Got files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let content = &config_toml.unwrap().content;
    assert!(
        content.contains("[permissions.ed]"),
        "Expected [permissions.ed] table in config.toml, got:\n{}",
        content
    );
    assert!(
        content.contains("write"),
        "Expected 'write' value for Write(**/*.py) glob, got:\n{}",
        content
    );
    assert!(
        content.contains("read"),
        "Expected 'read' value for Read(~/.ssh/*) glob, got:\n{}",
        content
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 4/42: disable-model-invocation silently dropped in c2x
// ────────────────────────────────────────────────────────────────────────────

/// c2x lift of a SKILL.md with disable-model-invocation=true must produce
/// an IR field with loss=Lossy and a warning (not silently dropped).
#[test]
fn test_skill_c2x_disable_model_invocation_in_report() {
    let skill_path = "tests/fixtures/claude/skills/s/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    let lossy_ids: Vec<_> = report
        .lossy
        .iter()
        .filter_map(|e| e.id.as_deref())
        .collect();
    assert!(
        lossy_ids.contains(&"skills.disable-model-invocation"),
        "Expected skills.disable-model-invocation in lossy report entries, got: {:?}",
        lossy_ids
    );
}

/// c2x lower of a SKILL.md with disable-model-invocation=true must emit
/// .agents/skills/s/agents/openai.yaml with allow_implicit_invocation: false.
#[test]
fn test_skill_c2x_disable_model_invocation_lower_emits_openai_yaml() {
    let skill_path = "tests/fixtures/claude/skills/s/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let openai_yaml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("agents/openai.yaml"))
        .expect("Expected .agents/skills/s/agents/openai.yaml in emit plan");

    assert!(
        openai_yaml
            .content
            .contains("allow_implicit_invocation: false"),
        "openai.yaml must contain 'allow_implicit_invocation: false', got:\n{}",
        openai_yaml.content
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 5/42: OAuth nested fields silently dropped in both c2x and x2c
// ────────────────────────────────────────────────────────────────────────────

/// c2x: .mcp.json with oauth sub-object must produce IR fields for
/// mcp.oauth.client_id (lossless), mcp.oauth.scopes (lossless, split by space),
/// mcp.oauth.callback_port (lossy, warn), and mcp.oauth.auth_server_metadata_url
/// (dropped + warn). The oauth-server fixture already contains clientId and scopes.
#[test]
fn test_mcp_c2x_oauth_fields_in_ir() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(mcp_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::C2x).expect("lift ok");

    let oauth_child = ir
        .children
        .iter()
        .find(|c| c.source_path == "oauth-server")
        .expect("Expected 'oauth-server' child");

    // mcp.oauth.client_id must be present and lossless
    let client_id = oauth_child
        .fields
        .get("mcp.oauth.client_id")
        .expect("Expected mcp.oauth.client_id in IR");
    assert_eq!(
        client_id.value,
        serde_json::Value::String("my-client-id".to_string()),
        "client_id value mismatch"
    );
    assert_eq!(
        client_id.loss,
        ccx::core::ir::Loss::Lossless,
        "mcp.oauth.client_id must be lossless"
    );

    // mcp.oauth.scopes must be present, lossless, and split into an array
    let scopes = oauth_child
        .fields
        .get("mcp.oauth.scopes")
        .expect("Expected mcp.oauth.scopes in IR");
    assert_eq!(
        scopes.loss,
        ccx::core::ir::Loss::Lossless,
        "mcp.oauth.scopes must be lossless"
    );
    let scopes_arr = scopes
        .value
        .as_array()
        .expect("mcp.oauth.scopes must be array after str_to_list:space");
    assert_eq!(
        scopes_arr,
        &vec![
            serde_json::Value::String("read".to_string()),
            serde_json::Value::String("write".to_string()),
            serde_json::Value::String("admin".to_string()),
        ],
        "scopes must be split by whitespace"
    );

    // No unknown-field diagnostic for oauth
    let has_unknown_oauth_diag = oauth_child
        .diagnostics
        .iter()
        .any(|d| d.message.contains("unknown MCP server field: oauth"));
    assert!(
        !has_unknown_oauth_diag,
        "oauth object must NOT produce 'unknown MCP server field: oauth' diagnostic"
    );
}

/// c2x lower: oauth-server must produce a Codex .mcp.json with oauth.client_id
/// and scopes array present.
#[test]
fn test_mcp_c2x_oauth_lower_output() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(mcp_path).expect("detect ok");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(mcp_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::C2x).expect("lift ok");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let oauth_server = &content["mcpServers"]["oauth-server"];
    assert!(
        oauth_server.is_object(),
        "Expected oauth-server in mcpServers"
    );

    // oauth.client_id must be present (renamed from clientId)
    let client_id = &oauth_server["oauth"]["client_id"];
    assert_eq!(
        client_id,
        &serde_json::Value::String("my-client-id".to_string()),
        "oauth.client_id must be present in Codex output"
    );

    // oauth.scopes must be an array
    let scopes = &oauth_server["oauth"]["scopes"];
    assert!(
        scopes.is_array(),
        "oauth.scopes must be array in Codex output"
    );
    let scopes_arr = scopes.as_array().unwrap();
    assert_eq!(scopes_arr.len(), 3, "Expected 3 scopes");
}

/// x2c: Codex config.toml with [mcp_servers.oauth-server.oauth] must produce IR
/// fields for mcp.oauth.client_id (lossless) and mcp.oauth.scopes (lossless,
/// joined to space-separated string).
#[test]
fn test_mcp_x2c_oauth_fields_in_ir() {
    let config_path = "tests/fixtures/codex/config.toml";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(config_path).expect("detect ok");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(config_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::X2c).expect("lift ok");

    let oauth_child = ir
        .children
        .iter()
        .find(|c| c.source_path == "oauth-server")
        .expect("Expected 'oauth-server' child in x2c IR");

    // mcp.oauth.client_id must be lossless
    let client_id = oauth_child
        .fields
        .get("mcp.oauth.client_id")
        .expect("Expected mcp.oauth.client_id in x2c IR");
    assert_eq!(
        client_id.value,
        serde_json::Value::String("my-client-id".to_string()),
        "client_id value mismatch in x2c"
    );
    assert_eq!(
        client_id.loss,
        ccx::core::ir::Loss::Lossless,
        "mcp.oauth.client_id must be lossless in x2c"
    );

    // mcp.oauth.scopes must be lossless and joined to a space-separated string
    let scopes = oauth_child
        .fields
        .get("mcp.oauth.scopes")
        .expect("Expected mcp.oauth.scopes in x2c IR");
    assert_eq!(
        scopes.loss,
        ccx::core::ir::Loss::Lossless,
        "mcp.oauth.scopes must be lossless in x2c"
    );
    assert_eq!(
        scopes.value,
        serde_json::Value::String("read write admin".to_string()),
        "scopes must be joined to space-separated string in x2c"
    );
}

/// x2c lower: oauth-server must produce a Claude .mcp.json with
/// oauth.clientId and oauth.scopes (space-separated string).
#[test]
fn test_mcp_x2c_oauth_lower_output() {
    let config_path = "tests/fixtures/codex/config.toml";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(config_path).expect("detect ok");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(config_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::X2c).expect("lift ok");

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
    let plan = handler.lower(&ir, ConvDir::X2c, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output in x2c");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let oauth_server = &content["mcpServers"]["oauth-server"];
    assert!(
        oauth_server.is_object(),
        "Expected oauth-server in mcpServers (x2c)"
    );

    // oauth.clientId must be present (renamed from client_id)
    let client_id = &oauth_server["oauth"]["clientId"];
    assert_eq!(
        client_id,
        &serde_json::Value::String("my-client-id".to_string()),
        "oauth.clientId must be present in Claude output"
    );

    // oauth.scopes must be a space-separated string
    let scopes = &oauth_server["oauth"]["scopes"];
    assert_eq!(
        scopes,
        &serde_json::Value::String("read write admin".to_string()),
        "oauth.scopes must be space-separated string in Claude output"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 6/42: c2x env_http_headers values emitted with '$' prefix
// ────────────────────────────────────────────────────────────────────────────

/// c2x lift: headers with ${VAR} form must produce env_http_headers with bare
/// variable name (no '$' prefix).
#[test]
fn test_mcp_c2x_env_http_headers_bare_var_name_in_ir() {
    // Use only non-Bearer env-var headers to test the env_http_headers path
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://example.com/mcp",
                    "headers": {
                        "X-Api-Key": "${API_KEY}",
                        "X-Tenant": "${TENANT_ID}"
                    }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

    let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

    // X-Api-Key: "${API_KEY}" must be in env_http_headers with bare var name
    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be an object");
    let api_key_val = hdr_obj.get("X-Api-Key").expect("X-Api-Key must be present");
    assert_eq!(
        api_key_val,
        &serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers value must be bare var name 'API_KEY', not '$API_KEY'"
    );
    let tenant_val = hdr_obj.get("X-Tenant").expect("X-Tenant must be present");
    assert_eq!(
        tenant_val,
        &serde_json::Value::String("TENANT_ID".to_string()),
        "env_http_headers value must be bare var name 'TENANT_ID', not '$TENANT_ID'"
    );
}

/// c2x lower: headers with ${VAR} form must produce env_http_headers with bare
/// variable name (no '$' prefix) in the emitted Codex .mcp.json.
#[test]
fn test_mcp_c2x_env_http_headers_bare_var_name_in_output() {
    let mcp_path = "tests/fixtures/claude/env_http_headers_project/.mcp.json";
    assert!(
        Path::new(mcp_path).exists(),
        "Fixture {} must exist",
        mcp_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(mcp_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::C2x).expect("lift ok");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let server = &content["mcpServers"]["env-header-server"];
    let env_http = &server["env_http_headers"];
    assert!(env_http.is_object(), "Expected env_http_headers object");

    let x_api_key = &env_http["X-Api-Key"];
    assert_eq!(
        x_api_key,
        &serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers['X-Api-Key'] must be bare 'API_KEY', not '$API_KEY'"
    );
}

/// c2x lift: http transport env with ${VAR} form must produce env_http_headers
/// with bare variable name (no '$' prefix).
#[test]
fn test_mcp_c2x_env_to_env_http_headers_bare_var_name() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "http-env-server": {
                    "type": "http",
                    "url": "https://example.com/mcp",
                    "env": {
                        "X-Service-Key": "${SERVICE_KEY}"
                    }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

    let server = ir
        .children
        .iter()
        .find(|c| c.source_path == "http-env-server")
        .unwrap();

    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present for http transport env");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be object");
    let val = hdr_obj
        .get("X-Service-Key")
        .expect("X-Service-Key must be present");
    assert_eq!(
        val,
        &serde_json::Value::String("SERVICE_KEY".to_string()),
        "env_http_headers value must be bare 'SERVICE_KEY', not '$SERVICE_KEY'"
    );
}

/// x2c lift: env_http_headers with bare var name must produce Claude headers
/// with ${VAR} form.
#[test]
fn test_mcp_x2c_env_http_headers_becomes_dollar_brace_in_headers() {
    // Codex parsed structure: env_http_headers values are bare var names
    let parsed = serde_json::json!({
        "frontmatter": {
            "mcp_servers": {
                "env-header-server": {
                    "url": "https://api.example.com/mcp",
                    "env_http_headers": {
                        "X-Api-Key": "API_KEY",
                        "X-Tenant": "TENANT_ID"
                    }
                }
            }
        },
        "body": ""
    });

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&parsed, ConvDir::X2c).expect("lift ok");

    let server = ir
        .children
        .iter()
        .find(|c| c.source_path == "env-header-server")
        .expect("Expected 'env-header-server' child");

    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present in x2c IR");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be object");
    let x_api_key = hdr_obj.get("X-Api-Key").expect("X-Api-Key must be present");
    assert_eq!(
        x_api_key,
        &serde_json::Value::String("API_KEY".to_string()),
        "x2c IR must preserve bare var name in env_http_headers"
    );

    // Lower to Claude and check headers become ${VAR} form
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
    let plan = handler.lower(&ir, ConvDir::X2c, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let server_cfg = &content["mcpServers"]["env-header-server"];
    let headers = &server_cfg["headers"];
    assert!(
        headers.is_object(),
        "Expected headers object in Claude .mcp.json"
    );

    let x_api_key_header = &headers["X-Api-Key"];
    assert_eq!(
        x_api_key_header,
        &serde_json::Value::String("${API_KEY}".to_string()),
        "x2c lower must convert bare 'API_KEY' to '${{API_KEY}}' in Claude headers"
    );
}

/// gap 7/42: x2c e2e — Codex config.toml with env_http_headers must produce
/// Claude .mcp.json headers with ${VAR} form (not bare var names).
///
/// Drives the full pipeline: parse from fixture file → lift → lower → assert
/// output file content.
#[test]
fn test_mcp_x2c_env_http_headers_e2e_dollar_brace_wrapping() {
    let config_path = "tests/fixtures/codex/env_http_headers_project/config.toml";
    assert!(
        Path::new(config_path).exists(),
        "Fixture {} must exist",
        config_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(config_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(config_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

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

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let headers = &content["mcpServers"]["auth-server"]["headers"];
    assert!(
        headers.is_object(),
        "Expected headers object in Claude .mcp.json, got: {headers}"
    );

    let authorization = &headers["Authorization"];
    assert_eq!(
        authorization,
        &serde_json::Value::String("${MY_AUTH_TOKEN}".to_string()),
        "env_http_headers 'MY_AUTH_TOKEN' must become '${{MY_AUTH_TOKEN}}' in Claude headers, got: {authorization}"
    );

    let x_custom = &headers["X-Custom"];
    assert_eq!(
        x_custom,
        &serde_json::Value::String("${MY_API_KEY}".to_string()),
        "env_http_headers 'MY_API_KEY' must become '${{MY_API_KEY}}' in Claude headers, got: {x_custom}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 8/42: c2x env_http_headers silently overwritten when server has both
// headers and env
// ────────────────────────────────────────────────────────────────────────────

/// c2x lift: when an http server has both headers (with ${VAR}) and env (with
/// ${VAR}), env_http_headers in the IR must contain entries from BOTH sources.
/// The headers-derived entry must not be silently overwritten.
#[test]
fn test_mcp_c2x_env_http_headers_merged_when_both_headers_and_env() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://x.com",
                    "headers": { "X-From-Headers": "${FROM_HEADERS}" },
                    "env":     { "API_KEY": "${API_KEY}" }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();
    let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be an object");

    assert!(
        hdr_obj.contains_key("X-From-Headers"),
        "X-From-Headers (from headers) must be in merged env_http_headers, got: {:?}",
        hdr_obj
    );
    assert_eq!(
        hdr_obj["X-From-Headers"],
        serde_json::Value::String("FROM_HEADERS".to_string()),
        "X-From-Headers value must be bare var name"
    );
    assert!(
        hdr_obj.contains_key("API_KEY"),
        "API_KEY (from env) must be in merged env_http_headers, got: {:?}",
        hdr_obj
    );
    assert_eq!(
        hdr_obj["API_KEY"],
        serde_json::Value::String("API_KEY".to_string()),
        "API_KEY value must be bare var name"
    );
}

/// c2x lower: when an http server has both headers and env, the emitted
/// env_http_headers must contain entries from both sources (no silent drop).
#[test]
fn test_mcp_c2x_env_http_headers_merged_in_output() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://x.com",
                    "headers": { "X-From-Headers": "${FROM_HEADERS}" },
                    "env":     { "API_KEY": "${API_KEY}" }
                }
            }
        },
        "body": ""
    });

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

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
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value = serde_json::from_str(&mcp_file.content).unwrap();

    let env_http = &content["mcpServers"]["s"]["env_http_headers"];
    assert!(
        env_http.is_object(),
        "Expected env_http_headers object in output"
    );
    assert!(
        env_http["X-From-Headers"] == serde_json::Value::String("FROM_HEADERS".to_string()),
        "X-From-Headers must be present in merged output, got: {:?}",
        env_http
    );
    assert!(
        env_http["API_KEY"] == serde_json::Value::String("API_KEY".to_string()),
        "API_KEY must be present in merged output, got: {:?}",
        env_http
    );

    // Report must reflect 0 unexpected drops
    let report = build_report(&ir, &plan);
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        !drop_ids.contains(&"mcp.env_http_headers"),
        "mcp.env_http_headers must not appear as dropped, got: {:?}",
        drop_ids
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 9/42: x2c from Codex hooks.json (flat JSON format) silently produces
// empty output — lift_x2c does not unwrap the frontmatter wrapper from
// parse_json_file before iterating event keys.
// ────────────────────────────────────────────────────────────────────────────

/// x2c from a flat Codex hooks.json must produce a Claude hooks.json with the
/// converted common events. The report must show at least one lossless entry
/// and no silently lost hooks.
#[test]
fn test_hooks_x2c_flat_json_produces_events() {
    let hooks_path = "tests/fixtures/codex/hooks.json";
    assert!(
        Path::new(hooks_path).exists(),
        "Fixture {} must exist",
        hooks_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(hooks_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Hooks);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // Both PreToolUse and Stop are common events → must be Lossless
    let pre_tool = ir.fields.get("hooks.event.PreToolUse");
    assert!(
        pre_tool.is_some(),
        "Expected hooks.event.PreToolUse in IR, but fields were: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        pre_tool.unwrap().loss,
        ccx::core::ir::Loss::Lossless,
        "PreToolUse must be Lossless in x2c"
    );

    let stop = ir.fields.get("hooks.event.Stop");
    assert!(stop.is_some(), "Expected hooks.event.Stop in IR");
    assert_eq!(
        stop.unwrap().loss,
        ccx::core::ir::Loss::Lossless,
        "Stop must be Lossless in x2c"
    );

    // Report must show lossless entries (not all empty)
    let report = build_report(&ir, &empty_plan());
    assert!(
        !report.lossless.is_empty(),
        "Report lossless must be non-empty; hooks were silently lost. lossless={:?}",
        report.lossless
    );
    assert!(
        report
            .lossless
            .contains(&"hooks.event.PreToolUse".to_string()),
        "hooks.event.PreToolUse must be in lossless report"
    );
}

/// x2c lower from a flat Codex hooks.json must produce a Claude hooks.json
/// with a non-empty "hooks" object containing the converted events.
#[test]
fn test_hooks_x2c_flat_json_lower_produces_hooks_json() {
    let hooks_path = "tests/fixtures/codex/hooks.json";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(hooks_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

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

    let hooks_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("hooks.json"))
        .expect("Expected hooks.json in output");

    let content: serde_json::Value =
        serde_json::from_str(&hooks_file.content).expect("output must be valid JSON");

    // The output wraps events under a "hooks" key
    let hooks_obj = content
        .get("hooks")
        .and_then(|v| v.as_object())
        .expect("Expected 'hooks' object in output Claude hooks.json");

    assert!(
        !hooks_obj.is_empty(),
        "hooks object must not be empty — all events were silently lost. content={}",
        hooks_file.content
    );
    assert!(
        hooks_obj.contains_key("PreToolUse"),
        "Expected PreToolUse in output hooks, got keys: {:?}",
        hooks_obj.keys().collect::<Vec<_>>()
    );
    assert!(
        hooks_obj.contains_key("Stop"),
        "Expected Stop in output hooks, got keys: {:?}",
        hooks_obj.keys().collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Regression: diagnostic deduplication (gap #10/42)
// Each dropped event/field should appear exactly once in the report.
// ────────────────────────────────────────────────────────────────────────────

/// A single CLAUDE_ONLY_EVENT drop must produce exactly one dropped entry in
/// the report, not two or three.
#[test]
fn test_hooks_c2x_claude_only_event_dropped_exactly_once() {
    use ccx::handlers::hooks::HooksHandler;
    use ccx::handlers::Handler;

    let hooks_json = serde_json::json!({
        "hooks": {
            "Notification": [
                {
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": "notify" }]
                }
            ]
        }
    });

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = HooksHandler {
        map: maps["hooks"].clone(),
    };

    let ir = handler.lift(&hooks_json, ConvDir::C2x).unwrap();

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
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();
    let report = build_report(&ir, &plan);

    // Filter to Notification-related dropped entries only (exclude #16430 etc.)
    let notification_drops: Vec<_> = report
        .dropped
        .iter()
        .filter(|d| {
            d.id.as_deref()
                .map(|id| id.contains("Notification"))
                .unwrap_or(false)
                || d.message.contains("Notification")
        })
        .collect();

    assert_eq!(
        notification_drops.len(),
        1,
        "Expected exactly 1 dropped entry for Notification, got {}: {:?}",
        notification_drops.len(),
        notification_drops
            .iter()
            .map(|d| (&d.id, &d.message))
            .collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 12/42: Codex interface.* sub-fields entirely lost in x2c direction
// ────────────────────────────────────────────────────────────────────────────

/// x2c: Codex plugin.json with a full `interface` object must expand each
/// sub-field through the mappings index rather than treating the whole object
/// as a single unknown field.
///
/// Asserts:
///   (a) interface.websiteURL → homepage (lossy) is present in IR
///   (b) interface.displayName → plugins.display-name (lossless) is present
///   (c) interface.brandColor is present with Loss::Dropped
///   (d) NO diagnostic with message "unknown plugin manifest field: interface"
#[test]
fn test_plugin_x2c_interface_fields_expanded() {
    let plugin_path = "tests/fixtures/codex/.codex-plugin/plugin.json";
    assert!(
        Path::new(plugin_path).exists(),
        "Fixture {} must exist",
        plugin_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    assert_eq!(kind, ccx::core::ir::Kind::Plugin);

    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // (a) interface.websiteURL → homepage must be present with Loss::Lossy
    let website_url = ir
        .fields
        .get("plugins.interface.websiteURL")
        .expect("plugins.interface.websiteURL must be present in IR");
    assert_eq!(
        website_url.loss,
        ccx::core::ir::Loss::Lossy,
        "plugins.interface.websiteURL must be Lossy"
    );
    assert_eq!(
        website_url.value,
        serde_json::Value::String("https://example.com".to_string()),
        "plugins.interface.websiteURL value mismatch"
    );

    // (b) interface.displayName → plugins.display-name must be present
    let display_name = ir
        .fields
        .get("plugins.display-name")
        .expect("plugins.display-name must be present in IR for interface.displayName");
    assert_eq!(
        display_name.value,
        serde_json::Value::String("Codex Plugin".to_string()),
        "plugins.display-name value mismatch"
    );

    // (c) interface.brandColor must be present with Loss::Dropped
    let brand_color = ir
        .fields
        .get("plugins.interface.brandColor")
        .expect("plugins.interface.brandColor must be present in IR");
    assert_eq!(
        brand_color.loss,
        ccx::core::ir::Loss::Dropped,
        "plugins.interface.brandColor must be Dropped"
    );

    // (d) NO undifferentiated "unknown plugin manifest field: interface" diagnostic
    let has_unknown_interface_diag = ir.diagnostics.iter().any(|d| {
        d.message
            .contains("unknown plugin manifest field: interface")
    });
    assert!(
        !has_unknown_interface_diag,
        "interface object must NOT produce 'unknown plugin manifest field: interface' diagnostic; each sub-field must be handled individually"
    );
}

/// x2c lower: Codex plugin.json with interface.websiteURL must emit `homepage`
/// in the Claude plugin.json output.
#[test]
fn test_plugin_x2c_interface_websiteurl_emits_homepage() {
    let plugin_path = "tests/fixtures/codex/.codex-plugin/plugin.json";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

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

    let claude_manifest = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude-plugin") && f.path.ends_with("plugin.json"))
        .expect("Expected .claude-plugin/plugin.json in x2c output");

    let content: serde_json::Value =
        serde_json::from_str(&claude_manifest.content).expect("output must be valid JSON");

    // interface.websiteURL → homepage
    assert_eq!(
        content["homepage"].as_str(),
        Some("https://example.com"),
        "interface.websiteURL must map to 'homepage' in Claude plugin.json, got: {}",
        content
    );

    // interface.displayName → displayName at top level
    assert_eq!(
        content["displayName"].as_str(),
        Some("Codex Plugin"),
        "interface.displayName must map to top-level 'displayName' in Claude plugin.json, got: {}",
        content
    );
}

/// A single exact-matcher normalization on a common event must produce exactly
/// one lossy entry (the matcher warn), excluding the #16430 warning.
#[test]
fn test_hooks_c2x_exact_matcher_lossy_exactly_once() {
    use ccx::handlers::hooks::HooksHandler;
    use ccx::handlers::Handler;

    let hooks_json = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": "echo pre" }]
                }
            ]
        }
    });

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = HooksHandler {
        map: maps["hooks"].clone(),
    };

    let ir = handler.lift(&hooks_json, ConvDir::C2x).unwrap();

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
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();
    let report = build_report(&ir, &plan);

    // Matcher-normalization warnings: exclude #16430 (plugin-bundled hooks warn)
    let matcher_lossy: Vec<_> = report
        .lossy
        .iter()
        .filter(|d| !d.message.contains("#16430"))
        .collect();

    assert_eq!(
        matcher_lossy.len(),
        1,
        "Expected exactly 1 lossy entry for matcher normalization (excl. #16430), got {}: {:?}",
        matcher_lossy.len(),
        matcher_lossy
            .iter()
            .map(|d| (&d.id, &d.message))
            .collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 14/42: Report duplication — warn:true + loss:dropped fields appear 2-3x
// ────────────────────────────────────────────────────────────────────────────

/// Each dropped field id must appear exactly once in report.dropped.
/// Fields like maxTurns (warn:true, loss:dropped) must not be duplicated.
#[test]
fn test_subagent_c2x_no_duplicate_dropped_entries() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");
    let report = build_report(&ir, &plan);

    // Every dropped field id must appear exactly once.
    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    for target_id in &[
        "subagents.maxTurns",
        "subagents.background",
        "subagents.color",
    ] {
        let count = dropped_ids.iter().filter(|id| *id == target_id).count();
        assert_eq!(
            count, 1,
            "Expected {} to appear exactly once in dropped, found {} times. dropped ids: {:?}",
            target_id, count, dropped_ids
        );
    }
}

/// A field that is in report.dropped must not also appear in report.lossy.
#[test]
fn test_subagent_c2x_dropped_and_lossy_are_disjoint() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");
    let report = build_report(&ir, &plan);

    let dropped_ids: std::collections::HashSet<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    let lossy_ids: Vec<_> = report
        .lossy
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    for lossy_id in &lossy_ids {
        assert!(
            !dropped_ids.contains(lossy_id),
            "Field '{}' appears in both dropped and lossy — fields must be in exactly one category",
            lossy_id
        );
    }
}

/// The spawn-model note must appear exactly once in report.lossy.
#[test]
fn test_subagent_c2x_spawn_model_appears_exactly_once_in_lossy() {
    let agent_path = "tests/fixtures/claude/agents/researcher.md";

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");
    let report = build_report(&ir, &plan);

    let spawn_model_id = "subagents.spawn-model";
    let count_in_lossy = report
        .lossy
        .iter()
        .filter(|e| e.id.as_deref() == Some(spawn_model_id))
        .count();
    assert_eq!(
        count_in_lossy, 1,
        "Expected spawn-model to appear exactly once in report.lossy, found {} times",
        count_in_lossy
    );

    // spawn-model must not be in dropped
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some(spawn_model_id));
    assert!(!in_dropped, "spawn-model must not appear in report.dropped");
}

// ────────────────────────────────────────────────────────────────────────────
// gap 15/42: permissionMode acceptEdits/dontAsk/auto must be dropped, not written
// ────────────────────────────────────────────────────────────────────────────

/// c2x of an agent with permissionMode=acceptEdits must NOT write sandbox_mode
/// to the output TOML, and the report must contain a drop diagnostic for
/// subagents.permissionMode.
#[test]
fn test_subagent_c2x_permission_mode_accept_edits_dropped() {
    let agent_path = "tests/fixtures/claude/agents/perm_accept_edits.md";
    assert!(
        Path::new(agent_path).exists(),
        "Fixture {} must exist",
        agent_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(agent_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(agent_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // The agent TOML must NOT contain sandbox_mode.
    let agent_toml = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("perm_accept_edits.toml"))
        .expect("Expected perm_accept_edits.toml in output");

    assert!(
        !agent_toml.content.contains("sandbox_mode"),
        "sandbox_mode must not appear in TOML for permissionMode=acceptEdits, got:\n{}",
        agent_toml.content
    );

    // A drop diagnostic for subagents.permissionMode must be in plan.diagnostics.
    let has_drop_diag = plan.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("subagents.permissionMode")
            && d.level == ccx::core::ir::DiagLevel::Drop
    });
    assert!(
        has_drop_diag,
        "Expected a Drop diagnostic for subagents.permissionMode in plan.diagnostics, got: {:?}",
        plan.diagnostics
            .iter()
            .map(|d| (d.id.as_deref(), &d.level))
            .collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 17/42: --keep-claude-frontmatter flag parsed but never applied
// ────────────────────────────────────────────────────────────────────────────

/// c2x with keep_claude_frontmatter=true must emit a SKILL.md that retains
/// Claude-specific frontmatter keys (when_to_use, allowed-tools) in addition
/// to the standard Codex fields (name, description).
#[test]
fn test_skill_c2x_keep_claude_frontmatter_retains_claude_keys() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: true,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let skill_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("SKILL.md"))
        .expect("Expected SKILL.md in emit plan");

    // Claude-specific keys must be present in the output frontmatter
    assert!(
        skill_file.content.contains("when_to_use"),
        "Expected 'when_to_use' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("allowed-tools"),
        "Expected 'allowed-tools' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    // Standard Codex fields must also be present
    assert!(
        skill_file.content.contains("name"),
        "Expected 'name' in frontmatter, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("description"),
        "Expected 'description' in frontmatter, got:\n{}",
        skill_file.content
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 19/42: --keep-claude-frontmatter retains allowed-tools, model, effort
// ────────────────────────────────────────────────────────────────────────────

/// c2x with keep_claude_frontmatter=true must retain allowed-tools, model, and
/// effort in the output SKILL.md (not just when_to_use and name/description).
/// Uses the deploy fixture which has all three Claude-specific fields.
#[test]
fn test_skill_c2x_keep_claude_frontmatter_model_effort_allowed_tools() {
    let skill_path = "tests/fixtures/claude/skills/deploy/SKILL.md";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: true,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let skill_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("SKILL.md"))
        .expect("Expected SKILL.md in emit plan");

    assert!(
        skill_file.content.contains("allowed-tools"),
        "Expected 'allowed-tools' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("model"),
        "Expected 'model' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("effort"),
        "Expected 'effort' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("name"),
        "Expected 'name' in frontmatter, got:\n{}",
        skill_file.content
    );
    assert!(
        skill_file.content.contains("description"),
        "Expected 'description' in frontmatter, got:\n{}",
        skill_file.content
    );
}

/// c2x of agents with permissionMode=dontAsk and permissionMode=auto must also
/// not produce sandbox_mode in the TOML output.
#[test]
fn test_subagent_c2x_permission_mode_dont_ask_and_auto_dropped() {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));

    for (agent_path, stem) in [
        (
            "tests/fixtures/claude/agents/perm_dont_ask.md",
            "perm_dont_ask.toml",
        ),
        (
            "tests/fixtures/claude/agents/perm_auto.md",
            "perm_auto.toml",
        ),
    ] {
        let out_dir = tempfile::TempDir::new().unwrap();
        let kind = detect(agent_path).expect("detect should succeed");
        let handler = pick_handler(&kind, &maps);
        let parsed = handler
            .parse(Path::new(agent_path))
            .expect("parse should succeed");
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .expect("lift should succeed");

        let opts = default_lower_opts(out_dir.path().to_str().unwrap());
        let plan = handler
            .lower(&ir, ConvDir::C2x, &opts)
            .expect("lower should succeed");

        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with(stem))
            .unwrap_or_else(|| {
                panic!(
                    "Expected {} in output, got: {:?}",
                    stem,
                    plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                )
            });

        assert!(
            !agent_toml.content.contains("sandbox_mode"),
            "sandbox_mode must not appear in TOML for {} (permissionMode has no Codex equivalent), got:\n{}",
            stem,
            agent_toml.content
        );

        let has_drop_diag = plan.diagnostics.iter().any(|d| {
            d.id.as_deref() == Some("subagents.permissionMode")
                && d.level == ccx::core::ir::DiagLevel::Drop
        });
        assert!(
            has_drop_diag,
            "Expected Drop diagnostic for subagents.permissionMode in plan for {}, got: {:?}",
            stem,
            plan.diagnostics
                .iter()
                .map(|d| (d.id.as_deref(), &d.level))
                .collect::<Vec<_>>()
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// gap 20/42: loss:dropped + warn:true entries must appear ONLY in dropped,
// not duplicated into lossy.
// ────────────────────────────────────────────────────────────────────────────

/// Integration test: the four warn:true + loss:dropped skill fields
/// (user-invocable, paths, argument-hint, arguments) must appear only in the
/// `dropped` section of the report and must NOT appear in `lossy`.
/// `summary.lossy` must be 0 for those entries.
#[test]
fn test_skill_c2x_dropped_warn_fields_not_in_lossy() {
    let skill_path = "tests/fixtures/claude/skills/dup-warn-dropped/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    let dropped_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    let lossy_ids: Vec<_> = report
        .lossy
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();

    for field_id in &[
        "skills.user-invocable",
        "skills.paths",
        "skills.argument-hint",
        "skills.arguments",
    ] {
        assert!(
            dropped_ids.contains(field_id),
            "{} must appear in dropped, dropped: {:?}",
            field_id,
            dropped_ids
        );
        assert!(
            !lossy_ids.contains(field_id),
            "{} must NOT appear in lossy (was promoted from dropped), lossy: {:?}",
            field_id,
            lossy_ids
        );
    }

    // summary.lossy should count only genuinely lossy entries, not dropped ones
    // For this fixture (name+description lossless, four fields dropped), lossy == 0.
    assert_eq!(
        report.lossy.len(),
        0,
        "Expected 0 lossy entries for dup-warn-dropped fixture, got: {:?}",
        report
            .lossy
            .iter()
            .map(|d| d.id.as_deref())
            .collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 23/42: Non-.md sibling files not path-remapped to output
// ────────────────────────────────────────────────────────────────────────────

/// c2x: Non-.md auxiliary files in skill dir are copied to the output with path remap.
///
/// The fixture has `tests/fixtures/claude/skills/aux-skill/scripts/run.sh` and
/// `tests/fixtures/claude/skills/aux-skill/README.txt` alongside SKILL.md.
/// After lower(c2x), both must appear at `.agents/skills/aux-skill/scripts/run.sh`
/// and `.agents/skills/aux-skill/README.txt` respectively, content unchanged.
#[test]
fn test_skill_c2x_aux_files_are_path_remapped() {
    let skill_path = "tests/fixtures/claude/skills/aux-skill/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

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
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // SKILL.md must be present
    let has_skill_md = plan.files.iter().any(|f| f.path.ends_with("SKILL.md"));
    assert!(has_skill_md, "Expected SKILL.md in emit plan");

    // scripts/run.sh must be remapped to .agents/skills/aux-skill/scripts/run.sh
    let run_sh = plan
        .files
        .iter()
        .find(|f| f.path.contains(".agents/skills/aux-skill/scripts/run.sh"));
    assert!(
        run_sh.is_some(),
        "Expected .agents/skills/aux-skill/scripts/run.sh in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        run_sh.unwrap().content.trim(),
        "#!/bin/bash\necho hi",
        "run.sh content must be unchanged"
    );

    // README.txt must be remapped to .agents/skills/aux-skill/README.txt
    let readme = plan
        .files
        .iter()
        .find(|f| f.path.contains(".agents/skills/aux-skill/README.txt"));
    assert!(
        readme.is_some(),
        "Expected .agents/skills/aux-skill/README.txt in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        readme.unwrap().content.trim(),
        "readme",
        "README.txt content must be unchanged"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// gap 25/42: c2x with Bearer auth: remaining ${VAR} headers not routed to
// env_http_headers
// ────────────────────────────────────────────────────────────────────────────

/// c2x lift: when Authorization is "Bearer ${TOKEN}", the bearer env var must be
/// extracted, non-Authorization headers with ${VAR} values must go to
/// env_http_headers, and literal-value headers must go to http_headers with a
/// Warn diagnostic.
#[test]
fn test_mcp_c2x_bearer_auth_remaining_var_headers_routed_to_env_http_headers() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://x.com",
                    "headers": {
                        "Authorization": "Bearer ${TOKEN}",
                        "X-Api-Key": "${API_KEY}",
                        "X-Static": "literal"
                    }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();
    let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

    // bearer_token_env_var must be extracted as "TOKEN"
    let bearer = server
        .fields
        .get("mcp.bearer")
        .expect("mcp.bearer must be present when Authorization is Bearer ${TOKEN}");
    assert_eq!(
        bearer.value,
        serde_json::Value::String("TOKEN".to_string()),
        "bearer_token_env_var must be bare var name 'TOKEN'"
    );

    // X-Api-Key: "${API_KEY}" must be in env_http_headers with bare var name "API_KEY"
    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present for ${VAR} headers alongside Bearer auth");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("mcp.env_http_headers must be an object");
    assert!(
        hdr_obj.contains_key("X-Api-Key"),
        "X-Api-Key (${{VAR}} value) must be in env_http_headers, not http_headers. got: {:?}",
        hdr_obj
    );
    assert_eq!(
        hdr_obj["X-Api-Key"],
        serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers['X-Api-Key'] must be bare var name 'API_KEY'"
    );

    // X-Static: "literal" must NOT be in env_http_headers
    assert!(
        !hdr_obj.contains_key("X-Static"),
        "X-Static (literal value) must not be in env_http_headers, got: {:?}",
        hdr_obj
    );

    // X-Static must be in mcp.headers (http_headers)
    let http_hdr = server
        .fields
        .get("mcp.headers")
        .expect("mcp.headers must be present for literal-value headers alongside Bearer auth");
    let http_obj = http_hdr
        .value
        .as_object()
        .expect("mcp.headers must be an object");
    assert!(
        http_obj.contains_key("X-Static"),
        "X-Static (literal value) must be in mcp.headers (http_headers), got: {:?}",
        http_obj
    );

    // There must be a Warn diagnostic for X-Static
    let has_static_warn = server
        .diagnostics
        .iter()
        .any(|d| d.level == ccx::core::ir::DiagLevel::Warn && d.message.contains("X-Static"));
    assert!(
        has_static_warn,
        "Expected a Warn diagnostic for literal-value header X-Static alongside Bearer auth, got: {:?}",
        server.diagnostics
    );
}

/// c2x lower: when Authorization is "Bearer ${TOKEN}", the Codex .mcp.json must
/// have bearer_token_env_var="TOKEN", env_http_headers={"X-Api-Key": "API_KEY"},
/// and http_headers={"X-Static": "literal"}.
#[test]
fn test_mcp_c2x_bearer_auth_remaining_var_headers_in_output() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://x.com",
                    "headers": {
                        "Authorization": "Bearer ${TOKEN}",
                        "X-Api-Key": "${API_KEY}",
                        "X-Static": "literal"
                    }
                }
            }
        },
        "body": ""
    });

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let handler = ccx::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use ccx::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

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
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value = serde_json::from_str(&mcp_file.content).unwrap();

    let server = &content["mcpServers"]["s"];

    // bearer_token_env_var must be "TOKEN"
    assert_eq!(
        server["bearer_token_env_var"],
        serde_json::Value::String("TOKEN".to_string()),
        "bearer_token_env_var must be 'TOKEN'"
    );

    // env_http_headers must contain X-Api-Key → API_KEY
    let env_http = &server["env_http_headers"];
    assert!(env_http.is_object(), "Expected env_http_headers in output");
    assert_eq!(
        env_http["X-Api-Key"],
        serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers['X-Api-Key'] must be 'API_KEY'"
    );

    // http_headers (if present) must contain X-Static
    if server["http_headers"].is_object() {
        assert_eq!(
            server["http_headers"]["X-Static"],
            serde_json::Value::String("literal".to_string()),
            "http_headers['X-Static'] must be 'literal'"
        );
    }
}

/// x2c: Non-.md auxiliary files in skill dir (excluding agents/openai.yaml) are
/// copied to the output with path remap (.agents/ → .claude/).
#[test]
fn test_skill_x2c_aux_files_are_path_remapped() {
    let skill_path = "tests/fixtures/codex/agents/aux-skill/SKILL.md";
    assert!(
        Path::new(skill_path).exists(),
        "Fixture {} must exist",
        skill_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(skill_path).expect("detect should succeed");
    let handler = pick_handler(&kind, &maps);
    let parsed = handler
        .parse(Path::new(skill_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

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

    // SKILL.md must be present
    let has_skill_md = plan.files.iter().any(|f| f.path.ends_with("SKILL.md"));
    assert!(has_skill_md, "Expected SKILL.md in emit plan");

    // scripts/run.sh must be remapped to .claude/skills/aux-skill/scripts/run.sh
    let run_sh = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude/skills/aux-skill/scripts/run.sh"));
    assert!(
        run_sh.is_some(),
        "Expected .claude/skills/aux-skill/scripts/run.sh in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        run_sh.unwrap().content.trim(),
        "#!/bin/bash\necho hi",
        "run.sh content must be unchanged"
    );

    // README.txt must be remapped to .claude/skills/aux-skill/README.txt
    let readme = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude/skills/aux-skill/README.txt"));
    assert!(
        readme.is_some(),
        "Expected .claude/skills/aux-skill/README.txt in emit plan. Got paths: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        readme.unwrap().content.trim(),
        "readme",
        "README.txt content must be unchanged"
    );
}
