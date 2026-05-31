use std::path::Path;

use walkdir::WalkDir;

use crate::core::ir::Kind;

/// ファイル / ディレクトリのパスから Kind を判定する。
///
/// ファイル指定の場合: パスのファイル名パターンと先頭バイトで即時判定。
/// - SKILL.md（**/skills/*/SKILL.md）      → Kind::Skill
/// - .mcp.json                            → Kind::Mcp
/// - plugin.json（.claude-plugin/ 配下等）→ Kind::Plugin
/// - CLAUDE.md / AGENTS.md               → Kind::Memory
/// - config.toml                         → 内容パースでテーブル判定（後述）
///
/// ディレクトリ指定の場合: walkdir で再帰発見。
///
/// config.toml の種別判定（toml_edit でパース）:
/// - [mcp_servers] テーブルあり → Kind::Mcp
/// - [hooks] テーブルあり       → Kind::Hooks
/// - 両方あり                   → Kind::Plugin
///
/// x2c 追加ルール:
/// - .agents/skills/<n>/agents/openai.yaml → Kind::Skill
/// - .codex/agents/<n>.toml               → Kind::Subagent
pub fn detect(path: &str) -> anyhow::Result<Kind> {
    let p = Path::new(path);

    if p.is_file() {
        detect_file(p)
    } else if p.is_dir() {
        detect_dir(p)
    } else {
        anyhow::bail!("Path does not exist or is not a file/directory: {}", path)
    }
}

fn detect_file(p: &Path) -> anyhow::Result<Kind> {
    let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");

    match file_name {
        "SKILL.md" => return Ok(Kind::Skill),
        ".mcp.json" => return Ok(Kind::Mcp),
        "CLAUDE.md" | "AGENTS.md" | "CLAUDE.local.md" | "AGENTS.override.md" => {
            return Ok(Kind::Memory)
        }
        "hooks.json" => return Ok(Kind::Hooks),
        "settings.json" => {
            return detect_settings_json(p);
        }
        "config.toml" => return detect_config_toml(p),
        _ => {}
    }

    if file_name.ends_with("plugin.json") || file_name == "plugin.json" {
        let path_str = p.to_str().unwrap_or("");
        if path_str.contains(".claude-plugin/") || path_str.contains(".codex-plugin/") {
            return Ok(Kind::Plugin);
        }
    }

    if file_name == "openai.yaml" {
        let path_str = p.to_str().unwrap_or("");
        if path_str.contains(".agents/skills/") && path_str.contains("/agents/openai.yaml") {
            return Ok(Kind::Skill);
        }
    }

    if file_name.ends_with(".toml") && file_name != "config.toml" {
        let path_str = p.to_str().unwrap_or("");
        if path_str.contains(".codex/agents/") || path_str.contains("/agents/") {
            return Ok(Kind::Subagent);
        }
    }

    if file_name.ends_with(".md")
        && !matches!(
            file_name,
            "SKILL.md"
                | "CLAUDE.md"
                | "AGENTS.md"
                | "CLAUDE.local.md"
                | "AGENTS.override.md"
                | "README.md"
        )
    {
        let path_str = p.to_str().unwrap_or("");
        if path_str.contains(".claude/agents/") || path_str.contains("/agents/") {
            return Ok(Kind::Subagent);
        }
    }

    anyhow::bail!("Cannot determine kind for file: {}", p.display())
}

fn detect_settings_json(p: &Path) -> anyhow::Result<Kind> {
    let content = std::fs::read_to_string(p)
        .with_context(|| format!("Failed to read settings.json: {}", p.display()))?;
    let val: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings.json: {}", p.display()))?;
    if val.get("hooks").is_some() {
        Ok(Kind::Hooks)
    } else {
        Ok(Kind::Settings)
    }
}

fn detect_config_toml(p: &Path) -> anyhow::Result<Kind> {
    let content = std::fs::read_to_string(p)
        .with_context(|| format!("Failed to read config.toml: {}", p.display()))?;

    let doc: toml_edit::DocumentMut = content
        .parse()
        .with_context(|| format!("Failed to parse config.toml: {}", p.display()))?;

    let has_mcp = doc.contains_key("mcp_servers");
    let has_hooks = doc.contains_key("hooks");

    match (has_mcp, has_hooks) {
        (true, true) => Ok(Kind::Plugin),
        (true, false) => Ok(Kind::Mcp),
        (false, true) => Ok(Kind::Hooks),
        (false, false) => Ok(Kind::Settings),
    }
}

