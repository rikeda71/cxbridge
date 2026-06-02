use std::path::Path;

use anyhow::Context;
use serde_json::Value;

/// Parse a Codex agent TOML file (.codex/agents/<n>.toml) into the handler-internal Value format.
pub(crate) fn parse_codex_agent_toml(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read agent TOML: {}", path.display()))?;

    let toml_val: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse agent TOML: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Convert toml::Value to serde_json::Value for the frontmatter
    let json_val = crate::core::serialize::toml_to_json(&toml_val)?;

    // The TOML agent file has a flat structure; all top-level keys go into "frontmatter"
    // and developer_instructions becomes the "body"
    let mut frontmatter = serde_json::Map::new();
    let mut body = String::new();

    if let Value::Object(map) = &json_val {
        for (k, v) in map {
            if k == "developer_instructions" {
                body = v.as_str().unwrap_or("").to_string();
            } else if let Value::Object(nested) = v {
                // Flatten one level of nested tables using dot notation so that
                // codex.field paths like "skills.config" resolve correctly.
                for (sk, sv) in nested {
                    frontmatter.insert(format!("{}.{}", k, sk), sv.clone());
                }
            } else {
                frontmatter.insert(k.clone(), v.clone());
            }
        }
    }

    Ok(serde_json::json!({
        "frontmatter": Value::Object(frontmatter),
        "body": body,
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// Extract agent name from path.
/// .claude/agents/<name>.md → <name>
/// .codex/agents/<name>.toml → <name>
pub(crate) fn extract_agent_name_from_path(source_path: &str) -> String {
    let path = Path::new(source_path);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem != "config" && !stem.is_empty() {
            return stem.to_string();
        }
    }
    "agent".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn agents_dir(tmp: &TempDir) -> std::path::PathBuf {
        let dir = tmp.path().join(".codex").join("agents");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- parse_codex_agent_toml ---

    #[test]
    fn parses_name_description_model_developer_instructions() {
        let tmp = TempDir::new().unwrap();
        let path = agents_dir(&tmp).join("writer.toml");
        fs::write(
            &path,
            r#"name = "writer"
description = "A writing assistant"
model = "gpt-5.4"
developer_instructions = "You help write documents."
"#,
        )
        .unwrap();

        let val = parse_codex_agent_toml(&path).unwrap();

        // name, description, model go into frontmatter
        let fm = val["frontmatter"].as_object().unwrap();
        assert_eq!(fm["name"].as_str().unwrap(), "writer");
        assert_eq!(fm["description"].as_str().unwrap(), "A writing assistant");
        assert_eq!(fm["model"].as_str().unwrap(), "gpt-5.4");

        // developer_instructions becomes the body, not frontmatter
        assert_eq!(val["body"].as_str().unwrap(), "You help write documents.");
        assert!(
            !fm.contains_key("developer_instructions"),
            "developer_instructions must not appear in frontmatter; keys: {:?}",
            fm.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn parses_multiline_developer_instructions_as_body() {
        let tmp = TempDir::new().unwrap();
        let path = agents_dir(&tmp).join("coder.toml");
        fs::write(
            &path,
            "name = \"coder\"\ndeveloper_instructions = \"\"\"\nLine one.\nLine two.\n\"\"\"\n",
        )
        .unwrap();

        let val = parse_codex_agent_toml(&path).unwrap();

        let body = val["body"].as_str().unwrap();
        assert!(
            body.contains("Line one."),
            "multiline body must contain first line; got: {:?}",
            body
        );
        assert!(
            body.contains("Line two."),
            "multiline body must contain second line; got: {:?}",
            body
        );
    }

    #[test]
    fn missing_developer_instructions_produces_empty_body() {
        let tmp = TempDir::new().unwrap();
        let path = agents_dir(&tmp).join("minimal.toml");
        fs::write(&path, "name = \"minimal\"\n").unwrap();

        let val = parse_codex_agent_toml(&path).unwrap();

        assert_eq!(
            val["body"].as_str().unwrap(),
            "",
            "Missing developer_instructions must produce empty body"
        );
    }

    #[test]
    fn nested_table_keys_are_flattened_with_dot_notation() {
        let tmp = TempDir::new().unwrap();
        let path = agents_dir(&tmp).join("skilled.toml");
        fs::write(
            &path,
            "name = \"skilled\"\n\n[skills]\nconfig = [{enabled = true, path = \"python\"}]\n",
        )
        .unwrap();

        let val = parse_codex_agent_toml(&path).unwrap();
        let fm = val["frontmatter"].as_object().unwrap();

        // [skills] table must be flattened as "skills.config"
        assert!(
            fm.contains_key("skills.config"),
            "Nested [skills].config must be flattened to skills.config; keys: {:?}",
            fm.keys().collect::<Vec<_>>()
        );
        // The original "skills" key must not appear as a top-level object
        assert!(
            !fm.contains_key("skills"),
            "Flat 'skills' key must not appear after flattening; keys: {:?}",
            fm.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn path_is_populated_in_result() {
        let tmp = TempDir::new().unwrap();
        let path = agents_dir(&tmp).join("pathtest.toml");
        fs::write(&path, "name = \"pathtest\"\n").unwrap();

        let val = parse_codex_agent_toml(&path).unwrap();

        let reported_path = val["path"].as_str().unwrap();
        assert!(
            reported_path.contains("pathtest"),
            "path field must reference the agent file; got: {:?}",
            reported_path
        );
    }

    #[test]
    fn invalid_toml_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = agents_dir(&tmp).join("bad.toml");
        fs::write(&path, "this is not = valid toml = at all\n[[[\n").unwrap();

        let result = parse_codex_agent_toml(&path);
        assert!(result.is_err(), "Invalid TOML must return Err; got Ok");
    }

    // --- extract_agent_name_from_path ---

    #[test]
    fn extracts_stem_from_claude_md_path() {
        assert_eq!(
            extract_agent_name_from_path(".claude/agents/researcher.md"),
            "researcher"
        );
    }

    #[test]
    fn extracts_stem_from_codex_toml_path() {
        assert_eq!(
            extract_agent_name_from_path(".codex/agents/coder.toml"),
            "coder"
        );
    }

    #[test]
    fn config_stem_falls_back_to_agent() {
        assert_eq!(extract_agent_name_from_path(".codex/config.toml"), "agent");
    }

    #[test]
    fn empty_path_falls_back_to_agent() {
        assert_eq!(extract_agent_name_from_path(""), "agent");
    }
}
