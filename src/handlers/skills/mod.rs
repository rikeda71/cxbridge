use std::path::Path;

use serde_json::Value;

use crate::core::ir::{IRNode, Kind};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, LowerOpts};

mod aux_files;
mod lift;
mod lower;
mod openai_yaml;

/// Handler for the skills domain.
pub struct SkillsHandler {
    pub map: DomainMap,
}

impl Handler for SkillsHandler {
    fn kind(&self) -> Kind {
        Kind::Skill
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name == "SKILL.md"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        crate::core::serialize::frontmatter::parse_frontmatter_file(path)
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        self.lift_impl(parsed, dir)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{Kind, Loss};
    use crate::core::mappings::load_mappings;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    use super::aux_files::extract_skill_name;

    fn make_handler() -> SkillsHandler {
        let maps = load_mappings();
        SkillsHandler {
            map: maps["skills"].clone(),
        }
    }

    fn default_opts() -> LowerOpts {
        LowerOpts {
            out: None,
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
    fn test_skills_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
        assert!(!h.detect(Path::new("README.md")));
    }

    #[test]
    fn test_skills_lift_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: deploy\ndescription: Deploy the app\n---\n\nRun deployment steps.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Skill);
        assert!(ir.fields.contains_key("skills.name"));
        assert!(ir.fields.contains_key("skills.description"));
        let name_field = &ir.fields["skills.name"];
        assert_eq!(name_field.value, Value::String("deploy".to_string()));
        assert_eq!(name_field.loss, Loss::Lossless);
    }

