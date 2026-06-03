use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

use crate::core::{
    detect::detect_files,
    ir::Kind,
    mappings::load_mappings,
    report::{build_report, print_report},
    transforms::ConvDir,
};
use crate::handlers::{pick_handler, EmitPlan, LowerOpts, Scope, SkillTargetMode};

#[derive(Parser)]
#[command(
    name = "cxbridge",
    version,
    about = "Claude Code ⇄ Codex config file bidirectional conversion CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Claude → Codex conversion
    C2x {
        /// Path to convert (file or directory)
        path: String,
        #[command(flatten)]
        opts: ConvertOpts,
    },
    /// Codex → Claude conversion
    X2c {
        /// Path to convert (file or directory)
        path: String,
        #[command(flatten)]
        opts: ConvertOpts,
    },
    /// Pre-conversion diagnostic (no writes)
    Check {
        /// Path to diagnose (file or directory)
        path: String,
    },
}

/// Conversion options (shared by c2x / x2c).
#[derive(Parser, Debug, Clone)]
pub struct ConvertOpts {
    /// Output directory (default: *.converted/ subdirectory)
    #[arg(long)]
    pub out: Option<String>,

    /// Limit conversion to specific domains, comma-separated (e.g. skills,mcp)
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,

    /// Scope for degraded outputs (.rules / agents placement). Default: project
    #[arg(long)]
    pub scope: Option<String>,

    /// Skill conversion target (auto|skill|subagent). Default: auto
    #[arg(long)]
    pub skill_target: Option<String>,

    /// Prompt interactively on TTY for ambiguous cases
    #[arg(long)]
    pub interactive: bool,

    /// Auto-rewrite body variables/syntax (default: detect only)
    #[arg(long)]
    pub rewrite_body: bool,

    /// Keep .claude-plugin/ and also generate .codex-plugin/ for plugins
    #[arg(long)]
    pub dual_manifest: bool,

    /// Destination for hooks output (user|project). Default: user
    #[arg(long)]
    pub hooks_target: Option<String>,

    /// Print detailed report (--report=json for machine-readable output)
    #[arg(long)]
    pub report: Option<Option<String>>,

    /// Print report only without writing files
    #[arg(long)]
    pub dry_run: bool,

    /// Exit with non-zero status if any fields are dropped (for CI)
    #[arg(long)]
    pub strict: bool,

    /// Preserve Claude-specific frontmatter keys in output
    #[arg(long)]
    pub keep_claude_frontmatter: bool,

    /// Allow overwriting existing files
    #[arg(long)]
    pub force: bool,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::C2x { path, opts } => run_convert(ConvDir::C2x, &path, &opts),
        Commands::X2c { path, opts } => run_convert(ConvDir::X2c, &path, &opts),
        Commands::Check { path } => run_check(&path),
    }
}

/// Report-header label for a converted file: its path relative to the input root, else the file name.
fn display_source(file_path: &Path, input_root: &str) -> String {
    if let Ok(rel) = file_path.strip_prefix(input_root) {
        if !rel.as_os_str().is_empty() {
            return rel.display().to_string();
        }
    }
    file_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| file_path.display().to_string())
}

/// Computes the spec-mandated default output root when `--out` is not provided.
///
/// - Skill file (`SKILL.md`): `<parent_dir>.converted` (skill dir + `.converted`).
/// - Skill directory: `<path>.converted`.
/// - `.mcp.json` file: `<parent_dir>/<stem>.converted` (e.g. `.mcp.converted`).
/// - Any other directory (project root): `<path>/.codex-converted`.
pub fn default_out_dir(path: &str, kind: &Kind) -> String {
    let p = Path::new(path);
    let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");

    match kind {
        Kind::Skill => {
            // If the path points to a SKILL.md file, use its parent as the skill dir.
            // If it points to a skill directory directly, use the path itself.
            if file_name == "SKILL.md" {
                let skill_dir = p.parent().unwrap_or(p);
                format!("{}.converted", skill_dir.display())
            } else {
                format!("{}.converted", p.display())
            }
        }
        Kind::Mcp if file_name.ends_with(".json") => {
            // Derive stem by stripping the last extension; ".mcp.json" → ".mcp".
            let parent = p.parent().unwrap_or(Path::new("."));
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(".mcp");
            format!("{}/{}.converted", parent.display(), stem)
        }
        _ => {
            // Project root or any other directory input → .codex-converted inside it.
            // Use the path as-is (whether a dir or a file's parent).
            if p.extension().is_some() {
                // Looks like a file path — use its parent directory.
                let parent = p.parent().unwrap_or(Path::new("."));
                format!("{}/.codex-converted", parent.display())
            } else {
                format!("{}/.codex-converted", p.display())
            }
        }
    }
}

