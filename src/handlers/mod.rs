use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{
    DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss,
};
use crate::core::mappings::{applies_direction, DomainMap, MapEntry};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};

/// Serializes `fm` as YAML frontmatter and wraps it with `body` as `---\n{fm}---\n{body}`.
///
/// Returns just `body` when `fm` is empty (no frontmatter block).
pub(crate) fn render_frontmatter_md(
    fm: &serde_json::Map<String, Value>,
    body: &str,
) -> anyhow::Result<String> {
    if fm.is_empty() {
        return Ok(body.to_string());
    }
    let yaml_val = Value::Object(fm.clone());
    let fm_yaml = serde_saphyr::to_string(&yaml_val)
        .with_context(|| "Failed to serialize frontmatter as YAML")?;
    Ok(format!("---\n{}---\n{}", fm_yaml, body))
}

/// Renders `s` as a TOML multi-line basic string (`"""..."""`), escaping `\` and
/// `"` so content containing quotes (including `'''`) cannot terminate the literal.
/// The leading newline after the opening delimiter is trimmed by TOML, so the
/// content is emitted unchanged.
pub(crate) fn toml_multiline_basic(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"\"\"\n{escaped}\n\"\"\"")
}

#[cfg(test)]
mod multiline_basic_tests {
    use super::toml_multiline_basic;

    #[test]
    fn handles_triple_quotes_and_backslashes() {
        // Content with ''' (which would terminate a literal string), a double
        // quote, and a backslash must still produce a parseable TOML document.
        let content = "uses ''' and \" and a \\ backslash";
        let toml_str = format!("v = {}", toml_multiline_basic(content));
        let parsed: toml::Value = toml::from_str(&toml_str).expect("must be valid TOML");
        assert_eq!(
            parsed["v"].as_str().unwrap().trim_end_matches('\n'),
            content
        );
    }
}

/// Lifts a single mapped frontmatter/manifest field into `node` following the
/// canonical sequence shared by the skills, subagents, and plugins handlers:
/// direction filter → transforms → loss/degrade/dropped classification → field
/// insertion → a single `Warn` diagnostic for genuinely lossy (non-dropped)
/// `warn: true` fields.
///
/// Dropped fields are surfaced via `IRField.dropped`; emitting a `Warn` for them
/// too would make `build_report` count them in the lossy list as well.
pub(crate) fn lift_mapped_field(
    entry: &MapEntry,
    key: &str,
    value: &Value,
    dir: ConvDir,
    node: &mut IRNode,
) {
    if !applies_direction(entry, dir) {
        return;
    }

    let ctx = TransformCtx {
        direction: dir,
        args: None,
        field: entry,
    };
    let (v, applied) = apply_transforms(value, entry.transform.as_deref(), &ctx);

    let loss = Loss::from(&entry.loss);
    let is_dropped = matches!(loss, Loss::Dropped);

    let degrade_info = entry.degrade.as_ref().map(|d| DegradeInfo {
        to: d.to.clone(),
        target: d.target.clone(),
    });

    let dropped_info = is_dropped.then(|| DroppedInfo {
        reason: entry
            .notes
            .clone()
            .unwrap_or_else(|| format!("{key} has no equivalent")),
    });

    let warning = (entry.warn == Some(true))
        .then(|| format!("{}: {}", entry.id, entry.notes.as_deref().unwrap_or("warn")));

    let id = entry.id.clone();
    node.fields.insert(
        id.clone(),
        IRField {
            id: id.clone(),
            value: v,
            loss,
            transforms_applied: applied,
            degrade: degrade_info,
            warning: warning.clone(),
            dropped: dropped_info,
        },
    );

    if !is_dropped {
        if let Some(msg) = warning {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some(id),
                message: msg,
            });
        }
    }
}

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
            maps: maps.clone(),
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
