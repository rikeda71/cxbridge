use crate::core::ir::{DiagLevel, Diagnostic, SideArtifact};

/// Classifies each tool pattern and produces the corresponding SideArtifact and Diagnostic.
///
/// Degradation targets by pattern:
/// - `Bash(<cmd>)` → `.codex/rules/<skill>.rules` (execpolicy Starlark)
/// - `Write(<glob>)` / `Edit(<glob>)` → `[permissions.<name>].filesystem.<glob> = "write"`
/// - `Read(<glob>)` → `[permissions.<name>].filesystem.<glob> = "read"`
/// - `WebFetch` / `WebSearch` → `[permissions.<name>].network` or `features.web_search`
/// - `mcp__<server>__<tool>` → `[mcp_servers.<server>].enabled_tools` / `disabled_tools`
/// - Built-ins (e.g. `AskUserQuestion`) → dropped
pub fn degrade_allowed_tools(
    skill_name: &str,
    tools: &[String],
    is_allow: bool,
) -> (Vec<SideArtifact>, Vec<Diagnostic>) {
    let mut artifacts: Vec<SideArtifact> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    let decision = if is_allow { "allow" } else { "forbidden" };

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

    if !bash_tools.is_empty() {
        let mut rules_lines = vec![
            format!("# Generated from skill '{}' allowed-tools", skill_name),
            String::new(),
        ];

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

        let rules_path = format!(".codex/rules/{}.rules", skill_name);
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
            id: Some(format!(
                "skills.{}",
                if is_allow {
                    "allowed-tools"
                } else {
                    "disallowed-tools"
                }
            )),
            message: format!(
                "Bash tools in {} degraded to {} (scope: skill→project). Generated: {}",
                if is_allow {
                    "allowed-tools"
                } else {
                    "disallowed-tools"
                },
                rules_path,
                decision
            ),
        });
    }

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

    if !write_tools.is_empty() {
        for glob in &write_tools {
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some(format!("skill.{}", if is_allow { "allowed-tools" } else { "disallowed-tools" })),
                message: format!(
                    "Write/Edit tool permission for '{}' degraded to [permissions.{}].filesystem (scope: skill→project)",
                    glob, skill_name
                ),
            });
        }
    }

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

    if !read_tools.is_empty() {
        for glob in &read_tools {
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some(format!("skill.{}", if is_allow { "allowed-tools" } else { "disallowed-tools" })),
                message: format!(
                    "Read tool permission for '{}' degraded to [permissions.{}].filesystem (scope: skill→project)",
                    glob, skill_name
                ),
            });
        }
    }

    let has_web_fetch = tools.iter().any(|t| t == "WebFetch");
    if has_web_fetch {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some(format!(
                "skills.{}",
                if is_allow {
                    "allowed-tools"
                } else {
                    "disallowed-tools"
                }
            )),
            message: format!(
                "WebFetch in {} degraded to [permissions.{}].network (manual: set network=true in config.toml)",
                if is_allow { "allowed-tools" } else { "disallowed-tools" },
                skill_name
            ),
        });
    }

    let has_web_search = tools.iter().any(|t| t == "WebSearch");
    if has_web_search {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some(format!(
                "skills.{}",
                if is_allow {
                    "allowed-tools"
                } else {
                    "disallowed-tools"
                }
            )),
            message: format!(
                "WebSearch in {} degraded to features.web_search (manual: set features.web_search=true in config.toml)",
                if is_allow { "allowed-tools" } else { "disallowed-tools" },
            ),
        });
    }

    for t in tools {
        if t.starts_with("mcp__") {
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
                    id: Some(format!(
                        "skills.{}",
                        if is_allow {
                            "allowed-tools"
                        } else {
                            "disallowed-tools"
                        }
                    )),
                    message: format!(
                        "mcp tool '{}' degraded to [mcp_servers.{}].{} = ['{}'] (manual: add to config.toml)",
                        t, server, list_name, tool
                    ),
                });
            } else {
                // Pattern does not match mcp__<server>__<tool>; flag for manual review.
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some(format!(
                        "skills.{}",
                        if is_allow {
                            "allowed-tools"
                        } else {
                            "disallowed-tools"
                        }
                    )),
                    message: format!(
                        "mcp tool '{}' does not match mcp__<server>__<tool> pattern; manual review required",
                        t
                    ),
                });
            }
        }
    }

    // Built-ins have no Codex equivalent; collect them separately so each gets an explicit Drop diagnostic.
    let builtin_tools: Vec<&str> = tools
        .iter()
        .filter_map(|t| {
            if !t.starts_with("Bash(")
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
            id: Some(format!(
                "skills.{}",
                if is_allow {
                    "allowed-tools"
                } else {
                    "disallowed-tools"
                }
            )),
            message: format!(
                "Built-in tool '{}' has no Codex equivalent and will be dropped",
                builtin
            ),
        });
    }

    (artifacts, diagnostics)
}