fn run_convert(dir: ConvDir, path: &str, opts: &ConvertOpts) -> anyhow::Result<()> {
    let maps = load_mappings();

    let pairs = detect_files(path)?;

    // Compute the spec-mandated default output root when --out is omitted.
    // For directory inputs the output is always <path>/.codex-converted.
    // For single-file inputs, use the per-kind naming from default_out_dir.
    let effective_out: Option<String> = opts.out.clone().or_else(|| {
        if Path::new(path).is_dir() {
            Some(format!("{}/.codex-converted", path))
        } else {
            pairs.first().map(|(kind, _)| default_out_dir(path, kind))
        }
    });
    let mut resolved_opts = opts.clone();
    resolved_opts.out = effective_out;

    let lower_opts = build_lower_opts(&resolved_opts);

    let mut combined_files = Vec::new();
    let mut combined_diags = Vec::new();
    let mut total_dropped = 0usize;

    for (kind, file_path) in &pairs {
        // Apply domain filter: skip files whose domain is not in the allow-list.
        if !lower_opts.only.is_empty() {
            let domain = kind.domain_name();
            if !lower_opts.only.iter().any(|d| d.as_str() == domain) {
                continue;
            }
        }

        let handler = pick_handler(kind, &maps);
        let parsed = match handler.parse(file_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping {}: {}", file_path.display(), e);
                continue;
            }
        };
        let ir = match handler.lift(&parsed, dir) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping {}: {}", file_path.display(), e);
                continue;
            }
        };
        let plan = match handler.lower(&ir, dir, &lower_opts) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping {}: {}", file_path.display(), e);
                continue;
            }
        };
        let report = build_report(&ir, &plan);
        total_dropped += report.dropped.len();
        // Print the report when explicitly requested, or on --dry-run (whose sole
        // purpose is to show the report without writing).
        if opts.report.is_some() || opts.dry_run {
            let report_fmt = opts.report.as_ref().and_then(|f| f.as_deref());
            let source = display_source(file_path, path);
            print_report(&report, report_fmt, &source, kind.domain_name());
        }
        combined_files.extend(plan.files);
        combined_diags.extend(plan.diagnostics);
    }

    let combined_plan = EmitPlan {
        files: combined_files,
        diagnostics: combined_diags,
    };

    if !opts.dry_run {
        write_plan(&combined_plan, opts.force)?;
    }

    if opts.strict && total_dropped > 0 {
        std::process::exit(2);
    }
    Ok(())
}

/// Infers the conversion direction from the source path.
///
/// Returns `ConvDir::X2c` for Codex-origin files and directories
/// (`config.toml`, `AGENTS.md`, `AGENTS.override.md`, paths under `.agents/`
/// or `.codex/`). Returns `ConvDir::C2x` for everything else.
pub fn infer_conv_dir(path: &str) -> ConvDir {
    let p = Path::new(path);
    let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Match Codex-origin directories by path component so relative paths like
    // `.agents/skills/x/SKILL.md` (no leading slash) are still recognised.
    let under_codex_dir = p.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".agents") | Some(".codex") | Some(".codex-plugin")
        )
    });

    if matches!(
        file_name,
        "config.toml" | "AGENTS.md" | "AGENTS.override.md"
    ) || under_codex_dir
    {
        ConvDir::X2c
    } else {
        ConvDir::C2x
    }
}

