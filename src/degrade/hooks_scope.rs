use crate::core::ir::{DiagLevel, Diagnostic, SideArtifact};
use crate::handlers::Scope;

/// Moves skill-scoped hooks into a session- or project-scoped target file.
///
/// Always emits a Warn diagnostic because the hooks will now fire for all sessions
/// in that scope, not only when the originating skill runs.
///
/// Codex does not read plugin-bundled hooks (openai/codex#16430), so the target
/// must be an explicit `--hooks-target=user|project` location.
pub fn degrade_skill_hooks(
    skill_name: &str,
    hooks_value: &serde_json::Value,
    hooks_target: &Scope,
) -> (Vec<SideArtifact>, Vec<Diagnostic>) {
    let mut artifacts = Vec::new();
    let mut diagnostics = Vec::new();

    let (target_path, target_desc) = match hooks_target {
        Scope::User => (
            "~/.codex/hooks.json".to_string(),
            "user scope (~/.codex/hooks.json)".to_string(),
        ),
        Scope::Project => (
            ".codex/config.toml".to_string(),
            "project scope (.codex/config.toml [hooks])".to_string(),
        ),
    };

    let hooks_content =
        serde_json::to_string_pretty(hooks_value).unwrap_or_else(|_| "{}".to_string());

    artifacts.push(SideArtifact {
        path: target_path.clone(),
        content: hooks_content,
        note: format!(
            "Hooks from skill '{}' degraded to {}",
            skill_name, target_desc
        ),
    });

    diagnostics.push(Diagnostic {
        level: DiagLevel::Warn,
        id: Some("skills.hooks".to_string()),
        message: format!(
            "skill '{}' の hooks を {} に移送しました。\
             skill スコープ（skill 実行中のみ）から {} への拡大が発生します。\
             #16430: plugin 同梱 hooks は Codex が読まないため、--hooks-target で書き出し先を指定してください。",
            skill_name, target_desc, target_desc
        ),
    });

    (artifacts, diagnostics)
}
