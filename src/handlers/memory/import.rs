use std::path::{Path, PathBuf};

use crate::core::ir::{DiagLevel, Diagnostic};

/// 32 KiB = default upper limit for Codex's project_doc_max_bytes
pub(crate) const CODEX_MAX_BYTES: usize = 32768;
/// 28 KiB = warn threshold after reserving buffer for global instructions
pub(crate) const CODEX_WARN_BYTES: usize = 28672;
/// Maximum @import expansion depth (official cap is 4 hops; conservative value adopted)
pub(crate) const MAX_IMPORT_DEPTH: usize = 4;

/// Inlines @import directives.
/// Supported patterns:
///   @path/to/file          - relative path
///   @/absolute/path        - absolute path
///   @~/relative-to-home    - home-relative (emits a warning when unresolvable)
///   @ inside code blocks is excluded
pub(crate) fn inline_imports(
    body: &str,
    source_path: &str,
    depth: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    if depth >= MAX_IMPORT_DEPTH {
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("memory.import-syntax".to_string()),
            message: format!(
                "@import depth exceeded {} hops (official max). Remaining imports not expanded.",
                MAX_IMPORT_DEPTH
            ),
        });
        return body.to_string();
    }

    let source_dir = Path::new(source_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut result = String::new();
    let mut in_code_fence = false;

    for line in body.lines() {
        // Track code fences
        if line.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if in_code_fence {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Detect @import syntax (lines starting with @)
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') && !trimmed.starts_with("@@") {
            let import_path = trimmed[1..].trim();

            if import_path.starts_with("~/") {
                // Home-relative path: warn when unresolvable
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("memory.import-syntax".to_string()),
                    message: format!(
                        "@~/ import '{}' cannot be resolved (home directory path); keeping as-is",
                        import_path
                    ),
                });
                result.push_str(line);
                result.push('\n');
                continue;
            }

            let resolved = if import_path.starts_with('/') {
                PathBuf::from(import_path)
            } else {
                source_dir.join(import_path)
            };

            match std::fs::read_to_string(&resolved) {
                Ok(imported_content) => {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("memory.import-syntax".to_string()),
                        message: format!(
                            "@import '{}' inlined (Codex has no @import equivalent, lossy)",
                            import_path
                        ),
                    });
                    // Expand recursively (depth-limited)
                    let expanded = inline_imports(
                        &imported_content,
                        resolved.to_str().unwrap_or(""),
                        depth + 1,
                        diagnostics,
                    );
                    result.push_str(&expanded);
                    if !expanded.ends_with('\n') {
                        result.push('\n');
                    }
                }
                Err(_) => {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("memory.import-syntax".to_string()),
                        message: format!(
                            "@import '{}' could not be resolved (file not found); keeping as-is",
                            import_path
                        ),
                    });
                    result.push_str(line);
                    result.push('\n');
                }
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}
