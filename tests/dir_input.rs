//! Integration tests for directory-input mode (batch-flags gap fix).
//!
//! Spec §13: `<path>` accepts a file or directory (recursive detection).
//! Spec §5 pipeline: directory walk discovers all recognizable files and converts them.

use std::path::Path;

use ccx::core::{
    detect::detect_files, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use ccx::handlers::{pick_handler, EmitPlan, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

fn default_lower_opts(out_dir: &str) -> LowerOpts {
    LowerOpts {
        out: Some(out_dir.to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Auto,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    }
}

fn empty_plan() -> EmitPlan {
    EmitPlan {
        files: vec![],
        diagnostics: vec![],
    }
}

/// `detect_files` on a file returns a single-element list with the correct kind.
#[test]
fn test_detect_files_single_file() {
    use ccx::core::ir::Kind;
    let pairs = detect_files("tests/fixtures/claude/skills/deploy/SKILL.md")
        .expect("detect_files should succeed on a file");
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, Kind::Skill);
    assert_eq!(
        pairs[0].1,
        Path::new("tests/fixtures/claude/skills/deploy/SKILL.md")
    );
}

/// `detect_files` on a directory returns ALL recognizable files, not just the dominant kind.
#[test]
fn test_detect_files_directory_returns_all_kinds() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create .claude/skills/s/SKILL.md
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // Create .mcp.json
    std::fs::write(base.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    let pairs =
        detect_files(base.to_str().unwrap()).expect("detect_files should succeed on directory");

    // Must include both Skill and Mcp — not just the dominant one
    let kinds: Vec<&Kind> = pairs.iter().map(|(k, _)| k).collect();
    assert!(
        kinds.contains(&&Kind::Skill),
        "Expected Kind::Skill in pairs, got: {:?}",
        kinds
    );
    assert!(
        kinds.contains(&&Kind::Mcp),
        "Expected Kind::Mcp in pairs, got: {:?}",
        kinds
    );

    // Each pair must point to the actual file, not the directory
    for (_, path) in &pairs {
        assert!(
            path.is_file(),
            "Expected file path in pair, got directory: {}",
            path.display()
        );
    }
}

/// c2x on a directory converts every discovered file individually.
///
/// Repro from the bug report: `c2x /path/to/dir` previously crashed with
/// "Is a directory" because the handler received the directory path instead of
/// the individual file paths.
#[test]
fn test_c2x_directory_converts_all_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create .claude/skills/s/SKILL.md
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // Create .mcp.json
    std::fs::write(base.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let pairs = detect_files(base.to_str().unwrap()).expect("detect_files should succeed");

    let mut all_files: Vec<ccx::handlers::EmitFile> = Vec::new();
    let mut all_diags: Vec<ccx::core::ir::Diagnostic> = Vec::new();

    for (kind, file_path) in &pairs {
        let handler = pick_handler(kind, &maps);
        let parsed = handler
            .parse(file_path)
            .unwrap_or_else(|e| panic!("parse failed for {}: {}", file_path.display(), e));
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .unwrap_or_else(|e| panic!("lift failed for {}: {}", file_path.display(), e));
        let plan = handler
            .lower(&ir, ConvDir::C2x, &opts)
            .unwrap_or_else(|e| panic!("lower failed for {}: {}", file_path.display(), e));
        all_files.extend(plan.files);
        all_diags.extend(plan.diagnostics);
    }

    // Converted SKILL.md must be present
    let has_skill = all_files.iter().any(|f| f.path.ends_with("SKILL.md"));
    assert!(
        has_skill,
        "Expected converted SKILL.md in output, got: {:?}",
        all_files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    // Converted .mcp.json must be present
    let has_mcp = all_files.iter().any(|f| f.path.ends_with(".mcp.json"));
    assert!(
        has_mcp,
        "Expected converted .mcp.json in output, got: {:?}",
        all_files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// check on a directory succeeds and produces diagnostics for every file.
#[test]
fn test_check_directory_processes_all_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    // Create .claude/skills/s/SKILL.md
    let skill_dir = base.join(".claude").join("skills").join("s");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody",
    )
    .unwrap();

    // Create CLAUDE.md (memory file)
    std::fs::write(base.join("CLAUDE.md"), "# Project Instructions\nHello.").unwrap();

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let pairs = detect_files(base.to_str().unwrap()).expect("detect_files should succeed");

    assert!(
        pairs.len() >= 2,
        "Expected at least 2 files detected, got {}",
        pairs.len()
    );

    // Simulate run_check: parse + lift each file
    for (kind, file_path) in &pairs {
        let handler = pick_handler(kind, &maps);
        let parsed = handler
            .parse(file_path)
            .unwrap_or_else(|e| panic!("parse failed for {}: {}", file_path.display(), e));
        let ir = handler
            .lift(&parsed, ConvDir::C2x)
            .unwrap_or_else(|e| panic!("lift failed for {}: {}", file_path.display(), e));
        let _report = build_report(&ir, &empty_plan());
    }
}

/// `detect_files` on a file path returns the actual `PathBuf` for that file,
/// not the parent directory (regression guard).
#[test]
fn test_detect_files_file_path_is_exact() {
    let path = "tests/fixtures/claude/.mcp.json";
    let pairs = detect_files(path).expect("detect_files should succeed");
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].1,
        Path::new(path),
        "Expected path to be the exact file, not its parent"
    );
}

/// Plugin directory input: c2x on a directory containing .claude-plugin/plugin.json
/// must succeed — detect_files must return the plugin.json file, not the directory.
#[test]
fn test_c2x_plugin_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let plugin_dir = base.join(".claude-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"dir-plugin","version":"1.0.0","description":"Dir plugin test"}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on directory with .claude-plugin/plugin.json");

    // Must find the plugin.json file with Kind::Plugin
    let plugin_pair = pairs.iter().find(|(k, _)| *k == Kind::Plugin);
    assert!(
        plugin_pair.is_some(),
        "Expected Kind::Plugin in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, plugin_path) = plugin_pair.unwrap();
    assert!(
        plugin_path.is_file(),
        "Plugin path must point to a file, not a directory: {}",
        plugin_path.display()
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Plugin, &maps);
    let parsed = handler
        .parse(plugin_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", plugin_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", plugin_path.display(), e));
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", plugin_path.display(), e));

    let has_codex_manifest = plan
        .files
        .iter()
        .any(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
    assert!(
        has_codex_manifest,
        "Expected .codex-plugin/plugin.json in output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Plugin directory input: x2c on a directory containing .codex-plugin/plugin.json succeeds.
#[test]
fn test_x2c_plugin_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let plugin_dir = base.join(".codex-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"codex-dir-plugin","version":"1.0.0","description":"Codex dir plugin"}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on directory with .codex-plugin/plugin.json");

    let plugin_pair = pairs.iter().find(|(k, _)| *k == Kind::Plugin);
    assert!(plugin_pair.is_some(), "Expected Kind::Plugin in pairs");
    let (_, plugin_path) = plugin_pair.unwrap();
    assert!(plugin_path.is_file(), "Plugin path must be a file");

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Plugin, &maps);
    let parsed = handler
        .parse(plugin_path)
        .unwrap_or_else(|e| panic!("parse failed: {}", e));
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .unwrap_or_else(|e| panic!("lift failed: {}", e));
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .unwrap_or_else(|e| panic!("lower failed: {}", e));

    let has_claude_manifest = plan
        .files
        .iter()
        .any(|f| f.path.contains(".claude-plugin") && f.path.ends_with("plugin.json"));
    assert!(
        has_claude_manifest,
        "Expected .claude-plugin/plugin.json in x2c output, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Hooks directory input: c2x on a directory containing hooks.json succeeds.
#[test]
fn test_c2x_hooks_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    std::fs::write(
        base.join("hooks.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on hooks directory");

    let hooks_pair = pairs.iter().find(|(k, _)| *k == Kind::Hooks);
    assert!(
        hooks_pair.is_some(),
        "Expected Kind::Hooks in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, hooks_path) = hooks_pair.unwrap();
    assert!(hooks_path.is_file(), "Hooks path must point to a file");

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Hooks, &maps);
    let parsed = handler
        .parse(hooks_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", hooks_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", hooks_path.display(), e));
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", hooks_path.display(), e));

    // c2x hooks should produce a config.toml with hooks section
    let has_hooks_output = plan
        .files
        .iter()
        .any(|f| f.path.ends_with("config.toml") || f.path.ends_with("hooks.json"));
    assert!(
        has_hooks_output,
        "Expected hooks output file, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Settings directory input: c2x on a directory containing settings.json succeeds.
#[test]
fn test_c2x_settings_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    std::fs::write(
        base.join("settings.json"),
        r#"{"model":"claude-sonnet-4-6","env":{"RUST_LOG":"info"}}"#,
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on settings directory");

    let settings_pair = pairs.iter().find(|(k, _)| *k == Kind::Settings);
    assert!(
        settings_pair.is_some(),
        "Expected Kind::Settings in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, settings_path) = settings_pair.unwrap();
    assert!(
        settings_path.is_file(),
        "Settings path must point to a file"
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Settings, &maps);
    let parsed = handler
        .parse(settings_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", settings_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", settings_path.display(), e));
    let _plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", settings_path.display(), e));
}

/// Subagent directory input: c2x on a directory with .claude/agents/*.md succeeds.
#[test]
fn test_c2x_subagent_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let agents_dir = base.join(".claude").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("researcher.md"),
        "---\nname: researcher\ndescription: Research specialist\n---\nYou are a researcher.\n",
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on subagent directory");

    let subagent_pair = pairs.iter().find(|(k, _)| *k == Kind::Subagent);
    assert!(
        subagent_pair.is_some(),
        "Expected Kind::Subagent in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, subagent_path) = subagent_pair.unwrap();
    assert!(
        subagent_path.is_file(),
        "Subagent path must point to a file, not directory: {}",
        subagent_path.display()
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Subagent, &maps);
    let parsed = handler
        .parse(subagent_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", subagent_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", subagent_path.display(), e));
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", subagent_path.display(), e));

    // c2x subagent should produce a .toml file
    let has_toml = plan.files.iter().any(|f| f.path.ends_with(".toml"));
    assert!(
        has_toml,
        "Expected .toml output for subagent c2x, got: {:?}",
        plan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Memory directory input: c2x on a directory with CLAUDE.md succeeds.
#[test]
fn test_c2x_memory_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    std::fs::write(
        base.join("CLAUDE.md"),
        "# Project Instructions\n\nAlways use Rust.\n",
    )
    .unwrap();

    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on memory directory");

    let memory_pair = pairs.iter().find(|(k, _)| *k == Kind::Memory);
    assert!(
        memory_pair.is_some(),
        "Expected Kind::Memory in pairs, got: {:?}",
        pairs
            .iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect::<Vec<_>>()
    );
    let (_, memory_path) = memory_pair.unwrap();
    assert!(
        memory_path.is_file(),
        "Memory path must point to a file, not directory: {}",
        memory_path.display()
    );

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());

    let handler = pick_handler(&Kind::Memory, &maps);
    let parsed = handler
        .parse(memory_path)
        .unwrap_or_else(|e| panic!("parse failed for {}: {}", memory_path.display(), e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("lift failed for {}: {}", memory_path.display(), e));
    let _plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .unwrap_or_else(|e| panic!("lower failed for {}: {}", memory_path.display(), e));
}

/// check on a directory with a plugin file processes it without 'Is a directory' error.
#[test]
fn test_check_plugin_directory_input() {
    use ccx::core::ir::Kind;

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path();

    let plugin_dir = base.join(".claude-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"check-plugin","version":"1.0.0","description":"Check test plugin"}"#,
    )
    .unwrap();

    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let pairs = detect_files(base.to_str().unwrap())
        .expect("detect_files should succeed on plugin directory");

    let plugin_pair = pairs.iter().find(|(k, _)| *k == Kind::Plugin);
    assert!(plugin_pair.is_some(), "Expected Kind::Plugin");
    let (kind, file_path) = plugin_pair.unwrap();

    assert!(
        file_path.is_file(),
        "check must receive a file path, not a directory: {}",
        file_path.display()
    );

    let handler = pick_handler(kind, &maps);
    let parsed = handler
        .parse(file_path)
        .unwrap_or_else(|e| panic!("check parse failed: {}", e));
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .unwrap_or_else(|e| panic!("check lift failed: {}", e));
    let _report = build_report(&ir, &empty_plan());
}
