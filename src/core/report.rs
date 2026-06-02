use crate::core::ir::{DiagLevel, IRNode, Loss};
use crate::handlers::EmitPlan;

/// Common representation of a diagnostic entry.
#[derive(Debug, Clone)]
pub struct DiagEntry {
    /// Entry id from mappings (e.g. "skill.allowed-tools")
    pub id: Option<String>,
    pub message: String,
}

/// Aggregated report returned by build_report.
/// dropped and degraded fields must always be enumerated — silent truncation is forbidden.
pub struct Report {
    /// List of lossless field ids
    pub lossless: Vec<String>,
    /// Lossy conversions (successful but with semantic differences)
    pub lossy: Vec<DiagEntry>,
    /// Fields dropped due to having no conversion target
    pub dropped: Vec<DiagEntry>,
    /// Fields relocated to a different scope by the degrade engine
    pub degraded: Vec<DiagEntry>,
    /// Warnings detected by the body scanner
    pub body_warnings: Vec<DiagEntry>,
}

/// Builds a Report from an IR node and an EmitPlan.
///
/// Aggregates the IR diagnostics and each IRField.
/// dropped and degraded fields must always be enumerated — silent truncation is forbidden.
pub fn build_report(ir: &IRNode, plan: &EmitPlan) -> Report {
    let mut lossless = Vec::new();
    let mut lossy = Vec::new();
    let mut dropped = Vec::new();
    let mut degraded = Vec::new();
    let mut body_warnings = Vec::new();

    for (id, field) in &ir.fields {
        // `__`-prefixed ids are internal bookkeeping (e.g. `__permissions.allow`,
        // `__body`) consumed by lower(); they are not user-facing fields.
        if id.starts_with("__") {
            continue;
        }
        match field.loss {
            Loss::Lossless => {
                lossless.push(id.clone());
            }
            Loss::Lossy => {
                if let Some(degrade) = &field.degrade {
                    degraded.push(DiagEntry {
                        id: Some(id.clone()),
                        message: format!(
                            "{} → {} (degrade: {}→{})",
                            id, degrade.target, id, degrade.to
                        ),
                    });
                } else {
                    lossy.push(DiagEntry {
                        id: Some(id.clone()),
                        message: field
                            .warning
                            .clone()
                            .unwrap_or_else(|| format!("{} lossy conversion", id)),
                    });
                }
            }
            Loss::Dropped => {
                dropped.push(DiagEntry {
                    id: Some(id.clone()),
                    message: field
                        .dropped
                        .as_ref()
                        .map(|d| d.reason.clone())
                        .unwrap_or_else(|| format!("{} has no Codex equivalent", id)),
                });
            }
        }
    }

    for diag in &ir.diagnostics {
        match diag.level {
            DiagLevel::Drop => {
                dropped.push(DiagEntry {
                    id: diag.id.clone(),
                    message: diag.message.clone(),
                });
            }
            DiagLevel::Warn => {
                // Route body-scanner findings to body_warnings rather than lossy
                if diag.message.contains("body L") || diag.message.starts_with("body ") {
                    body_warnings.push(DiagEntry {
                        id: diag.id.clone(),
                        message: diag.message.clone(),
                    });
                } else {
                    lossy.push(DiagEntry {
                        id: diag.id.clone(),
                        message: diag.message.clone(),
                    });
                }
            }
            DiagLevel::Info => {}
        }
    }

    for artifact in &ir.side_artifacts {
        degraded.push(DiagEntry {
            id: None,
            message: format!("generated: {} ({})", artifact.path, artifact.note),
        });
    }

    if let Some(body_seg) = &ir.body {
        use crate::scanner::body::Action;
        for finding in &body_seg.findings {
            let entry = DiagEntry {
                id: None,
                message: format!(
                    "body L{}: {} - {}",
                    finding.line, finding.matched, finding.note
                ),
            };
            match finding.action {
                Action::Drop => dropped.push(entry),
                Action::Warn | Action::Rewrite => body_warnings.push(entry),
            }
        }
    }

    for diag in &plan.diagnostics {
        match diag.level {
            DiagLevel::Drop => {
                dropped.push(DiagEntry {
                    id: diag.id.clone(),
                    message: diag.message.clone(),
                });
            }
            DiagLevel::Warn => {
                lossy.push(DiagEntry {
                    id: diag.id.clone(),
                    message: diag.message.clone(),
                });
            }
            DiagLevel::Info => {}
        }
    }

    for child in &ir.children {
        let child_report = build_report(
            child,
            &EmitPlan {
                files: vec![],
                diagnostics: vec![],
            },
        );
        lossless.extend(child_report.lossless);
        lossy.extend(child_report.lossy);
        dropped.extend(child_report.dropped);
        degraded.extend(child_report.degraded);
        body_warnings.extend(child_report.body_warnings);
    }

    // Defense-in-depth: deduplicate dropped by id (keep first occurrence).
    // A field may be recorded both via IRField{loss:Dropped} and via a
    // DiagLevel::Drop diagnostic; collapse them here regardless of source.
    let mut seen_dropped_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    dropped.retain(|e| {
        if let Some(id) = &e.id {
            seen_dropped_ids.insert(id.clone())
        } else {
            true
        }
    });

    // Defense-in-depth: a field classified as Dropped must never also appear
    // in lossy. Remove any lossy entry whose id matches a dropped id.
    lossy.retain(|e| {
        e.id.as_ref()
            .map(|id| !seen_dropped_ids.contains(id.as_str()))
            .unwrap_or(true)
    });

    Report {
        lossless,
        lossy,
        dropped,
        degraded,
        body_warnings,
    }
}

