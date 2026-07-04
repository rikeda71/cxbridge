use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{new_node, IRNode, Kind, Tool};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, LowerOpts};

mod approval;
mod lift;
mod lower_c2x;
mod lower_x2c;

/// Handler for the settings domain (partial-conversion subset).
pub struct SettingsHandler {
    pub map: DomainMap,
}

impl Handler for SettingsHandler {
    fn kind(&self) -> Kind {
        Kind::Settings
    }

    fn detect(&self, path: &Path) -> bool {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        matches!(
            file_name,
            "settings.json" | "settings.local.json" | "config.toml"
        )
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if file_name.ends_with(".json") {
            // Claude settings.json
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read settings file: {}", path.display()))?;
            let json_val: serde_json::Value = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse settings.json: {}", path.display()))?;

            Ok(serde_json::json!({
                "frontmatter": json_val,
                "body": "",
                "path": abs_path.to_str().unwrap_or(""),
                "format": "json"
            }))
        } else if file_name.ends_with(".toml") {
            // Codex config.toml
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read config.toml: {}", path.display()))?;
            let toml_val: toml::Value = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config.toml: {}", path.display()))?;
            let json_val = crate::core::serialize::toml_to_json(&toml_val)?;

            Ok(serde_json::json!({
                "frontmatter": json_val,
                "body": "",
                "path": abs_path.to_str().unwrap_or(""),
                "format": "toml"
            }))
        } else {
            anyhow::bail!("SettingsHandler: unsupported file: {}", path.display())
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Settings, source_tool, &source_path);

        let settings = match parsed["frontmatter"].as_object() {
            Some(obj) => obj,
            None => return Ok(node),
        };

        match dir {
            ConvDir::C2x => self.lift_c2x(settings, &mut node),
            ConvDir::X2c => self.lift_x2c(settings, &mut node),
        }

        Ok(node)
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
    use crate::core::mappings::load_mappings;
    use std::fs;
    use tempfile::TempDir;

    fn make_handler() -> SettingsHandler {
        let maps = load_mappings();
        SettingsHandler {
            map: maps["settings-config"].clone(),
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
    fn test_settings_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("settings.json")));
        assert!(h.detect(Path::new("settings.local.json")));
        assert!(h.detect(Path::new("config.toml")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
    }

    #[test]
    fn test_settings_c2x_model_effort() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"model": "claude-sonnet-4-6", "effortLevel": "max"}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // model and effortLevel should be present
        assert!(ir.fields.contains_key("settings.model"));
        assert!(ir.fields.contains_key("settings.effortLevel"));

        // effortLevel max → xhigh via enum_map
        let effort_f = &ir.fields["settings.effortLevel"];
        assert_eq!(effort_f.value, Value::String("xhigh".to_string()));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // config.toml should be generated
        let config_toml = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
        assert!(config_toml.is_some(), "Expected config.toml output");

        let content = &config_toml.unwrap().content;
        assert!(
            content.contains("model_reasoning_effort"),
            "Expected model_reasoning_effort in config.toml"
        );
        assert!(content.contains("xhigh"), "Expected xhigh in config.toml");
    }

