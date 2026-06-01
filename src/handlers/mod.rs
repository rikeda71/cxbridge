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

/// Output scope (placement target for .rules / agents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// ~/.codex/ (user-wide)
    User,
    /// .codex/ (project)
    Project,
}

/// Skill conversion target selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTargetMode {
    /// Automatic: deterministic cases resolved automatically; ambiguous cases use conservative default or interactive prompt
    Auto,
    /// Always convert to skill (.agents/skills/<n>/SKILL.md)
    Skill,
    /// Always convert to subagent (.codex/agents/<n>.toml)
    Subagent,
}

/// Options passed to handler.lower().
#[derive(Debug, Clone)]
pub struct LowerOpts {
    /// Output directory (default: *.converted/ subdirectory)
    pub out: Option<String>,
    /// Domain filter for conversion (empty means all domains)
    pub only: Vec<String>,
    /// Scope for degraded output (.rules / agents placement)
    pub scope: Scope,
    /// Retain .claude-plugin/ while also generating .codex-plugin/
    pub dual_manifest: bool,
    /// Destination scope for hooks output (workaround for #16430)
    pub hooks_target: Scope,
    /// Skill conversion target selection mode
    pub skill_target: SkillTargetMode,
    /// Confirm ambiguous cases interactively via TTY
    pub interactive: bool,
    /// Rewrite body variables/syntax automatically (default: false = detect only)
    pub rewrite_body: bool,
    /// Retain Claude-specific frontmatter keys in Codex output (Codex ignores them via fail-open)
    pub keep_claude_frontmatter: bool,
}

/// Output plan returned by handler.lower().
pub struct EmitPlan {
    /// List of files to write
    pub files: Vec<EmitFile>,
    /// Diagnostic entries produced during conversion
    pub diagnostics: Vec<Diagnostic>,
}

/// A single file entry to write (path stored relative to the output root).
pub struct EmitFile {
    /// Path relative to the output root
    pub path: String,
    /// File contents
    pub content: String,
}

/// Domain handler trait. Each handler holds its corresponding DomainMap.
pub trait Handler {
    fn kind(&self) -> Kind;

    /// Returns true if this handler should process the given path.
    fn detect(&self, path: &Path) -> bool;

    /// Reads a file and returns the shared internal Value representation.
    ///
    /// # Return value shape
    /// ```json
    /// {
    ///   "frontmatter": { "name": "...", "description": "..." },
    ///   "body": "...",
    ///   "path": "/abs/path"
    /// }
    /// ```
    fn parse(&self, path: &Path) -> anyhow::Result<Value>;

    /// Converts a parsed Value into an IRNode (mappings-driven).
    ///
    /// `dir` is the pipeline execution direction (ConvDir).
    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<crate::core::ir::IRNode>;

    /// Converts an IRNode into an output file set (EmitPlan).
    fn lower(
        &self,
        ir: &crate::core::ir::IRNode,
        dir: ConvDir,
        opts: &LowerOpts,
    ) -> anyhow::Result<EmitPlan>;
}

/// Helper that converts a JSON Value into a list of strings.
///
/// `Value::String` → `[string]`, `Value::Array` → each element as a string, anything else → `[]`.
pub(crate) fn json_to_string_list(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
    }
}

/// Returns a boxed handler for the given Kind, looked up from the full domain map set.
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