/// Prints a Report to standard output.
///
/// When fmt is Some("json"), outputs machine-readable JSON (for CI use).
/// When fmt is None, outputs human-readable text format.
///
/// Text format:
/// ```text
/// ✔ <source> → <output>
///   ◎ <lossless fields>                    lossless
///   ○ <lossy field> → <dest>               lossy
///   △ <degraded>                           lossy (degrade: ...)
///   ✕ <dropped>                            dropped
///   ⚠ body L<n>: <warning>                 body-warning
/// Summary: N lossless, N lossy(N degraded), N dropped, N body-warning
/// ```
pub fn print_report(report: &Report, fmt: Option<&str>) {
    if fmt == Some("json") {
        print_report_json(report);
    } else {
        print_report_text(report);
    }
}

fn print_report_text(report: &Report) {
    if !report.lossless.is_empty() {
        println!("  \u{25ce} {}  lossless", report.lossless.join(", "));
    }

    for entry in &report.lossy {
        let id = entry.id.as_deref().unwrap_or("?");
        println!("  \u{25cb} {}  lossy  {}", id, entry.message);
    }

    for entry in &report.degraded {
        let id = entry.id.as_deref().unwrap_or("?");
        println!("  \u{25b3} {}  lossy (degrade)  {}", id, entry.message);
    }

    for entry in &report.dropped {
        let id = entry.id.as_deref().unwrap_or("?");
        println!("  \u{2715} {}  dropped  {}", id, entry.message);
    }

    for entry in &report.body_warnings {
        println!("  \u{26a0} {}", entry.message);
    }

    println!(
        "Summary: {} lossless, {} lossy({} degraded), {} dropped, {} body-warning",
        report.lossless.len(),
        report.lossy.len() + report.degraded.len(),
        report.degraded.len(),
        report.dropped.len(),
        report.body_warnings.len(),
    );
}