/// check subcommand: reports dropped field counts without writing any files.
fn run_check(path: &str) -> anyhow::Result<()> {
    let maps = load_mappings();

    let pairs = detect_files(path)?;

    for (kind, file_path) in &pairs {
        // Infer direction from the individual file path so that Codex-origin
        // files are lifted with X2c and their dropped fields are reported.
        let dir = infer_conv_dir(file_path.to_str().unwrap_or(""));
        let handler = pick_handler(kind, &maps);
        let parsed = match handler.parse(file_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping {}: {}", file_path.display(), e);
                continue;
            }
        };
        let ir = match handler.lift(&parsed, dir) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: skipping {}: {}", file_path.display(), e);
                continue;
            }
        };

        let empty_plan = EmitPlan {
            files: vec![],
            diagnostics: vec![],
        };
        let report = build_report(&ir, &empty_plan);

        println!("check: {}", file_path.display());
        println!(
            "  dropped: {}, degraded: {}, lossy: {}, lossless: {}",
            report.dropped.len(),
            report.degraded.len(),
            report.lossy.len(),
            report.lossless.len()
        );

        if !report.dropped.is_empty() {
            println!("  Dropped fields:");
            for d in &report.dropped {
                let id = d.id.as_deref().unwrap_or("?");
                println!("    - {} : {}", id, d.message);
            }
        }

        if !report.body_warnings.is_empty() {
            println!("  Body warnings: {}", report.body_warnings.len());
        }
    }

    Ok(())
}

fn build_lower_opts(opts: &ConvertOpts) -> LowerOpts {
    LowerOpts {
        out: opts.out.clone(),
        only: opts.only.clone(),
        scope: opts
            .scope
            .as_deref()
            .map(parse_scope)
            .unwrap_or(Scope::Project),
        dual_manifest: opts.dual_manifest,
        hooks_target: opts
            .hooks_target
            .as_deref()
            .map(parse_scope)
            .unwrap_or(Scope::User),
        skill_target: opts
            .skill_target
            .as_deref()
            .map(parse_skill_target_mode)
            .unwrap_or(SkillTargetMode::Auto),
        interactive: opts.interactive,
        rewrite_body: opts.rewrite_body,
        keep_claude_frontmatter: opts.keep_claude_frontmatter,
    }
}

/// Append target kind for `config.toml` / `.rules`, which are merged into an
/// existing destination rather than overwritten.
enum AppendTarget {
    /// `config.toml`: additive TOML merge (existing keys win).
    ConfigToml,
    /// `*.rules`: concatenate, skipping content already present.
    Rules,
}

fn append_target(dest: &Path) -> Option<AppendTarget> {
    let name = dest.file_name()?.to_str()?;
    if name == "config.toml" {
        Some(AppendTarget::ConfigToml)
    } else if name.ends_with(".rules") {
        Some(AppendTarget::Rules)
    } else {
        None
    }
}

/// Writes every file in the plan, creating parent directories.
///
/// `config.toml` and `.rules` are append targets: they are merged into an
/// existing destination (additively for TOML, by concatenation for rules) so
/// that converting several skills into one output tree, or converting into an
/// existing Codex project, never clobbers prior `[agents.*]`/`[features]`
/// entries. Other files are protected: an existing destination is refused
/// unless `force` is set.
pub fn write_plan(plan: &EmitPlan, force: bool) -> anyhow::Result<()> {
    for file in &plan.files {
        let dest = PathBuf::from(&file.path);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let content = match append_target(&dest) {
            Some(target) if dest.exists() => {
                let existing = std::fs::read_to_string(&dest)
                    .with_context(|| format!("Failed to read file: {}", dest.display()))?;
                match target {
                    AppendTarget::ConfigToml => merge_config_toml(&existing, &file.content)
                        .with_context(|| format!("Failed to merge TOML into {}", dest.display()))?,
                    AppendTarget::Rules => append_rules(&existing, &file.content),
                }
            }
            None if dest.exists() && !force => {
                anyhow::bail!(
                    "Output file already exists (use --force to overwrite): {}",
                    dest.display()
                );
            }
            _ => file.content.clone(),
        };

        std::fs::write(&dest, &content)
            .with_context(|| format!("Failed to write file: {}", dest.display()))?;
    }
    Ok(())
}

