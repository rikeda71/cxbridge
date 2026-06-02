use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{
    new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::{applies_direction, DomainMap};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};
use crate::degrade::rules::degrade_allowed_tools;
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// Handler for the settings domain (partial-conversion subset).
pub struct SettingsHandler {
    pub map: DomainMap,
}

impl Handler for SettingsHandler {
    fn kind(&self) -> Kind {
        Kind::Settings
    }

    fn detect(&self, path: &Path) -> bool {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        matches!(
            file_name,
            "settings.json" | "settings.local.json" | "config.toml"
        )
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if file_name.ends_with(".json") {
            // Claude settings.json
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read settings file: {}", path.display()))?;
            let json_val: serde_json::Value = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse settings.json: {}", path.display()))?;

            Ok(serde_json::json!({
                "frontmatter": json_val,
                "body": "",
                "path": abs_path.to_str().unwrap_or(""),
                "format": "json"
            }))
        } else if file_name.ends_with(".toml") {
            // Codex config.toml
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read config.toml: {}", path.display()))?;
            let toml_val: toml::Value = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config.toml: {}", path.display()))?;
            let json_val = crate::core::serialize::toml_to_json(&toml_val)?;

            Ok(serde_json::json!({
                "frontmatter": json_val,
                "body": "",
                "path": abs_path.to_str().unwrap_or(""),
                "format": "toml"
            }))
        } else {
            anyhow::bail!("SettingsHandler: unsupported file: {}", path.display())
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Settings, source_tool, &source_path);

        let settings = match parsed["frontmatter"].as_object() {
            Some(obj) => obj,
            None => return Ok(node),
        };

        match dir {
            ConvDir::C2x => self.lift_c2x(settings, &mut node),
            ConvDir::X2c => self.lift_x2c(settings, &mut node),
        }

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

impl SettingsHandler {
    /// Lift Claude settings.json fields into IR (c2x direction).
    fn lift_c2x(&self, settings: &serde_json::Map<String, Value>, node: &mut IRNode) {
        // Flat key scan: model, effortLevel, editorMode, etc.
        self.lift_flat_key(settings, node, ConvDir::C2x, "model", "settings.model");
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "effortLevel",
            "settings.effortLevel",
        );
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "editorMode",
            "settings.editorMode",
        );
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "autoMemoryEnabled",
            "settings.autoMemoryEnabled",
        );
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "cleanupPeriodDays",
            "settings.cleanupPeriodDays",
        );
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "language",
            "settings.language",
        );
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "defaultShell",
            "settings.defaultShell",
        );
        self.lift_flat_key(
            settings,
            node,
            ConvDir::C2x,
            "outputStyle",
            "settings.outputStyle",
        );

        // env object
        if let Some(env_val) = settings.get("env") {
            self.add_field(node, "settings.env", env_val.clone(), ConvDir::C2x);
        }

        // attribution.commit
        if let Some(attr) = settings.get("attribution") {
            if let Some(commit) = attr.get("commit") {
                self.add_field(
                    node,
                    "settings.attribution.commit",
                    commit.clone(),
                    ConvDir::C2x,
                );
            }
        }

        // includeCoAuthoredBy
        if let Some(v) = settings.get("includeCoAuthoredBy") {
            self.add_field(
                node,
                "settings.includeCoAuthoredBy",
                v.clone(),
                ConvDir::C2x,
            );
        }

        // sandbox.network.allowAllUnixSockets
        if let Some(sandbox) = settings.get("sandbox") {
            if let Some(network) = sandbox.get("network") {
                if let Some(allow_unix) = network.get("allowAllUnixSockets") {
                    self.add_field(
                        node,
                        "settings.sandbox.network.allowAllUnixSockets",
                        allow_unix.clone(),
                        ConvDir::C2x,
                    );
                }
                if let Some(mach) = network.get("allowMachLookup") {
                    self.add_field(
                        node,
                        "settings.sandbox.network.allowMachLookup",
                        mach.clone(),
                        ConvDir::C2x,
                    );
                }
                if let Some(domains) = network.get("allowedDomains") {
                    self.add_field(
                        node,
                        "settings.sandbox.network.allowedDomains",
                        domains.clone(),
                        ConvDir::C2x,
                    );
                }
            }
            if let Some(fs_obj) = sandbox.get("filesystem") {
                for (k, v) in [
                    ("allowWrite", "settings.sandbox.filesystem.allowWrite"),
                    ("denyWrite", "settings.sandbox.filesystem.denyWrite"),
                    ("allowRead", "settings.sandbox.filesystem.allowRead"),
                    ("denyRead", "settings.sandbox.filesystem.denyRead"),
                ] {
                    if let Some(val) = fs_obj.get(k) {
                        self.add_field(node, v, val.clone(), ConvDir::C2x);
                    }
                }
            }
        }

        // permissions.allow / deny / ask
        if let Some(perms) = settings.get("permissions") {
            if let Some(allow) = perms.get("allow") {
                // Store the entire allow array; lower() will split by tool type
                self.add_field(node, "__permissions.allow", allow.clone(), ConvDir::C2x);
            }
            if let Some(deny) = perms.get("deny") {
                self.add_field(node, "__permissions.deny", deny.clone(), ConvDir::C2x);
            }
            if let Some(ask) = perms.get("ask") {
                self.add_field(node, "__permissions.ask", ask.clone(), ConvDir::C2x);
            }
            if let Some(default_mode) = perms.get("defaultMode") {
                self.add_field(
                    node,
                    "__permissions.defaultMode",
                    default_mode.clone(),
                    ConvDir::C2x,
                );
            }
        }

        // Dropped fields: record them so report sees them
        for dropped_key in &[
            "viewMode",
            "worktree",
            "autoUpdatesChannel",
            "spinnerTipsEnabled",
            "spinnerTipsOverride",
            "spinnerVerbs",
            "voice",
            "voiceEnabled",
            "maxSkillDescriptionChars",
            "skillListingBudgetFraction",
            "statusLine",
        ] {
            if let Some(v) = settings.get(*dropped_key) {
                node.fields.insert(
                    format!("settings.{}", dropped_key),
                    IRField {
                        id: format!("settings.{}", dropped_key),
                        value: v.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!("{} has no Codex equivalent", dropped_key)),
                        dropped: Some(DroppedInfo {
                            reason: format!("{} dropped (Claude-specific UI field)", dropped_key),
                        }),
                    },
                );
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(format!("settings.{}", dropped_key)),
                    message: format!("{} dropped: no Codex equivalent", dropped_key),
                });
            }
        }

        // skillOverrides, enabledPlugins → lossy
        for lossy_key in &["skillOverrides", "enabledPlugins"] {
            if let Some(v) = settings.get(*lossy_key) {
                node.fields.insert(
                    format!("settings.{}", lossy_key),
                    IRField {
                        id: format!("settings.{}", lossy_key),
                        value: v.clone(),
                        loss: Loss::Lossy,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!(
                            "{} is lossy (Claude and Codex plugin systems differ)",
                            lossy_key
                        )),
                        dropped: None,
                    },
                );
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some(format!("settings.{}", lossy_key)),
                    message: format!("{} conversion is lossy (manual review required)", lossy_key),
                });
            }
        }
    }

    /// Lift Codex config.toml fields into IR (x2c direction).
    fn lift_x2c(&self, settings: &serde_json::Map<String, Value>, node: &mut IRNode) {
        // model, model_reasoning_effort, dangerously_allow_all_unix_sockets, commit_attribution
        self.add_field_if_present(settings, node, "model", "settings.model");
        self.add_field_if_present(
            settings,
            node,
            "model_reasoning_effort",
            "settings.effortLevel",
        );
        // Nested under [features.network_proxy] in Codex config.toml.
        if let Some(allow_unix) = settings
            .get("features")
            .and_then(|f| f.get("network_proxy"))
            .and_then(|np| np.get("dangerously_allow_all_unix_sockets"))
        {
            self.add_field(
                node,
                "settings.sandbox.network.allowAllUnixSockets",
                allow_unix.clone(),
                ConvDir::X2c,
            );
        }
        self.add_field_if_present(
            settings,
            node,
            "commit_attribution",
            "settings.attribution.commit",
        );

        // env: shell_environment_policy.set
        if let Some(env_policy) = settings.get("shell_environment_policy") {
            if let Some(set_val) = env_policy.get("set") {
                self.add_field(node, "settings.env", set_val.clone(), ConvDir::X2c);
            }
        }

        // tui.vim_mode_default → editorMode
        if let Some(tui) = settings.get("tui") {
            if let Some(vim_mode) = tui.get("vim_mode_default") {
                // enum_map: true → vim, false → normal
                let editor_mode = if vim_mode.as_bool().unwrap_or(false) {
                    Value::String("vim".to_string())
                } else {
                    Value::String("normal".to_string())
                };
                node.fields.insert(
                    "settings.editorMode".to_string(),
                    IRField {
                        id: "settings.editorMode".to_string(),
                        value: editor_mode,
                        loss: Loss::Lossless,
                        transforms_applied: vec!["enum_map".to_string()],
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );
            }
        }

        // memories.use_memories + generate_memories → autoMemoryEnabled
        if let Some(memories) = settings.get("memories") {
            let use_mem = memories
                .get("use_memories")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let gen_mem = memories
                .get("generate_memories")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let auto_memory = use_mem && gen_mem;
            node.fields.insert(
                "settings.autoMemoryEnabled".to_string(),
                IRField {
                    id: "settings.autoMemoryEnabled".to_string(),
                    value: Value::Bool(auto_memory),
                    loss: Loss::Lossy,
                    transforms_applied: vec![],
                    degrade: None,
                    warning: Some(
                        "autoMemoryEnabled: Claude 1 field ← Codex use_memories+generate_memories (lossy)"
                            .to_string(),
                    ),
                    dropped: None,
                },
            );
            if let Some(max_age) = memories.get("max_rollout_age_days") {
                self.add_field(
                    node,
                    "settings.cleanupPeriodDays",
                    max_age.clone(),
                    ConvDir::X2c,
                );
            }
        }

        // Codex-specific dropped fields
        for dropped_key in &[
            "profiles",
            "approval_policy",
            "agents",
            "otel",
            "web_search",
            "project_doc_max_bytes",
            "model_verbosity",
            "model_reasoning_summary",
        ] {
            if settings.get(*dropped_key).is_some() {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(format!("settings.codex.{}", dropped_key)),
                    message: format!("{} dropped: no Claude equivalent", dropped_key),
                });
            }
        }

        // developer_instructions → CLAUDE.md (degrade)
        if let Some(dev_inst) = settings.get("developer_instructions") {
            node.fields.insert(
                "settings.codex.developer_instructions".to_string(),
                IRField {
                    id: "settings.codex.developer_instructions".to_string(),
                    value: dev_inst.clone(),
                    loss: Loss::Lossy,
                    transforms_applied: vec![],
                    degrade: Some(DegradeInfo {
                        to: "project".to_string(),
                        target: "CLAUDE.md".to_string(),
                    }),
                    warning: Some(
                        "developer_instructions → CLAUDE.md (lossy: scope fixed to project)"
                            .to_string(),
                    ),
                    dropped: None,
                },
            );
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("settings.codex.developer_instructions".to_string()),
                message: "developer_instructions degraded to CLAUDE.md (lossy)".to_string(),
            });
        }
    }

    /// Add an IRField for a flat settings key using mappings.
    fn lift_flat_key(
        &self,
        settings: &serde_json::Map<String, Value>,
        node: &mut IRNode,
        dir: ConvDir,
        settings_key: &str,
        entry_id: &str,
    ) {
        if let Some(v) = settings.get(settings_key) {
            if let Some(entry) = self.map.entries.iter().find(|e| e.id == entry_id) {
                if !applies_direction(entry, dir) {
                    return;
                }
                let ctx = TransformCtx {
                    direction: dir,
                    args: None,
                    field: entry,
                };
                let (transformed, applied) = apply_transforms(v, entry.transform.as_deref(), &ctx);

                let loss = Loss::from(&entry.loss);

                let warning = if entry.warn == Some(true) {
                    Some(format!(
                        "{}: {}",
                        entry.id,
                        entry.notes.as_deref().unwrap_or("warn")
                    ))
                } else {
                    None
                };

                node.fields.insert(
                    entry.id.clone(),
                    IRField {
                        id: entry.id.clone(),
                        value: transformed,
                        loss,
                        transforms_applied: applied,
                        degrade: None,
                        warning: warning.clone(),
                        dropped: None,
                    },
                );

                if let Some(msg) = warning {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some(entry.id.clone()),
                        message: msg,
                    });
                }
            }
        }
    }

    /// Add a field directly with default IR semantics.
    fn add_field(&self, node: &mut IRNode, id: &str, value: Value, dir: ConvDir) {
        if let Some(entry) = self.map.entries.iter().find(|e| e.id == id) {
            if !applies_direction(entry, dir) {
                return;
            }
            let ctx = TransformCtx {
                direction: dir,
                args: None,
                field: entry,
            };
            let (transformed, applied) = apply_transforms(&value, entry.transform.as_deref(), &ctx);
            let loss = Loss::from(&entry.loss);
            let warning = if entry.warn == Some(true) {
                Some(format!(
                    "{}: {}",
                    entry.id,
                    entry.notes.as_deref().unwrap_or("warn")
                ))
            } else {
                None
            };
            node.fields.insert(
                id.to_string(),
                IRField {
                    id: id.to_string(),
                    value: transformed,
                    loss,
                    transforms_applied: applied,
                    degrade: None,
                    warning: warning.clone(),
                    dropped: None,
                },
            );
            if let Some(msg) = warning {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some(id.to_string()),
                    message: msg,
                });
            }
        } else {
            // Unknown id (internal placeholder like __permissions.allow): store raw
            node.fields.insert(
                id.to_string(),
                IRField {
                    id: id.to_string(),
                    value,
                    loss: Loss::Lossless,
                    transforms_applied: vec![],
                    degrade: None,
                    warning: None,
                    dropped: None,
                },
            );
        }
    }

    fn add_field_if_present(
        &self,
        settings: &serde_json::Map<String, Value>,
        node: &mut IRNode,
        settings_key: &str,
        entry_id: &str,
    ) {
        if let Some(v) = settings.get(settings_key) {
            self.add_field(node, entry_id, v.clone(), ConvDir::X2c);
        }
    }

    /// c2x lower: produce Codex config.toml / rules files from IR.
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build TOML document
        let mut doc = toml_edit::DocumentMut::new();

        // model
        if let Some(f) = ir.fields.get("settings.model") {
            if let Some(s) = f.value.as_str() {
                doc.insert("model", toml_edit::value(s));
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.model".to_string()),
                    message: format!(
                        "model '{}' may not be a valid Codex model ID (different provider; manual review required)",
                        s
                    ),
                });
            }
        }

        // effortLevel → model_reasoning_effort (enum_map applied in lift)
        if let Some(f) = ir.fields.get("settings.effortLevel") {
            if let Some(s) = f.value.as_str() {
                doc.insert("model_reasoning_effort", toml_edit::value(s));
            }
        }

        // editorMode → tui.vim_mode_default (enum_map applied in lift: vim→"true", normal→"false")
        // Note: enum_map maps strings to strings, so "vim" → "true" (string), "normal" → "false" (string)
        if let Some(f) = ir.fields.get("settings.editorMode") {
            let vim_mode = match f.value.as_str() {
                Some("vim") | Some("true") => Some(true),
                Some("normal") | Some("false") => Some(false),
                // Might already be a bool
                _ => f.value.as_bool(),
            };
            if let Some(b) = vim_mode {
                let tui_item = doc
                    .entry("tui")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(tbl) = tui_item.as_table_mut() {
                    tbl.insert("vim_mode_default", toml_edit::value(b));
                }
            }
        }

        // env → shell_environment_policy.set
        if let Some(f) = ir.fields.get("settings.env") {
            if let Value::Object(env_map) = &f.value {
                let policy_item = doc
                    .entry("shell_environment_policy")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(policy_tbl) = policy_item.as_table_mut() {
                    let set_item = policy_tbl
                        .entry("set")
                        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                    if let Some(set_tbl) = set_item.as_table_mut() {
                        for (k, v) in env_map {
                            if let Some(s) = v.as_str() {
                                set_tbl.insert(k, toml_edit::value(s));
                            }
                        }
                    }
                }
            }
        }

        // attribution.commit → commit_attribution
        if let Some(f) = ir.fields.get("settings.attribution.commit") {
            if let Some(s) = f.value.as_str() {
                doc.insert("commit_attribution", toml_edit::value(s));
            }
        }

        // sandbox.network.allowAllUnixSockets →
        // features.network_proxy.dangerously_allow_all_unix_sockets (nested, per mapping)
        if let Some(f) = ir
            .fields
            .get("settings.sandbox.network.allowAllUnixSockets")
        {
            if let Some(b) = f.value.as_bool() {
                let features_item = doc
                    .entry("features")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(ftbl) = features_item.as_table_mut() {
                    let np_item = ftbl
                        .entry("network_proxy")
                        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                    if let Some(nptbl) = np_item.as_table_mut() {
                        nptbl.insert("dangerously_allow_all_unix_sockets", toml_edit::value(b));
                    }
                }
            }
        }

        // autoMemoryEnabled → memories.use_memories + memories.generate_memories
        if let Some(f) = ir.fields.get("settings.autoMemoryEnabled") {
            if let Some(b) = f.value.as_bool() {
                // features.memories must also be true
                let features_item = doc
                    .entry("features")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(ftbl) = features_item.as_table_mut() {
                    if ftbl.get("memories").is_none() {
                        ftbl.insert("memories", toml_edit::value(b));
                    }
                }
                let mem_item = doc
                    .entry("memories")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(mtbl) = mem_item.as_table_mut() {
                    if mtbl.get("use_memories").is_none() {
                        mtbl.insert("use_memories", toml_edit::value(b));
                    }
                    if mtbl.get("generate_memories").is_none() {
                        mtbl.insert("generate_memories", toml_edit::value(b));
                    }
                }
            }
        }

        // cleanupPeriodDays → memories.max_rollout_age_days (clamp 0-90)
        if let Some(f) = ir.fields.get("settings.cleanupPeriodDays") {
            if let Some(days) = f.value.as_i64() {
                let clamped = days.clamp(0, 90);
                let mem_item = doc
                    .entry("memories")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(mtbl) = mem_item.as_table_mut() {
                    if mtbl.get("max_rollout_age_days").is_none() {
                        mtbl.insert("max_rollout_age_days", toml_edit::value(clamped));
                    }
                }
                if clamped != days {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.cleanupPeriodDays".to_string()),
                        message: format!(
                            "cleanupPeriodDays={} clamped to {} (Codex max_rollout_age_days range: 0-90)",
                            days, clamped
                        ),
                    });
                }
            }
        }

        // sandbox.filesystem.* → [permissions.default].filesystem
        let mut fs_perms: Vec<(String, &str)> = Vec::new();
        if let Some(f) = ir.fields.get("settings.sandbox.filesystem.allowWrite") {
            for path in crate::handlers::json_to_string_list(&f.value) {
                fs_perms.push((path, "write"));
            }
        }
        if let Some(f) = ir.fields.get("settings.sandbox.filesystem.denyWrite") {
            for path in crate::handlers::json_to_string_list(&f.value) {
                fs_perms.push((path, "deny"));
            }
        }
        if let Some(f) = ir.fields.get("settings.sandbox.filesystem.allowRead") {
            for path in crate::handlers::json_to_string_list(&f.value) {
                fs_perms.push((path, "read"));
            }
        }
        if let Some(f) = ir.fields.get("settings.sandbox.filesystem.denyRead") {
            for path in crate::handlers::json_to_string_list(&f.value) {
                fs_perms.push((path, "deny"));
            }
        }

        // sandbox.network.allowedDomains → [permissions.default].network.domains
        let mut network_domains: Vec<String> = Vec::new();
        if let Some(f) = ir.fields.get("settings.sandbox.network.allowedDomains") {
            network_domains = crate::handlers::json_to_string_list(&f.value);
        }

        // permissions.deny WebFetch domains → [permissions.default].network.domains (deny)
        let mut network_deny_domains: Vec<String> = Vec::new();

        // permissions.allow/deny/ask → split by tool type
        if let Some(f) = ir.fields.get("__permissions.allow") {
            let tools = crate::handlers::json_to_string_list(&f.value);
            let (bash_tools, fs_allow_read, fs_allow_write, web_domains) =
                split_permissions_by_type(&tools);

            // Bash → .rules
            if !bash_tools.is_empty() {
                let (arts, diags) = degrade_allowed_tools("default", &bash_tools, true, opts.scope);
                for art in &arts {
                    files.push(EmitFile {
                        path: format!("{}/{}", out_root, art.path),
                        content: art.content.clone(),
                    });
                }
                diagnostics.extend(diags);
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.permissions.allow.bash".to_string()),
                    message: "permissions.allow Bash patterns → .codex/rules/default.rules (scope expanded to project)".to_string(),
                });
            }

            for path_str in fs_allow_read {
                fs_perms.push((path_str, "read"));
            }
            for path_str in fs_allow_write {
                fs_perms.push((path_str, "write"));
            }
            for domain in web_domains {
                network_domains.push(domain);
            }
        }

        if let Some(f) = ir.fields.get("__permissions.deny") {
            let tools = crate::handlers::json_to_string_list(&f.value);
            let (bash_tools, fs_deny_read, fs_deny_write, web_deny_domains) =
                split_permissions_by_type(&tools);

            if !bash_tools.is_empty() {
                let (arts, diags) =
                    degrade_allowed_tools("default", &bash_tools, false, opts.scope);
                for art in &arts {
                    files.push(EmitFile {
                        path: format!("{}/{}", out_root, art.path),
                        content: art.content.clone(),
                    });
                }
                diagnostics.extend(diags);
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.permissions.deny.bash".to_string()),
                    message: "permissions.deny Bash patterns → .codex/rules/default.rules (deny)"
                        .to_string(),
                });
            }

            for path_str in fs_deny_read.into_iter().chain(fs_deny_write) {
                fs_perms.push((path_str, "deny"));
            }
            for d in web_deny_domains {
                network_deny_domains.push(d);
            }
            if !network_deny_domains.is_empty() {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.permissions.deny.webfetch".to_string()),
                    message:
                        "permissions.deny WebFetch domains → [permissions.default].network.domains (deny)"
                            .to_string(),
                });
            }
        }

        if let Some(f) = ir.fields.get("__permissions.ask") {
            let tools = crate::handlers::json_to_string_list(&f.value);
            let (bash_tools, _, _, _) = split_permissions_by_type(&tools);
            if !bash_tools.is_empty() {
                // ask → prompt decision in .rules
                // We reuse degrade_allowed_tools with allow=true and note it as prompt
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.permissions.ask.bash".to_string()),
                    message: format!(
                        "permissions.ask Bash patterns ({}) require manual conversion to .rules prompt decision",
                        bash_tools.join(", ")
                    ),
                });
            }
        }

        // defaultMode
        if let Some(f) = ir.fields.get("__permissions.defaultMode") {
            if let Some(mode) = f.value.as_str() {
                let (approval, sandbox) = map_default_mode(mode);
                if let Some(ap) = approval {
                    doc.insert("approval_policy", toml_edit::value(ap));
                }
                if let Some(sm) = sandbox {
                    doc.insert("sandbox_mode", toml_edit::value(sm));
                }
                if mode == "plan" {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Drop,
                        id: Some("settings.permissions.defaultMode.plan".to_string()),
                        message: "defaultMode=plan dropped: Codex has no plan mode equivalent"
                            .to_string(),
                    });
                } else if mode == "dontAsk" {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.permissions.defaultMode.dontAsk".to_string()),
                        message: "defaultMode=dontAsk approximated by approval_policy=never + \
                                  sandbox_mode=danger-full-access; this grants broader access than \
                                  dontAsk intends — review the security implication"
                            .to_string(),
                    });
                }
            }
        }

        // Write permissions table to TOML
        if !fs_perms.is_empty() || !network_domains.is_empty() || !network_deny_domains.is_empty() {
            let perms_item = doc
                .entry("permissions")
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
            if let Some(ptbl) = perms_item.as_table_mut() {
                let default_item = ptbl
                    .entry("default")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(dtbl) = default_item.as_table_mut() {
                    // filesystem
                    if !fs_perms.is_empty() {
                        let fs_item = dtbl
                            .entry("filesystem")
                            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                        if let Some(fs_tbl) = fs_item.as_table_mut() {
                            for (path_str, access) in &fs_perms {
                                fs_tbl.insert(path_str.as_str(), toml_edit::value(*access));
                            }
                        }
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("settings.sandbox.filesystem".to_string()),
                            message: "sandbox.filesystem paths → [permissions.default].filesystem (lossy: tool-axis vs resource-axis)".to_string(),
                        });
                    }

                    // network
                    if !network_domains.is_empty() || !network_deny_domains.is_empty() {
                        let net_item = dtbl
                            .entry("network")
                            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                        if let Some(net_tbl) = net_item.as_table_mut() {
                            net_tbl.insert("enabled", toml_edit::value(true));
                            let domains_item = net_tbl
                                .entry("domains")
                                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                            if let Some(dom_tbl) = domains_item.as_table_mut() {
                                for domain in &network_domains {
                                    dom_tbl.insert(domain, toml_edit::value("allow"));
                                }
                                for domain in &network_deny_domains {
                                    dom_tbl.insert(domain, toml_edit::value("deny"));
                                }
                            }
                        }
                        if !network_domains.is_empty() {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: Some("settings.sandbox.network.allowedDomains".to_string()),
                                message: "network domains → [permissions.default].network.domains (network.enabled=true added)".to_string(),
                            });
                        }
                    }
                }
            }
        }

        let toml_content = doc.to_string();
        if !toml_content.trim().is_empty() {
            files.push(EmitFile {
                path: format!("{}/.codex/config.toml", out_root),
                content: toml_content,
            });
        }

        // Warn about un-converted remainder
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: None,
            message: "settings.json → config.toml: partial conversion only. \
                      hooks, mcpServers, plugins, and many Claude-specific fields require manual conversion. \
                      Review the full settings.json for remaining fields."
                .to_string(),
        });

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c lower: produce Claude settings.json from Codex config.toml IR.
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        let out_root = opts.out.as_deref().unwrap_or(".");

        let mut settings = serde_json::Map::new();

        // model
        if let Some(f) = ir.fields.get("settings.model") {
            if let Some(s) = f.value.as_str() {
                settings.insert("model".to_string(), Value::String(s.to_string()));
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.model".to_string()),
                    message: format!(
                        "model '{}' may not be a valid Claude model ID (different provider)",
                        s
                    ),
                });
            }
        }

        // effortLevel (already reverse-mapped via lift)
        if let Some(f) = ir.fields.get("settings.effortLevel") {
            if let Some(s) = f.value.as_str() {
                settings.insert("effortLevel".to_string(), Value::String(s.to_string()));
            }
        }

        // editorMode
        if let Some(f) = ir.fields.get("settings.editorMode") {
            settings.insert("editorMode".to_string(), f.value.clone());
        }

        // env
        if let Some(f) = ir.fields.get("settings.env") {
            settings.insert("env".to_string(), f.value.clone());
        }

        // attribution.commit
        if let Some(f) = ir.fields.get("settings.attribution.commit") {
            if let Some(s) = f.value.as_str() {
                let mut attr = serde_json::Map::new();
                attr.insert("commit".to_string(), Value::String(s.to_string()));
                settings.insert("attribution".to_string(), Value::Object(attr));
            }
        }

        // sandbox.network.allowAllUnixSockets
        if let Some(f) = ir
            .fields
            .get("settings.sandbox.network.allowAllUnixSockets")
        {
            if let Some(b) = f.value.as_bool() {
                let sandbox = settings
                    .entry("sandbox".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Value::Object(s_obj) = sandbox {
                    let network = s_obj
                        .entry("network".to_string())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                    if let Value::Object(n_obj) = network {
                        n_obj.insert("allowAllUnixSockets".to_string(), Value::Bool(b));
                    }
                }
            }
        }

        // autoMemoryEnabled
        if let Some(f) = ir.fields.get("settings.autoMemoryEnabled") {
            settings.insert("autoMemoryEnabled".to_string(), f.value.clone());
        }

        // cleanupPeriodDays
        if let Some(f) = ir.fields.get("settings.cleanupPeriodDays") {
            settings.insert("cleanupPeriodDays".to_string(), f.value.clone());
        }

        let json_content = serde_json::to_string_pretty(&Value::Object(settings))
            .with_context(|| "Failed to serialize settings.json")?;

        if json_content.trim() != "{}" {
            files.push(EmitFile {
                path: format!("{}/.claude/settings.json", out_root),
                content: json_content,
            });
        }

        // developer_instructions → CLAUDE.md (degrade: scope fixed to project)
        if let Some(f) = ir.fields.get("settings.codex.developer_instructions") {
            if let Some(text) = f.value.as_str() {
                let claude_md_content = format!(
                    "# Developer Instructions\n\n\
                     <!-- Converted from Codex developer_instructions (lossy: scope fixed to project) -->\n\n\
                     {}\n",
                    text
                );
                files.push(EmitFile {
                    path: format!("{}/.claude/CLAUDE.md", out_root),
                    content: claude_md_content,
                });
                // The warning diagnostic is already emitted during lift_x2c.
                // Emitting it again here would cause duplicate entries in the report.
            }
        }

        // Warn about remainder
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: None,
            message: "config.toml → settings.json: partial conversion only. \
                      hooks, mcp_servers, plugins, and many Codex-specific fields require manual conversion."
                .to_string(),
        });

        Ok(EmitPlan { files, diagnostics })
    }
}

