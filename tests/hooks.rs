mod common;
use common::*;

use std::path::Path;

use cxbridge::core::{
    detect::detect, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use cxbridge::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

/// hooks.json c2x: common events are converted; Claude-only events are dropped.
#[test]
fn test_hooks_c2x_basic() {
    let hooks_path = "tests/fixtures/claude/hooks.json";
    assert!(
        Path::new(hooks_path).exists(),
        "Fixture {} must exist",
        hooks_path
    );

    let maps = load_mappings();
    let kind = detect(hooks_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Hooks);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // Common events should be Lossless
    let pre_tool_use = ir.fields.get("hooks.event.PreToolUse");
    assert!(pre_tool_use.is_some(), "Expected PreToolUse field");
    assert_eq!(
        pre_tool_use.unwrap().loss,
        cxbridge::core::ir::Loss::Lossless,
        "PreToolUse should be lossless"
    );

    // Setup is Claude-only → Dropped
    let setup = ir.fields.get("hooks.event.Setup");
    assert!(setup.is_some(), "Expected Setup field");
    assert_eq!(
        setup.unwrap().loss,
        cxbridge::core::ir::Loss::Dropped,
        "Setup should be dropped"
    );

    // Report check
    let report = build_report(&ir, &empty_plan());
    assert!(
        !report.dropped.is_empty(),
        "Expected dropped events in report"
    );
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        drop_ids.contains(&"hooks.event.Setup"),
        "Expected Setup in dropped, got: {:?}",
        drop_ids
    );
    assert!(
        drop_ids.contains(&"hooks.event.Notification"),
        "Expected Notification in dropped, got: {:?}",
        drop_ids
    );
}