    #[test]
    fn test_settings_c2x_editor_mode() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(&settings_path, r#"{"editorMode": "vim"}"#).unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert!(ir.fields.contains_key("settings.editorMode"));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            config_toml.content.contains("vim_mode_default = true"),
            "Expected vim_mode_default=true, got: {}",
            config_toml.content
        );
    }

    #[test]
    fn test_settings_c2x_permissions_bash_to_rules() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"permissions": {"allow": ["Bash(cargo build)"]}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Should generate a .rules file
        let rules_file = plan.files.iter().find(|f| f.path.ends_with(".rules"));
        assert!(
            rules_file.is_some(),
            "Expected .rules file for Bash permission, got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_settings_x2c_otel_is_lossy_not_dropped() {
        use crate::core::report::build_report;
        use crate::handlers::EmitPlan;

        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "model = \"gpt-5-codex\"\n\n[otel]\nlog_user_prompt = true\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let report = build_report(
            &ir,
            &EmitPlan {
                files: vec![],
                diagnostics: vec![],
            },
        );

        // otel is lossy + degrade, so it must NOT appear as dropped.
        assert!(
            !report
                .dropped
                .iter()
                .any(|d| d.id.as_deref() == Some("settings.codex.otel")),
            "otel must not be in dropped: {:?}",
            report.dropped
        );
        assert!(
            report
                .degraded
                .iter()
                .any(|d| d.id.as_deref() == Some("settings.codex.otel")),
            "otel must be in degraded: {:?}",
            report.degraded
        );
    }

    #[test]
    fn test_settings_c2x_language_shell_output_style_lowered() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"language": "Japanese", "outputStyle": "concise", "defaultShell": "bash"}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml output");
        // language + outputStyle land in developer_instructions; defaultShell in the
        // shell policy — none are silently dropped.
        assert!(config.content.contains("developer_instructions"));
        assert!(config.content.contains("Japanese"));
        assert!(config.content.contains("concise"));
        assert!(config.content.contains("experimental_use_profile"));
        for id in [
            "settings.language",
            "settings.outputStyle",
            "settings.defaultShell",
        ] {
            assert!(
                plan.diagnostics.iter().any(|d| d.id.as_deref() == Some(id)),
                "Expected a diagnostic for {id}"
            );
        }
    }

    #[test]
    fn test_settings_c2x_default_mode_dont_ask_warns_and_converts() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"permissions": {"defaultMode": "dontAsk"}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // dontAsk converts to approval_policy=never + sandbox_mode=workspace-write.
        // The workspace boundary is preserved; only the approval gate is removed.
        let config = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml output");
        assert!(config.content.contains("approval_policy = \"never\""));
        assert!(config
            .content
            .contains("sandbox_mode = \"workspace-write\""));

        // The lossy approximation must be surfaced, not silent (warn:true contract).
        assert!(
            plan.diagnostics
                .iter()
                .any(|d| d.level == crate::core::ir::DiagLevel::Warn
                    && d.id.as_deref() == Some("settings.permissions.defaultMode.dontAsk")),
            "Expected a Warn diagnostic for defaultMode=dontAsk, got: {:?}",
            plan.diagnostics
        );
    }

    #[test]
    fn test_settings_c2x_dropped_fields_in_report() {
        use crate::core::report::build_report;
        use crate::handlers::EmitPlan;

        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"model": "claude-sonnet-4-6", "viewMode": "verbose", "worktree": {"enabled": true}, "autoUpdatesChannel": "latest"}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let empty_plan = EmitPlan {
            files: vec![],
            diagnostics: vec![],
        };
        let report = build_report(&ir, &empty_plan);

        // Dropped fields should be enumerated in the report
        assert!(
            !report.dropped.is_empty(),
            "Expected dropped fields in report"
        );
        let dropped_ids: Vec<_> = report
            .dropped
            .iter()
            .filter_map(|d| d.id.as_deref())
            .collect();
        assert!(
            dropped_ids.contains(&"settings.viewMode"),
            "Expected settings.viewMode in dropped, got: {:?}",
            dropped_ids
        );
        assert!(
            dropped_ids.contains(&"settings.worktree"),
            "Expected settings.worktree in dropped, got: {:?}",
            dropped_ids
        );
    }

    #[test]
    fn test_settings_c2x_sandbox_filesystem() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"sandbox": {"filesystem": {"allowWrite": ["/tmp/build"], "denyRead": ["~/.env"]}}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let config_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            config_toml.content.contains("[permissions"),
            "Expected permissions section, got: {}",
            config_toml.content
        );
        assert!(
            config_toml.content.contains("filesystem"),
            "Expected filesystem in permissions"
        );
    }

    #[test]
    fn test_settings_c2x_report_enumerates_remainder() {
        // The report should include un-converted fields as manual items
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"model": "claude-opus-4-8", "viewMode": "focus", "autoUpdatesChannel": "stable"}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // The plan should include a warning about partial conversion
        let has_partial_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("partial conversion"));
        assert!(
            has_partial_warn,
            "Expected partial conversion warning in diagnostics"
        );
    }

    #[test]
    fn test_settings_x2c_basic() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-5-codex"
