use std::path::Path;

use serde_json::Value;

use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, Kind, LowerOpts};

mod lift;
mod lower;
mod toml_convert;

use toml_convert::parse_codex_hooks_toml;

/// The 10 common events (both / lossless).
const COMMON_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];

/// Claude-specific events (claude_to_codex / dropped).
const CLAUDE_ONLY_EVENTS: &[&str] = &[
    "Setup",
    "UserPromptExpansion",
    "PermissionDenied",
    "PostToolUseFailure",
    "PostToolBatch",
    "Notification",
    "MessageDisplay",
    "TaskCreated",
    "TaskCompleted",
    "StopFailure",
    "TeammateIdle",
    "InstructionsLoaded",
    "ConfigChange",
    "CwdChanged",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "Elicitation",
    "ElicitationResult",
    "SessionEnd",
];

/// Hook fields dropped in c2x (args/shell/if/once/asyncRewake).
const DROPPED_C2X_HOOK_FIELDS: &[&str] = &["args", "shell", "if", "once", "asyncRewake"];

/// Handler for the hooks domain.
pub struct HooksHandler {
    pub map: DomainMap,
}

impl Handler for HooksHandler {
    fn kind(&self) -> Kind {
        Kind::Hooks
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // hooks.json (Claude) or config.toml containing a hooks section (Codex)
        name == "hooks.json" || name == "settings.json"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".toml") {
            parse_codex_hooks_toml(path)
        } else {
            crate::core::serialize::json::parse_json_file(path)
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<crate::core::ir::IRNode> {
        match dir {
            ConvDir::C2x => self.lift_c2x(parsed),
            ConvDir::X2c => self.lift_x2c(parsed),
        }
    }

    fn lower(
        &self,
        ir: &crate::core::ir::IRNode,
        dir: ConvDir,
        opts: &LowerOpts,
    ) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lift::{normalize_matcher_c2x, MatcherKind};
    use super::*;
    use crate::core::ir::{DiagLevel, Loss};
    use crate::core::mappings::load_mappings;
    use crate::handlers::Scope;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> HooksHandler {
        let maps = load_mappings();
        HooksHandler {
            map: maps["hooks"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            only: vec![],
            scope: Scope::Project,
            dual_manifest: false,
            hooks_target: Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    #[test]
    fn test_hooks_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("hooks.json")));
        assert!(h.detect(Path::new("settings.json")));
        assert!(!h.detect(Path::new(".mcp.json")));
        assert!(!h.detect(Path::new("SKILL.md")));
    }

    #[test]
    fn test_normalize_matcher_exact() {
        let (norm, kind) = normalize_matcher_c2x("Bash");
        assert_eq!(norm, "^Bash$");
        assert_eq!(kind, MatcherKind::Exact);
    }

    #[test]
    fn test_normalize_matcher_alternation() {
        let (norm, kind) = normalize_matcher_c2x("Edit|Write");
        assert_eq!(norm, "^(Edit|Write)$");
        assert_eq!(kind, MatcherKind::Exact);
    }

    #[test]
    fn test_normalize_matcher_wildcard_star() {
        let (norm, kind) = normalize_matcher_c2x("*");
        assert_eq!(norm, "");
        assert_eq!(kind, MatcherKind::Wildcard);
    }

    #[test]
    fn test_normalize_matcher_wildcard_empty() {
        let (norm, kind) = normalize_matcher_c2x("");
        assert_eq!(norm, "");
        assert_eq!(kind, MatcherKind::Wildcard);
    }

    /// Regression test for gap 40/42: empty-string matcher must emit id
    /// "hooks.matcher.wildcard", not "hooks.matcher.exact".
    /// The value does not change ("" → ""), so no exact-normalization diagnostic
    /// should be emitted; the only diagnostic must be the wildcard one.
    #[test]
    fn test_normalize_matcher_wildcard_empty_emits_correct_id() {
        let h = make_handler();
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": "echo stop" }]
                }]
            }
        });

        let ir = h.lift_c2x(&hooks_json).unwrap();

        // Must emit id "hooks.matcher.wildcard" for the empty-string matcher.
        let has_wildcard_id = ir
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("hooks.matcher.wildcard"));
        assert!(
            has_wildcard_id,
            "Empty-string matcher must emit id 'hooks.matcher.wildcard'; diagnostics: {:?}",
            ir.diagnostics
                .iter()
                .map(|d| d.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );

        // Must NOT emit "hooks.matcher.exact" — the value did not change.
        let has_exact_id = ir
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("hooks.matcher.exact"));
        assert!(
            !has_exact_id,
            "Empty-string matcher must NOT emit 'hooks.matcher.exact'; diagnostics: {:?}",
            ir.diagnostics
                .iter()
                .map(|d| d.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_normalize_matcher_regex_passthrough() {
        let (norm, kind) = normalize_matcher_c2x("^Bash.*");
        assert_eq!(norm, "^Bash.*");
        assert_eq!(kind, MatcherKind::Regex);
    }

    #[test]
    fn test_hooks_lift_c2x_common_event() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "echo pre-tool" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir.fields.get("hooks.event.PreToolUse");
        assert!(field.is_some(), "Expected PreToolUse field");
        let f = field.unwrap();
        assert_eq!(f.loss, Loss::Lossless);
        // matcher should be normalized
        let entries = f.value.as_array().unwrap();
        let first = &entries[0];
        let matcher = first.get("matcher").and_then(|v| v.as_str()).unwrap();
        assert_eq!(matcher, "^Bash$", "Expected normalized matcher");
    }

    #[test]
    fn test_hooks_lift_c2x_claude_only_event_dropped() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Setup": [
                    {
                        "matcher": "",
                        "hooks": [{ "type": "command", "command": "echo setup" }]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir.fields.get("hooks.event.Setup");
        assert!(field.is_some(), "Expected Setup field");
        let f = field.unwrap();
        assert_eq!(f.loss, Loss::Dropped);
        // The canonical dropped info lives in the IRField; no separate diagnostic is emitted
        // to avoid double-counting in build_report.
        assert!(
            f.dropped.is_some(),
            "Expected dropped reason in IRField for Setup"
        );
        assert!(
            f.dropped.as_ref().unwrap().reason.contains("Setup"),
            "Expected 'Setup' in dropped reason, got: {}",
            f.dropped.as_ref().unwrap().reason
        );
    }

    #[test]
    fn test_hooks_lift_c2x_http_type_dropped() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "http", "url": "https://example.com" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        // The event field is stored (not dropped), but http hook is filtered out
        // Check that there's a Drop diagnostic about http type
        let has_http_drop = ir
            .diagnostics
            .iter()
            .any(|d| d.message.contains("http") && d.level == DiagLevel::Drop);
        assert!(has_http_drop, "Expected Drop diagnostic for http type");
    }

    #[test]
    fn test_hooks_lower_c2x_user_scope_json() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo stop" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());
        let plan = h.lower_c2x(&ir, &opts).unwrap();

        // Should produce hooks.json
        let has_hooks_json = plan.files.iter().any(|f| f.path.ends_with("hooks.json"));
        assert!(has_hooks_json, "Expected hooks.json output for user scope");

        let hj = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("hooks.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&hj.content).unwrap();
        assert!(
            parsed.get("Stop").is_some(),
            "Expected Stop event in hooks.json"
        );
    }

    #[test]
    fn test_hooks_lower_c2x_project_scope_toml() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo stop" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let dir = TempDir::new().unwrap();
        let mut opts = default_opts(dir.path().to_str().unwrap());
        opts.hooks_target = Scope::Project;
        let plan = h.lower_c2x(&ir, &opts).unwrap();

        let has_config_toml = plan.files.iter().any(|f| f.path.ends_with("config.toml"));
        assert!(
            has_config_toml,
            "Expected config.toml output for project scope"
        );

        let ct = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            ct.content.contains("[[hooks.Stop]]"),
            "Expected [[hooks.Stop]] in config.toml, got: {}",
            ct.content
        );
    }

    #[test]
    fn test_hooks_lift_c2x_args_synthesized() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "my-script",
                                "args": ["--flag", "value with spaces"]
                            }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir.fields.get("hooks.event.PreToolUse").unwrap();
        let entries = field.value.as_array().unwrap();
        let hook = &entries[0]["hooks"][0];
        let cmd = hook["command"].as_str().unwrap();
        // Should contain the synthesized command with quoted args
        assert!(cmd.contains("my-script"), "Expected my-script in command");
        assert!(
            cmd.contains("--flag"),
            "Expected --flag in synthesized command"
        );
        // "value with spaces" should be quoted
        assert!(
            cmd.contains("value with spaces") || cmd.contains("'value with spaces'"),
            "Expected quoted arg in: {}",
            cmd
        );
    }

    #[test]
    fn test_hooks_lift_x2c_toml_roundtrip() {
        // Simulate parsing Codex TOML hooks structure
        let codex_parsed = serde_json::json!({
            "path": ".codex/config.toml",
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo done" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_x2c(&codex_parsed).unwrap();

        let field = ir.fields.get("hooks.event.Stop");
        assert!(field.is_some(), "Expected Stop event");
        assert_eq!(field.unwrap().loss, Loss::Lossless);
    }

    #[test]
    fn test_hooks_lower_x2c() {
        let codex_parsed = serde_json::json!({
            "path": ".codex/config.toml",
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "^Bash$",
                        "hooks": [
                            { "type": "command", "command": "echo post" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_x2c(&codex_parsed).unwrap();

        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());
        let plan = h.lower_x2c(&ir, &opts).unwrap();

        let hj = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("hooks.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&hj.content).unwrap();
        assert!(
            parsed["hooks"]["PostToolUse"].is_array(),
            "Expected PostToolUse in output hooks.json"
        );
    }

    /// Verifies that lift_x2c handles a parse_json_file-wrapped value correctly.
    /// parse_json_file wraps top-level JSON content under a "frontmatter" key;
    /// lift_x2c must unwrap it before iterating event keys.
    #[test]
    fn test_hooks_lift_x2c_json_flat_format() {
        // Simulate what parse_json_file produces from a flat Codex hooks.json
        // (i.e., {"PreToolUse":[...]} wrapped as {"frontmatter":{...}, "body":"", "path":"..."})
        let parsed = serde_json::json!({
            "frontmatter": {
                "PreToolUse": [
                    {
                        "matcher": "^Bash$",
                        "hooks": [
                            { "type": "command", "command": "echo test" }
                        ]
                    }
                ]
            },
            "body": "",
            "path": "/tmp/hooks.json"
        });

        let h = make_handler();
        let ir = h.lift_x2c(&parsed).unwrap();

        let field = ir.fields.get("hooks.event.PreToolUse");
        assert!(
            field.is_some(),
            "Expected hooks.event.PreToolUse in IR; fields were: {:?}",
            ir.fields.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            field.unwrap().loss,
            Loss::Lossless,
            "PreToolUse must be Lossless"
        );
    }

    /// Wildcard matchers ("*" and "") must emit `id: "hooks.matcher.wildcard"`, not
    /// `"hooks.matcher.exact"`.  Spec entry `hooks.matcher.wildcard` exists in
    /// mappings/hooks.yaml and covers the lossy conversion "*" / "" → "".
    #[test]
    fn test_hooks_matcher_wildcard_id() {
        let h = make_handler();

        for wildcard_matcher in &["*", ""] {
            let hooks_json = serde_json::json!({
                "hooks": {
                    "Stop": [{
                        "matcher": wildcard_matcher,
                        "hooks": [{ "type": "command", "command": "echo done" }]
                    }]
                }
            });

            let ir = h.lift_c2x(&hooks_json).unwrap();

            let has_wildcard_id = ir
                .diagnostics
                .iter()
                .any(|d| d.id.as_deref() == Some("hooks.matcher.wildcard"));
            assert!(
                has_wildcard_id,
                "matcher '{}' must emit id 'hooks.matcher.wildcard' but diagnostics were: {:?}",
                wildcard_matcher,
                ir.diagnostics
                    .iter()
                    .map(|d| d.id.as_deref().unwrap_or("<none>"))
                    .collect::<Vec<_>>()
            );

            // Must NOT emit hooks.matcher.exact for a wildcard
            let has_exact_id = ir
                .diagnostics
                .iter()
                .any(|d| d.id.as_deref() == Some("hooks.matcher.exact"));
            assert!(
                !has_exact_id,
                "matcher '{}' must NOT emit 'hooks.matcher.exact', diagnostics: {:?}",
                wildcard_matcher,
                ir.diagnostics
                    .iter()
                    .map(|d| d.id.as_deref().unwrap_or("<none>"))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Regex passthrough matchers (e.g. "^Bash.*") must emit NO diagnostic
    /// (loss:lossless, warn:false per mappings/hooks.yaml `hooks.matcher.regex`).
    /// The event must NOT have any "hooks.matcher.regex" Warn diagnostic.
    #[test]
    fn test_hooks_regex_matcher_no_warn() {
        let h = make_handler();

        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "^Bash.*",
                    "hooks": [{ "type": "command", "command": "echo test" }]
                }]
            }
        });

        let ir = h.lift_c2x(&hooks_json).unwrap();

        // Must NOT emit any hooks.matcher.regex Warn diagnostic
        let regex_warn_diags: Vec<_> = ir
            .diagnostics
            .iter()
            .filter(|d| {
                d.id.as_deref() == Some("hooks.matcher.regex") && d.level == DiagLevel::Warn
            })
            .collect();
        assert!(
            regex_warn_diags.is_empty(),
            "regex passthrough must produce no Warn diagnostic; got: {:?}",
            regex_warn_diags
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );

        // The event field must be Lossless (regex passthrough is lossless)
        let field = ir.fields.get("hooks.event.PreToolUse").unwrap();
        assert_eq!(
            field.loss,
            Loss::Lossless,
            "hooks.event.PreToolUse must be Lossless for regex matcher"
        );

        // The matcher value must be preserved unchanged
        let entries = field.value.as_array().unwrap();
        let matcher = entries[0].get("matcher").and_then(|v| v.as_str()).unwrap();
        assert_eq!(
            matcher, "^Bash.*",
            "regex matcher must be passed through unchanged"
        );
    }

    /// gap 29/42: `hooks.command.args` must emit DiagLevel::Drop (not Warn).
    /// mappings/hooks.yaml declares `id: hooks.command.args` with `loss: dropped`.
    /// Spec §7 invariant #1: dropped entries must always be listed as dropped.
    #[test]
    fn test_hooks_lift_c2x_args_emits_drop_diagnostic() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "my-script",
                        "args": ["--flag"]
                    }]
                }]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        // Must emit exactly one diagnostic with id "hooks.command.args"
        let args_diags: Vec<_> = ir
            .diagnostics
            .iter()
            .filter(|d| d.id.as_deref() == Some("hooks.command.args"))
            .collect();
        assert!(
            !args_diags.is_empty(),
            "Expected a diagnostic with id 'hooks.command.args'"
        );

        // That diagnostic MUST be DiagLevel::Drop, not Warn
        for diag in &args_diags {
            assert_eq!(
                diag.level,
                DiagLevel::Drop,
                "hooks.command.args diagnostic must be DiagLevel::Drop (mappings: loss:dropped), got {:?}",
                diag.level
            );
        }
    }

    /// gap 29/42: build_report must place hooks.command.args in `dropped`, not `lossy`.
    #[test]
    fn test_hooks_args_in_report_dropped_not_lossy() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "my-script",
                        "args": ["--flag"]
                    }]
                }]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());
        let plan = h.lower_c2x(&ir, &opts).unwrap();

        let report = crate::core::report::build_report(&ir, &plan);

        // Must appear in dropped
        let in_dropped = report
            .dropped
            .iter()
            .any(|e| e.id.as_deref() == Some("hooks.command.args"));
        assert!(
            in_dropped,
            "hooks.command.args must appear in report.dropped; dropped={:?}",
            report
                .dropped
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );

        // Must NOT appear in lossy
        let in_lossy = report
            .lossy
            .iter()
            .any(|e| e.id.as_deref() == Some("hooks.command.args"));
        assert!(
            !in_lossy,
            "hooks.command.args must NOT appear in report.lossy; lossy={:?}",
            report
                .lossy
                .iter()
                .map(|e| e.id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_hooks_c2x_16430_warn_only_for_plugin_bundled() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": "echo ok" }]
                }]
            }
        });

        let h = make_handler();
        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());

        // Standalone hooks conversion: #16430 does not apply, so no warning.
        let ir = h.lift_c2x(&hooks_json).unwrap();
        let plan = h.lower_c2x(&ir, &opts).unwrap();
        assert!(
            !plan
                .diagnostics
                .iter()
                .any(|d| d.message.contains("#16430")),
            "standalone hooks must not emit the #16430 warning"
        );

        // Plugin-bundled hooks (source under .claude-plugin/): the warning applies.
        let mut plugin_ir = h.lift_c2x(&hooks_json).unwrap();
        plugin_ir.source_path = "myplugin/.claude-plugin/hooks/hooks.json".to_string();
        let plugin_plan = h.lower_c2x(&plugin_ir, &opts).unwrap();
        assert!(
            plugin_plan
                .diagnostics
                .iter()
                .any(|d| d.message.contains("#16430")),
            "plugin-bundled hooks must emit the #16430 warning"
        );
    }

    /// gap 41/42: when all hook items within an event entry are dropped (e.g. only
    /// `type:http`), the event field must be `Loss::Dropped`, not `Loss::Lossless`.
    #[test]
    fn test_hooks_lift_c2x_all_http_event_is_dropped() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "",
                    "hooks": [{ "type": "http", "url": "https://example.com" }]
                }]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir
            .fields
            .get("hooks.event.PostToolUse")
            .expect("hooks.event.PostToolUse must exist");
        assert_eq!(
            field.loss,
            Loss::Dropped,
            "Event with only http hooks must be Loss::Dropped, got {:?}",
            field.loss
        );
        assert!(field.dropped.is_some(), "dropped reason must be populated");
    }

    /// gap 41/42: a common event where at least one command hook survives must
    /// remain `Loss::Lossless` even if other hooks in the same entry are dropped.
    #[test]
    fn test_hooks_lift_c2x_mixed_hooks_event_is_lossless() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "",
                    "hooks": [
                        { "type": "http", "url": "https://example.com" },
                        { "type": "command", "command": "echo post" }
                    ]
                }]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir
            .fields
            .get("hooks.event.PostToolUse")
            .expect("hooks.event.PostToolUse must exist");
        assert_eq!(
            field.loss,
            Loss::Lossless,
            "Event where at least one hook survives must remain Loss::Lossless"
        );
    }
}
