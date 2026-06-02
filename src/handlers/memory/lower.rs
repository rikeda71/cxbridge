use std::path::Path;

use crate::core::ir::{DiagLevel, Diagnostic, IRNode};
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};

use super::import::{inline_imports, CODEX_MAX_BYTES, CODEX_WARN_BYTES};

/// c2x: CLAUDE.md → AGENTS.md (with @import inlining).
pub(crate) fn lower_c2x(ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
    let mut diagnostics = Vec::new();
    let out_root = opts.out.as_deref().unwrap_or(".");

    // Skip dropped files
    if ir.fields.contains_key("memory.local-file")
        || ir.fields.contains_key("memory.managed-policy")
        || ir.fields.contains_key("memory.override-file")
    {
        return Ok(EmitPlan {
            files: vec![],
            diagnostics,
        });
    }

    let body = ir
        .fields
        .get("__body")
        .and_then(|f| f.value.as_str())
        .unwrap_or("")
        .to_string();

    let source_path = &ir.source_path;

    // Inline @import directives
    let expanded_body = inline_imports(&body, source_path, 0, &mut diagnostics);

    // Size check
    let byte_len = expanded_body.len();
    if byte_len > CODEX_MAX_BYTES {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("memory.project-doc-max-bytes".to_string()),
            message: format!(
                "Expanded AGENTS.md is {} bytes (exceeds Codex 32 KiB limit). \
                 Content may be silently truncated. Consider increasing project_doc_max_bytes.",
                byte_len
            ),
        });
    } else if byte_len > CODEX_WARN_BYTES {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("memory.project-doc-max-bytes".to_string()),
            message: format!(
                "Expanded AGENTS.md is {} bytes (approaching 32 KiB Codex limit, \
                 warn threshold is 28 KiB).",
                byte_len
            ),
        });
    }

    // Output path: CLAUDE.md / .claude/CLAUDE.md → AGENTS.md (same location)
    let out_path = compute_output_path(source_path, out_root, ConvDir::C2x);

    Ok(EmitPlan {
        files: vec![EmitFile {
            path: out_path,
            content: expanded_body,
        }],
        diagnostics,
    })
}

/// x2c: AGENTS.md → CLAUDE.md (path remapping only).
pub(crate) fn lower_x2c(ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
    let diagnostics = Vec::new();
    let out_root = opts.out.as_deref().unwrap_or(".");

    // AGENTS.override.md is dropped (no Claude equivalent); emit nothing.
    if ir.fields.contains_key("memory.override-file") {
        return Ok(EmitPlan {
            files: vec![],
            diagnostics,
        });
    }

    let body = ir
        .fields
        .get("__body")
        .and_then(|f| f.value.as_str())
        .unwrap_or("")
        .to_string();

    let source_path = &ir.source_path;
    let out_path = compute_output_path(source_path, out_root, ConvDir::X2c);

    Ok(EmitPlan {
        files: vec![EmitFile {
            path: out_path,
            content: body,
        }],
        diagnostics,
    })
}

/// Computes the output path.
/// c2x: CLAUDE.md → AGENTS.md, .claude/CLAUDE.md → AGENTS.md
/// x2c: AGENTS.md → CLAUDE.md
pub(crate) fn compute_output_path(source_path: &str, out_root: &str, dir: ConvDir) -> String {
    let path = Path::new(source_path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("CLAUDE.md");

    let out_filename = match (dir, filename) {
        (ConvDir::C2x, "CLAUDE.md") => "AGENTS.md",
        (ConvDir::C2x, "CLAUDE.local.md") => "AGENTS.md",
        (ConvDir::X2c, "AGENTS.md") => "CLAUDE.md",
        (ConvDir::X2c, "AGENTS.override.md") => "CLAUDE.md",
        _ => filename,
    };

    format!("{}/{}", out_root, out_filename)
}
