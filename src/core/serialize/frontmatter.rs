use std::path::Path;

use anyhow::Context;
use gray_matter::engine::YAML;
use gray_matter::Matter;
use serde_json::Value;

/// frontmatter 付き Markdown ファイルを parse する。
///
/// handler の parse() 契約に従う JSON Value `{frontmatter, body, path}` を返す。
pub fn parse_frontmatter_file(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let matter = Matter::<YAML>::new();
    let result = matter.parse(&content);

    let frontmatter: Value = if let Some(pod) = result.data {
        pod.into()
    } else {
        Value::Object(serde_json::Map::new())
    };

    let body = result.content;

    Ok(serde_json::json!({
        "frontmatter": frontmatter,
        "body": body,
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// frontmatter と本文テキストを Markdown ファイルとして書き出す。
pub fn emit_frontmatter_file(
    path: &Path,
    frontmatter: &serde_json::Map<String, Value>,
    body: &str,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let output = if frontmatter.is_empty() {
        body.to_string()
    } else {
        let json_obj = Value::Object(frontmatter.clone());
        let yaml_str = serde_saphyr::to_string(&json_obj)
            .with_context(|| "Failed to serialize frontmatter as YAML")?;
        format!("---\n{}---\n{}", yaml_str, body)
    };

    std::fs::write(path, output)
        .with_context(|| format!("Failed to write file: {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_frontmatter_file_basic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        fs::write(
            &path,
            "---\nname: deploy\ndescription: Deploy the app\n---\n\nDeploy the application.\n",
        )
        .unwrap();

        let result = parse_frontmatter_file(&path).unwrap();
        assert_eq!(result["frontmatter"]["name"], "deploy");
        assert_eq!(result["frontmatter"]["description"], "Deploy the app");
        assert!(result["body"]
            .as_str()
            .unwrap()
            .contains("Deploy the application"));
        assert!(result["path"].as_str().unwrap().ends_with("SKILL.md"));
    }

    #[test]
    fn test_parse_frontmatter_file_no_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("README.md");
        fs::write(&path, "Just a plain markdown file.\n").unwrap();

        let result = parse_frontmatter_file(&path).unwrap();
        assert!(result["frontmatter"].as_object().unwrap().is_empty());
        assert!(result["body"].as_str().unwrap().contains("plain markdown"));
    }

    #[test]
    fn test_emit_frontmatter_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("output.md");

        let mut fm = serde_json::Map::new();
        fm.insert("name".to_string(), Value::String("test-skill".to_string()));
        fm.insert(
            "description".to_string(),
            Value::String("A test skill".to_string()),
        );

        emit_frontmatter_file(&path, &fm, "This is the body.\n").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("---"));
        assert!(content.contains("name"));
        assert!(content.contains("test-skill"));
        assert!(content.contains("This is the body."));
    }

    #[test]
    fn test_emit_frontmatter_file_empty_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("output.md");
        let fm = serde_json::Map::new();
        emit_frontmatter_file(&path, &fm, "Body only.\n").unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("---"));
        assert!(content.contains("Body only."));
    }
}
