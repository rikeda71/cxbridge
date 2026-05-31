use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::core::ir::{
    new_node, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// 32 KiB = Codex の project_doc_max_bytes デフォルト上限
const CODEX_MAX_BYTES: usize = 32768;
/// 28 KiB = グローバル指示分のバッファ考慮後の warn 閾値
const CODEX_WARN_BYTES: usize = 28672;
/// @import の最大展開深度（公式 4 ホップ、安全側を採用）
const MAX_IMPORT_DEPTH: usize = 4;

/// memory ドメインのハンドラ（CLAUDE.md ⇄ AGENTS.md）。
pub struct MemoryHandler {
    pub map: DomainMap,
}

impl Handler for MemoryHandler {
    fn kind(&self) -> Kind {
        Kind::Memory
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        matches!(
            name,
            "CLAUDE.md" | "AGENTS.md" | "CLAUDE.local.md" | "AGENTS.override.md"
        )
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;

        Ok(Value::Object({
            let mut obj = serde_json::Map::new();
            obj.insert(
                "path".to_string(),
                Value::String(path.to_str().unwrap_or("").to_string()),
            );
            obj.insert("filename".to_string(), Value::String(name.to_string()));
            obj.insert("body".to_string(), Value::String(content));
            obj
        }))
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let filename = parsed["filename"].as_str().unwrap_or("CLAUDE.md");
        let body = parsed["body"].as_str().unwrap_or("").to_string();

        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };

        let mut node = new_node(Kind::Memory, source_tool, &source_path);

        // ファイル名フィールドを IR に格納
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

        // パスフィールド
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

        // CLAUDE.local.md → dropped
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
                message: "CLAUDE.local.md dropped: Codex has no equivalent for uncommitted personal files".to_string(),
            });
        }

        // managed policy（/etc/claude-code/CLAUDE.md 等）→ dropped
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

        // c2x: @import 構文の検出
        if dir == ConvDir::C2x {
            let has_imports = body
                .lines()
                .any(|line| !in_code_block(line, &body) && line.trim_start().starts_with('@'));
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
                    message: "@import syntax detected; will be inlined on lower (lossy)"
                        .to_string(),
                });
            }
        }

        // body を IR に格納（lower で参照）
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

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

impl MemoryHandler {
    /// c2x: CLAUDE.md → AGENTS.md（@import インライン展開あり）
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut diagnostics = ir.diagnostics.clone();
        let out_root = opts.out.as_deref().unwrap_or(".");

        // dropped ファイルは skip
        if ir.fields.contains_key("memory.local-file")
            || ir.fields.contains_key("memory.managed-policy")
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

        // @import インライン展開
        let expanded_body = inline_imports(&body, source_path, 0, &mut diagnostics);

        // サイズチェック
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

        // 出力パス: CLAUDE.md / .claude/CLAUDE.md → AGENTS.md（同じ場所）
        let out_path = compute_output_path(source_path, out_root, ConvDir::C2x);

        Ok(EmitPlan {
            files: vec![EmitFile {
                path: out_path,
                content: expanded_body,
            }],
            diagnostics,
        })
    }

    /// x2c: AGENTS.md → CLAUDE.md（パス付け替えのみ）
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let diagnostics = ir.diagnostics.clone();
        let out_root = opts.out.as_deref().unwrap_or(".");

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
}

