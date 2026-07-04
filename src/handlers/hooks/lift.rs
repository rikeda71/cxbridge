use serde_json::Value;

use crate::core::ir::{
    new_node, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};

use super::{HooksHandler, CLAUDE_ONLY_EVENTS, COMMON_EVENTS};

/// Resolves the event-keyed hooks object from whatever parse() produced.
///
/// Resolution order: top-level `"hooks"` key → frontmatter `"hooks"` key →
/// entire frontmatter → entire parsed object → `None`.
fn resolve_hooks_obj(parsed: &Value) -> Option<serde_json::Map<String, Value>> {
    if let Some(h) = parsed.get("hooks").and_then(|v| v.as_object()) {
        return Some(h.clone());
    }
    if let Some(fm) = parsed.get("frontmatter").and_then(|v| v.as_object()) {
        if let Some(h) = fm.get("hooks").and_then(|v| v.as_object()) {
            return Some(h.clone());
        }
        return Some(fm.clone());
    }
    parsed.as_object().cloned()
}

/// Resolves a Codex hooks.json top-level `description` metadata string, mirroring
/// `resolve_hooks_obj`'s lookup order (direct key, then nested under `frontmatter`).
/// This field is a sibling of `hooks`, not part of it, so `resolve_hooks_obj`
/// never surfaces it on its own.
fn resolve_description(parsed: &Value) -> Option<String> {
    if let Some(d) = parsed.get("description").and_then(|v| v.as_str()) {
        return Some(d.to_string());
    }
    parsed
        .get("frontmatter")
        .and_then(|fm| fm.get("description"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

impl HooksHandler {
    /// Lift Claude JSON hooks → IR (c2x).
    pub(super) fn lift_c2x(&self, parsed: &Value) -> anyhow::Result<IRNode> {
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Hooks, Tool::Claude, &source_path);

        let hooks_obj = match resolve_hooks_obj(parsed) {
            Some(obj) => obj,
            None => return Ok(node),
        };

        for (event_name, event_entries) in &hooks_obj {
            // Skip meta fields such as "path"
            if event_name == "path" || event_name == "body" || event_name == "frontmatter" {
                continue;
            }

            if CLAUDE_ONLY_EVENTS.contains(&event_name.as_str()) {
                // dropped — IRField is the single canonical source; no diagnostic needed here.
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: event_entries.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!(
                            "Event '{}' is Claude-specific and has no Codex equivalent (dropped)",
                            event_name
                        )),
                        dropped: Some(DroppedInfo {
                            reason: format!("Claude-only event: {}", event_name),
                        }),
                    },
                );
                continue;
            }

            if COMMON_EVENTS.contains(&event_name.as_str()) {
                // Process each hook entry in the event array
                let (processed, any_survived) =
                    process_hook_entries_c2x(event_name, event_entries, &mut node);
                // If every hook item was dropped (e.g. all are type:http), the event
                // carries no surviving semantic content and must be classified as Dropped.
                let (loss, dropped_info, warning_msg) = if any_survived {
                    (Loss::Lossless, None, None)
                } else {
                    (
                        Loss::Dropped,
                        Some(DroppedInfo {
                            reason: format!(
                                "All hook types for event '{}' were dropped \
                                 (http/mcp_tool/prompt/agent have no Codex equivalent)",
                                event_name
                            ),
                        }),
                        Some(format!(
                            "Event '{}': all hook items were dropped; no Codex equivalent",
                            event_name
                        )),
                    )
                };
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: processed,
                        loss,
                        transforms_applied: vec!["format:json_to_toml".to_string()],
                        degrade: None,
                        warning: warning_msg,
                        dropped: dropped_info,
                    },
                );
            } else {
                // unknown event → dropped — IRField is the single canonical source.
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: event_entries.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!("Unknown event '{}' dropped", event_name)),
                        dropped: Some(DroppedInfo {
                            reason: format!("Unknown event: {}", event_name),
                        }),
                    },
                );
            }
        }

        Ok(node)
    }

    /// Lift Codex TOML hooks → IR (x2c).
    pub(super) fn lift_x2c(&self, parsed: &Value) -> anyhow::Result<IRNode> {
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Hooks, Tool::Codex, &source_path);

        // Top-level `description` metadata (openai/codex#30229) is a sibling of
        // `hooks`, not part of it; resolve_hooks_obj never returns it, so it must
        // be checked separately before iterating events.
        if let Some(description) = resolve_description(parsed) {
            node.fields.insert(
                "hooks.toplevel.description".to_string(),
                IRField {
                    id: "hooks.toplevel.description".to_string(),
                    value: Value::String(description),
                    loss: Loss::Dropped,
                    transforms_applied: vec![],
                    degrade: None,
                    warning: None,
                    dropped: Some(DroppedInfo {
                        reason: "Claude hooks configuration has no top-level description field"
                            .to_string(),
                    }),
                },
            );
        }

        let hooks_obj = match resolve_hooks_obj(parsed) {
            Some(obj) => obj,
            None => return Ok(node),
        };

        for (event_name, event_entries) in &hooks_obj {
            if event_name == "path" || event_name == "body" || event_name == "frontmatter" {
                continue;
            }

            if COMMON_EVENTS.contains(&event_name.as_str()) {
                let processed = process_hook_entries_x2c(event_name, event_entries, &mut node);
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: processed,
                        loss: Loss::Lossless,
                        transforms_applied: vec!["format:toml_to_json".to_string()],
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );
            } else {
                // Codex-only or unknown event dropped
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: event_entries.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!("Event '{}' not in common set, dropped", event_name)),
                        dropped: Some(DroppedInfo {
                            reason: format!("Non-common event: {}", event_name),
                        }),
                    },
                );
            }
        }

        Ok(node)
    }
}

