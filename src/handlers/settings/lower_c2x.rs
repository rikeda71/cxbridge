use serde_json::Value;

use crate::core::ir::{DiagLevel, Diagnostic};
use crate::core::model_tiers::{claude_tier, tier_to_codex};
use crate::degrade::rules::degrade_allowed_tools;
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};

use super::SettingsHandler;

impl SettingsHandler {
    /// c2x lower: produce Codex config.toml / rules files from IR.
    pub(super) fn lower_c2x(
        &self,
        ir: &crate::core::ir::IRNode,
        opts: &LowerOpts,
    ) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        let out_root = opts.out.as_deref().unwrap_or(".");

        // Build TOML document
        let mut doc = toml_edit::DocumentMut::new();

        // model: tier mapping (lossy); unknown IDs pass through with a warning
        if let Some(f) = ir.fields.get("settings.model") {
            if let Some(s) = f.value.as_str() {
                if let Some(tier) = claude_tier(s) {
                    let codex_model = tier_to_codex(tier);
                    doc.insert("model", toml_edit::value(codex_model));
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.model".to_string()),
                        message: format!(
                            "model '{}' mapped to '{}' via tier mapping (lossy; different provider)",
                            s, codex_model
                        ),
                    });
                } else {
                    doc.insert("model", toml_edit::value(s));
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.model".to_string()),
                        message: format!(
                            "Unknown model '{}': copied as-is (no tier mapping; manual review required)",
                            s
                        ),
                    });
                }
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

        // language / outputStyle → developer_instructions (Codex has no dedicated
        // field; approximate by appending natural-language instructions). Lossy.
        let mut dev_instructions: Vec<String> = Vec::new();
        if let Some(s) = ir
            .fields
            .get("settings.language")
            .and_then(|f| f.value.as_str())
        {
            dev_instructions.push(format!("Always respond in {}.", s));
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("settings.language".to_string()),
                message:
                    "language approximated as a developer_instructions sentence (no dedicated Codex field)"
                        .to_string(),
            });
        }
        if let Some(s) = ir
            .fields
            .get("settings.outputStyle")
            .and_then(|f| f.value.as_str())
        {
            dev_instructions.push(format!("Output style: {}.", s));
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("settings.outputStyle".to_string()),
                message:
                    "outputStyle approximated as a developer_instructions sentence (no dedicated Codex field)"
                        .to_string(),
            });
        }
        if !dev_instructions.is_empty() {
            doc.insert(
                "developer_instructions",
                toml_edit::value(dev_instructions.join(" ")),
            );
        }

        // defaultShell → shell_environment_policy.experimental_use_profile.
        // Semantics differ (Claude selects the shell; Codex toggles profile sourcing).
        if let Some(s) = ir
            .fields
            .get("settings.defaultShell")
            .and_then(|f| f.value.as_str())
        {
            match s {
                "bash" => {
                    let policy_item = doc
                        .entry("shell_environment_policy")
                        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                    if let Some(policy_tbl) = policy_item.as_table_mut() {
                        policy_tbl.insert("experimental_use_profile", toml_edit::value(false));
                    }
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.defaultShell".to_string()),
                        message: "defaultShell=bash mapped to shell_environment_policy.experimental_use_profile=false (semantics differ)".to_string(),
                    });
                }
                other => {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.defaultShell".to_string()),
                        message: format!(
                            "defaultShell={} has no Codex shell-selection equivalent (Codex only toggles profile sourcing); not converted",
                            other
                        ),
                    });
                }
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
            let split = split_permissions_by_type(&tools);
            let (bash_tools, fs_allow_read, fs_allow_write, web_domains) =
                (split.bash, split.read, split.write, split.web);

            if !split.dropped.is_empty() {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some("settings.permissions.allow.dropped".to_string()),
                    message: format!(
                        "permissions.allow entries with no Codex equivalent dropped: {}",
                        split.dropped.join(", ")
                    ),
                });
            }

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
            let split = split_permissions_by_type(&tools);
            let (bash_tools, fs_deny_read, fs_deny_write, web_deny_domains) =
                (split.bash, split.read, split.write, split.web);

            if !split.dropped.is_empty() {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some("settings.permissions.deny.dropped".to_string()),
                    message: format!(
                        "permissions.deny entries with no Codex equivalent dropped: {}",
                        split.dropped.join(", ")
                    ),
                });
            }

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
            let bash_tools = split_permissions_by_type(&tools).bash;
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

        // defaultMode: map through the typed approval module (both axes always emitted).
        if let Some(f) = ir.fields.get("__permissions.defaultMode") {
            if let Some(mode_str) = f.value.as_str() {
                if let Some(mode) = super::approval::DefaultMode::from_config(mode_str) {
                    let (approval, sandbox) = mode.to_codex();
                    doc.insert("approval_policy", toml_edit::value(approval.as_str()));
                    doc.insert("sandbox_mode", toml_edit::value(sandbox.as_str()));

                    let diag_id = format!("settings.permissions.defaultMode.{}", mode_str);
                    let diag_msg = match mode {
                        super::approval::DefaultMode::Plan => {
                            "defaultMode=plan approximated by sandbox_mode=read-only + \
                             approval_policy=on-request (closest Codex equivalent; no true plan mode)"
                                .to_string()
                        }
                        super::approval::DefaultMode::DontAsk => {
                            "defaultMode=dontAsk approximated by approval_policy=never + \
                             sandbox_mode=workspace-write; retains the workspace sandbox boundary \
                             while suppressing all approval prompts"
                                .to_string()
                        }
                        super::approval::DefaultMode::BypassPermissions => {
                            "defaultMode=bypassPermissions mapped to approval_policy=never + \
                             sandbox_mode=danger-full-access; removes sandbox AND approval gate \
                             (security risk — review before deploying)"
                                .to_string()
                        }
                        _ => {
                            format!(
                                "defaultMode={} approximated across two Codex axes \
                                 (approval_policy={} + sandbox_mode={}); lossy 1-axis→2-axis mapping",
                                mode_str,
                                approval.as_str(),
                                sandbox.as_str()
                            )
                        }
                    };
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some(diag_id),
                        message: diag_msg,
                    });
                } else {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("settings.permissions.defaultMode".to_string()),
                        message: format!("unknown defaultMode '{}': no Codex mapping", mode_str),
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
}