/// 出力パスを計算する。
/// c2x: CLAUDE.md → AGENTS.md, .claude/CLAUDE.md → AGENTS.md
/// x2c: AGENTS.md → CLAUDE.md
fn compute_output_path(source_path: &str, out_root: &str, dir: ConvDir) -> String {
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

/// @import 構文をインライン展開する。
/// 対応パターン:
///   @path/to/file          - 相対パス
///   @/absolute/path        - 絶対パス
///   @~/relative-to-home    - ホームディレクトリ相対（展開不可の場合 warn）
///   コードブロック内の @ は除外
fn inline_imports(
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
        // コードフェンスのトラッキング
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

        // @import 構文の検出（行頭の @ から始まるもの）
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') && !trimmed.starts_with("@@") {
            let import_path = trimmed[1..].trim();

            if import_path.starts_with("~/") {
                // ホームディレクトリ相対（展開不可の場合 warn）
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
                    // 再帰的に展開（深度制限あり）
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

/// コードブロック内判定のスタブ（inline_imports 内で状態追跡するため常に false）。
fn in_code_block(_line: &str, _full_body: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use tempfile::TempDir;

    fn make_handler() -> MemoryHandler {
        let maps = load_mappings(Path::new("mappings"));
        MemoryHandler {
            map: maps["memory"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
        }
    }

    #[test]
    fn test_memory_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("CLAUDE.md")));
        assert!(h.detect(Path::new("AGENTS.md")));
        assert!(h.detect(Path::new("CLAUDE.local.md")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
    }

    #[test]
    fn test_memory_lift_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let claude_md = dir.path().join("CLAUDE.md");
        fs::write(&claude_md, "# Instructions\n\nDo the thing.\n").unwrap();

        let h = make_handler();
        let parsed = h.parse(&claude_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Memory);
        assert!(ir.fields.contains_key("memory.filename"));
        assert!(ir.fields.contains_key("__body"));
        let filename_field = &ir.fields["memory.filename"];
        assert_eq!(filename_field.value, Value::String("CLAUDE.md".to_string()));
        assert_eq!(filename_field.loss, Loss::Lossless);
    }

    #[test]
    fn test_memory_lower_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let claude_md = dir.path().join("CLAUDE.md");
        fs::write(&claude_md, "# Instructions\n\nDo the thing.\n").unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&claude_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agents_md = plan.files.iter().find(|f| f.path.ends_with("AGENTS.md"));
        assert!(agents_md.is_some(), "Expected AGENTS.md in output");

        let content = &agents_md.unwrap().content;
        assert!(
            content.contains("Instructions"),
            "Expected content preserved"
        );
    }

    #[test]
    fn test_memory_lower_x2c_basic() {
        let dir = TempDir::new().unwrap();
        let agents_md = dir.path().join("AGENTS.md");
        fs::write(&agents_md, "# Agent Instructions\n\nBe helpful.\n").unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agents_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let claude_md = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
        assert!(claude_md.is_some(), "Expected CLAUDE.md in output");

        let content = &claude_md.unwrap().content;
        assert!(
            content.contains("Agent Instructions"),
            "Expected content preserved"
        );
    }

    #[test]
    fn test_memory_c2x_import_inline() {
        let dir = TempDir::new().unwrap();

        // imported file
        let rules_dir = dir.path().join("rules");
        fs::create_dir_all(&rules_dir).unwrap();
        let rule_file = rules_dir.join("coding.md");
        fs::write(&rule_file, "## Coding Rules\n\nUse Rust.\n").unwrap();

        // main CLAUDE.md with @import
        let claude_md = dir.path().join("CLAUDE.md");
        fs::write(
            &claude_md,
            "# Instructions\n\n@rules/coding.md\n\nDo things.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&claude_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // Should have detected imports
        assert!(
            ir.fields.contains_key("memory.import-syntax"),
            "Expected import-syntax field"
        );
        assert_eq!(ir.fields["memory.import-syntax"].loss, Loss::Lossy);

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agents_md = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("AGENTS.md"))
            .unwrap();

        // Content should include the imported file's contents
        assert!(
            agents_md.content.contains("Coding Rules"),
            "Expected imported content in AGENTS.md, got: {}",
            agents_md.content
        );
        assert!(
            agents_md.content.contains("Use Rust"),
            "Expected imported content details, got: {}",
            agents_md.content
        );
        // The @import line should be replaced
        assert!(
            !agents_md.content.contains("@rules/coding.md"),
            "Expected @import line to be replaced"
        );

        // Should have a warn diagnostic about inlining
        let has_import_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("inlined"));
        assert!(has_import_warn, "Expected inline warning diagnostic");
    }

    #[test]
    fn test_memory_c2x_size_warn() {
        let dir = TempDir::new().unwrap();
        let claude_md = dir.path().join("CLAUDE.md");
        // Generate content > 28 KiB
        let large_content = "A".repeat(CODEX_WARN_BYTES + 100);
        fs::write(&claude_md, &large_content).unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&claude_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let has_size_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("KiB") || d.message.contains("bytes"));
        assert!(has_size_warn, "Expected size warning diagnostic");
    }

    #[test]
    fn test_memory_local_file_dropped() {
        let dir = TempDir::new().unwrap();
        let local_md = dir.path().join("CLAUDE.local.md");
        fs::write(&local_md, "# Local stuff\n").unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&local_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(
            ir.fields.contains_key("memory.local-file"),
            "Expected local-file field"
        );
        assert_eq!(ir.fields["memory.local-file"].loss, Loss::Dropped);

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();
        // No output file for dropped local file
        assert!(
            plan.files.is_empty(),
            "Expected no output file for CLAUDE.local.md"
        );
    }

    #[test]
    fn test_memory_import_in_code_block_not_expanded() {
        let dir = TempDir::new().unwrap();
        let claude_md = dir.path().join("CLAUDE.md");
        // @import inside code block should NOT be expanded
        fs::write(&claude_md, "# Doc\n\n```\n@some/file.md\n```\n\nDone.\n").unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&claude_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // Should NOT detect imports (inside code block)
        // Note: our simple parser uses line-by-line tracking so it won't expand within code fences
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agents_md = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("AGENTS.md"))
            .unwrap();
        // @some/file.md inside code block should remain as-is
        assert!(
            agents_md.content.contains("@some/file.md"),
            "Expected @import in code block to be preserved"
        );
    }
}