/// Split a Claude permissions list into typed buckets:
/// (bash_tools, read_paths, write_paths, web_domains)
fn split_permissions_by_type(
    tools: &[String],
) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let mut bash = Vec::new();
    let mut read = Vec::new();
    let mut write = Vec::new();
    let mut web = Vec::new();

    for tool in tools {
        let t = tool.trim();
        if t.starts_with("Bash(") || t == "Bash" {
            bash.push(t.to_string());
        } else if t.starts_with("Read(") {
            let path = extract_tool_arg(t);
            read.push(path);
        } else if t.starts_with("Write(") || t.starts_with("Edit(") {
            let path = extract_tool_arg(t);
            write.push(path);
        } else if t.starts_with("WebFetch(domain:") {
            let domain = t
                .trim_start_matches("WebFetch(domain:")
                .trim_end_matches(')')
                .to_string();
            web.push(domain);
        } else if t == "WebFetch" || t == "WebSearch" {
            // Coarse allow: no specific domain
            // We can't map this to a specific domain, record as dropped
        }
        // Other tools (AskUserQuestion, etc.) → dropped (no bucket)
    }

    (bash, read, write, web)
}

/// Extract the argument from a tool pattern like `Bash(git add)` → `git add`.
fn extract_tool_arg(tool: &str) -> String {
    if let Some(start) = tool.find('(') {
        let rest = &tool[start + 1..];
        rest.trim_end_matches(')').to_string()
    } else {
        tool.to_string()
    }
}

