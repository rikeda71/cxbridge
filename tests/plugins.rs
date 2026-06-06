mod common;
use common::*;

use std::path::Path;

use cxbridge::core::{
    detect::detect,
    ir::{DiagLevel, Kind},
    mappings::load_mappings,
    report::build_report,
    transforms::ConvDir,
};
use cxbridge::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

/// plugin.json c2x: .codex-plugin/plugin.json is generated.
#[test]
fn test_plugin_c2x_generates_codex_manifest() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";
    assert!(
        Path::new(plugin_path).exists(),
        "Fixture {} must exist",
        plugin_path
    );

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Plugin);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.kind, cxbridge::core::ir::Kind::Plugin);
    // name and description should be lossless
    assert!(
        ir.fields.contains_key("plugins.name"),
        "Expected plugins.name field"
    );
    assert_eq!(
        ir.fields["plugins.name"].loss,
        cxbridge::core::ir::Loss::Lossless
    );

    // lspServers and userConfig should be dropped
    let has_lsp_dropped = ir
        .fields
        .get("plugins.lspServers")
        .map(|f| matches!(f.loss, cxbridge::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(has_lsp_dropped, "lspServers should be dropped");

    let has_user_config_dropped = ir
        .fields
        .get("plugins.userConfig")
        .map(|f| matches!(f.loss, cxbridge::core::ir::Loss::Dropped))
        .unwrap_or(false);
    assert!(has_user_config_dropped, "userConfig should be dropped");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
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

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let skill_children: Vec<_> = ir
        .children
        .iter()
        .filter(|c| c.kind == cxbridge::core::ir::Kind::Skill)
        .collect();
    assert!(
        !skill_children.is_empty(),
        "Expected skill children from recursion"
    );

    let mcp_children: Vec<_> = ir
        .children
        .iter()
        .filter(|c| c.kind == cxbridge::core::ir::Kind::Mcp)
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

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
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
        d.id.as_deref() == Some("plugins.userConfig")
            && d.level == cxbridge::core::ir::DiagLevel::Warn
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

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
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

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(plugin_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
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

/// x2c: Codex plugin.json with interface sub-object must expand each sub-field
/// individually into IR rather than producing one undifferentiated drop.
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

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Plugin);

    let handler = pick_handler(&kind, maps);
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
        cxbridge::core::ir::Loss::Lossy,
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
        cxbridge::core::ir::Loss::Dropped,
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
    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
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

/// An npm-source plugin entry must produce a DiagLevel::Drop diagnostic
/// and must NOT leave a null source in the output.
#[test]
fn test_marketplace_c2x_npm_source_emits_drop_diagnostic() {
    let fixture =
        Path::new("tests/fixtures/claude/marketplace_npm_source/.claude-plugin/plugin.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Plugin, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // (1) A DiagLevel::Drop diagnostic with id "plugins.marketplace.plugins.source" must be present.
    let drop_diags: Vec<_> = plan
        .diagnostics
        .iter()
        .filter(|d| {
            d.level == DiagLevel::Drop
                && d.id.as_deref() == Some("plugins.marketplace.plugins.source")
        })
        .collect();

    assert!(
        !drop_diags.is_empty(),
        "Expected a DiagLevel::Drop diagnostic with id \
         'plugins.marketplace.plugins.source' for the npm source entry, \
         but none was found. diagnostics: {:?}",
        plan.diagnostics
    );

    // (2) The diagnostic message must mention 'npm' and the plugin name.
    let msg = &drop_diags[0].message;
    assert!(
        msg.to_lowercase().contains("npm"),
        "Drop diagnostic message must mention 'npm', got: {}",
        msg
    );
    assert!(
        msg.contains("plugin-c"),
        "Drop diagnostic message must contain the plugin name 'plugin-c', got: {}",
        msg
    );

    // (3) The generated marketplace.json must not contain a null source for plugin-c.
    let marketplace_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("marketplace.json"))
        .expect("Expected marketplace.json in c2x output");

    let content: serde_json::Value = serde_json::from_str(&marketplace_file.content)
        .expect("marketplace.json must be valid JSON");

    let plugins = content["plugins"]
        .as_array()
        .expect("plugins must be array");
    let plugin_c = plugins
        .iter()
        .find(|p| p["name"].as_str() == Some("plugin-c"))
        .expect("plugin-c must be present in output");

    assert!(
        plugin_c.get("source").is_none_or(|s| !s.is_null()),
        "plugin-c source must not be null; found: {}",
        plugin_c
    );

    // (4) The drop diagnostic must appear in report.dropped.
    let report = build_report(&ir, &plan);
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("plugins.marketplace.plugins.source"));
    assert!(
        in_dropped,
        "'plugins.marketplace.plugins.source' must appear in report.dropped; \
         dropped={:?}",
        report
            .dropped
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// The three Claude-only top-level fields must be absent from the generated
/// marketplace.json and must appear in the report.dropped section.
#[test]
fn test_marketplace_c2x_dropped_top_level_fields_absent_from_output() {
    let fixture =
        Path::new("tests/fixtures/claude/marketplace_dropped_fields/.claude-plugin/plugin.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Plugin, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // Find the generated marketplace.json content
    let marketplace_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("marketplace.json"))
        .expect("Expected marketplace.json in c2x output");

    let content: serde_json::Value = serde_json::from_str(&marketplace_file.content)
        .expect("marketplace.json must be valid JSON");

    // (1) The three Claude-only fields must NOT be present in the output
    assert!(
        content.get("owner").is_none(),
        "owner must be absent from c2x marketplace.json output, got: {}",
        content
    );
    assert!(
        content.get("allowCrossMarketplaceDependenciesOn").is_none(),
        "allowCrossMarketplaceDependenciesOn must be absent from c2x marketplace.json output"
    );
    assert!(
        content.get("forceRemoveDeletedPlugins").is_none(),
        "forceRemoveDeletedPlugins must be absent from c2x marketplace.json output"
    );

    // (2) The plugins[] array must still be present (other content preserved)
    assert!(
        content.get("plugins").is_some(),
        "plugins array must remain in c2x marketplace.json output"
    );

    // (3) Three Drop diagnostics must be emitted for the dropped fields
    let drop_ids: Vec<Option<&str>> = plan
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagLevel::Drop)
        .map(|d| d.id.as_deref())
        .collect();

    let expected_ids = [
        "plugins.marketplace.owner",
        "plugins.marketplace.allowCrossMarketplaceDependenciesOn",
        "plugins.marketplace.forceRemoveDeletedPlugins",
    ];

    for id in &expected_ids {
        assert!(
            drop_ids.contains(&Some(id)),
            "Expected DiagLevel::Drop diagnostic with id '{}'; found drop ids: {:?}",
            id,
            drop_ids
        );
    }

    // (4) All three must appear in report.dropped
    let report = build_report(&ir, &plan);

    for id in &expected_ids {
        let in_dropped = report.dropped.iter().any(|e| e.id.as_deref() == Some(id));
        assert!(
            in_dropped,
            "'{}' must appear in report.dropped; dropped={:?}",
            id,
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }
}

/// A Claude plugin.json with exactly 6 dropped fields
/// (lspServers, outputStyles, channels, settings, dependencies, userConfig) must
/// produce report.dropped.len() == 6 and summary.dropped == 6.
///
/// Before the fix each dropped field appeared 3× (from ir.fields, from the
/// DiagLevel::Drop diagnostic pushed by lift_single_field, and from the
/// DiagLevel::Drop diagnostic pushed by build_codex_manifest), yielding 18.
#[test]
fn test_plugin_c2x_six_dropped_fields_no_duplicates() {
    let fixture =
        Path::new("tests/fixtures/claude/plugin_six_dropped_fields/.claude-plugin/plugin.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Plugin, maps);

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

    // The 6 dropped fields by their mapping IDs.
    let expected_dropped_ids = [
        "plugins.lspServers",
        "plugins.outputStyles",
        "plugins.channels",
        "plugins.settings",
        "plugins.dependencies",
        "plugins.userConfig",
    ];

    // Each dropped field ID must appear exactly once.
    for id in &expected_dropped_ids {
        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some(id))
            .count();
        assert_eq!(
            count,
            1,
            "Dropped field '{}' must appear exactly once in report.dropped, found {} times. \
             Full dropped list: {:?}",
            id,
            count,
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }

    // Total dropped count must equal the number of distinct dropped fields (6).
    // In addition, there may be one entry for the userConfig warn diagnostic
    // (DiagLevel::Warn from node.diagnostics is NOT a dropped entry, but
    // DiagLevel::Drop diagnostics without a matching IRField ID may be present
    // for unknown-field drops). We check that IDs for the 6 known fields are
    // each unique.
    let dropped_ids_for_known: Vec<Option<&str>> = report
        .dropped
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| expected_dropped_ids.contains(&id))
                .unwrap_or(false)
        })
        .map(|e| e.id.as_deref())
        .collect();

    assert_eq!(
        dropped_ids_for_known.len(),
        6,
        "Expected exactly 6 entries in report.dropped for the 6 known dropped fields, \
         found {}. Full dropped: {:?}",
        dropped_ids_for_known.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // Summary dropped count must equal the total number of distinct dropped entries.
    assert_eq!(
        report.dropped.len(),
        6,
        "summary dropped count must be 6; got {}. Full dropped: {:?}",
        report.dropped.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// c2x: plugin with commands/foo.md and agents/bar.md must include them in the
/// EmitPlan remapped under .codex-plugin/commands/ and .codex-plugin/agents/.
#[test]
fn test_plugin_c2x_commands_and_agents_remapped() {
    let plugin_path = "tests/fixtures/claude/.claude-plugin/plugin.json";
    assert!(
        Path::new(plugin_path).exists(),
        "Fixture {} must exist",
        plugin_path
    );
    // The fixture already has commands/foo.md and agents/bar.md
    assert!(
        Path::new("tests/fixtures/claude/.claude-plugin/commands/foo.md").exists(),
        "commands/foo.md fixture must exist"
    );
    assert!(
        Path::new("tests/fixtures/claude/.claude-plugin/agents/bar.md").exists(),
        "agents/bar.md fixture must exist"
    );

    let maps = load_mappings();
    let kind = detect(plugin_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
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

    // commands/foo.md must be remapped to .codex-plugin/commands/foo.md
    let commands_file = plan
        .files
        .iter()
        .find(|f| f.path.contains(".codex-plugin/commands/foo.md"));
    assert!(
        commands_file.is_some(),
        "Expected .codex-plugin/commands/foo.md in c2x EmitPlan; files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    // Content must be unchanged
    assert!(
        commands_file.unwrap().content.contains("foo"),
        "commands/foo.md content should be preserved"
    );

    // agents/bar.md must be remapped to .codex-plugin/agents/bar.md
    let agents_file = plan
        .files
        .iter()
        .find(|f| f.path.contains(".codex-plugin/agents/bar.md"));
    assert!(
        agents_file.is_some(),
        "Expected .codex-plugin/agents/bar.md in c2x EmitPlan; files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    // Content must be unchanged
    assert!(
        agents_file.unwrap().content.contains("bar"),
        "agents/bar.md content should be preserved"
    );

    // An Info diagnostic for commands and a Warn for agents must be emitted
    let has_commands_info = ir
        .diagnostics
        .iter()
        .any(|d| d.id.as_deref() == Some("plugins.commands") && d.level == DiagLevel::Info);
    assert!(
        has_commands_info,
        "Expected Info diagnostic with id plugins.commands"
    );

    let has_agents_warn = ir
        .diagnostics
        .iter()
        .any(|d| d.id.as_deref() == Some("plugins.agents") && d.level == DiagLevel::Warn);
    assert!(
        has_agents_warn,
        "Expected Warn diagnostic with id plugins.agents"
    );
}

/// x2c: plugin with commands/foo.md and agents/bar.md (on the Codex side) must
/// include them in the EmitPlan remapped under .claude-plugin/commands/ and
/// .claude-plugin/agents/.
#[test]
fn test_plugin_x2c_commands_and_agents_remapped() {
    // Create a temp Codex plugin fixture that has commands/ and agents/.
    let dir = tempfile::TempDir::new().unwrap();
    let plugin_dir = dir.path().join(".codex-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name": "x2c-plugin", "version": "1.0.0", "description": "Test"}"#,
    )
    .unwrap();

    let commands_dir = plugin_dir.join("commands");
    std::fs::create_dir_all(&commands_dir).unwrap();
    std::fs::write(
        commands_dir.join("foo.md"),
        "# foo\nA codex plugin command.\n",
    )
    .unwrap();

    let agents_dir = plugin_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("bar.md"),
        "---\nname: bar\ndescription: A codex plugin agent\n---\nYou are bar.\n",
    )
    .unwrap();

    let plugin_json_path = plugin_dir.join("plugin.json");

    let maps = load_mappings();
    let kind = detect(plugin_json_path.to_str().unwrap()).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(&plugin_json_path)
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    // commands/foo.md must be remapped to .claude-plugin/commands/foo.md
    let commands_file = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude-plugin/commands/foo.md"));
    assert!(
        commands_file.is_some(),
        "Expected .claude-plugin/commands/foo.md in x2c EmitPlan; files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // agents/bar.md must be remapped to .claude-plugin/agents/bar.md
    let agents_file = plan
        .files
        .iter()
        .find(|f| f.path.contains(".claude-plugin/agents/bar.md"));
    assert!(
        agents_file.is_some(),
        "Expected .claude-plugin/agents/bar.md in x2c EmitPlan; files: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Dropped fields that do NOT have a secondary warn diagnostic (lspServers,
/// outputStyles, channels, settings, dependencies) must not appear in
/// report.lossy at all.
///
/// Note: plugins.userConfig also has a separate intentional Warn diagnostic
/// about unresolved ${user_config.KEY} references (not a dropped-field
/// duplicate), so it is excluded from this assertion.
#[test]
fn test_plugin_c2x_dropped_fields_without_secondary_warn_not_in_lossy() {
    let fixture =
        Path::new("tests/fixtures/claude/plugin_six_dropped_fields/.claude-plugin/plugin.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Plugin, maps);

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

    // These 5 dropped fields have no secondary warn diagnostic.
    // They must not appear in report.lossy.
    let pure_dropped_ids = [
        "plugins.lspServers",
        "plugins.outputStyles",
        "plugins.channels",
        "plugins.settings",
        "plugins.dependencies",
    ];

    let spurious_in_lossy: Vec<_> = report
        .lossy
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| pure_dropped_ids.contains(&id))
                .unwrap_or(false)
        })
        .collect();

    assert!(
        spurious_in_lossy.is_empty(),
        "Pure-dropped field IDs must NOT appear in report.lossy; found: {:?}",
        spurious_in_lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}
