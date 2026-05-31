use std::collections::HashMap;

use serde_json::Value;

use crate::scanner::body::BodyFinding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    Claude,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loss {
    Lossless,
    Lossy,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Plugin,
    Subagent,
    Hooks,
    Mcp,
    Memory,
    Settings,
}

/// mappings の1エントリに対応する正規化済み変換単位。
#[derive(Debug, Clone)]
pub struct IRField {
    /// mappings の entry id（例: "mcp.timeout"）
    pub id: String,
    /// lift 後の正規化値（serde_json::Value）
    pub value: Value,
    /// 変換の損失レベル
    pub loss: Loss,
    /// 適用した transform 名の一覧（report 用）
    pub transforms_applied: Vec<String>,
    /// 降格が起きた場合の情報
    pub degrade: Option<DegradeInfo>,
    /// warn:true 起因の警告メッセージ
    pub warning: Option<String>,
    /// dropped 時の理由情報
    pub dropped: Option<DroppedInfo>,
}

/// 降格エンジン経由で別スコープへ移送された場合に付与される情報。
#[derive(Debug, Clone)]
pub struct DegradeInfo {
    /// 降格先の種別（例: "session", "subagent"）
    pub to: String,
    /// 降格先ターゲット（例: ".codex/agents/deploy.toml"）
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct DroppedInfo {
    pub reason: String,
}

/// skill/command/prompt 本文の解析結果。
#[derive(Debug, Clone)]
pub struct BodySegment {
    /// 元の本文テキスト（未加工）
    pub raw: String,
    /// スキャナが検出した変数・記法・動的注入の一覧
    pub findings: Vec<BodyFinding>,
}

/// あらゆる領域（skill/mcp/hooks/plugin）を統一的に表す中間表現ノード。
#[derive(Debug, Clone)]
pub struct IRNode {
    pub kind: Kind,
    pub source_tool: Tool,
    pub source_path: String,
    pub fields: HashMap<String, IRField>,
    pub body: Option<BodySegment>,
    /// plugin が内包する skills/hooks/mcp の子ノード
    pub children: Vec<IRNode>,
    pub side_artifacts: Vec<SideArtifact>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 降格や変換で生成される追加ファイル。パスは出力ルートからの相対パス。
#[derive(Debug, Clone)]
pub struct SideArtifact {
    pub path: String,
    pub content: String,
    /// レポート用の補足説明
    pub note: String,
}

/// 変換中に発生した警告・dropped・degrade の診断エントリ。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagLevel,
    /// 関連する mappings の entry id
    pub id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagLevel {
    Info,
    Warn,
    Drop,
}

pub fn new_node(kind: Kind, source_tool: Tool, source_path: &str) -> IRNode {
    IRNode {
        kind,
        source_tool,
        source_path: source_path.to_string(),
        fields: HashMap::new(),
        body: None,
        children: Vec::new(),
        side_artifacts: Vec::new(),
        diagnostics: Vec::new(),
    }
}