model_reasoning_effort = "high"

[features.network_proxy]
dangerously_allow_all_unix_sockets = false
"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        assert!(ir.fields.contains_key("settings.model"));
        assert!(ir.fields.contains_key("settings.effortLevel"));
        assert!(ir
            .fields
            .contains_key("settings.sandbox.network.allowAllUnixSockets"));

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        let settings_json = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("settings.json"));
        assert!(settings_json.is_some(), "Expected settings.json in output");

        let content: Value = serde_json::from_str(&settings_json.unwrap().content).unwrap();
        assert!(content.get("model").is_some(), "Expected model field");
        assert!(
            content.get("effortLevel").is_some(),
            "Expected effortLevel field"
        );
    }

    #[test]
    fn test_developer_instructions_produces_claude_md() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-5-codex"
developer_instructions = "Always respond in English. Focus on clear answers."
"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();

        // IR must contain the developer_instructions field with degrade info
        assert!(
            ir.fields
                .contains_key("settings.codex.developer_instructions"),
            "IR must contain settings.codex.developer_instructions"
        );
        let f = &ir.fields["settings.codex.developer_instructions"];
        assert!(f.degrade.is_some(), "Field must have degrade info");
        assert_eq!(
            f.degrade.as_ref().unwrap().target,
            "CLAUDE.md",
            "Degrade target must be CLAUDE.md"
        );

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::X2c, &opts).unwrap();

        // Plan must contain a file ending with CLAUDE.md
        let claude_md = plan.files.iter().find(|f| f.path.ends_with("CLAUDE.md"));
        assert!(
            claude_md.is_some(),
            "Plan must contain CLAUDE.md file; got: {:?}",
            plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );

        // CLAUDE.md content must contain the original instruction text
        let content = &claude_md.unwrap().content;
        assert!(
            content.contains("Always respond in English"),
            "CLAUDE.md must contain original instruction text; got:\n{}",
            content
        );
        assert!(
            content.contains("Focus on clear answers"),
            "CLAUDE.md must contain full instruction text; got:\n{}",
            content
        );

        // The warning diagnostic is emitted during lift (ir.diagnostics), not lower.
        let has_diag = ir
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("settings.codex.developer_instructions"));
        assert!(
            has_diag,
            "Expected developer_instructions diagnostic in ir.diagnostics; got: {:?}",
            ir.diagnostics
                .iter()
                .map(|d| d.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_webfetch_deny_domains_in_config_toml_and_diagnostic() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            r#"{"permissions": {"deny": ["WebFetch(domain:bad.com)", "WebFetch(domain:evil.net)"]}}"#,
        )
        .unwrap();

        let out_dir = TempDir::new().unwrap();
        let h = make_handler();
        let parsed = h.parse(&settings_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let opts = default_opts(out_dir.path().to_str().unwrap());
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // config.toml must be generated
        let config_toml = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .expect("Expected config.toml output");

        let content = &config_toml.content;

        // bad.com and evil.net must appear with "deny"
        assert!(
            content.contains("bad.com") && content.contains("deny"),
            "Expected bad.com = \"deny\" in config.toml; got:\n{}",
            content
        );
        assert!(
            content.contains("evil.net"),
            "Expected evil.net in config.toml; got:\n{}",
            content
        );

        // Warn diagnostic with id "settings.permissions.deny.webfetch" must exist
        let has_diag = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("settings.permissions.deny.webfetch"));
        assert!(
            has_diag,
            "Expected diagnostic id 'settings.permissions.deny.webfetch'; diagnostics: {:?}",
            plan.diagnostics
                .iter()
                .map(|d| (d.id.as_deref().unwrap_or("<none>"), &d.message))
                .collect::<Vec<_>>()
        );

        let diag = plan
            .diagnostics
            .iter()
            .find(|d| d.id.as_deref() == Some("settings.permissions.deny.webfetch"))
            .unwrap();
        assert_eq!(
            diag.level,
            crate::core::ir::DiagLevel::Warn,
            "Expected DiagLevel::Warn for settings.permissions.deny.webfetch"
        );
    }
}
