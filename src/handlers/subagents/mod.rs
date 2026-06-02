use std::path::Path;

use serde_json::Value;

use crate::core::ir::{new_node, DiagLevel, Diagnostic, IRNode, Kind, Tool};
use crate::core::mappings::{index_by_claude_field, index_by_codex_field, DomainMap};
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, LowerOpts};

mod lower_c2x;
mod lower_x2c;
mod parse;

/// Handler for the subagents domain.
pub struct SubagentHandler {
    pub map: DomainMap,
}

impl Handler for SubagentHandler {
    fn kind(&self) -> Kind {
        Kind::Subagent
    }

    fn detect(&self, path: &Path) -> bool {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let path_str = path.to_str().unwrap_or("");

        // c2x: .claude/agents/<n>.md
        if file_name.ends_with(".md") {
            let parent = path.parent().and_then(|p| p.to_str()).unwrap_or("");
            if parent.ends_with("agents")
                || parent.contains("/agents")
                || parent.contains("\\agents")
            {
                return true;
            }
        }

        // x2c: .codex/agents/<n>.toml (not config.toml)
        if file_name.ends_with(".toml")
            && file_name != "config.toml"
            && (path_str.contains(".codex/agents/") || path_str.contains(".codex\\agents\\"))
        {
            return true;
        }

        false
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let path_str = path.to_str().unwrap_or("");

        if file_name.ends_with(".toml") && file_name != "config.toml" {
            // x2c: Codex TOML agent file
            parse::parse_codex_agent_toml(path)
        } else if file_name.ends_with(".md")
            && (path_str.contains("/agents/") || path_str.contains("\\agents\\"))
        {
            // c2x: Claude agent Markdown file
            crate::core::serialize::frontmatter::parse_frontmatter_file(path)
        } else {
            anyhow::bail!(
                "SubagentHandler: unrecognized file format for {}",
                path.display()
            )
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Subagent, source_tool, &source_path);

        let idx = match dir {
            ConvDir::C2x => index_by_claude_field(&self.map),
            ConvDir::X2c => index_by_codex_field(&self.map),
        };

        let frontmatter = match parsed["frontmatter"].as_object() {
            Some(fm) => fm,
            None => {
                // no frontmatter — still lift the body
                let body_raw = parsed["body"].as_str().unwrap_or("").to_string();
                node.body = Some(crate::core::ir::BodySegment {
                    raw: body_raw,
                    findings: vec![],
                });
                return Ok(node);
            }
        };

        for (key, value) in frontmatter {
            let Some(&entry) = idx.get(key.as_str()) else {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: None,
                    message: format!("unknown frontmatter key: {key}"),
                });
                continue;
            };