/// Map Claude defaultMode to Codex approval_policy + sandbox_mode.
fn map_default_mode(mode: &str) -> (Option<&'static str>, Option<&'static str>) {
    match mode {
        "bypassPermissions" => (Some("never"), Some("danger-full-access")),
        // dontAsk suppresses prompts; approximated by the same full-access combo
        // as bypassPermissions (a broader relaxation than dontAsk intends — warned).
        "dontAsk" => (Some("never"), Some("danger-full-access")),
        "acceptEdits" => (Some("untrusted"), None),
        "auto" => (Some("on-request"), None),
        "plan" => (None, None), // dropped; handled separately
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use tempfile::TempDir;

    fn make_handler() -> SettingsHandler {
        let maps = load_mappings(Path::new("mappings"));
        SettingsHandler {
            map: maps["settings-config"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
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

    #[test]
    fn test_settings_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("settings.json")));
        assert!(h.detect(Path::new("settings.local.json")));
        assert!(h.detect(Path::new("config.toml")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
    }

    #[test]
    fn test_settings_c2x_model_effort() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"model": "claude-sonnet-4-6", "effortLevel": "max"}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // model and effortLevel should be present
        assert!(ir.fields.contains_key("settings.model"));
        assert!(ir.fields.contains_key("settings.effortLevel"));

        // effortLevel max → xhigh via enum_map
        let effort_f = &ir.fields["settings.effortLevel"];
        assert_eq!(effort_f.value, Value::String("xhigh".to_string()));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // config.toml should be generated
        let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
        assert!(config_toml.is_some(), "Expected config.toml output");

        let content = &config_toml.unwrap().content;
        assert!(
            content.contains("model_reasoning_effort"),
            "Expected model_reasoning_effort in config.toml"
        );
        assert!(content.contains("xhigh"), "Expected xhigh in config.toml");
    }

    #[test]
    fn test_settings_c2x_editor_mode() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(&settings_path, r#"{"editorMode": "vim"}"#).unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(ir.fields.contains_key("settings.editorMode"));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            config_toml.content.contains("vim_mode_default = true"),
            "Expected vim_mode_default=true, got: {}",
            config_toml.content
        );
    }

    #[test]
    fn test_settings_c2x_permissions_bash_to_rules() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"permissions": {"allow": ["Bash(cargo build)"]}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Should generate a .rules file
        let rules_file = plan.files.iter().find(|f| f.path.ends_with(".rules"));
        assert!(
            rules_file.is_some(),
            "Expected .rules file for Bash permission, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_settings_c2x_default_mode_dont_ask_warns_and_converts() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"permissions": {"defaultMode": "dontAsk"}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // dontAsk converts to approval_policy=never + sandbox_mode=danger-full-access.
        let config = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml output");
        assert!(config.content.contains("approval_policy = \"never\""));
        assert!(config
            .content
            .contains("sandbox_mode = \"danger-full-access\""));

        // The lossy approximation must be surfaced, not silent (warn:true contract).
        assert!(
            plan.diagnostics.iter().any(|d| d.level == DiagLevel::Warn
                && d.id.as_deref() == Some("settings.permissions.defaultMode.dontAsk")),
            "Expected a Warn diagnostic for defaultMode=dontAsk, got: {:?}",
            plan.diagnostics
        );
    }

    #[test]
    fn test_settings_c2x_dropped_fields_in_report() {
        use crate::core::report::build_report;
        use crate::handlers::EmitPlan;

        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"model": "claude-sonnet-4-6", "viewMode": "verbose", "worktree": {"enabled": true}, "autoUpdatesChannel": "latest"}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let empty_plan = EmitPlan {
            files: vec![],
            diagnostics: vec![],
        };
        let report = build_report(&ir, &empty_plan);

        // Dropped fields should be enumerated in the report
        assert!(
            !report.dropped.is_empty(),
            "Expected dropped fields in report"
        );
        let dropped_ids: Vec<_> = report
            .dropped
            .iter()
            .filter_map(|d| d.id.as_deref())
            .collect();
        assert!(
            dropped_ids.contains(&"settings.viewMode"),
            "Expected settings.viewMode in dropped, got: {:?}",
            dropped_ids
        );
        assert!(
            dropped_ids.contains(&"settings.worktree"),
            "Expected settings.worktree in dropped, got: {:?}",
            dropped_ids
        );
    }

    #[test]
    fn test_settings_c2x_sandbox_filesystem() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"sandbox": {"filesystem": {"allowWrite": ["/tmp/build"], "denyRead": ["~/.env"]}}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            config_toml.content.contains("[permissions"),
            "Expected permissions section, got: {}",
            config_toml.content
        );
        assert!(
            config_toml.content.contains("filesystem"),
            "Expected filesystem in permissions"
        );
    }

    #[test]
    fn test_settings_c2x_report_enumerates_remainder() {
        // The report should include un-converted fields as manual items
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"model": "claude-opus-4-8", "viewMode": "focus", "autoUpdatesChannel": "stable"}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // The plan should include a warning about partial conversion
        let has_partial_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("partial conversion"));
        assert!(
            has_partial_warn,
            "Expected partial conversion warning in diagnostics"
        );
    }

    #[test]
    fn test_settings_x2c_basic() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-5-codex"
