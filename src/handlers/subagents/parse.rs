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