/// Processes each hook entry in the c2x direction (matcher normalization, filtering dropped fields).
/// Side effect: appends warnings to node.diagnostics.
///
/// Returns `(processed_value, any_survived)` where `any_survived` is `true` if
/// at least one hook item survived filtering across all matcher groups.
pub(super) fn process_hook_entries_c2x(
    event_name: &str,
    entries: &Value,
    node: &mut IRNode,
) -> (Value, bool) {
    let arr = match entries.as_array() {
        Some(a) => a,
        None => return (entries.clone(), true),
    };

    let mut any_survived = false;
    let processed: Vec<Value> = arr
        .iter()
        .filter_map(|entry| {
            let obj = entry.as_object()?;
            let mut new_obj = serde_json::Map::new();

            // Normalize the matcher
            if let Some(matcher) = obj.get("matcher").and_then(|v| v.as_str()) {
                let (normalized, kind) = normalize_matcher_c2x(matcher);
                new_obj.insert("matcher".to_string(), Value::String(normalized.clone()));
                match kind {
                    MatcherKind::Wildcard => {
                        node.diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("hooks.matcher.wildcard".to_string()),
                            message: format!(
                                "Event '{}' matcher '{}' is a Claude wildcard; normalized to '' for Codex (lossy)",
                                event_name, matcher
                            ),
                        });
                    }
                    MatcherKind::Exact => {
                        node.diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("hooks.matcher.exact".to_string()),
                            message: format!(
                                "Event '{}' matcher '{}' normalized to '{}' (lossy: Codex uses regex evaluation)",
                                event_name, matcher, normalized
                            ),
                        });
                    }
                    // Regex matchers are passed through unchanged (lossless, warn:false per
                    // mappings/hooks.yaml `hooks.matcher.regex`). No diagnostic is emitted.
                    MatcherKind::Regex => {}
                }
            }

            // hooks array
            if let Some(hooks_arr) = obj.get("hooks").and_then(|v| v.as_array()) {
                let processed_hooks: Vec<Value> = hooks_arr
                    .iter()
                    .filter_map(|h| process_single_hook_c2x(h, event_name, node))
                    .collect();
                if processed_hooks.is_empty() {
                    // All hooks in this matcher group were dropped; omit the whole entry
                    // so no dead `{ "matcher": ..., "hooks": [] }` appears in output.
                    return None;
                }
                any_survived = true;
                new_obj.insert("hooks".to_string(), Value::Array(processed_hooks));
            }

            Some(Value::Object(new_obj))
        })
        .collect();

    (Value::Array(processed), any_survived)
}

/// Processes each hook entry in the x2c direction (commandWindows → shell conversion, etc.).
pub(super) fn process_hook_entries_x2c(
    _event_name: &str,
    entries: &Value,
    node: &mut IRNode,
) -> Value {
    let arr = match entries.as_array() {
        Some(a) => a,
        None => return entries.clone(),
    };

    let processed: Vec<Value> = arr
        .iter()
        .filter_map(|entry| {
            let obj = entry.as_object()?;
            let mut new_obj = serde_json::Map::new();

            // matcher: Codex always uses regex, so pass it through as-is
            if let Some(matcher) = obj.get("matcher") {
                new_obj.insert("matcher".to_string(), matcher.clone());
            }

            // hooks array
            if let Some(hooks_arr) = obj.get("hooks").and_then(|v| v.as_array()) {
                let processed_hooks: Vec<Value> = hooks_arr
                    .iter()
                    .filter_map(|h| process_single_hook_x2c(h, node))
                    .collect();
                new_obj.insert("hooks".to_string(), Value::Array(processed_hooks));
            }

            Some(Value::Object(new_obj))
        })
        .collect();

    Value::Array(processed)
}

