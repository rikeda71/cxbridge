use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::core::ir::Kind;

/// Returns the single `Kind` for a path (file) or the dominant `Kind` for a directory.
///
/// For per-file multi-kind directory walks, use [`detect_files`] instead.
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

/// Returns every `(Kind, PathBuf)` pair reachable from `path`.
///
/// For a file input, returns a single-element vec containing `(kind, file_path)`.
/// For a directory input, walks recursively and returns one entry per recognizable
/// file (all kinds, not only the dominant one). Each `PathBuf` always points to an
/// individual file, never to the directory itself.
pub fn detect_files(path: &str) -> anyhow::Result<Vec<(Kind, PathBuf)>> {
    let p = Path::new(path);

    if p.is_file() {
        let kind = detect_file(p)?;
        return Ok(vec![(kind, p.to_path_buf())]);
    }

    if p.is_dir() {
        return detect_dir_files(p);
    }

    anyhow::bail!("Path does not exist or is not a file/directory: {}", path)
}

/// Walks `p` recursively and returns one `(Kind, PathBuf)` pair per recognizable file.
///
/// Each `PathBuf` always points to an individual file, never to the directory itself.
/// Returns an error if no recognizable files are found.
pub(crate) fn detect_dir_files(p: &Path) -> anyhow::Result<Vec<(Kind, PathBuf)>> {
    let mut results: Vec<(Kind, PathBuf)> = Vec::new();

    for entry in WalkDir::new(p).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        // Only files whose kind is determined by parsing (settings.json,
        // config.toml) can produce a meaningful error. Other unrecognized
        // files are silently skipped.
        let file_name = entry.file_name().to_str().unwrap_or("");
        let is_parsed_config = matches!(file_name, "settings.json" | "config.toml");
        match detect_file(entry.path()) {
            Ok(kind) => results.push((kind, entry.path().to_path_buf())),
            Err(e) if is_parsed_config => {
                eprintln!("warning: skipping {}: {}", entry.path().display(), e);
            }
            Err(_) => {}
        }
    }

    if results.is_empty() {
        anyhow::bail!(
            "No recognizable config files found in directory: {}",
            p.display()
        );
    }
    Ok(results)
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
        if path_str.contains("codex/agents/") {
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
        if path_str.contains("claude/agents/") {
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
    // Priority: Skill > Plugin > Mcp > Hooks > Memory > Subagent > Settings
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
                && path_str.contains("codex/agents/"))
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
                    && path_str.contains("claude/agents/"))
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

    #[test]
    fn test_detect_subagent_only_under_codex_claude_agents() {
        let dir = tmp();
        // A .toml under a Codex agents dir is a subagent.
        let codex_agent = dir.path().join(".codex").join("agents");
        fs::create_dir_all(&codex_agent).unwrap();
        let coder = codex_agent.join("coder.toml");
        fs::write(&coder, "name = \"coder\"\n").unwrap();
        assert_eq!(detect(coder.to_str().unwrap()).unwrap(), Kind::Subagent);

        // A .toml under an unrelated `docs/agents/` dir must NOT be a subagent.
        let unrelated = dir.path().join("docs").join("agents");
        fs::create_dir_all(&unrelated).unwrap();
        let note = unrelated.join("note.toml");
        fs::write(&note, "name = \"note\"\n").unwrap();
        assert_ne!(detect(note.to_str().unwrap()).ok(), Some(Kind::Subagent));
    }

    #[test]
    fn test_detect_dir_files_with_skill() {
        let dir = tmp();
        let skill_dir = dir.path().join(".claude").join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, "---\nname: deploy\n---\nbody").unwrap();

        let pairs = detect_dir_files(dir.path()).unwrap();

        let (found_kind, found_path) = pairs
            .iter()
            .find(|(k, _)| *k == Kind::Skill)
            .expect("expected Kind::Skill in result");

        assert_eq!(*found_path, skill_path);
        assert_eq!(*found_kind, Kind::Skill);
    }
}
