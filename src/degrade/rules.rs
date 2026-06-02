use toml_edit::{DocumentMut, Item, Table, Value as TomlValue};

use crate::core::ir::{DiagLevel, Diagnostic, SideArtifact};
use crate::handlers::Scope;

/// Classifies each tool pattern and produces the corresponding SideArtifact and Diagnostic.
///
/// Degradation targets by pattern:
/// - `Bash(<cmd>)` → `.codex/rules/<skill>.rules` (project) or `~/.codex/rules/default.rules` (user)
/// - `Write(<glob>)` / `Edit(<glob>)` → `[permissions.<name>].filesystem.<glob> = "write"`
/// - `Read(<glob>)` → `[permissions.<name>].filesystem.<glob> = "read"`
/// - `WebFetch` / `WebSearch` → `[permissions.<name>].network` or `features.web_search`
/// - `mcp__<server>__<tool>` → `[mcp_servers.<server>].enabled_tools` / `disabled_tools`
/// - Built-ins (e.g. `AskUserQuestion`) → dropped
pub fn degrade_allowed_tools(
    skill_name: &str,
    tools: &[String],
    is_allow: bool,
    scope: Scope,
) -> (Vec<SideArtifact>, Vec<Diagnostic>) {
    let mut artifacts: Vec<SideArtifact> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    let tool_kind_id = if is_allow {
        "allowed-tools"
    } else {
        "disallowed-tools"
    };

    degrade_bash_tools(
        skill_name,
        tools,
        is_allow,
        scope,
        tool_kind_id,
        &mut artifacts,
        &mut diagnostics,
    );
    degrade_filesystem_tools(
        skill_name,
        tools,
        is_allow,
        tool_kind_id,
        &mut artifacts,
        &mut diagnostics,
    );
    degrade_web_fetch(
        skill_name,
        tools,
        is_allow,
        tool_kind_id,
        &mut artifacts,
        &mut diagnostics,
    );
    degrade_web_search(
        tools,
        is_allow,
        tool_kind_id,
        &mut artifacts,
        &mut diagnostics,
    );
    degrade_mcp_tools(tools, is_allow, tool_kind_id, &mut diagnostics);
    collect_builtin_drops(tools, tool_kind_id, &mut diagnostics);

    (artifacts, diagnostics)
}

