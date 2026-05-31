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
        write_plan(&plan, opts)?;
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

/// `--force` なし時は既存ファイルへの上書きを拒否する。
/// config.toml と .rules は append 対象のため上書き保護対象外。
pub fn write_plan(plan: &EmitPlan, opts: &ConvertOpts) -> anyhow::Result<()> {
    for file in &plan.files {
        let dest = PathBuf::from(&file.path);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        // config.toml と .rules はハンドラ側でマージ済みなので上書き保護を免除する
        let is_append_target = dest
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "config.toml" || n.ends_with(".rules"))
            .unwrap_or(false);

        if dest.exists() && !is_append_target && !opts.force {
            anyhow::bail!(
                "Output file already exists (use --force to overwrite): {}",
                dest.display()
            );
        }

        std::fs::write(&dest, &file.content)
            .with_context(|| format!("Failed to write file: {}", dest.display()))?;
    }
    Ok(())
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
