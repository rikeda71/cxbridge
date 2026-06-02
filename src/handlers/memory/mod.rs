use std::path::Path;

use serde_json::Value;

use crate::core::ir::{IRNode, Kind};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, LowerOpts};

pub(crate) mod import;
pub(crate) mod lift;
pub(crate) mod lower;

/// Handler for the memory domain (CLAUDE.md ⇄ AGENTS.md).
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
        lift::lift(parsed, dir)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => lower::lower_c2x(ir, opts),
            ConvDir::X2c => lower::lower_x2c(ir, opts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{DiagLevel, Kind, Loss};
    use crate::core::mappings::load_mappings;
    use crate::core::transforms::ConvDir;
    use crate::handlers::{LowerOpts, Scope, SkillTargetMode};
    use import::CODEX_WARN_BYTES;
    use serde_json::Value;
    use std::fs;
    use tempfile::TempDir;

    fn make_handler() -> MemoryHandler {
        let maps = load_mappings();
        MemoryHandler {
            map: maps["memory"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            only: vec![],
            scope: Scope::Project,
            dual_manifest: false,
            hooks_target: Scope::User,
            skill_target: SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
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
    fn test_memory_agents_override_dropped_x2c() {
        let dir = TempDir::new().unwrap();
        let override_md = dir.path().join("AGENTS.override.md");
        fs::write(&override_md, "# Override stuff\n").unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&override_md).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        // Must be recorded as a dropped field with a Drop diagnostic, not silently
        // converted to CLAUDE.md.
        assert!(
            ir.fields.contains_key("memory.override-file"),
            "Expected override-file field"
        );
        assert_eq!(ir.fields["memory.override-file"].loss, Loss::Dropped);
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| d.level == DiagLevel::Drop
                    && d.id.as_deref() == Some("memory.override-file"))
        );

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();
        assert!(
            plan.files.is_empty(),
            "Expected no output file for AGENTS.override.md (must not become CLAUDE.md)"
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

        // lift() must NOT flag @-lines inside code fences as imports — the IR
        // should contain no "memory.import-syntax" field.
        assert!(
            !ir.fields.contains_key("memory.import-syntax"),
            "lift() must not flag @-lines inside code blocks as imports; \
             IR fields: {:?}",
            ir.fields.keys().collect::<Vec<_>>()
        );

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agents_md = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("AGENTS.md"))
            .unwrap();
        // @some/file.md inside code block should remain as-is in the output
        assert!(
            agents_md.content.contains("@some/file.md"),
            "Expected @import in code block to be preserved"
        );
    }
}