/// Converts a single hook entry in the c2x direction.
/// Drops the fields args/shell/if/once/asyncRewake; synthesizes args into command
/// when present; returns None (excluded) for http/mcp_tool types.
fn process_single_hook_c2x(hook: &Value, event_name: &str, node: &mut IRNode) -> Option<Value> {
    use super::DROPPED_C2X_HOOK_FIELDS;

    let obj = hook.as_object()?;
    let hook_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("command");

    // http/mcp_tool types are dropped
    if hook_type == "http" || hook_type == "mcp_tool" {
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some(format!("hooks.type.{}", hook_type)),
            message: format!(
                "Event '{}': hook type '{}' has no Codex equivalent (dropped)",
                event_name, hook_type
            ),
        });
        return None;
    }

    // Codex parses prompt/agent types but does not execute them, so they are dropped
    if hook_type == "prompt" || hook_type == "agent" {
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some(format!("hooks.type.{}", hook_type)),
            message: format!(
                "Event '{}': hook type '{}' has no Codex equivalent (dropped; loss:dropped per mappings)",
                event_name, hook_type
            ),
        });
        return None;
    }

    let mut new_obj = serde_json::Map::new();

    // Pass the "type" field through unchanged
    new_obj.insert("type".to_string(), Value::String(hook_type.to_string()));

    // Process the command field (synthesize args into it when present)
    let command = obj.get("command").and_then(|v| v.as_str());
    let args = obj.get("args").and_then(|v| v.as_array());

    if let Some(args_arr) = args {
        // args is dropped in c2x, but synthesized into command
        let synthesized = synthesize_command(command, args_arr);
        new_obj.insert("command".to_string(), Value::String(synthesized));
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some("hooks.command.args".to_string()),
            message: format!(
                "Event '{}': 'args' field dropped (synthesized into 'command' with shell escaping)",
                event_name
            ),
        });
    } else if let Some(cmd) = command {
        new_obj.insert("command".to_string(), Value::String(cmd.to_string()));
    }

    // timeout, statusMessage, async → pass through losslessly
    for field_name in &["timeout", "statusMessage", "async"] {
        if let Some(v) = obj.get(*field_name) {
            new_obj.insert(field_name.to_string(), v.clone());
        }
    }

    // Warn about dropped fields
    for dropped_field in DROPPED_C2X_HOOK_FIELDS {
        if obj.contains_key(*dropped_field) && *dropped_field != "args" {
            // args was already handled above
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Drop,
                id: Some(format!("hooks.command.{}", dropped_field)),
                message: format!(
                    "Event '{}': hook field '{}' has no Codex equivalent (dropped)",
                    event_name, dropped_field
                ),
            });
        }
    }

    Some(Value::Object(new_obj))
}

/// Converts a single hook entry in the x2c direction (warns about commandWindows → Claude shell:powershell).
fn process_single_hook_x2c(hook: &Value, node: &mut IRNode) -> Option<Value> {
    let obj = hook.as_object()?;
    let mut new_obj = serde_json::Map::new();

    for (k, v) in obj {
        match k.as_str() {
            "commandWindows" | "command_windows" => {
                // x2c: commandWindows has no direct Claude equivalent → warn
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("hooks.command.commandWindows".to_string()),
                    message:
                        "commandWindows has no direct Claude equivalent; consider shell:powershell"
                            .to_string(),
                });
                // Excluded from output (lossy)
            }
            _ => {
                new_obj.insert(k.clone(), v.clone());
            }
        }
    }

    Some(Value::Object(new_obj))
}

/// Classifier for the matcher normalization result.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum MatcherKind {
    /// "*" or "" — maps to `hooks.matcher.wildcard` (lossy).
    Wildcard,
    /// Alphanumeric/`_`/`|` only — maps to `hooks.matcher.exact` (lossy).
    Exact,
    /// Contains regex metacharacters — passed through as-is (`hooks.matcher.regex`, lossless).
    Regex,
}

/// Normalises a Claude matcher string for Codex (c2x direction).
///
/// Returns `(normalized, MatcherKind)`.
pub(super) fn normalize_matcher_c2x(matcher: &str) -> (String, MatcherKind) {
    if matcher.is_empty() || matcher == "*" {
        return ("".to_string(), MatcherKind::Wildcard);
    }

    let is_exact = matcher
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '|');

    if is_exact {
        if matcher.contains('|') {
            (format!("^({})$", matcher), MatcherKind::Exact)
        } else {
            (format!("^{}$", matcher), MatcherKind::Exact)
        }
    } else {
        (matcher.to_string(), MatcherKind::Regex)
    }
}

/// Synthesizes command + args array into shell form (escaping with shlex::quote).
fn synthesize_command(command: Option<&str>, args: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(cmd) = command {
        parts.push(cmd.to_string());
    }

    for arg in args {
        if let Some(s) = arg.as_str() {
            // Escape shell metacharacters with shlex::try_quote
            let quoted =
                shlex::try_quote(s).unwrap_or_else(|_| std::borrow::Cow::Owned(format!("'{}'", s)));
            parts.push(quoted.to_string());
        }
    }

    parts.join(" ")
}
