use serde_json::Value;

use crate::core::ir::{
    new_node, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::transforms::ConvDir;

/// Lifts a parsed memory file value into an IRNode.
pub(crate) fn lift(parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
    let source_path = parsed["path"].as_str().unwrap_or("").to_string();
    let filename = parsed["filename"].as_str().unwrap_or("CLAUDE.md");
    let body = parsed["body"].as_str().unwrap_or("").to_string();

    let source_tool = match dir {
        ConvDir::C2x => Tool::Claude,
        ConvDir::X2c => Tool::Codex,
    };

    let mut node = new_node(Kind::Memory, source_tool, &source_path);

    // Store the filename field in the IR
    node.fields.insert(
        "memory.filename".to_string(),
        IRField {
            id: "memory.filename".to_string(),
            value: Value::String(filename.to_string()),
            loss: Loss::Lossless,
            transforms_applied: vec!["rename".to_string()],
            degrade: None,
            warning: None,
            dropped: None,
        },
    );

    // Path field
    node.fields.insert(
        "memory.project-path".to_string(),
        IRField {
            id: "memory.project-path".to_string(),
            value: Value::String(source_path.clone()),
            loss: Loss::Lossless,
            transforms_applied: vec!["path:remap".to_string()],
            degrade: None,
            warning: None,
            dropped: None,
        },
    );

    // CLAUDE.local.md → dropped (no Codex equivalent)
    if filename == "CLAUDE.local.md" {
        node.fields.insert(
            "memory.local-file".to_string(),
            IRField {
                id: "memory.local-file".to_string(),
                value: Value::String(source_path.clone()),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: Some(
                    "CLAUDE.local.md has no Codex equivalent (AGENTS.override.md requires repo placement). \
                     Consider moving content to ~/.codex/AGENTS.md or discarding."
                        .to_string(),
                ),
                dropped: Some(DroppedInfo {
                    reason: "CLAUDE.local.md: no non-committed personal file concept in Codex"
                        .to_string(),
                }),
            },
        );
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some("memory.local-file".to_string()),
            message:
                "CLAUDE.local.md dropped: Codex has no equivalent for uncommitted personal files"
                    .to_string(),
        });
    }

    // AGENTS.override.md → dropped (x2c: Claude has no same-directory override file;
    // its content must not be silently folded into CLAUDE.md).
    if filename == "AGENTS.override.md" {
        node.fields.insert(
            "memory.override-file".to_string(),
            IRField {
                id: "memory.override-file".to_string(),
                value: Value::String(source_path.clone()),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: Some(
                    "AGENTS.override.md has no Claude equivalent; its content is not merged \
                     into CLAUDE.md. Move the content into CLAUDE.md manually if needed."
                        .to_string(),
                ),
                dropped: Some(DroppedInfo {
                    reason: "AGENTS.override.md: no same-directory override-file concept in Claude"
                        .to_string(),
                }),
            },
        );
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some("memory.override-file".to_string()),
            message: "AGENTS.override.md dropped: Claude has no override-file equivalent"
                .to_string(),
        });
    }

    // Managed policy (e.g. /etc/claude-code/CLAUDE.md) → dropped
    if source_path.contains("/etc/claude-code/")
        || source_path.contains("/Library/Application Support/ClaudeCode/")
    {
        node.fields.insert(
            "memory.managed-policy".to_string(),
            IRField {
                id: "memory.managed-policy".to_string(),
                value: Value::String(source_path.clone()),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: Some(
                    "Managed policy CLAUDE.md has no Codex equivalent (dropped)".to_string(),
                ),
                dropped: Some(DroppedInfo {
                    reason: "managed policy: no Codex org-level equivalent".to_string(),
                }),
            },
        );
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some("memory.managed-policy".to_string()),
            message: "Managed policy CLAUDE.md dropped: no Codex equivalent".to_string(),
        });
    }

    // c2x: detect @import syntax (@ inside code fences is excluded)
    if dir == ConvDir::C2x {
        let has_imports = {
            let mut in_fence = false;
            let mut found = false;
            for line in body.lines() {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                if !in_fence && line.trim_start().starts_with('@') {
                    found = true;
                    break;
                }
            }
            found
        };
        if has_imports {
            node.fields.insert(
                "memory.import-syntax".to_string(),
                IRField {
                    id: "memory.import-syntax".to_string(),
                    value: Value::String(body.clone()),
                    loss: Loss::Lossy,
                    transforms_applied: vec!["inline_imports".to_string()],
                    degrade: None,
                    warning: Some(
                        "@import syntax detected: will be inlined (Codex has no @import equivalent)"
                            .to_string(),
                    ),
                    dropped: None,
                },
            );
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("memory.import-syntax".to_string()),
                message: "@import syntax detected; will be inlined on lower (lossy)".to_string(),
            });
        }
    }

    // Store body in IR for reference during lower
    node.fields.insert(
        "__body".to_string(),
        IRField {
            id: "__body".to_string(),
            value: Value::String(body),
            loss: Loss::Lossless,
            transforms_applied: vec![],
            degrade: None,
            warning: None,
            dropped: None,
        },
    );

    Ok(node)
}
