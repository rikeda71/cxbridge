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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{DiagLevel, Diagnostic};
    use std::fs;
    use tempfile::TempDir;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn run(body: &str, source_path: &str) -> (String, Vec<Diagnostic>) {
        let mut diags = Vec::new();
        let out = inline_imports(body, source_path, 0, &mut diags);
        (out, diags)
    }

    // ── basic inlining ────────────────────────────────────────────────────────

    #[test]
    fn sibling_import_is_inlined() {
        let dir = TempDir::new().unwrap();
        let sibling = dir.path().join("rules.md");
        fs::write(&sibling, "## Rules\n\nBe strict.\n").unwrap();

        let source = dir.path().join("CLAUDE.md");
        let body = "# Main\n\n@rules.md\n\nEnd.\n";

        let (out, diags) = run(body, source.to_str().unwrap());

        assert!(
            out.contains("Be strict"),
            "Imported content must appear in output; got:\n{out}"
        );
        assert!(
            !out.contains("@rules.md"),
            "@import line must be replaced by file contents; got:\n{out}"
        );
        assert!(
            out.contains("# Main"),
            "Content before @import must be preserved; got:\n{out}"
        );
        assert!(
            out.contains("End."),
            "Content after @import must be preserved; got:\n{out}"
        );

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagLevel::Warn);
        assert_eq!(diags[0].id.as_deref(), Some("memory.import-syntax"));
        assert!(
            diags[0].message.contains("inlined"),
            "Diagnostic must mention 'inlined'; got: {}",
            diags[0].message
        );
    }

    #[test]
    fn subdirectory_import_is_inlined() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("rules");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("coding.md"), "Use Rust.\n").unwrap();

        let source = dir.path().join("CLAUDE.md");
        let body = "# Doc\n\n@rules/coding.md\n";

        let (out, _diags) = run(body, source.to_str().unwrap());

        assert!(
            out.contains("Use Rust"),
            "Subdirectory import must be inlined"
        );
        assert!(
            !out.contains("@rules/coding.md"),
            "@import line must be removed"
        );
    }

    // ── missing import target ─────────────────────────────────────────────────

    #[test]
    fn missing_import_target_keeps_line_and_warns() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("CLAUDE.md");
        // The file referenced by @import does not exist.
        let body = "# Header\n\n@missing/file.md\n\nFooter.\n";

        let (out, diags) = run(body, source.to_str().unwrap());

        // Original @import line must be kept as-is (not silently dropped).
        assert!(
            out.contains("@missing/file.md"),
            "@import line for missing file must be preserved; got:\n{out}"
        );
        // Surrounding content must also be preserved.
        assert!(out.contains("# Header"));
        assert!(out.contains("Footer."));

        // A warning diagnostic must be emitted.
        assert_eq!(diags.len(), 1, "Expected exactly one diagnostic");
        assert_eq!(diags[0].level, DiagLevel::Warn);
        assert_eq!(diags[0].id.as_deref(), Some("memory.import-syntax"));
        assert!(
            diags[0].message.contains("not found")
                || diags[0].message.contains("could not be resolved"),
            "Diagnostic must describe the resolution failure; got: {}",
            diags[0].message
        );
    }

    // ── home-relative path ────────────────────────────────────────────────────

    #[test]
    fn home_relative_import_keeps_line_and_warns() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("CLAUDE.md");
        let body = "@~/shared/rules.md\n";

        let (out, diags) = run(body, source.to_str().unwrap());

        assert!(
            out.contains("@~/shared/rules.md"),
            "Home-relative line must be kept; got:\n{out}"
        );
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagLevel::Warn);
        assert!(
            diags[0].message.contains("~/") || diags[0].message.contains("home"),
            "Diagnostic must mention the home-relative path issue; got: {}",
            diags[0].message
        );
    }

    // ── recursion depth cap ───────────────────────────────────────────────────

    #[test]
    fn depth_cap_stops_expansion_at_max_import_depth() {
        // Build a chain of depth MAX_IMPORT_DEPTH + 1: a.md → b.md → c.md → d.md → e.md
        // The last hop exceeds the cap and must not be expanded.
        let dir = TempDir::new().unwrap();

        let e = dir.path().join("e.md");
        fs::write(&e, "LEAF_CONTENT\n").unwrap();

        let d = dir.path().join("d.md");
        fs::write(&d, "@e.md\n").unwrap();

        let c = dir.path().join("c.md");
        fs::write(&c, "@d.md\n").unwrap();

        let b = dir.path().join("b.md");
        fs::write(&b, "@c.md\n").unwrap();

        let a = dir.path().join("a.md");
        fs::write(&a, "@b.md\n").unwrap();

        // Root call at depth 0: a.md → b(1) → c(2) → d(3) → e would be depth 4 = MAX
        let source = dir.path().join("CLAUDE.md");
        let body = "@a.md\n";

        let mut diags = Vec::new();
        let out = inline_imports(body, source.to_str().unwrap(), 0, &mut diags);

        // The chain starting at depth 0 will expand a.md (depth 1), b.md (depth 2),
        // c.md (depth 3), d.md (depth 4 = MAX_IMPORT_DEPTH) — at depth 4 the cap fires.
        // So LEAF_CONTENT from e.md must NOT appear.
        assert!(
            !out.contains("LEAF_CONTENT"),
            "Content beyond max depth must not be inlined; got:\n{out}"
        );

        // The depth-exceeded diagnostic must appear exactly once.
        let depth_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("depth exceeded") || d.message.contains("depth"))
            .collect();
        assert!(
            !depth_diags.is_empty(),
            "Expected at least one depth-exceeded diagnostic; diagnostics: {diags:?}"
        );
        let depth_warn = &depth_diags[0];
        assert_eq!(depth_warn.level, DiagLevel::Warn);
        assert_eq!(depth_warn.id.as_deref(), Some("memory.import-syntax"));
    }

    #[test]
    fn chain_within_depth_cap_is_fully_inlined() {
        // A chain of exactly MAX_IMPORT_DEPTH - 1 hops must be fully expanded.
        // depth 0: root calls inline_imports; depth 1: expands a.md;
        // depth 2: expands b.md (the leaf) — still within cap.
        let dir = TempDir::new().unwrap();

        let leaf = dir.path().join("leaf.md");
        fs::write(&leaf, "LEAF_OK\n").unwrap();

        let mid = dir.path().join("mid.md");
        fs::write(&mid, "@leaf.md\n").unwrap();

        let source = dir.path().join("CLAUDE.md");
        let body = "@mid.md\n";

        let mut diags = Vec::new();
        let out = inline_imports(body, source.to_str().unwrap(), 0, &mut diags);

        assert!(
            out.contains("LEAF_OK"),
            "Chain within depth cap must be fully expanded; got:\n{out}"
        );
        // No depth-exceeded diagnostic expected for a short chain.
        assert!(
            !diags.iter().any(|d| d.message.contains("depth exceeded")),
            "No depth-exceeded diagnostic for short chain; got: {diags:?}"
        );
    }

    // ── @import inside code fence ─────────────────────────────────────────────

    #[test]
    fn import_inside_code_fence_is_not_expanded() {
        let dir = TempDir::new().unwrap();
        // Create a file that WOULD be inlined if the fence were ignored.
        let target = dir.path().join("secret.md");
        fs::write(&target, "SECRET_CONTENT\n").unwrap();

        let source = dir.path().join("CLAUDE.md");
        let body = "# Example\n\n```\n@secret.md\n```\n\nNormal line.\n";

        let (out, diags) = run(body, source.to_str().unwrap());

        // @secret.md must stay verbatim inside the fence.
        assert!(
            out.contains("@secret.md"),
            "@import inside code fence must not be replaced; got:\n{out}"
        );
        // The file content must not appear.
        assert!(
            !out.contains("SECRET_CONTENT"),
            "Fenced @import must not inline its target; got:\n{out}"
        );
        // No diagnostic should be emitted for the fenced line.
        assert!(
            diags.is_empty(),
            "No diagnostic expected for @import inside code fence; got: {diags:?}"
        );
    }

    #[test]
    fn import_outside_code_fence_after_closed_fence_is_expanded() {
        // Verifies that the fence-tracking state resets properly after the fence closes.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("after.md");
        fs::write(&target, "AFTER_FENCE_CONTENT\n").unwrap();

        let source = dir.path().join("CLAUDE.md");
        let body = "```\nsome code\n```\n\n@after.md\n";

        let (out, diags) = run(body, source.to_str().unwrap());

        assert!(
            out.contains("AFTER_FENCE_CONTENT"),
            "@import after a closed code fence must be expanded; got:\n{out}"
        );
        assert!(!out.contains("@after.md"), "@import line must be replaced");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagLevel::Warn);
    }

    // ── double-@@ is not treated as an import ────────────────────────────────

    #[test]
    fn double_at_prefix_is_not_treated_as_import() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("CLAUDE.md");
        let body = "@@some/annotation\n";

        let (out, diags) = run(body, source.to_str().unwrap());

        assert!(
            out.contains("@@some/annotation"),
            "@@-prefixed line must be kept verbatim; got:\n{out}"
        );
        assert!(
            diags.is_empty(),
            "No diagnostic for @@ line; got: {diags:?}"
        );
    }
}
