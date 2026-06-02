use std::collections::HashMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::core::ir::{DiagLevel, Diagnostic, IRNode, SideArtifact};
use crate::core::mappings::MapEntry;
use crate::core::transforms::ConvDir;
use crate::handlers::Handler;

use super::fs::collect_md_files;
use super::PluginsHandler;

impl PluginsHandler {
    /// Lifts manifest fields driven by mappings.
    pub(super) fn lift_manifest_fields(
        &self,
        frontmatter: &Map<String, Value>,
        idx: &HashMap<String, MapEntry>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        // Save userConfig so we can warn about unresolved variable references later
        let user_config = frontmatter.get("userConfig");

        for (key, value) in frontmatter {
            // experimental is expanded so each sub-field gets its own mapping entry
            if key == "experimental" {
                if let Some(exp_obj) = value.as_object() {
                    for (sub_key, sub_value) in exp_obj {
                        let full_key = format!("experimental.{}", sub_key);
                        self.lift_single_field(&full_key, sub_value, idx, dir, node);
                    }
                } else {
                    // Malformed experimental value (not an object): treat as an unknown field
                    // so a dropped/unknown-field diagnostic is preserved.
                    self.lift_single_field(key, value, idx, dir, node);
                }
                continue;
            }

            // interface is expanded so each sub-field (interface.displayName, etc.)
            // gets routed individually through the mappings index
            if key == "interface" {
                if let Some(iface_obj) = value.as_object() {
                    for (sub_key, sub_value) in iface_obj {
                        let full_key = format!("interface.{}", sub_key);
                        self.lift_single_field(&full_key, sub_value, idx, dir, node);
                    }
                } else {
                    // Malformed interface value (not an object): treat as an unknown field
                    // so a dropped/unknown-field diagnostic is preserved.
                    self.lift_single_field(key, value, idx, dir, node);
                }
                continue;
            }

            self.lift_single_field(key, value, idx, dir, node);
        }

        // c2x: warn if userConfig is present; ${user_config.KEY} references in MCP/hooks may remain unresolved
        if dir == ConvDir::C2x {
            if let Some(uc) = user_config {
                if uc.is_object() || uc.is_array() {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("plugins.userConfig".to_string()),
                        message: "userConfig found: ${user_config.KEY} references in MCP/hooks may remain unresolved after c2x conversion (Codex has no userConfig equivalent)".to_string(),
                    });
                }
            }
        }
    }

    pub(super) fn lift_single_field(
        &self,
        key: &str,
        value: &Value,
        idx: &HashMap<String, MapEntry>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        let Some(entry) = idx.get(key) else {
            // Unknown field: treat as dropped
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Drop,
                id: None,
                message: format!("unknown plugin manifest field: {key}"),
            });
            return;
        };

        crate::handlers::lift_mapped_field(entry, key, value, dir, node);
    }

    /// Recursively converts the skills/ directory and appends the results to children.
    pub(super) fn lift_child_skills(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        // The `skills` manifest field is string|array.  Collect all paths.
        let skills_dirs: Vec<String> = match frontmatter.get("skills") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => {
                // Codex manifest `skills` is a single string, so a multi-path array
                // cannot be fully represented — warn so the caller can resolve it.
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.skills".to_string()),
                    message: format!(
                        "plugins.skills is an array with {} paths; all entries are converted as children but the Codex manifest `skills` field is a single string — only one path can be represented in the output manifest",
                        arr.len()
                    ),
                });
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            }
            _ => vec!["./skills/".to_string()],
        };

        let skills_handler = crate::handlers::skills::SkillsHandler {
            map: self.maps["skills"].clone(),
        };

        for skills_dir in &skills_dirs {
            // Normalize: ./skills/ → skills
            let skills_rel = skills_dir.trim_start_matches("./").trim_end_matches('/');
            let skills_path_str = format!("{}/{}", plugin_root, skills_rel);
            let skills_path = Path::new(&skills_path_str);

            if !skills_path.exists() {
                continue;
            }

            // Process each SKILL.md under the resolved skills directory
            if let Ok(entries) = std::fs::read_dir(skills_path) {
                for entry in entries.flatten() {
                    let skill_dir = entry.path();
                    if !skill_dir.is_dir() {
                        continue;
                    }
                    let skill_md = skill_dir.join("SKILL.md");
                    if !skill_md.exists() {
                        continue;
                    }

                    match skills_handler.parse(&skill_md) {
                        Ok(parsed) => match skills_handler.lift(&parsed, dir) {
                            Ok(child_ir) => {
                                node.children.push(child_ir);
                            }
                            Err(e) => {
                                node.diagnostics.push(Diagnostic {
                                    level: DiagLevel::Warn,
                                    id: None,
                                    message: format!("Failed to lift skill {:?}: {}", skill_md, e),
                                });
                            }
                        },
                        Err(e) => {
                            node.diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to parse skill {:?}: {}", skill_md, e),
                            });
                        }
                    }
                }
            }
        }
    }

    /// Recursively converts the hooks file and appends the result to children.
    pub(super) fn lift_child_hooks(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        let hooks_handler = crate::handlers::hooks::HooksHandler {
            map: self.maps["hooks"].clone(),
        };

        let hooks_value = frontmatter.get("hooks");

        // Inline object form: serialize and feed directly through the hooks handler.
        if let Some(hooks_obj) = hooks_value.and_then(|v| v.as_object()) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.hooks".to_string()),
                message: format!(
                    "Inline hooks object in plugin.json has {} entries; writing to hooks file for Codex compatibility",
                    hooks_obj.len()
                ),
            });

            // Build a synthetic parsed value as if it came from a hooks file.
            // The hooks handler expects the top-level value to be the hooks object itself.
            let synthetic = Value::Object(hooks_obj.clone());
            match hooks_handler.lift(&synthetic, dir) {
                Ok(mut child_ir) => {
                    child_ir.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("plugins.hooks".to_string()),
                        message: "Plugin-bundled hooks may not be loaded by Codex (#16430). Use --hooks-target=user|project to output hooks to ~/.codex/hooks.json or .codex/config.toml instead.".to_string(),
                    });
                    node.children.push(child_ir);
                }
                Err(e) => {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: None,
                        message: format!("Failed to lift inline hooks object: {}", e),
                    });
                }
            }
            return;
        }

        // String reference form: resolve path and parse the file.
        let hooks_path_str = hooks_value
            .and_then(|v| v.as_str())
            .unwrap_or("./hooks/hooks.json");

        let hooks_rel = hooks_path_str.trim_start_matches("./");
        let hooks_path_owned = format!("{}/{}", plugin_root, hooks_rel);
        let hooks_path = Path::new(&hooks_path_owned);

        if !hooks_path.exists() {
            return;
        }

        match hooks_handler.parse(hooks_path) {
            Ok(parsed) => match hooks_handler.lift(&parsed, dir) {
                Ok(mut child_ir) => {
                    child_ir.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("plugins.hooks".to_string()),
                        message: "Plugin-bundled hooks may not be loaded by Codex (#16430). Use --hooks-target=user|project to output hooks to ~/.codex/hooks.json or .codex/config.toml instead.".to_string(),
                    });
                    node.children.push(child_ir);
                }
                Err(e) => {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: None,
                        message: format!("Failed to lift hooks {:?}: {}", hooks_path, e),
                    });
                }
            },
            Err(e) => {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("Failed to parse hooks {:?}: {}", hooks_path, e),
                });
            }
        }
    }

    /// Recursively converts .mcp.json and appends the result to children.
    pub(super) fn lift_child_mcp(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        let mcp_handler = crate::handlers::mcp::McpHandler {
            map: self.maps["mcp"].clone(),
        };

        let mcp_value = frontmatter.get("mcpServers");

        // Inline object form: serialize and feed directly through the MCP handler.
        if let Some(mcp_obj) = mcp_value.and_then(|v| v.as_object()) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.mcpServers".to_string()),
                message: "Inline mcpServers object in plugin.json: Codex requires a file path reference. Will attempt to emit as .mcp.json.".to_string(),
            });

            // Wrap in the envelope that parse_json_file produces and lift_c2x/x2c expect.
            let synthetic = serde_json::json!({
                "frontmatter": { "mcpServers": mcp_obj },
                "body": "",
                "path": ""
            });
            match mcp_handler.lift(&synthetic, dir) {
                Ok(child_ir) => {
                    node.children.push(child_ir);
                }
                Err(e) => {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: None,
                        message: format!("Failed to lift inline mcpServers object: {}", e),
                    });
                }
            }
            return;
        }

        // String reference form: resolve path and parse the file.
        let mcp_path_str = mcp_value.and_then(|v| v.as_str()).unwrap_or("./.mcp.json");

        let mcp_rel = mcp_path_str.trim_start_matches("./");
        let mcp_path_owned = format!("{}/{}", plugin_root, mcp_rel);
        let mcp_path = Path::new(&mcp_path_owned);

        if !mcp_path.exists() {
            return;
        }

        match mcp_handler.parse(mcp_path) {
            Ok(parsed) => match mcp_handler.lift(&parsed, dir) {
                Ok(child_ir) => {
                    node.children.push(child_ir);
                }
                Err(e) => {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: None,
                        message: format!("Failed to lift .mcp.json {:?}: {}", mcp_path, e),
                    });
                }
            },
            Err(e) => {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("Failed to parse .mcp.json {:?}: {}", mcp_path, e),
                });
            }
        }
    }

    /// Processes marketplace.json and stores it in side_artifacts.
    /// plugin_root is the directory containing plugin.json (e.g. `.claude-plugin/`).
    pub(super) fn lift_marketplace(&self, plugin_root: &str, dir: ConvDir, node: &mut IRNode) {
        // marketplace.json lives in the same directory as plugin.json:
        // Claude: .claude-plugin/marketplace.json (= {plugin_root}/marketplace.json)
        // Codex:  .agents/plugins/marketplace.json (= {plugin_root}/marketplace.json)
        let local_marketplace = format!("{}/marketplace.json", plugin_root);

        let marketplace_path = match dir {
            ConvDir::C2x => {
                let p = Path::new(&local_marketplace);
                if p.exists() {
                    Some(p.to_path_buf())
                } else {
                    None
                }
            }
            ConvDir::X2c => {
                let p = Path::new(&local_marketplace);
                if p.exists() {
                    Some(p.to_path_buf())
                } else {
                    None
                }
            }
        };

        let Some(mp_path) = marketplace_path else {
            return;
        };

        match std::fs::read_to_string(&mp_path) {
            Ok(content) => {
                // Save marketplace.json for conversion and emission during lower
                node.side_artifacts.push(SideArtifact {
                    path: mp_path.to_string_lossy().to_string(),
                    content,
                    note: "marketplace.json".to_string(),
                });
            }
            Err(e) => {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("Failed to read marketplace.json {:?}: {}", mp_path, e),
                });
            }
        }
    }

    /// Discovers `commands/` at the plugin root and stores the files as side artifacts.
    /// Both Claude and Codex use an identically named directory — conversion is a
    /// lossless path-remap.
    pub(super) fn lift_child_commands(&self, plugin_root: &str, node: &mut IRNode) {
        let commands_path_str = format!("{}/commands", plugin_root);
        let commands_path = Path::new(&commands_path_str);
        if !commands_path.exists() {
            return;
        }

        for file in collect_md_files(commands_path) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Info,
                id: Some("plugins.commands".to_string()),
                message: format!(
                    "commands/{}: path-remapped losslessly to output commands/",
                    file.rel_path.trim_start_matches("commands/")
                ),
            });
            node.side_artifacts.push(SideArtifact {
                // path stores the relative path within the plugin dir (e.g. "commands/foo.md")
                path: file.rel_path,
                content: file.content,
                note: "commands".to_string(),
            });
        }
    }

    /// Discovers `agents/` at the plugin root and stores the files as side artifacts.
    /// Both Claude and Codex auto-discover agent `.md` files here — conversion is a
    /// lossy path-remap (per-file frontmatter may need subagent-rule conversion).
    pub(super) fn lift_child_agents(&self, plugin_root: &str, node: &mut IRNode) {
        let agents_path_str = format!("{}/agents", plugin_root);
        let agents_path = Path::new(&agents_path_str);
        if !agents_path.exists() {
            return;
        }

        for file in collect_md_files(agents_path) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.agents".to_string()),
                message: format!(
                    "agents/{}: path-remapped to output agents/ (lossy — per-agent frontmatter may need subagent-rule conversion)",
                    file.rel_path.trim_start_matches("agents/")
                ),
            });
            node.side_artifacts.push(SideArtifact {
                // path stores the relative path within the plugin dir (e.g. "agents/bar.md")
                path: file.rel_path,
                content: file.content,
                note: "agents".to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{new_node, Kind, Loss, Tool};
    use crate::core::mappings::load_mappings;
    use crate::handlers::plugins::index::build_plugin_scope_index;

    fn make_handler() -> PluginsHandler {
        let maps = load_mappings();
        PluginsHandler {
            map: maps["plugins"].clone(),
            maps,
        }
    }

    fn plugin_node() -> IRNode {
        new_node(Kind::Plugin, Tool::Claude, "test-path")
    }

    // ── lift_single_field ──────────────────────────────────────────────────────

    #[test]
    fn lift_single_field_known_lossless_inserts_ir_field() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        h.lift_single_field(
            "name",
            &serde_json::Value::String("test-plugin".to_string()),
            &idx,
            ConvDir::C2x,
            &mut node,
        );

        let field = node
            .fields
            .get("plugins.name")
            .expect("plugins.name must be inserted");
        assert_eq!(field.loss, Loss::Lossless);
        assert_eq!(
            field.value,
            serde_json::Value::String("test-plugin".to_string())
        );
    }

    #[test]
    fn lift_single_field_known_dropped_inserts_ir_field_with_dropped_loss() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        h.lift_single_field(
            "lspServers",
            &serde_json::Value::String("./lsp.json".to_string()),
            &idx,
            ConvDir::C2x,
            &mut node,
        );

        let field = node
            .fields
            .get("plugins.lspServers")
            .expect("plugins.lspServers must be inserted");
        assert_eq!(field.loss, Loss::Dropped);
        // No extra diagnostics on the node for a dropped field
        assert!(
            node.diagnostics.is_empty(),
            "dropped field must not push an extra diagnostic"
        );
    }

    #[test]
    fn lift_single_field_unknown_key_pushes_drop_diagnostic() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        h.lift_single_field(
            "nonExistentField",
            &serde_json::Value::Bool(true),
            &idx,
            ConvDir::C2x,
            &mut node,
        );

        assert!(
            node.fields.is_empty(),
            "unknown field must not insert an IRField"
        );
        assert_eq!(node.diagnostics.len(), 1);
        assert_eq!(node.diagnostics[0].level, DiagLevel::Drop);
        assert!(
            node.diagnostics[0].message.contains("nonExistentField"),
            "diagnostic message must name the unknown field"
        );
    }

    #[test]
    fn lift_single_field_direction_filter_skips_c2x_only_field_in_x2c() {
        let h = make_handler();
        // For X2c, build_plugin_scope_index uses the codex side.
        // lspServers is claude_to_codex only → must NOT appear in x2c index.
        let idx = build_plugin_scope_index(&h.map, ConvDir::X2c);
        assert!(
            !idx.contains_key("lspServers"),
            "lspServers (claude_to_codex) must not appear in x2c index"
        );
    }

    // ── lift_manifest_fields ───────────────────────────────────────────────────

    #[test]
    fn lift_manifest_fields_expands_experimental_subfields() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        let mut fm = serde_json::Map::new();
        fm.insert(
            "experimental".to_string(),
            serde_json::json!({"themes": "./themes/", "monitors": "./monitors/"}),
        );

        h.lift_manifest_fields(&fm, &idx, ConvDir::C2x, &mut node);

        // Each sub-key routes through "experimental.<sub_key>" in the index
        assert!(
            node.fields.contains_key("plugins.experimental.themes"),
            "experimental.themes must be lifted individually"
        );
        assert!(
            node.fields.contains_key("plugins.experimental.monitors"),
            "experimental.monitors must be lifted individually"
        );
        // Both are claude_to_codex dropped
        assert_eq!(
            node.fields["plugins.experimental.themes"].loss,
            Loss::Dropped
        );
        assert_eq!(
            node.fields["plugins.experimental.monitors"].loss,
            Loss::Dropped
        );

        // The "experimental" key itself must NOT produce an unknown-field diagnostic
        let has_unknown_experimental = node.diagnostics.iter().any(|d| {
            d.message
                .contains("unknown plugin manifest field: experimental")
        });
        assert!(
            !has_unknown_experimental,
            "experimental must not produce an unknown-field diagnostic"
        );
    }

    #[test]
    fn lift_manifest_fields_malformed_experimental_not_object_produces_drop_diag() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        let mut fm = serde_json::Map::new();
        // experimental with a non-object value
        fm.insert("experimental".to_string(), serde_json::json!("bad-string"));

        h.lift_manifest_fields(&fm, &idx, ConvDir::C2x, &mut node);

        // The non-object experimental value is treated as an unknown field
        let drop_diag = node
            .diagnostics
            .iter()
            .find(|d| d.level == DiagLevel::Drop && d.message.contains("experimental"));
        assert!(
            drop_diag.is_some(),
            "malformed experimental (non-object) must produce a Drop diagnostic"
        );
    }

    #[test]
    fn lift_manifest_fields_expands_interface_subfields() {
        let h = make_handler();
        // For x2c, build the Codex-side index
        let idx = build_plugin_scope_index(&h.map, ConvDir::X2c);
        let mut node = new_node(Kind::Plugin, Tool::Codex, "test-path");

        let mut fm = serde_json::Map::new();
        fm.insert(
            "interface".to_string(),
            serde_json::json!({
                "displayName": "My Plugin",
                "brandColor": "#FF0000"
            }),
        );

        h.lift_manifest_fields(&fm, &idx, ConvDir::X2c, &mut node);

        // interface.displayName → plugins.display-name (lossless rename)
        assert!(
            node.fields.contains_key("plugins.display-name"),
            "interface.displayName must map to plugins.display-name"
        );
        // interface.brandColor is codex_to_claude dropped
        assert!(
            node.fields.contains_key("plugins.interface.brandColor"),
            "interface.brandColor must be present"
        );
        assert_eq!(
            node.fields["plugins.interface.brandColor"].loss,
            Loss::Dropped
        );

        // Must NOT generate a single "unknown plugin manifest field: interface" diagnostic
        let has_unknown_iface = node.diagnostics.iter().any(|d| {
            d.message
                .contains("unknown plugin manifest field: interface")
        });
        assert!(
            !has_unknown_iface,
            "interface must not produce a single undifferentiated unknown-field diagnostic"
        );
    }

    #[test]
    fn lift_manifest_fields_malformed_interface_not_object_produces_drop_diag() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::X2c);
        let mut node = new_node(Kind::Plugin, Tool::Codex, "test-path");

        let mut fm = serde_json::Map::new();
        fm.insert("interface".to_string(), serde_json::json!(42));

        h.lift_manifest_fields(&fm, &idx, ConvDir::X2c, &mut node);

        let drop_diag = node
            .diagnostics
            .iter()
            .find(|d| d.level == DiagLevel::Drop && d.message.contains("interface"));
        assert!(
            drop_diag.is_some(),
            "malformed interface (non-object) must produce a Drop diagnostic"
        );
    }

    #[test]
    fn lift_manifest_fields_unknown_top_level_key_produces_drop_diag() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        let mut fm = serde_json::Map::new();
        fm.insert("totallyUnknownKey".to_string(), serde_json::json!("value"));

        h.lift_manifest_fields(&fm, &idx, ConvDir::C2x, &mut node);

        assert!(node.fields.is_empty());
        assert_eq!(node.diagnostics.len(), 1);
        assert_eq!(node.diagnostics[0].level, DiagLevel::Drop);
        assert!(node.diagnostics[0].message.contains("totallyUnknownKey"));
    }

    #[test]
    fn lift_manifest_fields_c2x_user_config_warn_diagnostic_emitted() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        let mut fm = serde_json::Map::new();
        fm.insert(
            "userConfig".to_string(),
            serde_json::json!({"MY_KEY": {"type": "string", "title": "T", "description": "D"}}),
        );

        h.lift_manifest_fields(&fm, &idx, ConvDir::C2x, &mut node);

        // userConfig must be lifted as Dropped (it's a claude_to_codex dropped field)
        let field = node
            .fields
            .get("plugins.userConfig")
            .expect("userConfig must be in fields");
        assert_eq!(field.loss, Loss::Dropped);

        // An additional Warn diagnostic must be emitted for the unresolved-variable risk
        let warn = node
            .diagnostics
            .iter()
            .find(|d| d.level == DiagLevel::Warn && d.id.as_deref() == Some("plugins.userConfig"));
        assert!(
            warn.is_some(),
            "Expected Warn diagnostic for userConfig in c2x"
        );
        assert!(
            warn.unwrap().message.contains("userConfig"),
            "Warn message must mention userConfig"
        );
    }

    #[test]
    fn lift_manifest_fields_x2c_user_config_warn_not_emitted() {
        let h = make_handler();
        // x2c direction: userConfig is claude_to_codex only, so it won't appear in the x2c index
        let idx = build_plugin_scope_index(&h.map, ConvDir::X2c);
        let mut node = new_node(Kind::Plugin, Tool::Codex, "test-path");

        let mut fm = serde_json::Map::new();
        fm.insert("userConfig".to_string(), serde_json::json!({"K": {}}));

        h.lift_manifest_fields(&fm, &idx, ConvDir::X2c, &mut node);

        // userConfig not in x2c index → unknown-field drop diagnostic, no userConfig Warn
        let has_user_config_warn = node
            .diagnostics
            .iter()
            .any(|d| d.level == DiagLevel::Warn && d.id.as_deref() == Some("plugins.userConfig"));
        assert!(
            !has_user_config_warn,
            "x2c direction must not emit userConfig Warn"
        );
    }

    #[test]
    fn lift_manifest_fields_known_lossless_fields_lifted_correctly() {
        let h = make_handler();
        let idx = build_plugin_scope_index(&h.map, ConvDir::C2x);
        let mut node = plugin_node();

        let mut fm = serde_json::Map::new();
        fm.insert("name".to_string(), serde_json::json!("my-plugin"));
        fm.insert("version".to_string(), serde_json::json!("1.2.3"));
        fm.insert("description".to_string(), serde_json::json!("A plugin"));
        fm.insert("license".to_string(), serde_json::json!("MIT"));

        h.lift_manifest_fields(&fm, &idx, ConvDir::C2x, &mut node);

        assert_eq!(
            node.fields["plugins.name"].value,
            serde_json::Value::String("my-plugin".to_string())
        );
        assert_eq!(node.fields["plugins.name"].loss, Loss::Lossless);
        assert_eq!(node.fields["plugins.license"].loss, Loss::Lossless);
        // version is lossy (warn:true in mappings)
        assert_eq!(node.fields["plugins.version"].loss, Loss::Lossy);
    }
}
