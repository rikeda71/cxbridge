use crate::core::ir::{DiagLevel, IRNode, Loss};
use crate::handlers::EmitPlan;

/// 各診断エントリの共通表現。
#[derive(Debug, Clone)]
pub struct DiagEntry {
    /// mappings の entry id（例: "skill.allowed-tools"）
    pub id: Option<String>,
    pub message: String,
}

/// build_report が返す集計済みレポート。
/// dropped / degraded は必ず列挙（silent な切り捨て厳禁）。
pub struct Report {
    /// lossless フィールド id の一覧
    pub lossless: Vec<String>,
    /// lossy 変換（変換成功だが意味差あり）
    pub lossy: Vec<DiagEntry>,
    /// 変換先なしで切り捨てられたフィールド
    pub dropped: Vec<DiagEntry>,
    /// degrade エンジンで別スコープへ降格されたフィールド
    pub degraded: Vec<DiagEntry>,
    /// 本文スキャナが検出した警告
    pub body_warnings: Vec<DiagEntry>,
}

/// IR ノードと EmitPlan から Report を構築する。
///
/// IR の diagnostics と各 IRField を集計する。
/// dropped / degraded は必ず列挙する（silent な切り捨て厳禁）。
pub fn build_report(ir: &IRNode, plan: &EmitPlan) -> Report {
    let mut lossless = Vec::new();
    let mut lossy = Vec::new();
    let mut dropped = Vec::new();
    let mut degraded = Vec::new();
    let mut body_warnings = Vec::new();

    for (id, field) in &ir.fields {
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

    Report {
        lossless,
        lossy,
        dropped,
        degraded,
        body_warnings,
    }
}

/// Report を標準出力に表示する。
///
/// fmt が Some("json") の場合は機械可読 JSON で出力（CI 用）。
/// fmt が None の場合はヒューマンリーダブルなテキスト形式で出力。
///
/// 表示形式（テキスト）:
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
                    reason: "Codex に概念なし".to_string(),
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
}