/// Additively merges `addition` into `existing` TOML: tables are merged
/// recursively and missing keys are inserted, but keys already present in
/// `existing` are kept (existing config wins). Formatting of `existing` is
/// preserved via `toml_edit`.
fn merge_config_toml(existing: &str, addition: &str) -> anyhow::Result<String> {
    use toml_edit::DocumentMut;

    let mut base: DocumentMut = existing
        .parse()
        .context("existing config.toml is not valid TOML")?;
    let add: DocumentMut = addition
        .parse()
        .context("generated config.toml is not valid TOML")?;

    merge_tables(base.as_table_mut(), add.as_table());
    Ok(base.to_string())
}

fn merge_tables(base: &mut toml_edit::Table, addition: &toml_edit::Table) {
    for (key, add_item) in addition.iter() {
        match base.get_mut(key) {
            None => {
                base.insert(key, add_item.clone());
            }
            Some(base_item) => {
                if let (Some(base_tbl), Some(add_tbl)) =
                    (base_item.as_table_mut(), add_item.as_table())
                {
                    merge_tables(base_tbl, add_tbl);
                }
                // Otherwise the key already exists as a value: keep existing.
            }
        }
    }
}

/// Concatenates `addition` onto `existing` rules, skipping it when already
/// present so repeated conversions stay idempotent.
fn append_rules(existing: &str, addition: &str) -> String {
    if existing.contains(addition.trim()) {
        return existing.to_string();
    }
    let mut out = existing.trim_end().to_string();
    out.push('\n');
    out.push_str(addition.trim_end());
    out.push('\n');
    out
}

fn parse_scope(s: &str) -> Scope {
    match s {
        "user" => Scope::User,
        _ => Scope::Project,
    }
}

