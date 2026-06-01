use std::path::Path;

use anyhow::Context;
use serde_json::{Map, Value};

use crate::core::ir::{
    new_node, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, SideArtifact, Tool,
};
use crate::core::mappings::{applies_direction, DomainMap};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// Handler for the plugins domain.
/// In addition to lifting/lowering plugin.json, it recursively converts
/// the nested skills/hooks/.mcp.json by delegating to the respective handlers
/// and stores the results as children.
pub struct PluginsHandler {
    pub map: DomainMap,
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
        let idx = build_plugin_scope_index(&self.map, dir);

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

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

/// Indexes only scope:"plugin" entries to avoid collisions with same-named fields in marketplace etc.
/// Indexes by the claude field for c2x, or the codex field for x2c.
fn build_plugin_scope_index(
    map: &DomainMap,
    dir: ConvDir,
) -> std::collections::HashMap<String, crate::core::mappings::MapEntry> {
    let mut idx = std::collections::HashMap::new();
    for entry in &map.entries {
        let spec = match dir {
            ConvDir::C2x => entry.claude.as_ref(),
            ConvDir::X2c => entry.codex.as_ref(),
        };
        let Some(spec) = spec else { continue };
        // Only include scope:"plugin" entries (exclude marketplace / null)
        if spec.scope.as_deref() != Some("plugin") {
            continue;
        }
        let Some(field) = spec.field.as_ref() else {
            continue;
        };
        // Skip placeholder fields starting with a fullwidth left parenthesis (U+FF08)
        if field.starts_with('\u{FF08}') {
            continue;
        }
        // First-registered entry wins; later duplicates for the same field are ignored
        idx.entry(field.clone()).or_insert_with(|| entry.clone());
    }
    idx
}

impl PluginsHandler {
    /// Lifts manifest fields driven by mappings.
    fn lift_manifest_fields(
        &self,
        frontmatter: &Map<String, Value>,
        idx: &std::collections::HashMap<String, crate::core::mappings::MapEntry>,
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

    fn lift_single_field(
        &self,
        key: &str,
        value: &Value,
        idx: &std::collections::HashMap<String, crate::core::mappings::MapEntry>,
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

        if !applies_direction(entry, dir) {
            return;
        }

        let ctx = TransformCtx {
            direction: dir,
            args: None,
            field: entry,
        };
        let (v, applied) = apply_transforms(value, entry.transform.as_deref(), &ctx);

        let loss = Loss::from(&entry.loss);

        let dropped_info = if matches!(loss, Loss::Dropped) {
            Some(DroppedInfo {
                reason: entry
                    .notes
                    .clone()
                    .unwrap_or_else(|| format!("{} has no equivalent", key)),
            })
        } else {
            None
        };

        let warning = if entry.warn == Some(true) {
            Some(format!(
                "{}: {}",
                entry.id,
                entry.notes.as_deref().unwrap_or("warn")
            ))
        } else {
            None
        };

        // Dropped fields are already recorded via IRField.dropped — no additional
        // Diagnostic push is needed.  For genuinely lossy (non-dropped) warn:true
        // fields, emit a single Warn diagnostic so build_report routes them to the
        // lossy list.  Pushing a diagnostic for dropped fields would cause
        // build_report to count each dropped field multiple times.
        if entry.warn == Some(true) && !matches!(loss, Loss::Dropped) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some(entry.id.clone()),
                message: entry
                    .notes
                    .clone()
                    .unwrap_or_else(|| format!("{} (warn)", entry.id)),
            });
        }

