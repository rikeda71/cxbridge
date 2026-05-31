use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::core::ir::{Diagnostic, Kind};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;

pub mod hooks;
pub mod mcp;
pub mod memory;
pub mod plugins;
pub mod settings;
pub mod skills;
pub mod subagents;

/// 出力先スコープ（.rules / agents の配置先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// ~/.codex/（ユーザー全体）
    User,
    /// .codex/（プロジェクト）
    Project,
}

/// skill の変換先選択モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTargetMode {
    /// 自動判定（決定的ケース自動 + グレーケースは保守的デフォルト or 対話）
    Auto,
    /// 常に skill（.agents/skills/<n>/SKILL.md）へ変換
    Skill,
    /// 常に subagent（.codex/agents/<n>.toml）へ変換
    Subagent,
}

/// handler.lower() に渡すオプション群。
#[derive(Debug, Clone)]
pub struct LowerOpts {
    /// 出力先ディレクトリ（省略時: *.converted/ サブディレクトリ）
    pub out: Option<String>,
    /// 降格先スコープ（.rules / agents の配置）
    pub scope: Scope,
    /// plugin で .claude-plugin/ を残し .codex-plugin/ を追加生成
    pub dual_manifest: bool,
    /// hooks の書き出し先（#16430 回避）
    pub hooks_target: Scope,
    /// skill の変換先選択モード
    pub skill_target: SkillTargetMode,
    /// グレーケースを TTY 対話で確認する
    pub interactive: bool,
    /// 本文の変数/記法を自動書き換え（既定: false = 検出のみ）
    pub rewrite_body: bool,
}

/// handler.lower() が返す出力計画。
pub struct EmitPlan {
    /// 書き出すファイルの一覧
    pub files: Vec<EmitFile>,
    /// 変換中に発生した診断エントリ
    pub diagnostics: Vec<Diagnostic>,
}

/// 書き出すファイルの1エントリ（出力ルートからの相対パスで保持）。
pub struct EmitFile {
    /// 出力ルートからの相対パス
    pub path: String,
    /// ファイル内容
    pub content: String,
}

/// 領域ハンドラのトレイト。各ハンドラは対応する DomainMap を保持する。
pub trait Handler {
    fn kind(&self) -> Kind;

    /// このハンドラが対象とするパスかどうかを判定する。
    fn detect(&self, path: &Path) -> bool;

    /// ファイルを読み込み、handler 間共通の内部表現 Value を返す。
    ///
    /// # 返値の構造
    /// ```json
    /// {
    ///   "frontmatter": { "name": "...", "description": "..." },
    ///   "body": "...",
    ///   "path": "/abs/path"
    /// }
    /// ```
    fn parse(&self, path: &Path) -> anyhow::Result<Value>;

    /// パース済み Value を IRNode に変換する（mappings 駆動）。
    ///
    /// `dir` は pipeline の実行方向（ConvDir）。
    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<crate::core::ir::IRNode>;

    /// IRNode を出力ファイル群（EmitPlan）に変換する。
    fn lower(
        &self,
        ir: &crate::core::ir::IRNode,
        dir: ConvDir,
        opts: &LowerOpts,
    ) -> anyhow::Result<EmitPlan>;
}

/// Kind と全 domain map を受け取り、対応するハンドラをボックス化して返す。
pub fn pick_handler(kind: &Kind, maps: &HashMap<String, DomainMap>) -> Box<dyn Handler> {
    match kind {
        Kind::Skill => Box::new(skills::SkillsHandler {
            map: maps["skills"].clone(),
        }),
        Kind::Mcp => Box::new(mcp::McpHandler {
            map: maps["mcp"].clone(),
        }),
        Kind::Hooks => Box::new(hooks::HooksHandler {
            map: maps["hooks"].clone(),
        }),
        Kind::Plugin => Box::new(plugins::PluginsHandler {
            map: maps["plugins"].clone(),
        }),
        Kind::Memory => Box::new(memory::MemoryHandler {
            map: maps["memory"].clone(),
        }),
        Kind::Subagent => Box::new(subagents::SubagentHandler {
            map: maps["subagents"].clone(),
        }),
        Kind::Settings => Box::new(settings::SettingsHandler {
            map: maps["settings-config"].clone(),
        }),
    }
}