fn parse_skill_target_mode(s: &str) -> SkillTargetMode {
    match s {
        "skill" => SkillTargetMode::Skill,
        "subagent" => SkillTargetMode::Subagent,
        _ => SkillTargetMode::Auto,
    }
}

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::Kind;
    use crate::handlers::{EmitFile, EmitPlan};
    use tempfile::TempDir;

    #[test]
    fn test_infer_conv_dir_codex_files() {
        assert_eq!(infer_conv_dir("config.toml"), ConvDir::X2c);
        assert_eq!(infer_conv_dir("/some/path/config.toml"), ConvDir::X2c);
        assert_eq!(infer_conv_dir("AGENTS.md"), ConvDir::X2c);
        assert_eq!(infer_conv_dir("/project/AGENTS.md"), ConvDir::X2c);
        assert_eq!(infer_conv_dir("AGENTS.override.md"), ConvDir::X2c);
        assert_eq!(
            infer_conv_dir("/project/.agents/skills/foo/SKILL.md"),
            ConvDir::X2c
        );
        assert_eq!(
            infer_conv_dir("/project/.codex/agents/foo.toml"),
            ConvDir::X2c
        );
    }

    #[test]
    fn test_infer_conv_dir_claude_files() {
        assert_eq!(infer_conv_dir("CLAUDE.md"), ConvDir::C2x);
        assert_eq!(infer_conv_dir(".mcp.json"), ConvDir::C2x);
        assert_eq!(infer_conv_dir("settings.json"), ConvDir::C2x);
        assert_eq!(
            infer_conv_dir("/project/.claude/skills/foo/SKILL.md"),
            ConvDir::C2x
        );
        assert_eq!(infer_conv_dir("hooks.json"), ConvDir::C2x);
    }

    // ── default_out_dir ──────────────────────────────────────────────────────

    #[test]
    fn default_out_dir_skill_file_uses_parent_dir() {
        // SKILL.md → parent dir + ".converted"
        assert_eq!(
            default_out_dir("/project/.claude/skills/deploy/SKILL.md", &Kind::Skill),
            "/project/.claude/skills/deploy.converted"
        );
    }

    #[test]
    fn default_out_dir_skill_directory_appends_converted() {
        // directory path (no SKILL.md tail) → path + ".converted"
        assert_eq!(
            default_out_dir("/project/.claude/skills/deploy", &Kind::Skill),
            "/project/.claude/skills/deploy.converted"
        );
    }

    #[test]
    fn default_out_dir_mcp_json_file_uses_stem() {
        // ".mcp.json" → parent/<stem>.converted (stem = ".mcp" after stripping ".json")
        assert_eq!(
            default_out_dir("/project/.mcp.json", &Kind::Mcp),
            "/project/.mcp.converted"
        );
    }

    #[test]
    fn default_out_dir_project_root_directory() {
        // Project root directory → <path>/.codex-converted
        assert_eq!(
            default_out_dir("/project", &Kind::Settings),
            "/project/.codex-converted"
        );
    }

    #[test]
    fn default_out_dir_project_file_uses_parent() {
        // A file with an extension that is not Mcp + not Skill → parent/.codex-converted
        assert_eq!(
            default_out_dir("/project/settings.json", &Kind::Settings),
            "/project/.codex-converted"
        );
    }

    // ── merge_config_toml ────────────────────────────────────────────────────

    #[test]
    fn merge_config_toml_existing_key_is_kept() {
        let existing = "[features]\nweb_search = false\n";
        let addition = "[features]\nweb_search = true\n";
        let merged = merge_config_toml(existing, addition).unwrap();
        // The existing value must win.
        assert!(
            merged.contains("web_search = false"),
            "existing key must not be overwritten; got:\n{merged}"
        );
        assert!(
            !merged.contains("web_search = true"),
            "addition value must not appear; got:\n{merged}"
        );
    }

    #[test]
    fn merge_config_toml_new_key_is_inserted() {
        let existing = "[features]\nweb_search = true\n";
        let addition = "[features]\ncode_search = true\n";
        let merged = merge_config_toml(existing, addition).unwrap();
        // Both keys must be present.
        assert!(
            merged.contains("web_search = true"),
            "original key must be preserved; got:\n{merged}"
        );
        assert!(
            merged.contains("code_search = true"),
            "new key from addition must be inserted; got:\n{merged}"
        );
    }

    #[test]
    fn merge_config_toml_nested_tables_merged() {
        // The existing table has [permissions.deploy]; the addition carries
        // [permissions.build] which is absent in existing. Both must appear.
        let existing = "[permissions.deploy]\nnetwork = true\n";
        let addition = "[permissions.build]\nnetwork = true\n";
        let merged = merge_config_toml(existing, addition).unwrap();

        assert!(
            merged.contains("network = true"),
            "existing nested key must be preserved; got:\n{merged}"
        );
        // The addition's [permissions.build] table must be inserted.
        assert!(
            merged.contains("build"),
            "new nested table from addition must be inserted; got:\n{merged}"
        );
    }

    #[test]
    fn merge_config_toml_nested_existing_key_wins() {
        // Nested key conflict: existing value must survive.
        let existing = "[agents.deploy]\nmodel = \"high\"\n";
        let addition = "[agents.deploy]\nmodel = \"low\"\n";
        let merged = merge_config_toml(existing, addition).unwrap();

        assert!(
            merged.contains("model = \"high\""),
            "existing nested key must win; got:\n{merged}"
        );
        assert!(
            !merged.contains("model = \"low\""),
            "addition nested value must not appear; got:\n{merged}"
        );
    }

    #[test]
    fn merge_config_toml_invalid_toml_returns_error() {
        let result = merge_config_toml("not = valid = toml", "[ok]\nk = 1\n");
        assert!(
            result.is_err(),
            "invalid existing TOML must return an error"
        );
    }

    // ── write_plan ───────────────────────────────────────────────────────────

    #[test]
    fn write_plan_creates_file_with_correct_content() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("out").join("SKILL.md");
        let plan = EmitPlan {
            files: vec![EmitFile {
                path: dest.to_str().unwrap().to_string(),
                content: "---\nname: test\n---\nBody.\n".to_string(),
            }],
            diagnostics: vec![],
        };

        write_plan(&plan, false).unwrap();

        let written = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(written, "---\nname: test\n---\nBody.\n");
    }

    #[test]
    fn write_plan_refuses_overwrite_without_force() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("SKILL.md");
        std::fs::write(&dest, "original content\n").unwrap();

        let plan = EmitPlan {
            files: vec![EmitFile {
                path: dest.to_str().unwrap().to_string(),
                content: "new content\n".to_string(),
            }],
            diagnostics: vec![],
        };

        let result = write_plan(&plan, false);
        assert!(
            result.is_err(),
            "write_plan must fail when file exists and force=false"
        );
        // Original content must be intact.
        let still_original = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(still_original, "original content\n");
    }

    #[test]
    fn write_plan_overwrites_with_force() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("output.toml");
        std::fs::write(&dest, "old = true\n").unwrap();

        let plan = EmitPlan {
            files: vec![EmitFile {
                path: dest.to_str().unwrap().to_string(),
                content: "new = true\n".to_string(),
            }],
            diagnostics: vec![],
        };

        write_plan(&plan, true).unwrap();

        let written = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(written, "new = true\n");
    }

    #[test]
    fn write_plan_config_toml_merges_additively() {
        // When a config.toml already exists, write_plan must merge rather than
        // overwrite, even without --force.
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("config.toml");
        std::fs::write(&dest, "[features]\nweb_search = false\n").unwrap();

        let plan = EmitPlan {
            files: vec![EmitFile {
                path: dest.to_str().unwrap().to_string(),
                content: "[features]\ncode_search = true\n".to_string(),
            }],
            diagnostics: vec![],
        };

        write_plan(&plan, false).unwrap();

        let written = std::fs::read_to_string(&dest).unwrap();
        // Existing key preserved, new key inserted.
        assert!(
            written.contains("web_search = false"),
            "existing key must survive merge; got:\n{written}"
        );
        assert!(
            written.contains("code_search = true"),
            "new key from addition must be inserted; got:\n{written}"
        );
    }

    #[test]
    fn write_plan_rules_file_appended_not_overwritten() {
        // .rules files must be concatenated, not replaced.
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("project.rules");
        std::fs::write(&dest, "existing rule\n").unwrap();

        let plan = EmitPlan {
            files: vec![EmitFile {
                path: dest.to_str().unwrap().to_string(),
                content: "new rule\n".to_string(),
            }],
            diagnostics: vec![],
        };

        write_plan(&plan, false).unwrap();

        let written = std::fs::read_to_string(&dest).unwrap();
        assert!(
            written.contains("existing rule"),
            "original rules must be preserved; got:\n{written}"
        );
        assert!(
            written.contains("new rule"),
            "new rule must be appended; got:\n{written}"
        );
    }

    #[test]
    fn write_plan_rules_file_idempotent() {
        // Appending the same content twice must not duplicate it.
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("project.rules");
        std::fs::write(&dest, "rule A\n").unwrap();

        let plan = EmitPlan {
            files: vec![EmitFile {
                path: dest.to_str().unwrap().to_string(),
                content: "rule A\n".to_string(),
            }],
            diagnostics: vec![],
        };

        write_plan(&plan, false).unwrap();

        let written = std::fs::read_to_string(&dest).unwrap();
        // "rule A" must appear exactly once.
        let count = written.matches("rule A").count();
        assert_eq!(
            count, 1,
            "duplicate rule must not be appended; got:\n{written}"
        );
    }
}