        node.fields.insert(
            entry.id.clone(),
            IRField {
                id: entry.id.clone(),
                value: v,
                loss,
                transforms_applied: applied,
                degrade: None,
                warning,
                dropped: dropped_info,
            },
        );
    }

    /// Recursively converts the skills/ directory and appends the results to children.
    fn lift_child_skills(
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

        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        let skills_handler = crate::handlers::skills::SkillsHandler {
            map: maps["skills"].clone(),
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
    fn lift_child_hooks(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        let hooks_handler = crate::handlers::hooks::HooksHandler {
            map: maps["hooks"].clone(),
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
    fn lift_child_mcp(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        let mcp_handler = crate::handlers::mcp::McpHandler {
            map: maps["mcp"].clone(),
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
    fn lift_marketplace(&self, plugin_root: &str, dir: ConvDir, node: &mut IRNode) {
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

    /// c2x: Claude plugin → Codex plugin conversion
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build the manifest JSON
        let codex_manifest = self.build_codex_manifest(ir, &mut diagnostics);

        // --dual-manifest: retain .claude-plugin/ while also generating .codex-plugin/
        if opts.dual_manifest {
            // Retain the Claude-side manifest by re-reading and emitting the original file
            if let Ok(content) = std::fs::read_to_string(&ir.source_path) {
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/plugin.json", out_root),
                    content,
                });
            }
        }

        // Generate the Codex-side manifest
        let codex_json = serde_json::to_string_pretty(&codex_manifest)
            .with_context(|| "Failed to serialize Codex plugin.json")?;
        files.push(EmitFile {
            path: format!("{}/.codex-plugin/plugin.json", out_root),
            content: codex_json,
        });

        // Merge EmitPlans from child nodes
        // skills children
        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        for child_ir in &ir.children {
            match child_ir.kind {
                Kind::Skill => {
                    let skill_handler = crate::handlers::skills::SkillsHandler {
                        map: maps["skills"].clone(),
                    };
                    match skill_handler.lower(child_ir, ConvDir::C2x, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower skill child: {}", e),
                            });
                        }
                    }
                }
                Kind::Hooks => {
                    let hooks_handler = crate::handlers::hooks::HooksHandler {
                        map: maps["hooks"].clone(),
                    };
                    match hooks_handler.lower(child_ir, ConvDir::C2x, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower hooks child: {}", e),
                            });
                        }
                    }
                }
                Kind::Mcp => {
                    let mcp_handler = crate::handlers::mcp::McpHandler {
                        map: maps["mcp"].clone(),
                    };
                    match mcp_handler.lower(child_ir, ConvDir::C2x, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower MCP child: {}", e),
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        // Convert marketplace.json
        for artifact in &ir.side_artifacts {
            if artifact.note == "marketplace.json" {
                let transformed =
                    self.transform_marketplace_c2x(&artifact.content, &mut diagnostics);
                files.push(EmitFile {
                    path: format!("{}/.agents/plugins/marketplace.json", out_root),
                    content: transformed,
                });
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: Codex plugin → Claude plugin conversion
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build the manifest JSON
        let claude_manifest = self.build_claude_manifest(ir, &mut diagnostics);

        // Generate the Claude-side manifest
        let claude_json = serde_json::to_string_pretty(&claude_manifest)
            .with_context(|| "Failed to serialize Claude plugin.json")?;
        files.push(EmitFile {
            path: format!("{}/.claude-plugin/plugin.json", out_root),
            content: claude_json,
        });

        // Merge EmitPlans from child nodes
        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        for child_ir in &ir.children {
            match child_ir.kind {
                Kind::Skill => {
                    let skill_handler = crate::handlers::skills::SkillsHandler {
                        map: maps["skills"].clone(),
                    };
                    match skill_handler.lower(child_ir, ConvDir::X2c, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower skill child: {}", e),
                            });
                        }
                    }
                }
                Kind::Hooks => {
                    let hooks_handler = crate::handlers::hooks::HooksHandler {
                        map: maps["hooks"].clone(),
                    };
                    match hooks_handler.lower(child_ir, ConvDir::X2c, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower hooks child: {}", e),
                            });
                        }
                    }
                }
                Kind::Mcp => {
                    let mcp_handler = crate::handlers::mcp::McpHandler {
                        map: maps["mcp"].clone(),
                    };
                    match mcp_handler.lower(child_ir, ConvDir::X2c, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower MCP child: {}", e),
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        // Convert marketplace.json
        for artifact in &ir.side_artifacts {
            if artifact.note == "marketplace.json" {
                let transformed =
                    self.transform_marketplace_x2c(&artifact.content, &mut diagnostics);
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/marketplace.json", out_root),
                    content: transformed,
                });
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// Builds a Codex-target plugin.json from the IR (c2x).
    fn build_codex_manifest(&self, ir: &IRNode, diagnostics: &mut Vec<Diagnostic>) -> Value {
        let mut manifest = Map::new();

        // Convert IR fields to Codex fields
        for (id, field) in &ir.fields {
            // Dropped fields are already represented by IRField.loss == Dropped and
            // recorded via IRField.dropped.  build_report reads those directly, so
            // pushing an additional Diagnostic here would cause each dropped field
            // to appear multiple times in the report summary.
            if matches!(field.loss, Loss::Dropped) {
                continue;
            }

            // Retrieve the Codex field name from the entry
            let codex_field = self
                .map
                .entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.codex.as_ref())
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());

            let Some(cf) = codex_field else {
                continue;
            };

            // Handle nested fields (e.g. interface.displayName)
            if let Some(dot_pos) = cf.find('.') {
                let parent = &cf[..dot_pos];
                let child_key = &cf[dot_pos + 1..];
                let parent_obj = manifest
                    .entry(parent.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(obj) = parent_obj.as_object_mut() {
                    obj.insert(child_key.to_string(), field.value.clone());
                }
            } else {
                manifest.insert(cf.to_string(), field.value.clone());
            }
        }

        // Fill in semver "0.0.0" if version is missing
        if !manifest.contains_key("version") {
            manifest.insert("version".to_string(), Value::String("0.0.0".to_string()));
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.version".to_string()),
                message: "version field missing: auto-completed as '0.0.0' (Codex requires strict semver)".to_string(),
            });
        } else if let Some(ver) = manifest.get("version").and_then(|v| v.as_str()) {
            // semver completion: complete to major.minor.patch if not already in that form
            let completed = complete_semver(ver);
            if completed != ver {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.version".to_string()),
                    message: format!(
                        "version '{}' completed to semver '{}' (Codex requires strict semver)",
                        ver, completed
                    ),
                });
                manifest.insert("version".to_string(), Value::String(completed));
            }
        }

        // Fill in description from name if missing
        if !manifest.contains_key("description") {
            if let Some(name) = manifest.get("name").and_then(|v| v.as_str()) {
                manifest.insert(
                    "description".to_string(),
                    Value::String(format!("Plugin: {}", name)),
                );
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.description".to_string()),
                    message: "description field missing: auto-filled from name (Codex requires description)".to_string(),
                });
            }
        }

        Value::Object(manifest)
    }

    /// Builds a Claude-target plugin.json from the IR (x2c).
    fn build_claude_manifest(&self, ir: &IRNode, _diagnostics: &mut Vec<Diagnostic>) -> Value {
        let mut manifest = Map::new();

        for (id, field) in &ir.fields {
            // Dropped fields are already captured by IRField.loss == Dropped;
            // no additional Diagnostic needed here.
            if matches!(field.loss, Loss::Dropped) {
                continue;
            }

            // Retrieve the Claude field name from the entry
            let claude_field = self
                .map
                .entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.claude.as_ref())
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());

            let Some(cf) = claude_field else {
                continue;
            };

            // Handle nested fields (e.g. experimental.themes)
            if let Some(dot_pos) = cf.find('.') {
                let parent = &cf[..dot_pos];
                let child_key = &cf[dot_pos + 1..];
                let parent_obj = manifest
                    .entry(parent.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(obj) = parent_obj.as_object_mut() {
                    obj.insert(child_key.to_string(), field.value.clone());
                }
            } else {
                manifest.insert(cf.to_string(), field.value.clone());
            }
        }

        Value::Object(manifest)
    }

    /// Converts marketplace.json for Codex (c2x).
    /// - Claude-only top-level fields are dropped with DiagLevel::Drop diagnostics
    /// - Normalizes the source schema (Claude `relative`/string → Codex `{source:"local",...}`)
    /// - Fills in default policy values if missing
    fn transform_marketplace_c2x(
        &self,
        content: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> String {
        let Ok(mut json): Result<Value, _> = serde_json::from_str(content) else {
            return content.to_string();
        };

        // Drop top-level Claude-only fields that have no Codex marketplace equivalent.
        // Corresponding mappings entries all carry direction:claude_to_codex + loss:dropped.
        const CLAUDE_ONLY_FIELDS: &[(&str, &str)] = &[
            ("owner", "plugins.marketplace.owner"),
            (
                "allowCrossMarketplaceDependenciesOn",
                "plugins.marketplace.allowCrossMarketplaceDependenciesOn",
            ),
            (
                "forceRemoveDeletedPlugins",
                "plugins.marketplace.forceRemoveDeletedPlugins",
            ),
        ];
        if let Some(obj) = json.as_object_mut() {
            for (field, mapping_id) in CLAUDE_ONLY_FIELDS {
                if obj.remove(*field).is_some() {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Drop,
                        id: Some(mapping_id.to_string()),
                        message: format!("`{}` dropped (no Codex marketplace equivalent)", field),
                    });
                }
            }
        }

        if let Some(plugins) = json.get_mut("plugins").and_then(|v| v.as_array_mut()) {
            for plugin_entry in plugins.iter_mut() {
                if let Some(obj) = plugin_entry.as_object_mut() {
                    // Normalize the source schema
                    normalize_marketplace_source_c2x(obj, diagnostics);

                    // Fill in default policy if not set
                    if !obj.contains_key("policy") {
                        obj.insert(
                            "policy".to_string(),
                            serde_json::json!({
                                "installation": "AVAILABLE",
                                "authentication": "ON_INSTALL"
                            }),
                        );
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("plugins.marketplace.plugins.policy".to_string()),
                            message: "marketplace plugin.policy auto-filled with defaults (installation=AVAILABLE, authentication=ON_INSTALL)".to_string(),
                        });
                    }
                }
            }
        }

        serde_json::to_string_pretty(&json).unwrap_or_else(|_| content.to_string())
    }

    /// Converts marketplace.json for Claude (x2c).
    /// - Normalizes the source schema (Codex `local` → Claude relative path)
    /// - policy has no Claude equivalent (dropped)
    fn transform_marketplace_x2c(
        &self,
        content: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> String {
        let Ok(mut json): Result<Value, _> = serde_json::from_str(content) else {
            return content.to_string();
        };

        if let Some(plugins) = json.get_mut("plugins").and_then(|v| v.as_array_mut()) {
            for plugin_entry in plugins.iter_mut() {
                if let Some(obj) = plugin_entry.as_object_mut() {
                    // Normalize the source schema
                    normalize_marketplace_source_x2c(obj);

                    // policy has no Claude equivalent (dropped)
                    if obj.remove("policy").is_some() {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: Some("plugins.marketplace.plugins.policy".to_string()),
                            message: "marketplace plugin.policy dropped (no Claude equivalent)"
                                .to_string(),
                        });
                    }
                }
            }
        }

        serde_json::to_string_pretty(&json).unwrap_or_else(|_| content.to_string())
    }
}