model_reasoning_effort = "high"

[features.network_proxy]
dangerously_allow_all_unix_sockets = false
"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        assert!(ir.fields.contains_key("settings.model"));
        assert!(ir.fields.contains_key("settings.effortLevel"));
        assert!(ir
            .fields
            .contains_key("settings.sandbox.network.allowAllUnixSockets"));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let settings_json = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("settings.json"));
        assert!(settings_json.is_some(), "Expected settings.json in output");

        let content: Value = serde_json::from_str(&settings_json.unwrap().content).unwrap();
        assert!(content.get("model").is_some(), "Expected model field");
        assert!(
            content.get("effortLevel").is_some(),
            "Expected effortLevel field"
        );
    }

    #[test]
    fn test_developer_instructions_produces_claude_md() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-5-codex"
developer_instructions = "Always respond in English. Focus on clear answers."
"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        // IR must contain the developer_instructions field with degrade info
        assert!(
            ir.fields
                .contains_key("settings.codex.developer_instructions"),
            "IR must contain settings.codex.developer_instructions"
        );
        let f = &ir.fields["settings.codex.developer_instructions"];
        assert!(f.degrade.is_some(), "Field must have degrade info");
        assert_eq!(
            f.degrade.as_ref().unwrap().target,
            "CLAUDE.md",
            "Degrade target must be CLAUDE.md"
        );

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        // Plan must contain a file ending with CLAUDE.md
        let claude_md = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
        assert!(
            claude_md.is_some(),
            "Plan must contain CLAUDE.md file; got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        // CLAUDE.md content must contain the original instruction text
        let content = &claude_md.unwrap().content;
        assert!(
            content.contains("Always respond in English"),
            "CLAUDE.md must contain original instruction text; got:\n{}",
            content
        );
        assert!(
            content.contains("Focus on clear answers"),
            "CLAUDE.md must contain full instruction text; got:\n{}",
            content
        );

        // The warning diagnostic is emitted during lift (ir.diagnostics), not lower.
        let has_diag = ir
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("settings.codex.developer_instructions"));
        assert!(
            has_diag,
            "Expected developer_instructions diagnostic in ir.diagnostics; got: {:?}",
            ir.diagnostics
                .iter()
                .map(|d| d.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_webfetch_deny_domains_in_config_toml_and_diagnostic() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"permissions": {"deny": ["WebFetch(domain:bad.com)", "WebFetch(domain:evil.net)"]}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // config.toml must be generated
        let config_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml output");

        let content = &config_toml.content;

        // bad.com and evil.net must appear with "deny"
        assert!(
            content.contains("bad.com") && content.contains("deny"),
            "Expected bad.com = \"deny\" in config.toml; got:\n{}",
            content
        );
        assert!(
            content.contains("evil.net"),
            "Expected evil.net in config.toml; got:\n{}",
            content
        );

        // Warn diagnostic with id "settings.permissions.deny.webfetch" must exist
        let has_diag = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("settings.permissions.deny.webfetch"));
        assert!(
            has_diag,
            "Expected diagnostic id 'settings.permissions.deny.webfetch'; diagnostics: {:?}",
            plan.diagnostics
                .iter()
                .map(|d| (d.id.as_deref().unwrap_or("<none>"), &d.message))
                .collect::<Vec<_>>()
        );

        let diag = plan
            .diagnostics
            .iter()
            .find(|d| d.id.as_deref() == Some("settings.permissions.deny.webfetch"))
            .unwrap();
        assert_eq!(
            diag.level,
            DiagLevel::Warn,
            "Expected DiagLevel::Warn for settings.permissions.deny.webfetch"
        );
    }
}
