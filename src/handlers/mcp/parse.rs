use std::path::Path;

use anyhow::Context;
use serde_json::Value;

/// Parses config.toml and returns a Value conforming to the handler's parse() contract.
/// The [mcp_servers.*] section is stored as "mcp_servers" under "frontmatter".
pub(super) fn parse_toml_mcp_config(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config.toml: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Convert TOML to serde_json::Value using the toml crate
    let toml_val: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse TOML: {}", path.display()))?;

    // Convert TOML Value → JSON Value
    let json_val = crate::core::serialize::toml_to_json(&toml_val)?;

    Ok(serde_json::json!({
        "frontmatter": json_val,
        "body": "",
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// Extracts VAR from a "Bearer ${VAR}" string.
pub(super) fn extract_bearer_env_var(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix("Bearer ${") {
        if let Some(end) = rest.rfind('}') {
            let var_name = &rest[..end];
            if !var_name.is_empty() {
                return Some(var_name.to_string());
            }
        }
    }
    None
}

/// Extracts VAR from a "${VAR}" string.
/// Only pure environment-variable references of the form `${VAR}` are extracted.
/// Composite values such as `${VAR} suffix` return None (the trailing `}` is
/// verified to prevent partial matches).
pub(super) fn extract_env_var_ref(s: &str) -> Option<String> {
    if s.starts_with("${") && s.ends_with('}') {
        let inner = &s[2..s.len() - 1];
        // For ${VAR:-default}, ignore the default part and extract only VAR
        let var_name = inner.split(":-").next().unwrap_or(inner);
        if !var_name.is_empty() {
            return Some(var_name.to_string());
        }
    }
    None
}