/// Completes a partial semver string (major-only → major.0.0; major.minor → major.minor.0).
fn complete_semver(ver: &str) -> String {
    // Convert a 40-char git SHA to "0.0.0"
    if ver.len() == 40 && ver.chars().all(|c| c.is_ascii_hexdigit()) {
        return "0.0.0".to_string();
    }

    let parts: Vec<&str> = ver.split('.').collect();
    match parts.len() {
        1 => {
            // Major only
            if parts[0].parse::<u64>().is_ok() {
                format!("{}.0.0", parts[0])
            } else {
                "0.0.0".to_string()
            }
        }
        2 => {
            // Major.minor
            if parts[0].parse::<u64>().is_ok() && parts[1].parse::<u64>().is_ok() {
                format!("{}.{}.0", parts[0], parts[1])
            } else {
                "0.0.0".to_string()
            }
        }
        _ => ver.to_string(), // 3 or more components: pass through unchanged
    }
}

/// Normalizes the marketplace.json source schema for Codex.
/// - Relative path string → `{source: "local", path: "..."}`
/// - `github` passes through mostly unchanged (warn if field names differ)
/// - `npm` has no Codex equivalent: removes the source field and emits a Drop diagnostic
fn normalize_marketplace_source_c2x(
    obj: &mut Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(source) = obj.get("source").cloned() {
        match &source {
            Value::String(s) => {
                // Relative path string → Codex local format
                let normalized = serde_json::json!({
                    "source": "local",
                    "path": s
                });
                obj.insert("source".to_string(), normalized);
            }
            Value::Object(src_obj) => {
                // Already in object form: inspect the source type
                if let Some(src_type) = src_obj.get("source").and_then(|v| v.as_str()) {
                    if src_type == "relative" {
                        // Claude `relative` → Codex `local`
                        let mut new_src = src_obj.clone();
                        new_src.insert("source".to_string(), Value::String("local".to_string()));
                        obj.insert("source".to_string(), Value::Object(new_src));
                    } else if src_type == "npm" {
                        // npm has no Codex equivalent; remove the field and report it dropped
                        let plugin_name = obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        obj.remove("source");
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: Some("plugins.marketplace.plugins.source".to_string()),
                            message: format!(
                                "marketplace plugin source type 'npm' dropped \
                                 (no Codex equivalent): plugin '{}'",
                                plugin_name
                            ),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Normalizes the marketplace.json source schema for Claude.
/// - `{source: "local", path: "..."}` → relative path string
fn normalize_marketplace_source_x2c(obj: &mut Map<String, Value>) {
    if let Some(source) = obj.get("source").cloned() {
        if let Some(src_obj) = source.as_object() {
            if let Some(src_type) = src_obj.get("source").and_then(|v| v.as_str()) {
                if src_type == "local" {
                    // Codex `local` → Claude relative path string
                    if let Some(path) = src_obj.get("path").and_then(|v| v.as_str()) {
                        obj.insert("source".to_string(), Value::String(path.to_string()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> PluginsHandler {
        let maps = load_mappings(Path::new("mappings"));
        PluginsHandler {
            map: maps["plugins"].clone(),
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