/// hooks.json c2x lower (user scope): hooks.json is generated and the matcher is normalized.
#[test]
fn test_hooks_c2x_lower_user_scope() {
    let hooks_path = "tests/fixtures/claude/hooks.json";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings();
    let kind = detect(hooks_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let hooks_file = plan.files.iter().find(|f| f.path.ends_with("hooks.json"));
    assert!(hooks_file.is_some(), "Expected hooks.json output");

    let content: serde_json::Value = serde_json::from_str(&hooks_file.unwrap().content).unwrap();

    // PreToolUse should be present with normalized matcher
    let pre_tool_entries = content["PreToolUse"].as_array();
    assert!(
        pre_tool_entries.is_some(),
        "Expected PreToolUse in hooks.json"
    );
    let first_entry = &pre_tool_entries.unwrap()[0];
    let matcher = first_entry["matcher"].as_str().unwrap_or("");
    assert_eq!(
        matcher, "^Bash$",
        "Expected normalized matcher ^Bash$, got: {}",
        matcher
    );

    // Setup should NOT be present (dropped)
    assert!(
        content.get("Setup").is_none(),
        "Setup event should not be in output"
    );

    // A standalone hooks.json is not plugin-bundled, so #16430 does not apply.
    let has_16430 = plan
        .diagnostics
        .iter()
        .any(|d| d.message.contains("#16430"));
    assert!(
        !has_16430,
        "standalone hooks.json must not emit the #16430 warning"
    );
}

/// hooks.json c2x lower (project scope): .codex/config.toml is generated.
#[test]
fn test_hooks_c2x_lower_project_scope() {
    let hooks_path = "tests/fixtures/claude/hooks.json";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings();
    let kind = detect(hooks_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::Project,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let config_file = plan.files.iter().find(|f| f.path.ends_with("config.toml"));
    assert!(config_file.is_some(), "Expected config.toml output");

    let content = &config_file.unwrap().content;
    assert!(
        content.contains("[[hooks.PreToolUse]]"),
        "Expected [[hooks.PreToolUse]] in config.toml, got: {}",
        content
    );
    assert!(
        !content.contains("[[hooks.Setup]]"),
        "Setup event should not be in config.toml"
    );
}

/// Hooks matcher normalization test (Edit|Write → ^(Edit|Write)$).
#[test]
fn test_hooks_c2x_matcher_alternation_normalized() {
    let hooks_json = serde_json::json!({
        "hooks": {
            "PostToolUse": [
                {
                    "matcher": "Edit|Write",
                    "hooks": [
                        { "type": "command", "command": "lint.sh" }
                    ]
                }
            ]
        }
    });

    // Use hooks handler directly (not via fixture)
    let maps = load_mappings();
    let handler = cxbridge::handlers::hooks::HooksHandler {
        map: maps["hooks"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&hooks_json, ConvDir::C2x).unwrap();

    let field = ir.fields.get("hooks.event.PostToolUse").unwrap();
    let entries = field.value.as_array().unwrap();
    let matcher = entries[0]["matcher"].as_str().unwrap();
    assert_eq!(
        matcher, "^(Edit|Write)$",
        "Expected alternation matcher normalized"
    );
    let has_lossy_warn = ir
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Edit|Write"));
    assert!(
        has_lossy_warn,
        "Expected matcher normalization warn diagnostic"
    );
}

/// x2c: flat Codex hooks.json produces IR events correctly
/// and no silently lost hooks.
#[test]
fn test_hooks_x2c_flat_json_produces_events() {
    let hooks_path = "tests/fixtures/codex/hooks.json";
    assert!(
        Path::new(hooks_path).exists(),
        "Fixture {} must exist",
        hooks_path
    );

    let maps = load_mappings();
    let kind = detect(hooks_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Hooks);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // Both PreToolUse and Stop are common events → must be Lossless
    let pre_tool = ir.fields.get("hooks.event.PreToolUse");
    assert!(
        pre_tool.is_some(),
        "Expected hooks.event.PreToolUse in IR, but fields were: {:?}",
        ir.fields.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        pre_tool.unwrap().loss,
        cxbridge::core::ir::Loss::Lossless,
        "PreToolUse must be Lossless in x2c"
    );

    let stop = ir.fields.get("hooks.event.Stop");
    assert!(stop.is_some(), "Expected hooks.event.Stop in IR");
    assert_eq!(
        stop.unwrap().loss,
        cxbridge::core::ir::Loss::Lossless,
        "Stop must be Lossless in x2c"
    );

    // Report must show lossless entries (not all empty)
    let report = build_report(&ir, &empty_plan());
    assert!(
        !report.lossless.is_empty(),
        "Report lossless must be non-empty; hooks were silently lost. lossless={:?}",
        report.lossless
    );
    assert!(
        report
            .lossless
            .contains(&"hooks.event.PreToolUse".to_string()),
        "hooks.event.PreToolUse must be in lossless report"
    );
}

/// x2c lower from a flat Codex hooks.json must produce a Claude hooks.json
/// with a non-empty "hooks" object containing the converted events.
#[test]
fn test_hooks_x2c_flat_json_lower_produces_hooks_json() {
    let hooks_path = "tests/fixtures/codex/hooks.json";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(hooks_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(hooks_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let hooks_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("hooks.json"))
        .expect("Expected hooks.json in output");

    let content: serde_json::Value =
        serde_json::from_str(&hooks_file.content).expect("output must be valid JSON");

    // The output wraps events under a "hooks" key
    let hooks_obj = content
        .get("hooks")
        .and_then(|v| v.as_object())
        .expect("Expected 'hooks' object in output Claude hooks.json");

    assert!(
        !hooks_obj.is_empty(),
        "hooks object must not be empty — all events were silently lost. content={}",
        hooks_file.content
    );
    assert!(
        hooks_obj.contains_key("PreToolUse"),
        "Expected PreToolUse in output hooks, got keys: {:?}",
        hooks_obj.keys().collect::<Vec<_>>()
    );
    assert!(
        hooks_obj.contains_key("Stop"),
        "Expected Stop in output hooks, got keys: {:?}",
        hooks_obj.keys().collect::<Vec<_>>()
    );
}

/// A single CLAUDE_ONLY_EVENT drop must produce exactly one dropped entry in
/// the report, not two or three.
#[test]
fn test_hooks_c2x_claude_only_event_dropped_exactly_once() {
    use cxbridge::handlers::hooks::HooksHandler;
    use cxbridge::handlers::Handler;

    let hooks_json = serde_json::json!({
        "hooks": {
            "Notification": [
                {
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": "notify" }]
                }
            ]
        }
    });

    let maps = load_mappings();
    let handler = HooksHandler {
        map: maps["hooks"].clone(),
    };

    let ir = handler.lift(&hooks_json, ConvDir::C2x).unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();
    let report = build_report(&ir, &plan);

    // Filter to Notification-related dropped entries only (exclude #16430 etc.)
    let notification_drops: Vec<_> = report
        .dropped
        .iter()
        .filter(|d| {
            d.id.as_deref()
                .map(|id| id.contains("Notification"))
                .unwrap_or(false)
                || d.message.contains("Notification")
        })
        .collect();

    assert_eq!(
        notification_drops.len(),
        1,
        "Expected exactly 1 dropped entry for Notification, got {}: {:?}",
        notification_drops.len(),
        notification_drops
            .iter()
            .map(|d| (&d.id, &d.message))
            .collect::<Vec<_>>()
    );
}

/// A single exact-matcher normalization on a common event must produce exactly
/// one lossy entry (the matcher warn), excluding the #16430 warning.
#[test]
fn test_hooks_c2x_exact_matcher_lossy_exactly_once() {
    use cxbridge::handlers::hooks::HooksHandler;
    use cxbridge::handlers::Handler;

    let hooks_json = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": "echo pre" }]
                }
            ]
        }
    });

    let maps = load_mappings();
    let handler = HooksHandler {
        map: maps["hooks"].clone(),
    };

    let ir = handler.lift(&hooks_json, ConvDir::C2x).unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();
    let report = build_report(&ir, &plan);

    // Matcher-normalization warnings: exclude #16430 (plugin-bundled hooks warn)
    let matcher_lossy: Vec<_> = report
        .lossy
        .iter()
        .filter(|d| !d.message.contains("#16430"))
        .collect();

    assert_eq!(
        matcher_lossy.len(),
        1,
        "Expected exactly 1 lossy entry for matcher normalization (excl. #16430), got {}: {:?}",
        matcher_lossy.len(),
        matcher_lossy
            .iter()
            .map(|d| (&d.id, &d.message))
            .collect::<Vec<_>>()
    );
}