            crate::handlers::lift_mapped_field(entry, key, value, dir, &mut node);
        }

        // body: for c2x, the Markdown body is the system prompt content
        let body_raw = parsed["body"].as_str().unwrap_or("").to_string();
        node.body = Some(crate::core::ir::BodySegment {
            raw: body_raw,
            findings: vec![],
        });

        // Only relevant when converting Claude → Codex: Claude auto-delegates via
        // description match, but Codex requires explicit spawn_agent calls.
        if matches!(dir, ConvDir::C2x) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("subagents.spawn-model".to_string()),
                message: "Claude auto-delegates via description match. \
                          Codex requires explicit spawn_agent call (multi_agent=true). \
                          Add spawn instructions to developer_instructions."
                    .to_string(),
            });
        }

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => lower_c2x::lower_c2x(ir, opts),
            ConvDir::X2c => lower_x2c::lower_x2c(ir, opts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::Loss;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use tempfile::TempDir;

    fn make_handler() -> SubagentHandler {
        let maps = load_mappings();
        SubagentHandler {
            map: maps["subagents"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    #[test]
    fn test_subagent_detect_claude_md() {
        let h = make_handler();
        // .claude/agents/my-agent.md
        assert!(h.detect(Path::new(".claude/agents/my-agent.md")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new("CLAUDE.md")));
    }

    #[test]
    fn test_subagent_detect_codex_toml() {
        let h = make_handler();
        assert!(h.detect(Path::new(".codex/agents/my-agent.toml")));
        assert!(!h.detect(Path::new("config.toml")));
        assert!(!h.detect(Path::new(".codex/config.toml")));
    }

    #[test]
    fn test_subagent_c2x_basic_roundtrip() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("researcher.md");
        fs::write(
            &agent_path,
            "---\nname: researcher\ndescription: Research tasks\n---\n\nYou are a research agent.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Subagent);
        assert!(ir.fields.contains_key("subagents.name"));
        assert!(ir.fields.contains_key("subagents.description"));
        let name_f = &ir.fields["subagents.name"];
        assert_eq!(name_f.value, Value::String("researcher".to_string()));
        assert_eq!(name_f.loss, Loss::Lossless);

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // .codex/agents/researcher.toml should be generated
        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("researcher.toml"));
        assert!(
            agent_toml.is_some(),
            "Expected researcher.toml in output, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        let content = &agent_toml.unwrap().content;
        assert!(content.contains("researcher"), "name should be in TOML");
        assert!(
            content.contains("Research tasks"),
            "description should be in TOML"
        );
        assert!(
            content.contains("research agent"),
            "body should be in developer_instructions"
        );
    }

    #[test]
    fn test_subagent_c2x_model_effort() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("heavy.md");
        fs::write(
            &agent_path,
            "---\nname: heavy\ndescription: Heavy processing\nmodel: claude-opus-4-8\neffort: max\n---\n\nDo heavy work.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(ir.fields.contains_key("subagents.model"));
        assert!(ir.fields.contains_key("subagents.effort"));

        // model should be lossy (different providers)
        let model_f = &ir.fields["subagents.model"];
        assert_eq!(model_f.loss, Loss::Lossy);

        // effort: max → xhigh via enum_map
        let effort_f = &ir.fields["subagents.effort"];
        assert_eq!(effort_f.value, Value::String("xhigh".to_string()));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("heavy.toml"))
            .unwrap();
        assert!(
            agent_toml.content.contains("model_reasoning_effort"),
            "Expected model_reasoning_effort in TOML"
        );
        assert!(
            agent_toml.content.contains("xhigh"),
            "Expected xhigh in TOML"
        );
    }

    #[test]
    fn test_subagent_c2x_dropped_fields() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("bg.md");
        fs::write(
            &agent_path,
            "---\nname: bg\ndescription: Background agent\nmaxTurns: 10\nbackground: true\nisolation: worktree\ncolor: blue\n---\n\nBackground work.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // maxTurns, background, isolation, color → dropped
        let max_turns = ir.fields.get("subagents.maxTurns").unwrap();
        assert_eq!(max_turns.loss, Loss::Dropped);

        let background = ir.fields.get("subagents.background").unwrap();
        assert_eq!(background.loss, Loss::Dropped);

        let isolation = ir.fields.get("subagents.isolation").unwrap();
        assert_eq!(isolation.loss, Loss::Dropped);

        let color = ir.fields.get("subagents.color").unwrap();
        assert_eq!(color.loss, Loss::Dropped);
    }

    #[test]
    fn test_subagent_x2c_basic_roundtrip() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".codex").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("coder.toml");
        fs::write(
            &agent_path,
            r#"name = "coder"
description = "Code writing agent"
developer_instructions = '''
You are a coding assistant.
'''
"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        assert_eq!(ir.kind, Kind::Subagent);
        assert!(ir.fields.contains_key("subagents.name"));
        assert_eq!(
            ir.fields["subagents.name"].value,
            Value::String("coder".to_string())
        );

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let agent_md = plan.files.iter().find(|f| f.path.ends_with("coder.md"));
        assert!(
            agent_md.is_some(),
            "Expected coder.md in output, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        let content = &agent_md.unwrap().content;
        assert!(content.contains("coder"), "name should be in frontmatter");
        assert!(
            content.contains("Code writing agent"),
            "description should be in frontmatter"
        );
        assert!(
            content.contains("coding assistant"),
            "developer_instructions should be in body"
        );
    }

    #[test]
    fn test_subagent_c2x_emits_config_toml_agents_and_features() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("researcher.md");
        fs::write(
            &agent_path,
            "---\nname: researcher\ndescription: Research tasks\nmodel: claude-opus-4-8\neffort: max\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // agent TOML must be present
        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("researcher.toml"));
        assert!(
            agent_toml.is_some(),
            "Expected researcher.toml, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        // config.toml must be present with [agents.researcher] and multi_agent
        let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
        assert!(
            config_toml.is_some(),
            "Expected config.toml, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        let content = &config_toml.unwrap().content;
        assert!(
            content.contains("[agents.researcher]"),
            "Expected [agents.researcher] in config.toml, got:\n{}",
            content
        );
        assert!(
            content.contains("config_file"),
            "Expected config_file in config.toml, got:\n{}",
            content
        );
        assert!(
            content.contains("multi_agent"),
            "Expected multi_agent in config.toml, got:\n{}",
            content
        );
        assert!(
            content.contains("true"),
            "Expected multi_agent = true in config.toml, got:\n{}",
            content
        );
    }

    #[test]
    fn test_subagent_c2x_report_enumerates_dropped() {
        use crate::core::report::build_report;
        use crate::handlers::EmitPlan;

        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("complex.md");
        fs::write(
            &agent_path,
            "---\nname: complex\ndescription: Complex agent\nmaxTurns: 5\nbackground: true\nisolation: worktree\ncolor: red\n---\n\nDo complex tasks.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let empty_plan = EmitPlan {
            files: vec![],
            diagnostics: vec![],
        };
        let report = build_report(&ir, &empty_plan);

        // Dropped fields should be enumerated in the report
        assert!(
            !report.dropped.is_empty(),
            "Expected dropped entries in report"
        );
        let dropped_ids: Vec<_> = report
            .dropped
            .iter()
            .filter_map(|d| d.id.as_deref())
            .collect();
        assert!(
            dropped_ids.contains(&"subagents.maxTurns"),
            "Expected subagents.maxTurns in dropped, got: {:?}",
            dropped_ids
        );
        assert!(
            dropped_ids.contains(&"subagents.background"),
            "Expected subagents.background in dropped, got: {:?}",
            dropped_ids
        );
    }

    /// permissionMode values with no Codex equivalent (acceptEdits, auto, dontAsk)
    /// must not produce sandbox_mode in the output TOML, and a Drop diagnostic
    /// must appear in plan.diagnostics.
    #[test]
    fn test_c2x_permission_mode_unmapped_values_dropped() {
        let h = make_handler();

        for (perm_mode, label) in [
            ("acceptEdits", "acceptEdits"),
            ("auto", "auto"),
            ("dontAsk", "dontAsk"),
        ] {
            let dir = TempDir::new().unwrap();
            let agents_dir = dir.path().join(".claude").join("agents");
            fs::create_dir_all(&agents_dir).unwrap();

            let agent_path = agents_dir.join("t.md");
            fs::write(
                &agent_path,
                format!(
                    "---\nname: t\ndescription: D\npermissionMode: {}\n---\nBody.\n",
                    perm_mode
                ),
            )
            .unwrap();

            let out_dir = TempDir::new().unwrap();
            let parsed = h.parse(&agent_path).unwrap();
            let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
            let opts = default_opts(out_dir.path().to_str().unwrap());
            let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

            let agent_toml = plan
                .files
                .iter()
                .find(|f| f.path.ends_with("t.toml"))
                .unwrap_or_else(|| {
                    panic!(
                        "Expected t.toml in output for permissionMode={}, got: {:?}",
                        label,
                        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                    )
                });

            assert!(
                !agent_toml.content.contains("sandbox_mode"),
                "sandbox_mode must not appear in TOML for permissionMode={}, got:\n{}",
                label,
                agent_toml.content
            );

            let has_drop = plan.diagnostics.iter().any(|d| {
                d.id.as_deref() == Some("subagents.permissionMode") && d.level == DiagLevel::Drop
            });
            assert!(
                has_drop,
                "Expected Drop diagnostic for subagents.permissionMode (permissionMode={}), got: {:?}",
                label,
                plan.diagnostics
                    .iter()
                    .map(|d| (d.id.as_deref(), &d.level))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Valid permissionMode values (default, bypassPermissions, plan) that map
    /// to a Codex sandbox_mode must still produce sandbox_mode in the TOML output.
    #[test]
    fn test_c2x_permission_mode_valid_values_emitted() {
        let h = make_handler();

        for (perm_mode, expected_sandbox, label) in [
            (
                "bypassPermissions",
                "danger-full-access",
                "bypassPermissions",
            ),
            ("plan", "read-only", "plan"),
            ("default", "workspace-write", "default"),
        ] {
            let dir = TempDir::new().unwrap();
            let agents_dir = dir.path().join(".claude").join("agents");
            fs::create_dir_all(&agents_dir).unwrap();

            let agent_path = agents_dir.join("t.md");
            fs::write(
                &agent_path,
                format!(
                    "---\nname: t\ndescription: D\npermissionMode: {}\n---\nBody.\n",
                    perm_mode
                ),
            )
            .unwrap();

            let out_dir = TempDir::new().unwrap();
            let parsed = h.parse(&agent_path).unwrap();
            let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
            let opts = default_opts(out_dir.path().to_str().unwrap());
            let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

            let agent_toml = plan
                .files
                .iter()
                .find(|f| f.path.ends_with("t.toml"))
                .unwrap_or_else(|| {
                    panic!(
                        "Expected t.toml in output for permissionMode={}, got: {:?}",
                        label,
                        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                    )
                });

            assert!(
                agent_toml
                    .content
                    .contains(&format!("sandbox_mode = \"{}\"", expected_sandbox)),
                "Expected sandbox_mode=\"{}\" for permissionMode={}, got:\n{}",
                expected_sandbox,
                label,
                agent_toml.content
            );

            let has_drop = plan.diagnostics.iter().any(|d| {
                d.id.as_deref() == Some("subagents.permissionMode") && d.level == DiagLevel::Drop
            });
            assert!(
                !has_drop,
                "Must not have Drop diagnostic for valid permissionMode={}, got: {:?}",
                label,
                plan.diagnostics
                    .iter()
                    .map(|d| (d.id.as_deref(), &d.level))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// x2c: Codex TOML with [skills]\nconfig = [{enabled=true, path="python"}] must lift
    /// subagents.skills into the IR (not dropped as unknown key) and lower it to
    /// a `skills:` list in the Claude agent frontmatter.
    #[test]
    fn test_subagent_x2c_skills_lifted() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".codex").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("coder.toml");
        fs::write(
            &agent_path,
            "name = \"coder\"\ndescription = \"D\"\ndeveloper_instructions = \"Body\"\n\n[skills]\nconfig = [{enabled = true, path = \"python\"}]\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        // The IR must have subagents.skills — it must NOT be dropped as unknown
        assert!(
            ir.fields.contains_key("subagents.skills"),
            "IR must contain subagents.skills; got fields: {:?}",
            ir.fields.keys().collect::<Vec<_>>()
        );

        // No drop diagnostic for "skills"
        let has_unknown_skills_drop = ir
            .diagnostics
            .iter()
            .any(|d| d.level == DiagLevel::Drop && d.message.contains("skills"));
        assert!(
            !has_unknown_skills_drop,
            "Must not have Drop diagnostic for skills; diagnostics: {:?}",
            ir.diagnostics
        );

        // lower → Claude .md should contain skills: [python]
        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let agent_md = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("coder.md"))
            .unwrap();
        assert!(
            agent_md.content.contains("python"),
            "Output .md must contain 'python' in skills list; got:\n{}",
            agent_md.content
        );
        assert!(
            agent_md.content.contains("skills"),
            "Output .md must contain 'skills' frontmatter key; got:\n{}",
            agent_md.content
        );

        // A Warn diagnostic for the lossy mapping must be emitted
        let has_skills_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("subagents.skills") && d.level == DiagLevel::Warn);
        assert!(
            has_skills_warn,
            "Expected subagents.skills Warn diagnostic; got: {:?}",
            plan.diagnostics
        );
    }

    /// gap 37/42: fields with loss:dropped + warn:true must appear in report.dropped
    /// exactly once and must NOT appear in report.lossy at all.
    ///
    /// The four subagents fields disallowedTools, maxTurns, background, and
    /// isolation are all loss:dropped + warn:true. Each must be counted once in
    /// dropped[] only — never in lossy[] and never duplicated.
    ///
    /// This is a full-pipeline test: lift → lower (obtaining a real plan with its
    /// diagnostics) → build_report. That ensures no duplication from any of the
    /// three diagnostic sources (IRField loop, ir.diagnostics loop,
    /// plan.diagnostics loop).
    #[test]
    fn test_subagent_c2x_dropped_warn_fields_not_in_lossy_not_duplicated() {
        use crate::core::report::build_report;

        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("full.md");
        fs::write(
            &agent_path,
            "---\nname: full\ndescription: Full agent\nmaxTurns: 5\nbackground: true\nisolation: worktree\ndisallowedTools:\n  - Bash\n---\n\nFull agent body.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let report = build_report(&ir, &plan);

        // Each loss:dropped + warn:true field must appear exactly once in dropped[].
        for field_id in &[
            "subagents.maxTurns",
            "subagents.background",
            "subagents.isolation",
            "subagents.disallowedTools",
        ] {
            let dropped_count = report
                .dropped
                .iter()
                .filter(|e| e.id.as_deref() == Some(field_id))
                .count();
            assert_eq!(
                dropped_count, 1,
                "{field_id} must appear exactly once in report.dropped, found {dropped_count} times. \
                 Full dropped: {:?}",
                report
                    .dropped
                    .iter()
                    .map(|e| e.id.as_deref().unwrap_or("<none>"))
                    .collect::<Vec<_>>()
            );

            // Must NOT appear in lossy[].
            let in_lossy = report
                .lossy
                .iter()
                .any(|e| e.id.as_deref() == Some(field_id));
            assert!(
                !in_lossy,
                "{field_id} must NOT appear in report.lossy. \
                 Full lossy: {:?}",
                report
                    .lossy
                    .iter()
                    .map(|e| e.id.as_deref().unwrap_or("<none>"))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// c2x regression: skills: [python] in Claude .md must still convert to
    /// skills = [...] in Codex TOML (regression guard for the c2x direction).
    #[test]
    fn test_subagent_c2x_skills_roundtrip() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let agent_path = agents_dir.join("dev.md");
        fs::write(
            &agent_path,
            "---\nname: dev\ndescription: D\nskills:\n  - python\n  - javascript\n---\nBody.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&agent_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(
            ir.fields.contains_key("subagents.skills"),
            "IR must contain subagents.skills"
        );
        assert_eq!(
            ir.fields["subagents.skills"].value,
            Value::Array(vec![
                Value::String("python".to_string()),
                Value::String("javascript".to_string()),
            ])
        );

        let out_dir = TempDir::new().unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let agent_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("dev.toml"))
            .unwrap();
        assert!(
            agent_toml.content.contains("python"),
            "Codex TOML must contain python skill; got:\n{}",
            agent_toml.content
        );
        assert!(
            agent_toml.content.contains("javascript"),
            "Codex TOML must contain javascript skill; got:\n{}",
            agent_toml.content
        );
        assert!(
            agent_toml.content.contains("enabled"),
            "Codex TOML skills must have enabled field; got:\n{}",
            agent_toml.content
        );
    }
}
