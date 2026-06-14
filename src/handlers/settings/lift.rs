use serde_json::Value;

use crate::core::ir::{DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Loss};
use crate::core::transforms::ConvDir;

use super::approval::{ApprovalPolicy, DefaultMode, SandboxMode};

use super::SettingsHandler;

impl SettingsHandler {
    /// Lift Claude settings.json fields into IR (c2x direction).
    pub(super) fn lift_c2x(&self, settings: &serde_json::Map<String, Value>, node: &mut IRNode) {
        const FLAT_KEYS: &[(&str, &str)] = &[
            ("model", "settings.model"),
            ("effortLevel", "settings.effortLevel"),
            ("editorMode", "settings.editorMode"),
            ("autoMemoryEnabled", "settings.autoMemoryEnabled"),
            ("cleanupPeriodDays", "settings.cleanupPeriodDays"),
            ("language", "settings.language"),
            ("defaultShell", "settings.defaultShell"),
            ("outputStyle", "settings.outputStyle"),
        ];
        for (settings_key, entry_id) in FLAT_KEYS {
            self.lift_flat_key(settings, node, ConvDir::C2x, settings_key, entry_id);
        }

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

        // Dropped fields: record them under their canonical mappings entry id so the
        // report cross-references mappings/*.yaml. Several settings keys share one
        // entry (e.g. spinnerTips*); the first key present wins to avoid duplicates.
        const DROPPED_KEYS: &[(&str, &str)] = &[
            ("viewMode", "settings.viewMode"),
            ("worktree", "settings.worktree"),
            ("autoUpdatesChannel", "settings.autoUpdatesChannel"),
            ("spinnerTipsEnabled", "settings.spinnerTips"),
            ("spinnerTipsOverride", "settings.spinnerTips"),
            ("spinnerVerbs", "settings.spinnerTips"),
            ("voice", "settings.voice"),
            ("voiceEnabled", "settings.voice"),
            (
                "maxSkillDescriptionChars",
                "settings.maxSkillDescriptionChars",
            ),
            (
                "skillListingBudgetFraction",
                "settings.maxSkillDescriptionChars",
            ),
            ("statusLine", "settings.statusLine"),
            (
                "wheelScrollAccelerationEnabled",
                "settings.wheelScrollAcceleration",
            ),
            ("fallbackModel", "settings.fallbackModel"),
            ("availableModels", "settings.availableModels"),
            ("enforceAvailableModels", "settings.availableModels"),
            ("disableBundledSkills", "settings.disableBundledSkills"),
            ("requiredMinimumVersion", "settings.requiredVersionRange"),
            ("requiredMaximumVersion", "settings.requiredVersionRange"),
            ("agent", "settings.agent"),
        ];
        for (settings_key, entry_id) in DROPPED_KEYS {
            let Some(v) = settings.get(*settings_key) else {
                continue;
            };
            if node.fields.contains_key(*entry_id) {
                continue;
            }
            node.fields.insert(
                (*entry_id).to_string(),
                IRField {
                    id: (*entry_id).to_string(),
                    value: v.clone(),
                    loss: Loss::Dropped,
                    transforms_applied: vec![],
                    degrade: None,
                    warning: Some(format!("{} has no Codex equivalent", settings_key)),
                    dropped: Some(DroppedInfo {
                        reason: format!("{} dropped (Claude-specific field)", settings_key),
                    }),
                },
            );
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Drop,
                id: Some((*entry_id).to_string()),
                message: format!("{} dropped: no Codex equivalent", settings_key),
            });
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
    pub(super) fn lift_x2c(&self, settings: &serde_json::Map<String, Value>, node: &mut IRNode) {
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
            // Display customization (theme / status_line / terminal_title) has no Claude
            // receptacle: Claude's statusLine renders a shell command, not item lists.
            if ["theme", "status_line", "terminal_title"]
                .iter()
                .any(|k| tui.get(*k).is_some())
            {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some("settings.codex.tui_display".to_string()),
                    message: "tui.theme/status_line/terminal_title dropped: no Claude equivalent"
                        .to_string(),
                });
            }
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

        // otel is lossy (approximated via env OTEL_* vars), not dropped — route it
        // through the mapping so it lands in the degraded bucket, not dropped.
        if let Some(otel) = settings.get("otel") {
            self.add_field(node, "settings.codex.otel", otel.clone(), ConvDir::X2c);
        }

        // approval_policy + sandbox_mode: two-axis reverse mapping to defaultMode.
        // Only the FLAT string form feeds this path; a table/object value means
        // granular approval (handled below as dropped).
        let approval_val = settings.get("approval_policy");
        let is_granular = approval_val.map(|v| !v.is_string()).unwrap_or(false);

        if is_granular {
            // Object form of approval_policy (granular sub-table) → dropped.
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Drop,
                id: Some("settings.codex.granular_approval".to_string()),
                message: "approval_policy (granular table form) dropped: no Claude equivalent for per-category approval".to_string(),
            });
        } else {
            // Flat string axes: parse each with fallback to Codex default.
            let approval_str = approval_val.and_then(|v| v.as_str());
            let sandbox_str = settings.get("sandbox_mode").and_then(|v| v.as_str());

            // Only proceed when at least one axis is explicitly present.
            if approval_str.is_some() || sandbox_str.is_some() {
                let approval = approval_str
                    .and_then(|s| {
                        let parsed = ApprovalPolicy::from_config(s);
                        if parsed.is_none() {
                            node.diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: Some("settings.codex.approval_policy".to_string()),
                                message: format!(
                                    "unknown approval_policy '{}': defaulting to on-request",
                                    s
                                ),
                            });
                        }
                        parsed
                    })
                    .unwrap_or_default();

                let sandbox = sandbox_str
                    .and_then(|s| {
                        let parsed = SandboxMode::from_config(s);
                        if parsed.is_none() {
                            node.diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: Some("settings.codex.sandbox_mode".to_string()),
                                message: format!(
                                    "unknown sandbox_mode '{}': defaulting to workspace-write",
                                    s
                                ),
                            });
                        }
                        parsed
                    })
                    .unwrap_or_default();

                let mode = DefaultMode::from_codex(approval, sandbox);

                // Store as internal IR field consumed by lower_x2c; `__`-prefix
                // keeps it out of the public report.
                node.fields.insert(
                    "__permissions.defaultMode".to_string(),
                    IRField {
                        id: "__permissions.defaultMode".to_string(),
                        value: Value::String(mode.as_str().to_string()),
                        loss: Loss::Lossless,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );

                // Surface the source axes as lossy IR fields so the report shows
                // them in the degraded/lossy bucket rather than dropping silently.
                if let Some(s) = approval_str {
                    self.add_field(
                        node,
                        "settings.codex.approval_policy",
                        Value::String(s.to_string()),
                        ConvDir::X2c,
                    );
                }
                if let Some(s) = sandbox_str {
                    self.add_field(
                        node,
                        "settings.codex.sandbox_mode",
                        Value::String(s.to_string()),
                        ConvDir::X2c,
                    );
                }

                // Joint-collapse summary for the operator.
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("settings.codex.approval_policy".to_string()),
                    message: format!(
                        "approval_policy={} + sandbox_mode={} jointly collapsed to \
                         permissions.defaultMode={} (lossy: 2 Codex axes → 1 Claude axis)",
                        approval.as_str(),
                        sandbox.as_str(),
                        mode.as_str()
                    ),
                });
            }
        }

        // Codex-specific dropped fields, keyed by their canonical mappings entry id.
        // Combined entries (e.g. approvals_reviewer + auto_review.policy) emit a
        // single diagnostic even when both config keys are present.
        // NOTE: approval_policy is NOT listed here; it is handled above via the
        // two-axis reverse mapping (flat string) or the granular guard (table form).
        const CODEX_DROPPED_KEYS: &[(&str, &str)] = &[
            ("profiles", "settings.codex.profiles"),
            ("agents", "settings.codex.agents_config"),
            ("web_search", "settings.codex.web_search"),
            (
                "project_doc_max_bytes",
                "settings.codex.project_doc_max_bytes",
            ),
            ("model_verbosity", "settings.codex.model_verbosity"),
            ("model_reasoning_summary", "settings.codex.model_verbosity"),
            (
                "plan_mode_reasoning_effort",
                "settings.codex.plan_mode_reasoning_effort",
            ),
            ("approvals_reviewer", "settings.codex.auto_review"),
            ("auto_review", "settings.codex.auto_review"),
            ("projects", "settings.codex.projects_trust"),
        ];
        let mut dropped_seen = std::collections::HashSet::new();
        for (codex_key, entry_id) in CODEX_DROPPED_KEYS {
            if settings.get(*codex_key).is_some() && dropped_seen.insert(*entry_id) {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some((*entry_id).to_string()),
                    message: format!("{} dropped: no Claude equivalent", codex_key),
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
    pub(super) fn lift_flat_key(
        &self,
        settings: &serde_json::Map<String, Value>,
        node: &mut IRNode,
        dir: ConvDir,
        settings_key: &str,
        entry_id: &str,
    ) {
        if let Some(v) = settings.get(settings_key) {
            if let Some(entry) = self.map.entries.iter().find(|e| e.id == entry_id) {
                crate::handlers::lift_mapped_field(entry, settings_key, v, dir, node);
            }
        }
    }

    /// Add a field directly with default IR semantics.
    pub(super) fn add_field(&self, node: &mut IRNode, id: &str, value: Value, dir: ConvDir) {
        if let Some(entry) = self.map.entries.iter().find(|e| e.id == id) {
            crate::handlers::lift_mapped_field(entry, id, &value, dir, node);
        } else {
            // Unknown id (internal placeholder like __permissions.allow): store raw.
            // build_report skips `__`-prefixed ids so these never reach the report.
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

    pub(super) fn add_field_if_present(
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
}