/// A Claude permissions list split into typed buckets.
#[derive(Default)]
pub(super) struct SplitTools {
    pub(super) bash: Vec<String>,
    pub(super) read: Vec<String>,
    pub(super) write: Vec<String>,
    pub(super) web: Vec<String>,
    /// Tools with no Codex equivalent (bare WebFetch/WebSearch, AskUserQuestion, …).
    pub(super) dropped: Vec<String>,
}

pub(super) fn split_permissions_by_type(tools: &[String]) -> SplitTools {
    let mut out = SplitTools::default();
    let (bash, read, write, web, dropped) = (
        &mut out.bash,
        &mut out.read,
        &mut out.write,
        &mut out.web,
        &mut out.dropped,
    );

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
            // Coarse allow with no specific domain: Codex network rules are
            // domain-scoped, so a blanket allow has no equivalent. Record it so
            // the caller can surface a Drop diagnostic (no silent loss).
            dropped.push(t.to_string());
        } else {
            // Other tools (AskUserQuestion, etc.) → dropped (no bucket)
            dropped.push(t.to_string());
        }
    }

    out
}

/// Extract the argument from a tool pattern like `Bash(git add)` → `git add`.
pub(super) fn extract_tool_arg(tool: &str) -> String {
    if let Some(start) = tool.find('(') {
        let rest = &tool[start + 1..];
        rest.trim_end_matches(')').to_string()
    } else {
        tool.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_arg_basic() {
        assert_eq!(extract_tool_arg("Bash(git add)"), "git add");
        assert_eq!(extract_tool_arg("Read(/tmp/foo)"), "/tmp/foo");
        assert_eq!(extract_tool_arg("Bash"), "Bash");
    }

    #[test]
    fn test_split_permissions_by_type() {
        let tools = vec![
            "Bash(cargo build)".to_string(),
            "Read(/tmp)".to_string(),
            "Write(/out)".to_string(),
            "WebFetch(domain:example.com)".to_string(),
            "WebFetch".to_string(),
            "AskUserQuestion".to_string(),
        ];
        let split = split_permissions_by_type(&tools);
        assert_eq!(split.bash, vec!["Bash(cargo build)"]);
        assert_eq!(split.read, vec!["/tmp"]);
        assert_eq!(split.write, vec!["/out"]);
        assert_eq!(split.web, vec!["example.com"]);
        assert_eq!(split.dropped, vec!["WebFetch", "AskUserQuestion"]);
    }
}