fn degrade_bash_tools(
    skill_name: &str,
    tools: &[String],
    is_allow: bool,
    scope: Scope,
    tool_kind_id: &str,
    artifacts: &mut Vec<SideArtifact>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let decision = if is_allow { "allow" } else { "forbidden" };

    // A bare "Bash" (no parens) means allow all bash commands; treat it as a
    // catch-all by generating a prefix_rule with an empty pattern.
    let has_bare_bash = tools.iter().any(|t| t == "Bash");

    let bash_tools: Vec<&str> = tools
        .iter()
        .filter_map(|t| {
            if t.starts_with("Bash(") && t.ends_with(')') {
                Some(&t[5..t.len() - 1])
            } else {
                None
            }
        })
        .collect();

    if bash_tools.is_empty() && !has_bare_bash {
        return;
    }

    let mut rules_lines = vec![
        format!("# Generated from skill '{}' allowed-tools", skill_name),
        String::new(),
    ];

    // Bare "Bash" (catch-all) is emitted first as an empty-pattern rule.
    if has_bare_bash {
        rules_lines.push(format!(
            r#"prefix_rule(pattern=[], decision="{}", justification="from skill {}")"#,
            decision, skill_name
        ));
    }

    for cmd in &bash_tools {
        let parts: Vec<String> = cmd
            .split_whitespace()
            .map(|p| format!(r#""{}""#, p))
            .collect();
        let pattern = parts.join(", ");
        rules_lines.push(format!(
            r#"prefix_rule(pattern=[{}], decision="{}", justification="from skill {}")"#,
            pattern, decision, skill_name
        ));
    }

    let (rules_path, scope_label) = match scope {
        Scope::User => {
            let home = std::env::var("HOME").unwrap_or_else(|_| {
                #[allow(deprecated)]
                std::env::home_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "~".to_string())
            });
            (format!("{}/.codex/rules/default.rules", home), "skill→user")
        }
        Scope::Project => (
            format!(".codex/rules/{}.rules", skill_name),
            "skill→project",
        ),
    };
    artifacts.push(SideArtifact {
        path: rules_path.clone(),
        content: rules_lines.join("\n") + "\n",
        note: format!(
            "Bash tool permissions degraded to execpolicy ({})",
            decision
        ),
    });
    diagnostics.push(Diagnostic {
        level: DiagLevel::Warn,
        id: Some(format!("skills.{}", tool_kind_id)),
        message: format!(
            "Bash tools in {} degraded to {} (scope: {}). Generated: {}",
            tool_kind_id, rules_path, scope_label, decision
        ),
    });
}

fn degrade_filesystem_tools(
    skill_name: &str,
    tools: &[String],
    is_allow: bool,
    tool_kind_id: &str,
    artifacts: &mut Vec<SideArtifact>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let write_tools: Vec<&str> = tools
        .iter()
        .filter_map(|t| {
            if (t.starts_with("Write(") || t.starts_with("Edit(")) && t.ends_with(')') {
                let start = t
                    .find('(')
                    .expect("'(' guaranteed by starts_with filter guard")
                    + 1;
                Some(&t[start..t.len() - 1])
            } else {
                None
            }
        })
        .collect();

    let read_tools: Vec<&str> = tools
        .iter()
        .filter_map(|t| {
            if t.starts_with("Read(") && t.ends_with(')') {
                Some(&t[5..t.len() - 1])
            } else {
                None
            }
        })
        .collect();

    // Build config.toml SideArtifact when Write/Edit or Read globs are present.
    if write_tools.is_empty() && read_tools.is_empty() {
        return;
    }

    let perm_value = if is_allow { "write" } else { "deny" };
    let read_perm_value = if is_allow { "read" } else { "deny" };

    let mut doc = DocumentMut::new();
    {
        if let Some(permissions) = doc
            .entry("permissions")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
        {
            if let Some(skill_table) = permissions
                .entry(skill_name)
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
            {
                if let Some(fs_table) = skill_table
                    .entry("filesystem")
                    .or_insert(Item::Table(Table::new()))
                    .as_table_mut()
                {
                    for glob in &write_tools {
                        fs_table[glob] = Item::Value(TomlValue::from(perm_value));
                    }
                    for glob in &read_tools {
                        fs_table[glob] = Item::Value(TomlValue::from(read_perm_value));
                    }
                }
            }
        }
    }

    let toml_string = doc.to_string();
    artifacts.push(SideArtifact {
        path: "config.toml".to_string(),
        content: toml_string,
        note: format!(
            "Write/Edit/Read tool permissions degraded to [permissions.{}].filesystem (scope: skill→project)",
            skill_name
        ),
    });

    for glob in &write_tools {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some(format!("skill.{}", tool_kind_id)),
            message: format!(
                "Write/Edit tool permission for '{}' degraded to [permissions.{}].filesystem.\"{}\" = \"{}\" (scope: skill→project)",
                glob, skill_name, glob, perm_value
            ),
        });
    }
    for glob in &read_tools {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some(format!("skill.{}", tool_kind_id)),
            message: format!(
                "Read tool permission for '{}' degraded to [permissions.{}].filesystem.\"{}\" = \"{}\" (scope: skill→project)",
                glob, skill_name, glob, read_perm_value
            ),
        });
    }
}

fn degrade_web_fetch(
    skill_name: &str,
    tools: &[String],
    is_allow: bool,
    tool_kind_id: &str,
    artifacts: &mut Vec<SideArtifact>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !tools.iter().any(|t| t == "WebFetch") {
        return;
    }
    // allow → grant network; disallow → deny it. Never grant on a deny.
    let net_value = if is_allow { "true" } else { "false" };
    let content = format!("[permissions.{}]\nnetwork = {}\n", skill_name, net_value);
    artifacts.push(SideArtifact {
        path: "config.toml".to_string(),
        content,
        note: format!(
            "WebFetch degraded to [permissions.{}].network = {} (scope: skill→project)",
            skill_name, net_value
        ),
    });
    diagnostics.push(Diagnostic {
        level: DiagLevel::Warn,
        id: Some(format!("skills.{}", tool_kind_id)),
        message: format!(
            "WebFetch in {} degraded to [permissions.{}].network = {} in config.toml",
            tool_kind_id, skill_name, net_value
        ),
    });
}

fn degrade_web_search(
    tools: &[String],
    is_allow: bool,
    tool_kind_id: &str,
    artifacts: &mut Vec<SideArtifact>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !tools.iter().any(|t| t == "WebSearch") {
        return;
    }
    // allow → enable web search; disallow → disable it. Never enable on a deny.
    let ws_value = if is_allow { "true" } else { "false" };
    artifacts.push(SideArtifact {
        path: "config.toml".to_string(),
        content: format!("[features]\nweb_search = {}\n", ws_value),
        note: format!(
            "WebSearch degraded to [features].web_search = {} (scope: skill→project)",
            ws_value
        ),
    });
    diagnostics.push(Diagnostic {
        level: DiagLevel::Warn,
        id: Some(format!("skills.{}", tool_kind_id)),
        message: format!(
            "WebSearch in {} degraded to [features].web_search = {} in config.toml",
            tool_kind_id, ws_value
        ),
    });
}

