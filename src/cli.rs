use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

use crate::core::{
    detect::detect,
    mappings::load_mappings,
    report::{build_report, print_report},
    transforms::ConvDir,
};
use crate::handlers::{pick_handler, EmitPlan, LowerOpts, Scope, SkillTargetMode};

const MAPPINGS_DIR: &str = "mappings";

#[derive(Parser)]
#[command(name = "ccx", about = "Claude Code ⇄ Codex 設定ファイル双方向変換 CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Claude → Codex 変換
    C2x {
        /// 変換対象のパス（ファイルまたはディレクトリ）
        path: String,
        #[command(flatten)]
        opts: ConvertOpts,
    },
    /// Codex → Claude 変換
    X2c {
        /// 変換対象のパス（ファイルまたはディレクトリ）
        path: String,
        #[command(flatten)]
        opts: ConvertOpts,
    },
    /// 変換可能性の事前診断（書き込まない）
    Check {
        /// 診断対象のパス（ファイルまたはディレクトリ）
        path: String,
    },
}

/// 変換オプション（c2x / x2c 共通）。
#[derive(Parser, Debug, Clone)]
pub struct ConvertOpts {
    /// 出力先ディレクトリ（省略時: *.converted/ サブディレクトリ）
    #[arg(long)]
    pub out: Option<String>,

    /// 変換対象ドメインをカンマ区切りで限定（例: skills,mcp）
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,

    /// 降格先スコープ（.rules / agents の配置）。既定: project
    #[arg(long)]
    pub scope: Option<String>,

    /// skill の変換先選択（auto|skill|subagent）。既定: auto
    #[arg(long)]
    pub skill_target: Option<String>,

    /// グレーケースを TTY 対話で確認する
    #[arg(long)]
    pub interactive: bool,

    /// 本文の変数/記法を自動書き換え（既定: 検出のみ）
    #[arg(long)]
    pub rewrite_body: bool,

    /// plugin で .claude-plugin/ を残し .codex-plugin/ を追加生成
    #[arg(long)]
    pub dual_manifest: bool,

    /// hooks の書き出し先（user|project）。既定: user
    #[arg(long)]
    pub hooks_target: Option<String>,

    /// 詳細レポートを出力（--report=json で機械可読）
    #[arg(long)]
    pub report: Option<Option<String>>,

    /// 書き込まず report のみ出力
    #[arg(long)]
    pub dry_run: bool,

    /// dropped が 1 件でもあれば非ゼロ終了（CI 用）
    #[arg(long)]
    pub strict: bool,

    /// Claude 固有 frontmatter キーを出力に残置
    #[arg(long)]
    pub keep_claude_frontmatter: bool,

    /// 既存ファイルへの上書きを許可
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

fn run_convert(dir: ConvDir, path: &str, opts: &ConvertOpts) -> anyhow::Result<()> {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(path)?;
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(path))?;
    let ir = handler.lift(&parsed, dir)?;
    let lower_opts = build_lower_opts(opts);
    let plan = handler.lower(&ir, dir, &lower_opts)?;
    let report = build_report(&ir, &plan);
    if !opts.dry_run {
        write_plan(&plan, opts.force)?;
    }
    let report_fmt = opts.report.as_ref().and_then(|r| r.as_deref());
    print_report(&report, report_fmt);
    let exit_code = if opts.strict && !report.dropped.is_empty() {
        2
    } else {
        0
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// check サブコマンド: 書き込まず dropped 件数のみ報告する。
fn run_check(path: &str) -> anyhow::Result<()> {
    let maps = load_mappings(Path::new(MAPPINGS_DIR));
    let kind = detect(path)?;
    let handler = pick_handler(&kind, &maps);
    let parsed = handler.parse(Path::new(path))?;

    // check はどちらの方向も診断できるが、デフォルト c2x で lift する
    let ir = handler.lift(&parsed, ConvDir::C2x)?;

    let empty_plan = EmitPlan {
        files: vec![],
        diagnostics: vec![],
    };
    let report = build_report(&ir, &empty_plan);

    println!("check: {}", path);
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

    Ok(())
}

fn build_lower_opts(opts: &ConvertOpts) -> LowerOpts {
    LowerOpts {
        out: opts.out.clone(),
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
