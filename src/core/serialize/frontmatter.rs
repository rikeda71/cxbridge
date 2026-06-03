use std::path::Path;

use anyhow::Context;
use gray_matter::engine::YAML;
use gray_matter::Matter;
use serde_json::Value;

/// Parses a Markdown file with frontmatter.
///
/// Returns a JSON Value `{frontmatter, body, path}` conforming to the handler parse() contract.
/// Tries strict YAML first; falls back to lenient line-by-line parsing when strict fails
/// (e.g. unquoted colons in values common in real skill files).
pub fn parse_frontmatter_file(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let matter = Matter::<YAML>::new();
    let (frontmatter, body) = match matter.parse::<Value>(&content) {
        Ok(parsed) => {
            let fm = parsed
                .data
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
            (fm, parsed.content)
        }
        Err(_) => {
            // Strict YAML failed — try the lenient path before surfacing an error.
            match parse_frontmatter_lenient(&content) {
                Some((map, body)) => (Value::Object(map), body),
                None => {
                    // No valid fence found; re-run strict parse to get the original error.
                    let parsed = matter.parse::<Value>(&content).with_context(|| {
                        format!("Failed to parse frontmatter in {}", path.display())
                    })?;
                    let fm = parsed
                        .data
                        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                    (fm, parsed.content)
                }
            }
        }
    };

    Ok(serde_json::json!({
        "frontmatter": frontmatter,
        "body": body,
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// Lenient line-by-line frontmatter parser for files that fail strict YAML.
///
/// Requires a `---` fence at the start and a closing `---` fence. Top-level
/// `key: value` lines are stored as strings (preserving unquoted colons in values).
/// A `key:` with no value followed by indented `- item` lines produces a string array.
/// Returns `None` when no valid `---…---` fence is found.
fn parse_frontmatter_lenient(content: &str) -> Option<(serde_json::Map<String, Value>, String)> {
    let mut lines = content.lines();

    // First non-empty line must be `---`.
    let first = lines.next()?;
    if first.trim() != "---" {
        return None;
    }

    // Collect frontmatter lines until the closing `---`.
    let mut fm_lines: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        fm_lines.push(line);
    }
    if !closed {
        return None;
    }

    // Everything after the closing fence is the body.
    let body: String = lines.collect::<Vec<_>>().join("\n");
    // Restore a leading newline that gray_matter normally includes in body.
    let body = if body.is_empty() {
        String::new()
    } else {
        format!("\n{}", body)
    };

    let mut map = serde_json::Map::new();
    let mut last_key: Option<String> = None;
    let mut i = 0;
    while i < fm_lines.len() {
        let line = fm_lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // A new top-level key requires a non-indented `key: …` whose key is an
        // ASCII identifier. Lines that don't match (continuation text, or a value
        // line like `例: …` inside a multi-line description) are appended to the
        // previous value so wrapped descriptions survive intact.
        let key_value = if line.starts_with(' ') || line.starts_with('\t') {
            None
        } else {
            line.find(':')
                .map(|c| (line[..c].trim(), line[c + 1..].trim()))
                .filter(|(k, _)| is_ascii_key(k))
        };

        let Some((key, raw_value)) = key_value else {
            if let Some(k) = &last_key {
                if let Some(Value::String(v)) = map.get_mut(k) {
                    if !v.is_empty() {
                        v.push('\n');
                    }
                    v.push_str(line.trim());
                }
            }
            i += 1;
            continue;
        };

        // Look ahead for indented list items (`- item`).
        let mut list_items: Vec<Value> = Vec::new();
        let mut j = i + 1;
        while j < fm_lines.len() {
            let next = fm_lines[j];
            if next.trim().is_empty() {
                j += 1;
                continue;
            }
            let trimmed = next.trim();
            if (next.starts_with(' ') || next.starts_with('\t')) && trimmed.starts_with("- ") {
                let item = strip_surrounding_quotes(trimmed[2..].trim());
                list_items.push(Value::String(item.to_string()));
                j += 1;
            } else {
                break;
            }
        }

        if !list_items.is_empty() {
            map.insert(key.to_string(), Value::Array(list_items));
            last_key = None;
            i = j;
        } else {
            let value = strip_surrounding_quotes(raw_value);
            map.insert(key.to_string(), Value::String(value.to_string()));
            last_key = Some(key.to_string());
            i += 1;
        }
    }

    Some((map, body))
}

/// True when `s` is an ASCII frontmatter key (e.g. `description`, `allowed-tools`).
/// Excludes non-ASCII text such as `例` so it is treated as a value continuation.
fn is_ascii_key(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Strips a single layer of surrounding `"…"` or `'…'` quotes, if present.
fn strip_surrounding_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Writes frontmatter and body text to a Markdown file.
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

    #[test]
    fn emit_then_parse_round_trip_preserves_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("round_trip.md");

        let mut fm = serde_json::Map::new();
        fm.insert("name".to_string(), Value::String("my-skill".to_string()));
        fm.insert(
            "description".to_string(),
            Value::String("Does something useful".to_string()),
        );
        let body = "\nRun the task.\n";

        emit_frontmatter_file(&path, &fm, body).unwrap();
        let result = parse_frontmatter_file(&path).unwrap();

        assert_eq!(result["frontmatter"]["name"], "my-skill");
        assert_eq!(
            result["frontmatter"]["description"],
            "Does something useful"
        );
        assert!(
            result["body"].as_str().unwrap().contains("Run the task."),
            "body must survive the round-trip"
        );
    }

    #[test]
    fn parse_frontmatter_file_missing_returns_err_with_context() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.md");

        let err = parse_frontmatter_file(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Failed to read file"),
            "error must mention 'Failed to read file', got: {msg}"
        );
    }

    #[test]
    fn emit_frontmatter_file_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("dir").join("out.md");
        let fm = serde_json::Map::new();

        emit_frontmatter_file(&path, &fm, "content\n").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("content"));
    }

    #[test]
    fn parse_frontmatter_file_multiline_body_preserved() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("multi.md");
        fs::write(
            &path,
            "---\nname: multi\n---\nLine one.\nLine two.\nLine three.\n",
        )
        .unwrap();

        let result = parse_frontmatter_file(&path).unwrap();
        let body = result["body"].as_str().unwrap();
        assert!(body.contains("Line one."));
        assert!(body.contains("Line two."));
        assert!(body.contains("Line three."));
    }

    // --- Lenient parsing tests ---

    #[test]
    fn lenient_unquoted_colon_in_description_parses_full_value() {
        // Strict YAML would parse `コンテキスト: ...` as a nested mapping and fail.
        let content = "---\nname: casino-poker-expert\ndescription: カジノポーカーのルール。コンテキスト: ユーザーが実装中。例: \"...\"\nmodel: opus\n---\n\nBody text.\n";
        let result = parse_frontmatter_lenient(content).unwrap();
        let (map, body) = result;
        assert_eq!(map["name"].as_str().unwrap(), "casino-poker-expert");
        assert_eq!(
            map["description"].as_str().unwrap(),
            "カジノポーカーのルール。コンテキスト: ユーザーが実装中。例: \"...\""
        );
        assert_eq!(map["model"].as_str().unwrap(), "opus");
        assert!(body.contains("Body text."));
    }

    #[test]
    fn lenient_multiline_description_continuation_not_a_new_key() {
        // A wrapped description whose continuation lines start with non-ASCII
        // `例:` must append to `description`, not become a new top-level key.
        let content =
            "---\nname: terraform-reviewer\ndescription: Terraform をレビューします。\n例: plan の差分を確認。\nさらに state も検証。\nmodel: opus\n---\n\nBody.\n";
        let (map, _body) = parse_frontmatter_lenient(content).unwrap();
        assert!(map.get("例").is_none(), "`例` must not become a key");
        let desc = map["description"].as_str().unwrap();
        assert!(desc.contains("Terraform をレビューします。"));
        assert!(desc.contains("例: plan の差分を確認。"));
        assert!(desc.contains("さらに state も検証。"));
        assert_eq!(map["model"].as_str().unwrap(), "opus");
    }

    #[test]
    fn lenient_tools_list_yields_string_array() {
        let content =
            "---\nname: my-skill\ntools:\n  - Bash\n  - Read\n  - Edit\n---\n\nDo things.\n";
        let result = parse_frontmatter_lenient(content).unwrap();
        let (map, _body) = result;
        let tools = map["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].as_str().unwrap(), "Bash");
        assert_eq!(tools[1].as_str().unwrap(), "Read");
        assert_eq!(tools[2].as_str().unwrap(), "Edit");
    }

    #[test]
    fn strict_yaml_list_stays_array_not_string() {
        // A well-formed YAML list must parse via strict path and produce an array Value,
        // not be flattened to a string by the lenient path.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("strict_list.md");
        fs::write(
            &path,
            "---\nname: test\nallowed-tools:\n  - Bash\n  - Read\n---\n\nBody.\n",
        )
        .unwrap();

        let result = parse_frontmatter_file(&path).unwrap();
        let tools = result["frontmatter"]["allowed-tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2, "strict path must preserve YAML arrays");
        assert_eq!(tools[0].as_str().unwrap(), "Bash");
    }

    #[test]
    fn no_frontmatter_file_still_returns_empty_map() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.md");
        fs::write(&path, "Just plain text with no frontmatter.\n").unwrap();

        let result = parse_frontmatter_file(&path).unwrap();
        assert!(
            result["frontmatter"].as_object().unwrap().is_empty(),
            "frontmatter must be empty for a file without frontmatter"
        );
        assert!(result["body"].as_str().unwrap().contains("plain text"));
    }

    #[test]
    fn lenient_via_parse_frontmatter_file_round_trip() {
        // parse_frontmatter_file must activate the lenient path for a file that
        // strict YAML cannot parse and return the full description string.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("skill.md");
        fs::write(
            &path,
            "---\nname: poker\ndescription: ルール説明。コンテキスト: 実装中。\nmodel: opus\n---\n\nSkill body.\n",
        )
        .unwrap();

        let result = parse_frontmatter_file(&path).unwrap();
        assert_eq!(result["frontmatter"]["name"].as_str().unwrap(), "poker");
        assert_eq!(
            result["frontmatter"]["description"].as_str().unwrap(),
            "ルール説明。コンテキスト: 実装中。"
        );
        assert_eq!(result["frontmatter"]["model"].as_str().unwrap(), "opus");
        assert!(result["body"].as_str().unwrap().contains("Skill body."));
    }
}