fn print_report_json(report: &Report) {
    let json = serde_json::json!({
        "lossless": report.lossless,
        "lossy": report.lossy.iter().map(|e| serde_json::json!({
            "id": e.id,
            "message": e.message,
        })).collect::<Vec<_>>(),
        "dropped": report.dropped.iter().map(|e| serde_json::json!({
            "id": e.id,
            "message": e.message,
        })).collect::<Vec<_>>(),
        "degraded": report.degraded.iter().map(|e| serde_json::json!({
            "id": e.id,
            "message": e.message,
        })).collect::<Vec<_>>(),
        "body_warnings": report.body_warnings.iter().map(|e| serde_json::json!({
            "id": e.id,
            "message": e.message,
        })).collect::<Vec<_>>(),
        "summary": {
            "lossless": report.lossless.len(),
            "lossy": report.lossy.len() + report.degraded.len(),
            "degraded": report.degraded.len(),
            "dropped": report.dropped.len(),
            "body_warnings": report.body_warnings.len(),
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{
        new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, Kind, Loss, Tool,
    };
    use crate::handlers::EmitPlan;

    fn empty_plan() -> EmitPlan {
        EmitPlan {
            files: vec![],
            diagnostics: vec![],
        }
    }

    #[test]
    fn test_build_report_lossless() {
        let mut node = new_node(Kind::Skill, Tool::Claude, "/test/SKILL.md");
        node.fields.insert(
            "skills.name".to_string(),
            IRField {
                id: "skills.name".to_string(),
                value: serde_json::json!("test"),
                loss: Loss::Lossless,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );

        let plan = empty_plan();
        let report = build_report(&node, &plan);
        assert!(report.lossless.contains(&"skills.name".to_string()));
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn test_build_report_dropped() {
        let mut node = new_node(Kind::Skill, Tool::Claude, "/test/SKILL.md");
        node.fields.insert(
            "skills.user-invocable".to_string(),
            IRField {
                id: "skills.user-invocable".to_string(),
                value: serde_json::json!(true),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: Some(DroppedInfo {
                    reason: "No Codex equivalent".to_string(),
                }),
            },
        );

        let plan = empty_plan();
        let report = build_report(&node, &plan);
        assert!(!report.dropped.is_empty());
        assert_eq!(
            report.dropped[0].id,
            Some("skills.user-invocable".to_string())
        );
    }

    #[test]
    fn test_build_report_degraded() {
        let mut node = new_node(Kind::Skill, Tool::Claude, "/test/SKILL.md");
        node.fields.insert(
            "skills.model".to_string(),
            IRField {
                id: "skills.model".to_string(),
                value: serde_json::json!("claude-opus-4-8"),
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: Some(DegradeInfo {
                    to: "subagent".to_string(),
                    target: ".codex/agents/deploy.toml".to_string(),
                }),
                warning: None,
                dropped: None,
            },
        );

        let plan = empty_plan();
        let report = build_report(&node, &plan);
        assert!(!report.degraded.is_empty());
        assert!(report.degraded[0].message.contains("subagent"));
    }

    #[test]
    fn test_build_report_from_diagnostics() {
        let mut node = new_node(Kind::Skill, Tool::Claude, "/test/SKILL.md");
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some("unknown.field".to_string()),
            message: "unknown frontmatter: my_field".to_string(),
        });

        let plan = empty_plan();
        let report = build_report(&node, &plan);
        assert!(!report.dropped.is_empty());
        assert!(report
            .dropped
            .iter()
            .any(|e| e.message.contains("my_field")));
    }

    // gap 20/42: loss:dropped + warn:true must appear only in `dropped`, not in `lossy`.
    // A DiagLevel::Warn diagnostic on a field whose IRField.loss == Dropped must
    // not cause that field to be promoted into the lossy list.
    #[test]
    fn test_build_report_dropped_with_warn_diag_not_in_lossy() {
        let mut node = new_node(Kind::Skill, Tool::Claude, "/test/SKILL.md");

        // Simulate a warn:true + loss:dropped field (e.g. skills.user-invocable)
        node.fields.insert(
            "skills.user-invocable".to_string(),
            IRField {
                id: "skills.user-invocable".to_string(),
                value: serde_json::json!(false),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: Some("skills.user-invocable: warn".to_string()),
                dropped: Some(DroppedInfo {
                    reason: "Codex has no user-invocable concept".to_string(),
                }),
            },
        );

        // Simulate what the broken lift() used to push:
        // a DiagLevel::Warn diagnostic for a dropped field.
        // After the fix this should NOT be pushed; this test verifies the
        // report builder itself is resilient even if such a diagnostic
        // were present (e.g. from an older handler that was not yet fixed).
        // The IRField.loss takes precedence: it is Dropped, so the entry
        // must end up only in `dropped`.
        // (The actual fix prevents the Warn diag from being pushed for
        //  dropped fields, but we test the report routing here too.)
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("skills.user-invocable".to_string()),
            message: "skills.user-invocable: warn".to_string(),
        });

        let plan = empty_plan();
        let report = build_report(&node, &plan);

        // Must be in dropped
        assert!(
            report
                .dropped
                .iter()
                .any(|e| e.id.as_deref() == Some("skills.user-invocable")),
            "skills.user-invocable must appear in dropped"
        );

        // Must NOT appear in lossy: build_report enforces that a dropped field
        // is never also listed as lossy, even when a spurious DiagLevel::Warn
        // diagnostic carrying the same id is present.
        assert!(
            !report
                .lossy
                .iter()
                .any(|e| e.id.as_deref() == Some("skills.user-invocable")),
            "skills.user-invocable must NOT appear in lossy when it is classified as Dropped"
        );

        // dropped must have exactly one entry for this id (no duplicates).
        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some("skills.user-invocable"))
            .count();
        assert_eq!(
            count, 1,
            "skills.user-invocable must appear exactly once in dropped, found {} times",
            count
        );
    }

    // A dropped field must appear exactly once in report.dropped regardless of
    // whether a duplicate DiagLevel::Drop diagnostic with the same id is also
    // present (defense-in-depth dedup inside build_report).
    #[test]
    fn test_build_report_dropped_field_once_from_ir_fields_only() {
        let mut node = new_node(Kind::Plugin, Tool::Claude, "/test/plugin.json");

        node.fields.insert(
            "plugins.lspServers".to_string(),
            IRField {
                id: "plugins.lspServers".to_string(),
                value: serde_json::json!("./lsp.json"),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: Some(DroppedInfo {
                    reason: "lspServers has no Codex equivalent".to_string(),
                }),
            },
        );

        // No plan diagnostics — only the IRField source.
        let plan = empty_plan();
        let report = build_report(&node, &plan);

        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some("plugins.lspServers"))
            .count();
        assert_eq!(
            count, 1,
            "plugins.lspServers must appear exactly once in report.dropped when only \
             the IRField source is present; found {} times",
            count
        );

        // Also not in lossy.
        assert!(
            !report
                .lossy
                .iter()
                .any(|e| e.id.as_deref() == Some("plugins.lspServers")),
            "plugins.lspServers must NOT appear in lossy"
        );
    }

    // When BOTH an IRField{loss:Dropped} AND a DiagLevel::Drop diagnostic with
    // the same id are present, build_report must deduplicate to exactly one entry.
    #[test]
    fn test_build_report_dropped_dedup_when_both_ir_field_and_diag_present() {
        let mut node = new_node(Kind::Plugin, Tool::Claude, "/test/plugin.json");

        node.fields.insert(
            "plugins.lspServers".to_string(),
            IRField {
                id: "plugins.lspServers".to_string(),
                value: serde_json::json!("./lsp.json"),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: Some(DroppedInfo {
                    reason: "lspServers has no Codex equivalent".to_string(),
                }),
            },
        );

        // Simulate a handler that also pushes a DiagLevel::Drop for the same id.
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some("plugins.lspServers".to_string()),
            message: "plugins.lspServers: dropped (duplicate diag)".to_string(),
        });

        let plan = empty_plan();
        let report = build_report(&node, &plan);

        let count = report
            .dropped
            .iter()
            .filter(|e| e.id.as_deref() == Some("plugins.lspServers"))
            .count();
        assert_eq!(
            count, 1,
            "plugins.lspServers must appear exactly once in report.dropped even when \
             both IRField and DiagLevel::Drop sources are present; found {} times",
            count
        );
    }

    // gap 32/42: a dropped+warn field must appear only in dropped, not in lossy,
    // when the handler correctly omits the spurious DiagLevel::Drop/Warn diagnostic.
    #[test]
    fn test_build_report_dropped_field_not_in_lossy_without_spurious_diagnostic() {
        let mut node = new_node(Kind::Plugin, Tool::Claude, "/test/plugin.json");

        // IRField with loss:Dropped and a warning (warn:true in mappings).
        node.fields.insert(
            "plugins.channels".to_string(),
            IRField {
                id: "plugins.channels".to_string(),
                value: serde_json::json!([]),
                loss: Loss::Dropped,
                transforms_applied: vec![],
                degrade: None,
                warning: Some("channels: no Codex equivalent".to_string()),
                dropped: Some(DroppedInfo {
                    reason: "channels has no Codex equivalent".to_string(),
                }),
            },
        );

        // No spurious diagnostic pushed — this is the post-fix state.
        let plan = empty_plan();
        let report = build_report(&node, &plan);

        assert!(
            report
                .dropped
                .iter()
                .any(|e| e.id.as_deref() == Some("plugins.channels")),
            "plugins.channels must appear in dropped"
        );

        let in_lossy = report
            .lossy
            .iter()
            .any(|e| e.id.as_deref() == Some("plugins.channels"));
        assert!(
            !in_lossy,
            "plugins.channels must NOT appear in lossy when no spurious Warn diagnostic is pushed"
        );
    }
}
