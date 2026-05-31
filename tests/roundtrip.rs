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
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Subagent, // subagent to trigger degrade
        interactive: false,
        rewrite_body: false,
    }
}

fn empty_plan() -> EmitPlan {
    EmitPlan {
        files: vec![],
        diagnostics: vec![],
    }
}

/// SKILL.md を c2x 変換して report が期待通りか検証する。
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

/// SKILL.md を c2x 変換して dropped 件数が報告される。
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

/// .mcp.json を c2x 変換して基本的な変換が機能するか確認。
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

    // filesystem サーバーの timeout 変換確認
    let fs_server = ir.children.iter().find(|c| c.source_path == "filesystem");
    assert!(fs_server.is_some(), "Expected 'filesystem' server");
    let fs = fs_server.unwrap();
    let timeout = fs.fields.get("mcp.timeout");
    assert!(timeout.is_some(), "Expected timeout field");
    // 30000ms → 30.0sec
    assert_eq!(
        timeout.unwrap().value.as_f64().unwrap(),
        30.0,
        "Expected timeout converted to 30.0 sec"
    );

    // api-server の Bearer 抽出確認
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

/// .mcp.json c2x で dropped/lossy フィールドが report に列挙される。
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

    // alwaysLoad は claude 固有で dropped になるはず (unknown field か dropped)
    // disabled-server の alwaysLoad は unknown フィールドとして Drop 診断が出る
    let total_drops = report.dropped.len();
    assert!(
        total_drops >= 1,
        "Expected at least 1 dropped entry, got {}",
        total_drops
    );
}

/// .mcp.json c2x lower でファイルが生成される。
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

/// Codex config.toml の x2c 変換テスト。
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

    // filesystem と api-server が変換される (disabled-server は enabled=false)
    assert!(ir.children.len() >= 2, "Expected at least 2 children");

    // filesystem server: timeout が変換される
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

    // disabled-server は disabled フラグが設定されているか
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

/// x2c で .mcp.json が生成される。
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
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
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

/// ccx check コマンドのシミュレーション: dropped 件数を報告する。
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

    // body warnings が存在すること (動的注入や変数参照がある)
    assert!(
        !report.body_warnings.is_empty(),
        "Expected body warnings from skill body"
    );
}

/// c2x lower でファイルが生成され、skill.md の内容が正しい。
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

    // .rules ファイルが生成されていること (Bash tool degrade)
    let rules_file = plan.files.iter().find(|f| f.path.ends_with(".rules"));
    assert!(
        rules_file.is_some(),
        "Expected .rules file for Bash tool degrade"
    );

    // subagent TOML が生成されていること (model/effort degrade)
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
// P2: Hooks テスト
// ────────────────────────────────────────────────────────────────────────────

/// hooks.json c2x: 共通イベントが変換される、Claude 固有イベントが dropped になる。
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

/// hooks.json c2x lower (user scope): hooks.json が生成され、matcher が正規化されている。
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
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
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

/// hooks.json c2x lower (project scope): .codex/config.toml が生成される。
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
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::Project,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
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

/// hooks matcher の正規化テスト（Edit|Write → ^(Edit|Write)$）。
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
// P2: Memory テスト
// ────────────────────────────────────────────────────────────────────────────

/// CLAUDE.md c2x: AGENTS.md が生成される、内容が保持される。
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

/// AGENTS.md x2c: CLAUDE.md が生成される。
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

/// insta スナップショットテスト: report の JSON が安定していること。
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

    // 安定した出力のみ snapshot 化する
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
// P3: Plugins テスト
// ────────────────────────────────────────────────────────────────────────────

/// plugin.json c2x: .codex-plugin/plugin.json が生成される。
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

/// plugin.json c2x: 再帰変換で skills と .mcp.json が処理される。
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

/// plugin.json c2x dropped 分類の検証。
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

    // userConfig warn（未解決変数リスク）が出ること
    let has_user_config_warn = ir.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("plugins.userConfig") && d.level == ccx::core::ir::DiagLevel::Warn
    });
    assert!(
        has_user_config_warn,
        "Expected userConfig unresolved-variable warn"
    );
}

/// plugin.json c2x --dual-manifest: 両方の manifest が生成される。
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
        scope: Scope::Project,
        dual_manifest: true,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
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

/// plugin.json c2x: marketplace.json が変換されて policy が補完される。
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
// P4: Subagent テスト
// ────────────────────────────────────────────────────────────────────────────

/// Claude agents/<n>.md c2x: .codex/agents/<n>.toml が生成される。
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

/// .codex/agents/<n>.toml x2c: .claude/agents/<n>.md が生成される。
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
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
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

    // spawn-model の warn が出ていること
    let has_spawn_warn = ir
        .diagnostics
        .iter()
        .any(|d| d.message.contains("spawn_agent"));
    assert!(
        has_spawn_warn,
        "Expected spawn_agent warning about auto-delegation difference"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// P4: Settings テスト
// ────────────────────────────────────────────────────────────────────────────

/// settings.json c2x: config.toml が生成され、変換サブセットが正しい。
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

/// Codex settings.toml x2c: settings.json が生成される。
#[test]
fn test_settings_x2c_generates_claude_settings() {
    let settings_path = "tests/fixtures/codex/settings.toml";
    assert!(
        Path::new(settings_path).exists(),
        "Fixture {} must exist",
        settings_path
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));

    // SettingsHandler で直接テスト（detect は config.toml 向けなので直接呼ぶ）
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
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
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