fn detect_dir(p: &Path) -> anyhow::Result<Kind> {
    // 優先順位: Skill > Plugin > Mcp > Hooks > Memory > Subagent > Settings
    let mut found_kinds: Vec<Kind> = Vec::new();

    for entry in WalkDir::new(p).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().is_file() {
            let file_name = entry.file_name().to_str().unwrap_or("");
            let path_str = entry.path().to_str().unwrap_or("");

            let kind = if file_name == "SKILL.md" {
                Some(Kind::Skill)
            } else if file_name == ".mcp.json" {
                Some(Kind::Mcp)
            } else if (file_name.ends_with("plugin.json") || file_name == "plugin.json")
                && (path_str.contains(".claude-plugin/") || path_str.contains(".codex-plugin/"))
            {
                Some(Kind::Plugin)
            } else if matches!(
                file_name,
                "CLAUDE.md" | "AGENTS.md" | "CLAUDE.local.md" | "AGENTS.override.md"
            ) {
                Some(Kind::Memory)
            } else if file_name == "hooks.json" {
                Some(Kind::Hooks)
            } else if file_name == "openai.yaml"
                && path_str.contains(".agents/skills/")
                && path_str.contains("/agents/openai.yaml")
            {
                Some(Kind::Skill)
            } else if (file_name.ends_with(".toml")
                && file_name != "config.toml"
                && (path_str.contains(".codex/agents/") || path_str.contains("/agents/")))
                || (file_name.ends_with(".md")
                    && !matches!(
                        file_name,
                        "SKILL.md"
                            | "CLAUDE.md"
                            | "AGENTS.md"
                            | "CLAUDE.local.md"
                            | "AGENTS.override.md"
                            | "README.md"
                    )
                    && (path_str.contains(".claude/agents/") || path_str.contains("/agents/")))
            {
                Some(Kind::Subagent)
            } else if file_name == "config.toml" {
                detect_config_toml(entry.path()).ok()
            } else {
                None
            };

            if let Some(k) = kind {
                if !found_kinds.contains(&k) {
                    found_kinds.push(k);
                }
            }
        }
    }

    if found_kinds.is_empty() {
        anyhow::bail!(
            "No recognizable config files found in directory: {}",
            p.display()
        )
    }

    let priority = [
        Kind::Skill,
        Kind::Plugin,
        Kind::Mcp,
        Kind::Hooks,
        Kind::Memory,
        Kind::Subagent,
        Kind::Settings,
    ];

    for k in &priority {
        if found_kinds.contains(k) {
            return Ok(k.clone());
        }
    }

    Ok(found_kinds.remove(0))
}

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::TempDir::new().unwrap()
    }

    #[test]
    fn test_detect_skill_md() {
        let dir = tmp();
        let path = dir.path().join("SKILL.md");
        fs::write(&path, "---\nname: test\n---\nbody").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Skill);
    }

    #[test]
    fn test_detect_mcp_json() {
        let dir = tmp();
        let path = dir.path().join(".mcp.json");
        fs::write(&path, "{}").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Mcp);
    }

    #[test]
    fn test_detect_claude_md() {
        let dir = tmp();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, "# Memory").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Memory);
    }

    #[test]
    fn test_detect_agents_md() {
        let dir = tmp();
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "# Agents").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Memory);
    }

    #[test]
    fn test_detect_config_toml_mcp() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[mcp_servers]\n").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Mcp);
    }

    #[test]
    fn test_detect_config_toml_hooks() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[hooks]\n").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Hooks);
    }

    #[test]
    fn test_detect_config_toml_both() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[mcp_servers]\n[hooks]\n").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Plugin);
    }

    #[test]
    fn test_detect_plugin_json_in_claude_plugin() {
        let dir = tmp();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let path = plugin_dir.join("plugin.json");
        fs::write(&path, "{}").unwrap();
        let kind = detect(path.to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Plugin);
    }

    #[test]
    fn test_detect_dir_with_skill() {
        let dir = tmp();
        let skill_dir = dir.path().join(".claude").join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: deploy\n---\n").unwrap();
        let kind = detect(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(kind, Kind::Skill);
    }

    #[test]
    fn test_detect_nonexistent() {
        let result = detect("/nonexistent/path/that/does/not/exist");
        assert!(result.is_err());
    }
}