    #[test]
    fn test_skills_lift_c2x_dropped_user_invocable() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: test-skill\ndescription: Test\nuser-invocable: true\n---\nBody.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // user-invocable must be dropped
        let f = ir.fields.get("skills.user-invocable").unwrap();
        assert_eq!(f.loss, Loss::Dropped);
    }

    #[test]
    fn test_skills_lower_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: deploy\ndescription: Deploy the app\n---\n\nRun deployment steps.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that the output file was generated
        let has_skill_md = plan.files.iter().any(|f| f.path.ends_with("SKILL.md"));
        assert!(has_skill_md, "Expected SKILL.md in emit plan");

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .unwrap();
        assert!(skill_file.content.contains("deploy"));
    }

    #[test]
    fn test_skills_lower_c2x_with_allowed_tools() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("build");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: build\ndescription: Build the project\nallowed-tools:\n  - \"Bash(cargo build)\"\n---\nBuild.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that the .rules file was generated
        let has_rules = plan.files.iter().any(|f| f.path.ends_with(".rules"));
        assert!(has_rules, "Expected .rules file for Bash tool degrade");
    }

    #[test]
    fn test_skills_lower_c2x_when_to_use_concat() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("analyze");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: analyze\ndescription: Analyze code\nwhen_to_use: Use this when you need analysis\n---\nAnalyze.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .unwrap();

        // Verify that when_to_use was concatenated into description
        assert!(skill_file.content.contains("Analyze code"));
        assert!(skill_file
            .content
            .contains("Use this when you need analysis"));
    }

    #[test]
    fn test_extract_skill_name() {
        assert_eq!(
            extract_skill_name("/home/user/.claude/skills/deploy/SKILL.md"),
            "deploy"
        );
        assert_eq!(extract_skill_name(".agents/skills/build/SKILL.md"), "build");
    }

    #[test]
    fn test_skills_lift_c2x_model_degrade() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("heavy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: heavy\ndescription: Heavy task\nmodel: claude-opus-4-8\neffort: max\n---\nDo heavy work.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // model / effort → degrade info
        let model_f = ir.fields.get("skills.model").unwrap();
        assert_eq!(model_f.loss, Loss::Lossy);
        assert!(model_f.degrade.is_some());

        let effort_f = ir.fields.get("skills.effort").unwrap();
        assert_eq!(effort_f.loss, Loss::Lossy);
        assert!(effort_f.degrade.is_some());
    }

    #[test]
    fn test_skills_lower_c2x_subagent_degrade() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("heavy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: heavy\ndescription: Heavy task\nmodel: claude-opus-4-8\neffort: max\n---\nDo heavy work.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        // skill_target=Subagent for test
        let opts = LowerOpts {
            out: Some(out_dir.to_str().unwrap().to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Subagent,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        };

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that the .codex/agents/<skill>.toml file was generated
        let has_agent_toml = plan
            .files
            .iter()
            .any(|f| f.path.contains(".codex/agents/") && f.path.ends_with(".toml"));
        assert!(has_agent_toml, "Expected subagent TOML file");
    }

    // ── gap 17/42: --keep-claude-frontmatter flag never applied ─────────────

    /// lower (c2x) with keep_claude_frontmatter=true must retain Claude-specific
    /// frontmatter keys (when_to_use and allowed-tools) in the output SKILL.md
    /// alongside the standard Codex fields (name, description).
    #[test]
    fn test_skills_lower_c2x_keep_claude_frontmatter() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("kfm");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: kfm\ndescription: KFM task\nwhen_to_use: Use when needed\nallowed-tools:\n  - \"Bash(make)\"\n---\nDo the task.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = LowerOpts {
            out: Some(out_dir.to_str().unwrap().to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: true,
        };

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .expect("Expected SKILL.md in emit plan");

        assert!(
            skill_file.content.contains("when_to_use"),
            "Expected 'when_to_use' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("allowed-tools"),
            "Expected 'allowed-tools' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
            skill_file.content
        );
        // Standard Codex fields must still be present
        assert!(
            skill_file.content.contains("name"),
            "Expected 'name' in frontmatter, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("description"),
            "Expected 'description' in frontmatter, got:\n{}",
            skill_file.content
        );
    }

    // ── gap 23/42: Non-.md sibling files not path-remapped ──────────────────

    /// lower (c2x): Non-.md auxiliary files in skill dir must be copied with path-remap.
    /// `scripts/run.sh` → `.agents/skills/<name>/scripts/run.sh`, content unchanged.
    /// `README.txt` → `.agents/skills/<name>/README.txt`, content unchanged.
    #[test]
    fn test_skills_lower_c2x_aux_files() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("s");
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(scripts_dir.join("run.sh"), "#!/bin/bash\necho hi\n").unwrap();
        fs::write(skill_dir.join("README.txt"), "readme\n").unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&skill_dir.join("SKILL.md")).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let run_sh = plan
            .files
            .iter()
            .find(|f| f.path.contains(".agents/skills/s/scripts/run.sh"));
        assert!(
            run_sh.is_some(),
            "Expected .agents/skills/s/scripts/run.sh in emit plan. Got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert_eq!(
            run_sh.unwrap().content.trim(),
            "#!/bin/bash\necho hi",
            "run.sh content must be unchanged"
        );

        let readme = plan
            .files
            .iter()
            .find(|f| f.path.contains(".agents/skills/s/README.txt"));
        assert!(
            readme.is_some(),
            "Expected .agents/skills/s/README.txt in emit plan. Got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert_eq!(
            readme.unwrap().content.trim(),
            "readme",
            "README.txt content must be unchanged"
        );
    }

    /// lower (x2c): Non-.md auxiliary files in skill dir (excluding agents/openai.yaml)
    /// must be copied with path-remap to .claude/skills/<name>/.
    #[test]
    fn test_skills_lower_x2c_aux_files() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".agents").join("skills").join("s");
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(scripts_dir.join("run.sh"), "#!/bin/bash\necho hi\n").unwrap();
        fs::write(skill_dir.join("README.txt"), "readme\n").unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&skill_dir.join("SKILL.md")).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let run_sh = plan
            .files
            .iter()
            .find(|f| f.path.contains(".claude/skills/s/scripts/run.sh"));
        assert!(
            run_sh.is_some(),
            "Expected .claude/skills/s/scripts/run.sh in emit plan. Got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert_eq!(
            run_sh.unwrap().content.trim(),
            "#!/bin/bash\necho hi",
            "run.sh content must be unchanged"
        );

        let readme = plan
            .files
            .iter()
            .find(|f| f.path.contains(".claude/skills/s/README.txt"));
        assert!(
            readme.is_some(),
            "Expected .claude/skills/s/README.txt in emit plan. Got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert_eq!(
            readme.unwrap().content.trim(),
            "readme",
            "README.txt content must be unchanged"
        );
    }

    /// lower (x2c): agents/openai.yaml is NOT included in aux file copy
    /// (it is already lifted and handled separately).
    #[test]
    fn test_skills_lower_x2c_aux_files_excludes_openai_yaml() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".agents").join("skills").join("s");
        let agents_dir = skill_dir.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(
            agents_dir.join("openai.yaml"),
            "policy:\n  allow_implicit_invocation: true\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&skill_dir.join("SKILL.md")).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        // agents/openai.yaml must NOT appear in the output
        let has_openai_yaml = plan
            .files
            .iter()
            .any(|f| f.path.ends_with("agents/openai.yaml"));
        assert!(
            !has_openai_yaml,
            "agents/openai.yaml must not be blindly copied to output; it is already handled"
        );
    }

    /// lower (c2x) without keep_claude_frontmatter (default false) must NOT retain
    /// Claude-specific frontmatter keys; only name and description are written.
    #[test]
    fn test_skills_lower_c2x_no_keep_claude_frontmatter_by_default() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("nkfm");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: nkfm\ndescription: NKFM task\nwhen_to_use: Use when needed\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = LowerOpts {
            out: Some(out_dir.to_str().unwrap().to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        };

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .expect("Expected SKILL.md in emit plan");

        // when_to_use is merged into description, not kept as standalone key
        assert!(
            !skill_file.content.contains("when_to_use"),
            "Expected 'when_to_use' NOT in frontmatter with keep_claude_frontmatter=false, got:\n{}",
            skill_file.content
        );
    }

    // ── gap 20/42: warn:true + loss:dropped must NOT push DiagLevel::Warn ────

    /// lift (c2x): warn:true + loss:dropped fields must NOT push a DiagLevel::Warn
    /// diagnostic. Dropped fields are already enumerated via IRField.dropped; a
    /// spurious DiagLevel::Warn would cause build_report to route the entry to the
    /// lossy list as well, inflating summary counts.
    #[test]
    fn test_skills_lift_c2x_dropped_warn_no_spurious_warn_diag() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("gap20");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: t\ndescription: d\nuser-invocable: false\npaths:\n  - src/**\nargument-hint: \"[--env]\"\narguments:\n  - env\n---\nBody.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // All four fields must be Dropped
        for field_id in &[
            "skills.user-invocable",
            "skills.paths",
            "skills.argument-hint",
            "skills.arguments",
        ] {
            let f = ir
                .fields
                .get(*field_id)
                .unwrap_or_else(|| panic!("{} must be in IR fields", field_id));
            assert_eq!(f.loss, Loss::Dropped, "{} must have loss:Dropped", field_id);
        }

        // None of those fields must have pushed a DiagLevel::Warn diagnostic
        // (that would cause double-counting in build_report).
        for field_id in &[
            "skills.user-invocable",
            "skills.paths",
            "skills.argument-hint",
            "skills.arguments",
        ] {
            let has_warn_diag = ir.diagnostics.iter().any(|d| {
                d.level == crate::core::ir::DiagLevel::Warn && d.id.as_deref() == Some(field_id)
            });
            assert!(
                !has_warn_diag,
                "DiagLevel::Warn must NOT be pushed for dropped field {}; \
                 diagnostics: {:?}",
                field_id, ir.diagnostics
            );
        }
    }

    // ── fix: disable-model-invocation=false (explicit allow) was silently dropped ─

    /// lower (c2x): disable-model-invocation=false must produce agents/openai.yaml
    /// containing `allow_implicit_invocation: true` (symmetric with the =true case).
    #[test]
    fn test_skills_lower_c2x_disable_model_invocation_false_emits_allow_true() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("s");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: s\ndescription: d\ndisable-model-invocation: false\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let openai_yaml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agents/openai.yaml"))
            .expect("Expected .agents/skills/s/agents/openai.yaml in emit plan");

        assert!(
            openai_yaml
                .content
                .contains("allow_implicit_invocation: true"),
            "openai.yaml must contain 'allow_implicit_invocation: true' for disable-model-invocation=false, got:\n{}",
            openai_yaml.content
        );
    }

    // ── gap 4/42: disable-model-invocation silently dropped in c2x ──────────

    /// lift (c2x): disable-model-invocation=true must produce an IRField with
    /// loss=Lossy and a non-empty warning.
    #[test]
    fn test_skills_lift_c2x_disable_model_invocation() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("s");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: s\ndescription: d\ndisable-model-invocation: true\n---\nBody.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let f = ir
            .fields
            .get("skills.disable-model-invocation")
            .expect("skills.disable-model-invocation must be in IR fields after c2x lift");
        assert_eq!(
            f.loss,
            Loss::Lossy,
            "skills.disable-model-invocation should be Lossy in c2x direction"
        );
        assert!(
            f.warning.is_some(),
            "skills.disable-model-invocation should carry a warning"
        );
    }

    /// lower (c2x): disable-model-invocation=true must produce an EmitFile at
    /// `.agents/skills/s/agents/openai.yaml` containing
    /// `policy:\n  allow_implicit_invocation: false`.
    #[test]
    fn test_skills_lower_c2x_disable_model_invocation_emits_openai_yaml() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("s");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: s\ndescription: d\ndisable-model-invocation: true\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let openai_yaml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("agents/openai.yaml"))
            .expect("Expected .agents/skills/s/agents/openai.yaml in emit plan");

        assert!(
            openai_yaml
                .content
                .contains("allow_implicit_invocation: false"),
            "openai.yaml must contain 'allow_implicit_invocation: false', got:\n{}",
            openai_yaml.content
        );
        assert!(
            openai_yaml
                .path
                .contains(".agents/skills/s/agents/openai.yaml"),
            "openai.yaml path must be .agents/skills/s/agents/openai.yaml, got: {}",
            openai_yaml.path
        );
    }

    // ── gap 22/42: x2c silently drops interface.* and dependencies.tools ───────

    /// lift (x2c): interface.display_name / icon_small / brand_color must appear
    /// in IR fields as Loss::Lossy with a warning.
    #[test]
    fn test_skills_lift_x2c_openai_yaml_interface_display_name_lossy() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir
            .path()
            .join(".agents")
            .join("skills")
            .join("iface_skill");
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: iface_skill\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "policy:\n  allow_implicit_invocation: true\ninterface:\n  display_name: \"My Skill\"\n  icon_small: icon.png\n  brand_color: \"#FF0\"\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&skill_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        let f = ir
            .fields
            .get("skills.openai-yaml.interface")
            .expect("skills.openai-yaml.interface must be in IR fields after x2c lift");
        assert_eq!(
            f.loss,
            Loss::Lossy,
            "skills.openai-yaml.interface must have Loss::Lossy"
        );
        assert!(
            f.warning.is_some(),
            "skills.openai-yaml.interface must carry a warning"
        );
    }

    /// lift (x2c): interface.default_prompt must appear as Loss::Lossy in IR fields.
    #[test]
    fn test_skills_lift_x2c_openai_yaml_default_prompt_lossy() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".agents").join("skills").join("dp_skill");
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: dp_skill\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "interface:\n  default_prompt: \"Start with:\"\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&skill_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        let f = ir
            .fields
            .get("skills.openai-yaml.default_prompt")
            .expect("skills.openai-yaml.default_prompt must be in IR fields after x2c lift");
        assert_eq!(
            f.loss,
            Loss::Lossy,
            "skills.openai-yaml.default_prompt must have Loss::Lossy"
        );
        assert_eq!(
            f.value,
            Value::String("Start with:".to_string()),
            "default_prompt value must be preserved"
        );
    }

    /// lower (x2c): default_prompt must be prepended to the body in the output SKILL.md.
    #[test]
    fn test_skills_lower_x2c_default_prompt_prepended_to_body() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".agents").join("skills").join("dp_skill");
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: dp_skill\ndescription: d\n---\nBody text here.\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "interface:\n  default_prompt: \"Start with:\"\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&skill_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .expect("Expected SKILL.md in emit plan");

        assert!(
            skill_file.content.contains("Start with:"),
            "Expected default_prompt 'Start with:' prepended to body, got:\n{}",
            skill_file.content
        );
        let prompt_pos = skill_file.content.find("Start with:").unwrap();
        let body_pos = skill_file.content.find("Body text here.").unwrap();
        assert!(
            prompt_pos < body_pos,
            "default_prompt must appear before original body, got:\n{}",
            skill_file.content
        );
    }

    /// lift (x2c): dependencies.tools must appear as Loss::Lossy with a warning.
    #[test]
    fn test_skills_lift_x2c_openai_yaml_dependencies_tools_lossy() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".agents").join("skills").join("dep_skill");
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: dep_skill\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "dependencies:\n  tools:\n    - mcp__srv__tool\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&skill_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        let f = ir
            .fields
            .get("skills.openai-yaml.dependencies-tools")
            .expect("skills.openai-yaml.dependencies-tools must be in IR fields after x2c lift");
        assert_eq!(
            f.loss,
            Loss::Lossy,
            "skills.openai-yaml.dependencies-tools must have Loss::Lossy"
        );
        assert!(
            f.warning.is_some(),
            "skills.openai-yaml.dependencies-tools must carry a warning"
        );
    }

    /// Integration: x2c with fixture that has all interface.* and dependencies.tools fields.
    /// Report must contain entries for all these fields in lossy (not silently dropped).
    #[test]
    fn test_skills_x2c_interface_and_deps_in_report() {
        use crate::core::mappings::load_mappings;
        use crate::core::report::build_report;
        use crate::handlers::EmitPlan;

        let maps = load_mappings();
        let h = SkillsHandler {
            map: maps["skills"].clone(),
        };

        let dir = TempDir::new().unwrap();
        let skill_dir = dir
            .path()
            .join(".agents")
            .join("skills")
            .join("iface_skill");
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: iface_skill\ndescription: d\n---\nBody.\n",
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "policy:\n  allow_implicit_invocation: true\ninterface:\n  display_name: \"My Skill\"\n  icon_small: icon.png\n  brand_color: \"#FF0\"\n  default_prompt: \"Start with:\"\ndependencies:\n  tools:\n    - mcp__srv__tool\n",
        )
        .unwrap();

        let parsed = h.parse(&skill_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        let empty_plan = EmitPlan {
            files: vec![],
            diagnostics: vec![],
        };
        let report = build_report(&ir, &empty_plan);

        let lossy_ids: Vec<_> = report
            .lossy
            .iter()
            .filter_map(|e| e.id.as_deref())
            .collect();

        assert!(
            lossy_ids.contains(&"skills.openai-yaml.interface"),
            "skills.openai-yaml.interface must appear in lossy report, got lossy: {:?}",
            lossy_ids
        );
        assert!(
            lossy_ids.contains(&"skills.openai-yaml.default_prompt"),
            "skills.openai-yaml.default_prompt must appear in lossy report, got lossy: {:?}",
            lossy_ids
        );
        assert!(
            lossy_ids.contains(&"skills.openai-yaml.dependencies-tools"),
            "skills.openai-yaml.dependencies-tools must appear in lossy report, got lossy: {:?}",
            lossy_ids
        );
    }

    // ── gap 33/42: WebFetch/WebSearch allowed-tools generate config.toml permissions ─

    /// lower (c2x) with WebFetch in allowed-tools must produce a config.toml EmitFile
    /// containing [permissions.<skill>].network = true.
    #[test]
    fn test_skills_lower_c2x_web_fetch_produces_config_toml() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("net-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: net-skill\ndescription: Network skill\nallowed-tools:\n  - WebFetch\n---\nFetch.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml in emit plan for WebFetch allowed-tool");

        assert!(
            config_file.content.contains("[permissions.net-skill]"),
            "Expected [permissions.net-skill] in config.toml, got:\n{}",
            config_file.content
        );
        assert!(
            config_file.content.contains("network = true"),
            "Expected 'network = true' in config.toml, got:\n{}",
            config_file.content
        );
    }

    /// lower (c2x) with WebSearch in allowed-tools must produce a config.toml EmitFile
    /// containing [features].web_search = true.
    #[test]
    fn test_skills_lower_c2x_web_search_produces_config_toml() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir
            .path()
            .join(".claude")
            .join("skills")
            .join("search-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: search-skill\ndescription: Search skill\nallowed-tools:\n  - WebSearch\n---\nSearch.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml in emit plan for WebSearch allowed-tool");

        assert!(
            config_file.content.contains("[features]"),
            "Expected [features] section in config.toml, got:\n{}",
            config_file.content
        );
        assert!(
            config_file.content.contains("web_search = true"),
            "Expected 'web_search = true' in config.toml, got:\n{}",
            config_file.content
        );
    }

    // ── gap 19/42: --keep-claude-frontmatter retains allowed-tools, model, effort ─

    /// lower (c2x) with keep_claude_frontmatter=true must retain allowed-tools,
    /// model, and effort in the output SKILL.md in addition to name and description.
    #[test]
    fn test_skills_lower_c2x_keep_claude_frontmatter_model_effort_allowed_tools() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("gap19");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: gap19\ndescription: Gap 19 task\nallowed-tools:\n  - \"Bash(git *)\"\nmodel: claude-opus-4-5\neffort: max\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = LowerOpts {
            out: Some(out_dir.to_str().unwrap().to_string()),
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: true,
        };

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .expect("Expected SKILL.md in emit plan");

        assert!(
            skill_file.content.contains("allowed-tools"),
            "Expected 'allowed-tools' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("model"),
            "Expected 'model' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("effort"),
            "Expected 'effort' in frontmatter with keep_claude_frontmatter=true, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("name"),
            "Expected 'name' in frontmatter, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("description"),
            "Expected 'description' in frontmatter, got:\n{}",
            skill_file.content
        );
    }

    // ── Agent Skills open standard: metadata (skills.metadata) ──────────────

    /// lift+lower (c2x): metadata is preserved verbatim in the emitted SKILL.md
    /// frontmatter, with no drop diagnostic for it.
    #[test]
    fn test_skills_c2x_metadata_preserved() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("meta");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: meta\ndescription: Metadata skill\nmetadata:\n  author: example-org\n  version: \"1.0\"\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let f = ir
            .fields
            .get("skills.metadata")
            .expect("skills.metadata must be in IR fields after c2x lift");
        assert_eq!(f.loss, Loss::Lossless);

        // No drop diagnostic for metadata
        assert!(
            !ir.diagnostics
                .iter()
                .any(|d| d.level == crate::core::ir::DiagLevel::Drop
                    && d.message.contains("metadata")),
            "metadata must not be dropped; diagnostics: {:?}",
            ir.diagnostics
        );

        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();
        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .expect("Expected SKILL.md in emit plan");

        assert!(
            skill_file.content.contains("author: example-org"),
            "Expected metadata.author preserved in output frontmatter, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("version:"),
            "Expected metadata.version preserved in output frontmatter, got:\n{}",
            skill_file.content
        );
    }

    /// lift+lower (x2c): metadata written in a Codex SKILL.md is preserved verbatim
    /// when converting to Claude, with no drop diagnostic for it.
    #[test]
    fn test_skills_x2c_metadata_preserved() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".agents").join("skills").join("meta");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: meta\ndescription: Metadata skill\nmetadata:\n  author: example-org\n  version: \"1.0\"\n---\nBody.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        let f = ir
            .fields
            .get("skills.metadata")
            .expect("skills.metadata must be in IR fields after x2c lift");
        assert_eq!(f.loss, Loss::Lossless);

        assert!(
            !ir.diagnostics
                .iter()
                .any(|d| d.level == crate::core::ir::DiagLevel::Drop
                    && d.message.contains("metadata")),
            "metadata must not be dropped; diagnostics: {:?}",
            ir.diagnostics
        );

        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();
        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .expect("Expected SKILL.md in emit plan");

        assert!(
            skill_file.content.contains("author: example-org"),
            "Expected metadata.author preserved in output frontmatter, got:\n{}",
            skill_file.content
        );
        assert!(
            skill_file.content.contains("version:"),
            "Expected metadata.version preserved in output frontmatter, got:\n{}",
            skill_file.content
        );
    }
}