fn degrade_mcp_tools(
    tools: &[String],
    is_allow: bool,
    tool_kind_id: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for t in tools {
        if !t.starts_with("mcp__") {
            continue;
        }
        let parts: Vec<&str> = t.splitn(3, "__").collect();
        if parts.len() == 3 {
            let server = parts[1];
            let tool = parts[2];
            let list_name = if is_allow {
                "enabled_tools"
            } else {
                "disabled_tools"
            };
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some(format!("skills.{}", tool_kind_id)),
                message: format!(
                    "mcp tool '{}' degraded to [mcp_servers.{}].{} = ['{}'] (manual: add to config.toml)",
                    t, server, list_name, tool
                ),
            });
        } else {
            // Pattern does not match mcp__<server>__<tool>; flag for manual review.
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some(format!("skills.{}", tool_kind_id)),
                message: format!(
                    "mcp tool '{}' does not match mcp__<server>__<tool> pattern; manual review required",
                    t
                ),
            });
        }
    }
}

fn collect_builtin_drops(tools: &[String], tool_kind_id: &str, diagnostics: &mut Vec<Diagnostic>) {
    // Built-ins have no Codex equivalent; collect them separately so each gets an explicit Drop diagnostic.
    let builtin_tools: Vec<&str> = tools
        .iter()
        .filter_map(|t| {
            if !t.starts_with("Bash(")
                && t != "Bash"
                && !t.starts_with("Write(")
                && !t.starts_with("Edit(")
                && !t.starts_with("Read(")
                && !t.starts_with("mcp__")
                && t != "WebFetch"
                && t != "WebSearch"
            {
                Some(t.as_str())
            } else {
                None
            }
        })
        .collect();

    for builtin in builtin_tools {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some(format!("skills.{}", tool_kind_id)),
            message: format!(
                "Built-in tool '{}' has no Codex equivalent and will be dropped",
                builtin
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Scope;

    /// When scope=User, the .rules artifact path must be the user-scope path
    /// (~/.codex/rules/default.rules), and the diagnostic message must say
    /// "scope: skill→user".
    #[test]
    fn test_degrade_allowed_tools_scope_user_rules_path() {
        let tools = vec!["Bash(npm run *)".to_string()];
        let (artifacts, diagnostics) = degrade_allowed_tools("t", &tools, true, Scope::User);

        let rules_art = artifacts
            .iter()
            .find(|a| a.path.ends_with(".rules"))
            .expect("Expected a .rules SideArtifact for Scope::User");

        // Must be a user-scope path (contains home dir and .codex/rules/default.rules)
        assert!(
            rules_art.path.contains(".codex/rules/default.rules"),
            "User-scope .rules path must end with .codex/rules/default.rules, got: {}",
            rules_art.path
        );
        // Must NOT be the project-scope path
        assert!(
            !rules_art.path.starts_with(".codex/"),
            "User-scope .rules path must not be project-relative, got: {}",
            rules_art.path
        );

        // Diagnostic message must reflect user scope
        let diag = diagnostics
            .iter()
            .find(|d| d.message.contains("scope:"))
            .expect("Expected a diagnostic with 'scope:' in message");
        assert!(
            diag.message.contains("skill→user"),
            "Diagnostic must say 'skill→user' for Scope::User, got: {}",
            diag.message
        );
    }

    /// When scope=Project (default), the .rules artifact path must remain the
    /// project-scope path (.codex/rules/<skill>.rules).
    #[test]
    fn test_degrade_allowed_tools_scope_project_rules_path() {
        let tools = vec!["Bash(cargo build)".to_string()];
        let (artifacts, diagnostics) = degrade_allowed_tools("build", &tools, true, Scope::Project);

        let rules_art = artifacts
            .iter()
            .find(|a| a.path.ends_with(".rules"))
            .expect("Expected a .rules SideArtifact for Scope::Project");

        assert_eq!(
            rules_art.path, ".codex/rules/build.rules",
            "Project-scope .rules path must be .codex/rules/build.rules, got: {}",
            rules_art.path
        );

        let diag = diagnostics
            .iter()
            .find(|d| d.message.contains("scope:"))
            .expect("Expected a diagnostic with 'scope:' in message");
        assert!(
            diag.message.contains("skill→project"),
            "Diagnostic must say 'skill→project' for Scope::Project, got: {}",
            diag.message
        );
    }

    /// Write/Edit and Read patterns must produce a config.toml SideArtifact with
    /// [permissions.<skill>].filesystem entries.
    #[test]
    fn test_degrade_allowed_tools_produces_config_toml_artifact() {
        let tools = vec!["Write(**/*.py)".to_string(), "Read(~/.ssh/*)".to_string()];
        let (artifacts, _diagnostics) = degrade_allowed_tools("ed", &tools, true, Scope::Project);

        let config_artifact = artifacts
            .iter()
            .find(|a| a.path == "config.toml")
            .expect("Expected a config.toml SideArtifact");

        assert!(
            config_artifact.content.contains("[permissions.ed]"),
            "Expected [permissions.ed] in config.toml, got:\n{}",
            config_artifact.content
        );
        assert!(
            config_artifact.content.contains("\"write\"")
                || config_artifact.content.contains("= \"write\""),
            "Expected write permission entry in config.toml, got:\n{}",
            config_artifact.content
        );
        assert!(
            config_artifact.content.contains("\"read\"")
                || config_artifact.content.contains("= \"read\""),
            "Expected read permission entry in config.toml, got:\n{}",
            config_artifact.content
        );
    }

    /// disallowed-tools Write/Edit/Read patterns must produce config.toml with "deny" entries.
    #[test]
    fn test_degrade_disallowed_tools_produces_deny_config_toml() {
        let tools = vec!["Write(**/*.py)".to_string(), "Read(~/.ssh/*)".to_string()];
        let (artifacts, _diagnostics) = degrade_allowed_tools("ed", &tools, false, Scope::Project);

        let config_artifact = artifacts
            .iter()
            .find(|a| a.path == "config.toml")
            .expect("Expected a config.toml SideArtifact for disallowed-tools");

        assert!(
            config_artifact.content.contains("[permissions.ed]"),
            "Expected [permissions.ed] in config.toml, got:\n{}",
            config_artifact.content
        );
        assert!(
            config_artifact.content.contains("\"deny\"")
                || config_artifact.content.contains("= \"deny\""),
            "Expected deny permission entry in config.toml, got:\n{}",
            config_artifact.content
        );
    }

    /// WebFetch in allowed-tools must produce a config.toml SideArtifact with
    /// [permissions.<skill>].network = true.
    #[test]
    fn test_degrade_web_fetch_produces_config_toml_artifact() {
        let tools = vec!["WebFetch".to_string()];
        let (artifacts, _diagnostics) =
            degrade_allowed_tools("net-skill", &tools, true, Scope::Project);

        let config_artifact = artifacts
            .iter()
            .find(|a| a.path == "config.toml")
            .expect("Expected a config.toml SideArtifact for WebFetch");

        assert!(
            config_artifact.content.contains("[permissions.net-skill]"),
            "Expected [permissions.net-skill] in config.toml, got:\n{}",
            config_artifact.content
        );
        assert!(
            config_artifact.content.contains("network = true"),
            "Expected 'network = true' in config.toml, got:\n{}",
            config_artifact.content
        );
    }

    /// A bare "Bash" (no parentheses) must produce a .rules SideArtifact (catch-all
    /// bash rule), NOT a Drop diagnostic via the builtin-tool path.
    #[test]
    fn test_degrade_bare_bash_produces_rules_artifact() {
        let tools = vec!["Bash".to_string()];
        let (artifacts, diagnostics) =
            degrade_allowed_tools("my-skill", &tools, true, Scope::Project);

        // Must produce a .rules artifact
        let rules_art = artifacts
            .iter()
            .find(|a| a.path.ends_with(".rules"))
            .expect("bare 'Bash' must produce a .rules SideArtifact");

        assert_eq!(rules_art.path, ".codex/rules/my-skill.rules");

        // The rules content must contain a prefix_rule with an empty pattern
        assert!(
            rules_art.content.contains("prefix_rule"),
            "rules content must contain prefix_rule, got:\n{}",
            rules_art.content
        );

        // Must NOT produce a Drop diagnostic (i.e. must not be treated as unrecognized builtin)
        let has_drop = diagnostics
            .iter()
            .any(|d| d.level == crate::core::ir::DiagLevel::Drop);
        assert!(
            !has_drop,
            "bare 'Bash' must NOT produce a Drop diagnostic; diagnostics: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// WebSearch in allowed-tools must produce a config.toml SideArtifact with
    /// [features].web_search = true.
    #[test]
    fn test_degrade_web_search_produces_config_toml_artifact() {
        let tools = vec!["WebSearch".to_string()];
        let (artifacts, _diagnostics) =
            degrade_allowed_tools("search-skill", &tools, true, Scope::Project);

        let config_artifact = artifacts
            .iter()
            .find(|a| a.path == "config.toml")
            .expect("Expected a config.toml SideArtifact for WebSearch");

        assert!(
            config_artifact.content.contains("[features]"),
            "Expected [features] section in config.toml, got:\n{}",
            config_artifact.content
        );
        assert!(
            config_artifact.content.contains("web_search = true"),
            "Expected 'web_search = true' in config.toml, got:\n{}",
            config_artifact.content
        );
    }
}
