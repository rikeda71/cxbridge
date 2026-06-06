use std::path::Path;

use serde_json::Value;

use crate::core::ir::{new_node, IRNode, Kind, Tool};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, LowerOpts};

mod fs;
mod index;
mod lift;
mod lower;
mod marketplace;

/// Handler for the plugins domain.
/// In addition to lifting/lowering plugin.json, it recursively converts
/// the nested skills/hooks/.mcp.json by delegating to the respective handlers
/// and stores the results as children.
pub struct PluginsHandler {
    pub map: DomainMap,
    /// All domain maps, held to avoid re-parsing YAML on each nested conversion.
    pub(crate) maps: std::collections::HashMap<String, DomainMap>,
}

impl Handler for PluginsHandler {
    fn kind(&self) -> Kind {
        Kind::Plugin
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name == "plugin.json"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        crate::core::serialize::json::parse_json_file(path)
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Plugin, source_tool, &source_path);

        let frontmatter = match parsed["frontmatter"].as_object() {
            Some(fm) => fm,
            None => return Ok(node),
        };

        // Index only scope:"plugin" entries to avoid collisions with same-named fields in marketplace etc.
        let idx = index::build_plugin_scope_index(&self.map, dir);

        // Lift manifest fields driven by mappings
        self.lift_manifest_fields(frontmatter, &idx, dir, &mut node);

        // Recursively convert nested child components.
        // Use the parent directory of plugin.json as the plugin root.
        let plugin_root = Path::new(&source_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Recursively convert skills/ directory via SkillsHandler
        self.lift_child_skills(&plugin_root, frontmatter, dir, &mut node);

        // Recursively convert hooks file via HooksHandler
        self.lift_child_hooks(&plugin_root, frontmatter, dir, &mut node);

        // Recursively convert .mcp.json via McpHandler
        self.lift_child_mcp(&plugin_root, frontmatter, dir, &mut node);

        // Process marketplace.json if present in the same directory
        self.lift_marketplace(&plugin_root, dir, &mut node);

        // Collect commands/ and agents/ directories (lossless path-remap / lossy path-remap)
        self.lift_child_commands(&plugin_root, &mut node);
        self.lift_child_agents(&plugin_root, &mut node);

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{DiagLevel, Loss};
    use crate::core::mappings::load_mappings;
    use crate::core::transforms::ConvDir;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> PluginsHandler {
        let maps = load_mappings();
        PluginsHandler {
            map: maps["plugins"].clone(),
            maps: maps.clone(),
        }
    }

    fn default_opts(out: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out.to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    /// Creates a basic plugin fixture.
    fn create_claude_plugin_fixture(dir: &Path) -> std::path::PathBuf {
        // Create .claude-plugin/plugin.json
        let plugin_dir = dir.join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        fs::write(
            &plugin_json,
            r#"{
  "name": "test-plugin",
  "version": "1.2.3",
  "description": "A test plugin",
  "author": {"name": "Test Author", "email": "test@example.com"},
  "homepage": "https://example.com",
  "license": "MIT",
  "keywords": ["test", "plugin"],
  "skills": "./skills/"
}"#,
        )
        .unwrap();

        // Create skills/ directory and SKILL.md
        let skills_dir = dir.join(".claude-plugin").join("skills").join("my-skill");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: My skill\n---\nDo something.\n",
        )
        .unwrap();

        // Create .mcp.json
        let mcp_json = dir.join(".claude-plugin").join(".mcp.json");
        fs::write(
            &mcp_json,
            r#"{"mcpServers": {"my-server": {"command": "npx", "args": ["-y", "@my/server"]}}}"#,
        )
        .unwrap();

        plugin_json
    }

