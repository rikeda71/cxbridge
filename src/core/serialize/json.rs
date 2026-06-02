use std::path::Path;

use anyhow::Context;
use serde_json::Value;

/// Reads a JSON file and returns a Value conforming to the handler parse() contract.
///
/// Stores the top-level object in `frontmatter`; `body` is set to an empty string.
pub fn parse_json_file(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read JSON file: {}", path.display()))?;

    let parsed: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON file: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Non-object top-level JSON (arrays, scalars) is not a valid config; normalise to empty.
    let frontmatter = match parsed {
        Value::Object(map) => Value::Object(map),
        other => {
            let _ = other;
            Value::Object(serde_json::Map::new())
        }
    };

    Ok(serde_json::json!({
        "frontmatter": frontmatter,
        "body": "",
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// Writes a Value to a JSON file with pretty-printing.
pub fn emit_json_file(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let content =
        serde_json::to_string_pretty(value).with_context(|| "Failed to serialize JSON")?;
    std::fs::write(path, content)
        .with_context(|| format!("Failed to write JSON file: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_json_file_basic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.json");
        fs::write(
            &path,
            r#"{"mcpServers": {"my-server": {"command": "npx"}}}"#,
        )
        .unwrap();

        let result = parse_json_file(&path).unwrap();
        assert!(result["frontmatter"]["mcpServers"].is_object());
        assert_eq!(result["body"], "");
        assert!(result["path"].as_str().unwrap().ends_with("test.json"));
    }

    #[test]
    fn test_emit_json_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("output.json");
        let value = serde_json::json!({"key": "value"});
        emit_json_file(&path, &value).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn parse_json_file_malformed_returns_err_with_context() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        fs::write(&path, r#"{"key": NOTJSON}"#).unwrap();

        let err = parse_json_file(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Failed to parse JSON file"),
            "error message must contain 'Failed to parse JSON file', got: {msg}"
        );
    }

    #[test]
    fn parse_json_file_missing_returns_err_with_context() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");

        let err = parse_json_file(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Failed to read JSON file"),
            "error message must contain 'Failed to read JSON file', got: {msg}"
        );
    }

    #[test]
    fn parse_json_file_array_normalizes_to_empty_object() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("array.json");
        fs::write(&path, r#"["a", "b", "c"]"#).unwrap();

        let result = parse_json_file(&path).unwrap();
        assert!(
            result["frontmatter"].as_object().unwrap().is_empty(),
            "top-level JSON array must normalize to empty frontmatter object"
        );
        assert_eq!(result["body"], "");
    }

    #[test]
    fn parse_json_file_scalar_normalizes_to_empty_object() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scalar.json");
        fs::write(&path, "42").unwrap();

        let result = parse_json_file(&path).unwrap();
        assert!(
            result["frontmatter"].as_object().unwrap().is_empty(),
            "top-level JSON scalar must normalize to empty frontmatter object"
        );
    }

    #[test]
    fn emit_json_file_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("deep").join("output.json");
        let value = serde_json::json!({"created": true});

        emit_json_file(&path, &value).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["created"], true);
    }

    #[test]
    fn emit_json_file_produces_pretty_printed_output() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pretty.json");
        let value = serde_json::json!({"a": 1, "b": [1, 2]});

        emit_json_file(&path, &value).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        // pretty-printed output contains newlines and indentation
        assert!(
            content.contains('\n'),
            "emit_json_file must produce pretty-printed (multi-line) output"
        );
    }
}