/// End-to-end: a PostToolUse event containing only `type:http` hooks must NOT
/// be listed in `report.lossless` after c2x conversion.
///
/// When all hook items are dropped (http has no Codex equivalent), the event's
/// semantic content is entirely lost. The field must be classified as
/// `Loss::Dropped` so `build_report` routes it to `dropped`, not `lossless`.
#[test]
fn test_event_with_all_hooks_dropped_not_in_lossless() {
    let fixture = Path::new("tests/fixtures/claude/hooks_all_http_dropped/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Hooks, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // The event field must be classified as Dropped (not Lossless)
    let field = ir
        .fields
        .get("hooks.event.PostToolUse")
        .expect("hooks.event.PostToolUse must exist in IR");
    assert_eq!(
        field.loss,
        cxbridge::core::ir::Loss::Dropped,
        "hooks.event.PostToolUse must be Loss::Dropped when all hooks are dropped; got {:?}",
        field.loss
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // Must NOT appear in lossless
    let in_lossless = report
        .lossless
        .iter()
        .any(|id| id == "hooks.event.PostToolUse");
    assert!(
        !in_lossless,
        "hooks.event.PostToolUse must NOT be in report.lossless when all hooks are dropped; \
         lossless={:?}",
        report.lossless
    );

    // Must appear in dropped
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.event.PostToolUse"));
    assert!(
        in_dropped,
        "hooks.event.PostToolUse must appear in report.dropped; dropped={:?}",
        report
            .dropped
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// Verify that when at least one hook survives (command type), the event remains
/// lossless — only the all-dropped case triggers the dropped classification.
#[test]
fn test_event_with_surviving_hook_remains_lossless() {
    let fixture = Path::new("tests/fixtures/claude/hooks_wildcard/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Hooks, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // Stop has a command hook → must remain lossless
    let field = ir
        .fields
        .get("hooks.event.Stop")
        .expect("hooks.event.Stop must exist in IR");
    assert_eq!(
        field.loss,
        cxbridge::core::ir::Loss::Lossless,
        "hooks.event.Stop must remain Loss::Lossless when command hooks survive"
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // The surviving event must appear in report.lossless
    let in_lossless = report.lossless.iter().any(|id| id == "hooks.event.Stop");
    assert!(
        in_lossless,
        "hooks.event.Stop must appear in report.lossless when command hooks survive; \
         lossless={:?}",
        report.lossless
    );
}

/// End-to-end: converting a hooks.json with `args` must place `hooks.command.args`
/// in the `dropped` section of the report, not in `lossy`.
///
/// mappings/hooks.yaml: `id: hooks.command.args` with `loss: dropped`.
/// The args are synthesized into `command` (shell-escaped), then dropped.
/// DiagLevel must be Drop so build_report routes it to `dropped`, not `lossy`.
#[test]
fn test_hooks_args_report_section_is_dropped() {
    let fixture = Path::new("tests/fixtures/claude/hooks_args_drop/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Hooks, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // The diagnostic for hooks.command.args must have DiagLevel::Drop
    let args_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.command.args"))
        .collect();
    assert!(
        !args_diags.is_empty(),
        "Expected a diagnostic with id 'hooks.command.args'"
    );
    for diag in &args_diags {
        assert_eq!(
            diag.level,
            cxbridge::core::ir::DiagLevel::Drop,
            "hooks.command.args diagnostic must be DiagLevel::Drop, got {:?}: {}",
            diag.level,
            diag.message
        );
    }

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // Must appear in dropped
    let in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.command.args"));
    assert!(
        in_dropped,
        "hooks.command.args must be in report.dropped; dropped={:?}",
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
        "hooks.command.args must NOT be in report.lossy; lossy={:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// End-to-end: converting a hooks.json with a regex matcher ("^Bash.*") must
/// NOT emit any diagnostic with id "hooks.matcher.regex" (loss:lossless, warn:false).
/// The event must appear in the lossless section of the report, not the lossy section.
#[test]
fn test_hooks_regex_matcher_no_warn_e2e() {
    let fixture = Path::new("tests/fixtures/claude/hooks_regex_matcher/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Hooks, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // Must NOT have any "hooks.matcher.regex" Warn diagnostic
    let regex_warn_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| {
            d.id.as_deref() == Some("hooks.matcher.regex")
                && d.level == cxbridge::core::ir::DiagLevel::Warn
        })
        .collect();
    assert!(
        regex_warn_diags.is_empty(),
        "Expected NO 'hooks.matcher.regex' Warn diagnostics for regex passthrough, got: {:?}",
        regex_warn_diags
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // hooks.event.PreToolUse must appear in lossless, not in lossy
    let in_lossless = report
        .lossless
        .iter()
        .any(|s| s == "hooks.event.PreToolUse");
    assert!(
        in_lossless,
        "hooks.event.PreToolUse must be in lossless section; lossless={:?}, lossy={:?}",
        report.lossless,
        report.lossy.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    // hooks.matcher.regex must NOT appear in the lossy section of the report
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("hooks.matcher.regex"));
    assert!(
        !in_lossy,
        "hooks.matcher.regex must NOT be in lossy section; lossy={:?}",
        report.lossy.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

/// End-to-end: converting a hooks.json with wildcard matchers ("*" and "")
/// must produce IR diagnostics with id "hooks.matcher.wildcard" and must NOT
/// produce any diagnostic with id "hooks.matcher.exact".
#[test]
fn test_hooks_wildcard_matcher_id_e2e() {
    let fixture = Path::new("tests/fixtures/claude/hooks_wildcard/hooks.json");
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&cxbridge::core::ir::Kind::Hooks, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    // There must be at least one "hooks.matcher.wildcard" diagnostic
    let wildcard_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.matcher.wildcard"))
        .collect();
    assert!(
        !wildcard_diags.is_empty(),
        "Expected at least one diagnostic with id 'hooks.matcher.wildcard', got: {:?}",
        ir.diagnostics
            .iter()
            .map(|d| d.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );

    // Must NOT have any "hooks.matcher.exact" diagnostics (both matchers are wildcards)
    let exact_diags: Vec<_> = ir
        .diagnostics
        .iter()
        .filter(|d| d.id.as_deref() == Some("hooks.matcher.exact"))
        .collect();
    assert!(
        exact_diags.is_empty(),
        "Expected NO 'hooks.matcher.exact' diagnostics for wildcard-only fixture, got: {:?}",
        exact_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    // Both wildcard matchers ("*" and "") should each produce a wildcard diagnostic
    assert_eq!(
        wildcard_diags.len(),
        2,
        "Expected 2 wildcard diagnostics (one for '*', one for ''), got: {:?}",
        wildcard_diags
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    // Smoke-check: hooks.json must be emitted
    assert!(
        plan.files.iter().any(|f| f.path.ends_with("hooks.json")),
        "Expected hooks.json in output files"
    );
}