    #[test]
    fn test_plugins_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("plugin.json")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
    }

    #[test]
    fn test_plugins_lift_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Plugin);
        // name, description, version should be lifted losslessly
        assert!(ir.fields.contains_key("plugins.name"));
        assert!(ir.fields.contains_key("plugins.version"));
        assert!(ir.fields.contains_key("plugins.description"));
        let name_f = &ir.fields["plugins.name"];
        assert_eq!(name_f.value, Value::String("test-plugin".to_string()));
        assert_eq!(name_f.loss, Loss::Lossless);
    }

    #[test]
    fn test_plugins_lift_c2x_dropped_fields() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        // plugin.json containing dropped fields
        fs::write(
            &plugin_json,
            r#"{
  "name": "test-plugin",
  "version": "1.0.0",
  "description": "A test plugin",
  "lspServers": "./lsp.json",
  "outputStyles": "./styles/",
  "channels": [],
  "settings": {"agent": "test"},
  "dependencies": ["other-plugin"],
  "userConfig": {"MY_KEY": {"type": "string", "title": "My Key", "description": "desc"}}
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // dropped fields should be present with Loss::Dropped
        let has_lsp_dropped = ir
            .fields
            .get("plugins.lspServers")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_output_dropped = ir
            .fields
            .get("plugins.outputStyles")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_channels_dropped = ir
            .fields
            .get("plugins.channels")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_settings_dropped = ir
            .fields
            .get("plugins.settings")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_deps_dropped = ir
            .fields
            .get("plugins.dependencies")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_user_config_dropped = ir
            .fields
            .get("plugins.userConfig")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);

        assert!(has_lsp_dropped, "lspServers should be dropped");
        assert!(has_output_dropped, "outputStyles should be dropped");
        assert!(has_channels_dropped, "channels should be dropped");
        assert!(has_settings_dropped, "settings should be dropped");
        assert!(has_deps_dropped, "dependencies should be dropped");
        assert!(has_user_config_dropped, "userConfig should be dropped");

        // An additional warn for userConfig should be emitted
        let has_user_config_warn = ir
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("plugins.userConfig") && d.level == DiagLevel::Warn);
        assert!(has_user_config_warn, "Expected userConfig warn diagnostic");
    }

    #[test]
    fn test_plugins_lift_c2x_with_recursion() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // skills/ and .mcp.json should be recursively converted as child nodes
        let skill_children: Vec<_> = ir
            .children
            .iter()
            .filter(|c| c.kind == Kind::Skill)
            .collect();
        assert!(
            !skill_children.is_empty(),
            "Expected skill children from recursion"
        );

        let mcp_children: Vec<_> = ir.children.iter().filter(|c| c.kind == Kind::Mcp).collect();
        assert!(
            !mcp_children.is_empty(),
            "Expected MCP children from recursion"
        );
    }

    #[test]
    fn test_plugins_lower_c2x_generates_codex_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that .codex-plugin/plugin.json is generated
        let codex_manifest = plan
            .files
            .iter()
            .find(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
        assert!(
            codex_manifest.is_some(),
            "Expected .codex-plugin/plugin.json"
        );

        let content: Value = serde_json::from_str(&codex_manifest.unwrap().content).unwrap();
        assert_eq!(content["name"].as_str(), Some("test-plugin"));
        assert_eq!(content["version"].as_str(), Some("1.2.3"));
    }

    #[test]
    fn test_plugins_lower_c2x_dual_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let out_dir = dir.path().join("out");
        let mut opts = default_opts(out_dir.to_str().unwrap());
        opts.dual_manifest = true;

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that both .claude-plugin/plugin.json and .codex-plugin/plugin.json are generated
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

    #[test]
    fn test_plugins_c2x_version_semver_completion() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        // Case where version is omitted
        fs::write(
            &plugin_json,
            r#"{"name": "test-plugin", "description": "A test plugin"}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // A version completion warn should be emitted
        let has_version_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("plugins.version") || d.message.contains("version"));
        assert!(
            has_version_warn,
            "Expected version semver completion warning"
        );

        // The generated manifest's version should be "0.0.0"
        let codex_manifest = plan
            .files
            .iter()
            .find(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"))
            .unwrap();
        let content: Value = serde_json::from_str(&codex_manifest.content).unwrap();
        assert_eq!(content["version"].as_str(), Some("0.0.0"));
    }

    #[test]
    fn test_plugins_c2x_marketplace_policy_defaults() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        // plugin.json
        fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "test-plugin", "version": "1.0.0", "description": "Test"}"#,
        )
        .unwrap();

        // marketplace.json without policy
        fs::write(
            plugin_dir.join("marketplace.json"),
            r#"{
  "plugins": [
    {
      "name": "test-plugin",
      "source": "./",
      "category": "productivity"
    }
  ]
}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_dir.join("plugin.json")).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that marketplace.json is included in the output
        let marketplace_file = plan
            .files
            .iter()
            .find(|f| f.path.contains("marketplace.json"));
        assert!(
            marketplace_file.is_some(),
            "Expected marketplace.json in output"
        );

        let content: Value = serde_json::from_str(&marketplace_file.unwrap().content).unwrap();
        let plugins = content["plugins"].as_array().unwrap();
        assert!(!plugins.is_empty());

        // Verify that policy was filled in
        let policy = &plugins[0]["policy"];
        assert!(policy.is_object(), "Expected policy object");
        assert_eq!(policy["installation"].as_str(), Some("AVAILABLE"));
        assert_eq!(policy["authentication"].as_str(), Some("ON_INSTALL"));

        // A policy auto-fill warn should be emitted
        let has_policy_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("policy"));
        assert!(has_policy_warn, "Expected policy auto-fill warning");
    }

    #[test]
    fn test_complete_semver() {
        use super::marketplace::complete_semver;
        assert_eq!(complete_semver("1"), "1.0.0");
        assert_eq!(complete_semver("1.2"), "1.2.0");
        assert_eq!(complete_semver("1.2.3"), "1.2.3");
        // git SHA
        let sha = "a".repeat(40);
        assert_eq!(complete_semver(&sha), "0.0.0");
    }

    /// x2c: a Codex plugin.json with a full `interface` object must expand each
    /// sub-field individually through the mappings index.
    ///
    /// Asserts:
    ///   (a) interface.websiteURL → plugins.interface.websiteURL is Lossy
    ///   (b) interface.displayName → plugins.display-name is present
    ///   (c) interface.brandColor → plugins.interface.brandColor is Dropped
    ///   (d) NO "unknown plugin manifest field: interface" diagnostic
    ///   (e) lower_x2c emits `homepage` in the Claude plugin.json
    #[test]
    fn test_plugins_lift_x2c_interface_fields() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".codex-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        fs::write(
            &plugin_json,
            r##"{
  "name": "codex-plugin",
  "version": "1.0.0",
  "description": "A Codex plugin",
  "interface": {
    "displayName": "Codex Plugin",
    "websiteURL": "https://example.com",
    "developerName": "OpenAI",
    "category": "utility",
    "brandColor": "#FF0000"
  }
}"##,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        // (a) interface.websiteURL must be Lossy (maps to homepage)
        let website_url = ir
            .fields
            .get("plugins.interface.websiteURL")
            .expect("plugins.interface.websiteURL must be present in IR");
        assert_eq!(
            website_url.loss,
            Loss::Lossy,
            "plugins.interface.websiteURL must be Lossy"
        );
        assert_eq!(
            website_url.value,
            Value::String("https://example.com".to_string()),
            "plugins.interface.websiteURL value mismatch"
        );

        // (b) interface.displayName → plugins.display-name must be present
        assert!(
            ir.fields.contains_key("plugins.display-name"),
            "plugins.display-name must be present for interface.displayName; fields: {:?}",
            ir.fields.keys().collect::<Vec<_>>()
        );

        // (c) interface.brandColor must be Dropped
        let brand_color = ir
            .fields
            .get("plugins.interface.brandColor")
            .expect("plugins.interface.brandColor must be present in IR");
        assert_eq!(
            brand_color.loss,
            Loss::Dropped,
            "plugins.interface.brandColor must be Dropped"
        );

        // (d) NO undifferentiated "unknown plugin manifest field: interface" diagnostic
        let has_unknown_interface_diag = ir.diagnostics.iter().any(|d| {
            d.message
                .contains("unknown plugin manifest field: interface")
        });
        assert!(
            !has_unknown_interface_diag,
            "interface must NOT produce a single undifferentiated unknown-field diagnostic"
        );

        // (e) lower_x2c emits `homepage` in the Claude plugin.json
        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let claude_manifest = plan
            .files
            .iter()
            .find(|f| f.path.contains(".claude-plugin") && f.path.ends_with("plugin.json"))
            .expect("Expected .claude-plugin/plugin.json in x2c output");

        let content: Value = serde_json::from_str(&claude_manifest.content).unwrap();
        assert_eq!(
            content["homepage"].as_str(),
            Some("https://example.com"),
            "interface.websiteURL must map to 'homepage' in Claude plugin.json, got: {}",
            content
        );
    }

    /// c2x: top-level Claude-only marketplace fields are dropped from the output
    /// and reported as DiagLevel::Drop with the correct mapping IDs.
    #[test]
    fn test_plugins_c2x_marketplace_dropped_top_level_fields() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "test-plugin", "version": "1.0.0", "description": "Test"}"#,
        )
        .unwrap();

        fs::write(
            plugin_dir.join("marketplace.json"),
            r#"{
  "owner": {"name": "ACME", "email": "acme@example.com"},
  "allowCrossMarketplaceDependenciesOn": ["other"],
  "forceRemoveDeletedPlugins": true,
  "plugins": [
    {"name": "test-plugin", "source": "./", "category": "productivity"}
  ]
}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_dir.join("plugin.json")).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let marketplace_file = plan
            .files
            .iter()
            .find(|f| f.path.contains("marketplace.json"))
            .expect("Expected marketplace.json in output");

        let content: Value = serde_json::from_str(&marketplace_file.content).unwrap();

        // (1) Claude-only fields must be absent from output
        assert!(
            content.get("owner").is_none(),
            "owner must be absent from output"
        );
        assert!(
            content.get("allowCrossMarketplaceDependenciesOn").is_none(),
            "allowCrossMarketplaceDependenciesOn must be absent from output"
        );
        assert!(
            content.get("forceRemoveDeletedPlugins").is_none(),
            "forceRemoveDeletedPlugins must be absent from output"
        );

        // (2) Three DiagLevel::Drop entries with the correct mapping IDs
        let drop_ids: Vec<Option<&str>> = plan
            .diagnostics
            .iter()
            .filter(|d| d.level == DiagLevel::Drop)
            .map(|d| d.id.as_deref())
            .collect();

        assert!(
            drop_ids.contains(&Some("plugins.marketplace.owner")),
            "Expected Drop diagnostic for plugins.marketplace.owner; drop_ids={:?}",
            drop_ids
        );
        assert!(
            drop_ids.contains(&Some(
                "plugins.marketplace.allowCrossMarketplaceDependenciesOn"
            )),
            "Expected Drop diagnostic for plugins.marketplace.allowCrossMarketplaceDependenciesOn; drop_ids={:?}",
            drop_ids
        );
        assert!(
            drop_ids.contains(&Some("plugins.marketplace.forceRemoveDeletedPlugins")),
            "Expected Drop diagnostic for plugins.marketplace.forceRemoveDeletedPlugins; drop_ids={:?}",
            drop_ids
        );
    }

    /// An npm-source entry in marketplace.json must produce a DiagLevel::Drop
    /// diagnostic (id "plugins.marketplace.plugins.source") and the source field
    /// must be absent from the output — not set to null.
    #[test]
    fn test_normalize_marketplace_source_c2x_npm_drop_diagnostic() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "test-plugin", "version": "1.0.0", "description": "Test"}"#,
        )
        .unwrap();

        fs::write(
            plugin_dir.join("marketplace.json"),
            r#"{
  "plugins": [
    {
      "name": "plugin-c",
      "source": {"source": "npm", "package": "my-plugin"},
      "category": "tools"
    }
  ]
}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_dir.join("plugin.json")).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // (1) A DiagLevel::Drop diagnostic with the correct id must be present.
        let drop_diag = plan.diagnostics.iter().find(|d| {
            d.level == DiagLevel::Drop
                && d.id.as_deref() == Some("plugins.marketplace.plugins.source")
        });
        assert!(
            drop_diag.is_some(),
            "Expected DiagLevel::Drop with id 'plugins.marketplace.plugins.source'; \
             diagnostics: {:?}",
            plan.diagnostics
        );

        let msg = &drop_diag.unwrap().message;
        assert!(
            msg.to_lowercase().contains("npm"),
            "Drop message must mention 'npm', got: {}",
            msg
        );
        assert!(
            msg.contains("plugin-c"),
            "Drop message must contain plugin name 'plugin-c', got: {}",
            msg
        );

        // (2) The output marketplace.json must not contain a null source for plugin-c.
        let marketplace_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("marketplace.json"))
            .expect("Expected marketplace.json in output");

        let content: Value = serde_json::from_str(&marketplace_file.content).unwrap();
        let plugin_c = content["plugins"][0].as_object().unwrap();
        assert!(
            plugin_c.get("source").is_none_or(|s| !s.is_null()),
            "source must not be null; found: {:?}",
            plugin_c
        );
    }
}
