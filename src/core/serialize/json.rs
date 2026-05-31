use std::path::Path;

use anyhow::Context;
use serde_json::Value;

/// JSON ファイルを読み込み、handler の parse() 契約に従う Value を返す。
///
/// トップレベルオブジェクトを `frontmatter` に格納し、`body` は空文字列とする。
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

/// Value を JSON ファイルとして書き出す（整形付き）。
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
}
