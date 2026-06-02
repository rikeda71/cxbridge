use std::path::Path;

use serde_json::Value;
use toml_edit::{Array, DocumentMut, Item, Table};

/// Reads the [hooks] section from Codex config.toml and returns it as a JSON Value.
/// Format: {"path": "...", "hooks": {"EventName": [{matcher, hooks:[{type,...}]}]}}
pub(super) fn parse_codex_hooks_toml(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
    let doc: DocumentMut = content
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse TOML {}: {}", path.display(), e))?;

    let mut hooks_map = serde_json::Map::new();

    if let Some(hooks_item) = doc.get("hooks") {
        if let Some(hooks_tbl) = hooks_item.as_table() {
            for (event_name, event_val) in hooks_tbl {
                if let Some(aot) = event_val.as_array_of_tables() {
                    let mut entries_arr = Vec::new();
                    for entry_tbl in aot {
                        let mut entry_obj = serde_json::Map::new();

                        // matcher
                        if let Some(m) = entry_tbl.get("matcher").and_then(|v| v.as_str()) {
                            entry_obj.insert("matcher".to_string(), Value::String(m.to_string()));
                        }

                        // hooks array-of-tables
                        if let Some(hooks_aot_item) = entry_tbl.get("hooks") {
                            if let Some(hooks_aot) = hooks_aot_item.as_array_of_tables() {
                                let mut hooks_json = Vec::new();
                                for h_tbl in hooks_aot {
                                    let hook_obj = toml_table_to_json(h_tbl);
                                    hooks_json.push(Value::Object(hook_obj));
                                }
                                entry_obj.insert("hooks".to_string(), Value::Array(hooks_json));
                            }
                        }

                        entries_arr.push(Value::Object(entry_obj));
                    }
                    hooks_map.insert(event_name.to_string(), Value::Array(entries_arr));
                }
            }
        }
    }

    Ok(Value::Object({
        let mut root = serde_json::Map::new();
        root.insert(
            "path".to_string(),
            Value::String(path.to_str().unwrap_or("").to_string()),
        );
        root.insert("hooks".to_string(), Value::Object(hooks_map));
        root
    }))
}

/// Converts hooks entries to Codex TOML [[hooks.EventName]] format.
pub(super) fn build_hooks_toml(hooks_entries: &[(String, Value)]) -> anyhow::Result<String> {
    let mut doc = DocumentMut::new();

    // Build the [hooks] table
    let hooks_item = doc.entry("hooks").or_insert(Item::Table(Table::new()));
    let hooks_tbl = hooks_item
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[hooks] is not a table"))?;

    for (event_name, entries) in hooks_entries {
        let arr = match entries.as_array() {
            Some(a) => a,
            None => continue,
        };

        // [[hooks.EventName]] array-of-tables
        let aot_item = hooks_tbl
            .entry(event_name)
            .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        let aot = aot_item
            .as_array_of_tables_mut()
            .ok_or_else(|| anyhow::anyhow!("hooks.{} is not array-of-tables", event_name))?;

        for entry_val in arr {
            let entry_obj = match entry_val.as_object() {
                Some(o) => o,
                None => continue,
            };
            let mut tbl = Table::new();

            // matcher
            if let Some(m) = entry_obj.get("matcher").and_then(|v| v.as_str()) {
                tbl.insert("matcher", toml_edit::value(m));
            }

            // hooks array-of-tables inside the entry
            if let Some(hooks_arr) = entry_obj.get("hooks").and_then(|v| v.as_array()) {
                let mut inner_aot = toml_edit::ArrayOfTables::new();
                for h in hooks_arr {
                    let h_obj = match h.as_object() {
                        Some(o) => o,
                        None => continue,
                    };
                    let mut h_tbl = Table::new();
                    for (k, v) in h_obj {
                        json_value_to_toml_item(v).map(|item| h_tbl.insert(k, item));
                    }
                    inner_aot.push(h_tbl);
                }
                tbl.insert("hooks", Item::ArrayOfTables(inner_aot));
            }

            aot.push(tbl);
        }
    }

    Ok(doc.to_string())
}

/// Converts a JSON Value to a toml_edit Item (best-effort).
pub(super) fn json_value_to_toml_item(v: &Value) -> Option<Item> {
    match v {
        Value::String(s) => Some(toml_edit::value(s.as_str())),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml_edit::value(i))
            } else {
                n.as_f64().map(toml_edit::value)
            }
        }
        Value::Bool(b) => Some(toml_edit::value(*b)),
        Value::Array(arr) => {
            let mut toml_arr = Array::new();
            for item in arr {
                match item {
                    Value::String(s) => toml_arr.push(s.as_str()),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            toml_arr.push(i);
                        }
                    }
                    Value::Bool(b) => toml_arr.push(*b),
                    _ => {}
                }
            }
            Some(toml_edit::value(toml_arr))
        }
        Value::Null => None,
        Value::Object(_) => None, // nested objects not supported for simple values
    }
}

/// Converts a toml_edit Table to a serde_json Map.
pub(super) fn toml_table_to_json(tbl: &Table) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    for (k, v) in tbl {
        if let Some(jv) = toml_item_to_json(v) {
            map.insert(k.to_string(), jv);
        }
    }
    map
}

/// Converts a toml_edit Item to a serde_json Value.
pub(super) fn toml_item_to_json(item: &Item) -> Option<Value> {
    match item {
        Item::Value(v) => toml_value_to_json(v),
        Item::Table(tbl) => {
            let obj = toml_table_to_json(tbl);
            Some(Value::Object(obj))
        }
        Item::ArrayOfTables(aot) => {
            let arr: Vec<Value> = aot
                .iter()
                .map(|t| Value::Object(toml_table_to_json(t)))
                .collect();
            Some(Value::Array(arr))
        }
        Item::None => None,
    }
}

/// Converts a toml_edit::Value to a serde_json::Value.
pub(super) fn toml_value_to_json(tv: &toml_edit::Value) -> Option<Value> {
    use toml_edit::Value as TV;
    match tv {
        TV::String(s) => Some(Value::String(s.value().to_string())),
        TV::Integer(i) => Some(Value::Number(serde_json::Number::from(*i.value()))),
        TV::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number),
        TV::Boolean(b) => Some(Value::Bool(*b.value())),
        TV::Array(arr) => {
            let items: Vec<Value> = arr.iter().filter_map(toml_value_to_json).collect();
            Some(Value::Array(items))
        }
        TV::InlineTable(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, v) in tbl {
                if let Some(jv) = toml_value_to_json(v) {
                    map.insert(k.to_string(), jv);
                }
            }
            Some(Value::Object(map))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── json_value_to_toml_item ────────────────────────────────────────────

    #[test]
    fn json_to_toml_string() {
        let v = Value::String("hello".to_string());
        let item = json_value_to_toml_item(&v).expect("string must convert");
        assert_eq!(
            item.as_value()
                .and_then(|v| v.as_str())
                .expect("must be string"),
            "hello"
        );
    }

    #[test]
    fn json_to_toml_integer() {
        let v = serde_json::json!(42i64);
        let item = json_value_to_toml_item(&v).expect("integer must convert");
        assert_eq!(
            item.as_value()
                .and_then(|v| v.as_integer())
                .expect("must be integer"),
            42
        );
    }

    #[test]
    fn json_to_toml_bool_true() {
        let v = Value::Bool(true);
        let item = json_value_to_toml_item(&v).expect("bool must convert");
        assert!(item
            .as_value()
            .and_then(|v| v.as_bool())
            .expect("must be bool"));
    }

    #[test]
    fn json_to_toml_bool_false() {
        let v = Value::Bool(false);
        let item = json_value_to_toml_item(&v).expect("bool must convert");
        assert!(!item
            .as_value()
            .and_then(|v| v.as_bool())
            .expect("must be bool"));
    }

    #[test]
    fn json_to_toml_string_array() {
        let v = serde_json::json!(["a", "b", "c"]);
        let item = json_value_to_toml_item(&v).expect("array must convert");
        let arr = item
            .as_value()
            .and_then(|v| v.as_array())
            .expect("must be array");
        let strings: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(strings, vec!["a", "b", "c"]);
    }

    #[test]
    fn json_to_toml_mixed_array_skips_objects() {
        // Objects inside an array are silently skipped (not supported for simple values)
        let v = serde_json::json!(["keep", {"nested": "skip"}, "also_keep"]);
        let item = json_value_to_toml_item(&v).expect("mixed array must convert");
        let arr = item
            .as_value()
            .and_then(|v| v.as_array())
            .expect("must be array");
        let strings: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            strings,
            vec!["keep", "also_keep"],
            "nested object must be silently skipped"
        );
    }

    #[test]
    fn json_to_toml_null_returns_none() {
        let v = Value::Null;
        assert!(
            json_value_to_toml_item(&v).is_none(),
            "null must produce None"
        );
    }

    #[test]
    fn json_to_toml_object_returns_none() {
        // Nested objects are not supported via json_value_to_toml_item
        let v = serde_json::json!({"key": "value"});
        assert!(
            json_value_to_toml_item(&v).is_none(),
            "plain object must produce None"
        );
    }

    // ── toml_value_to_json ────────────────────────────────────────────────

    #[test]
    fn toml_value_string_roundtrip() {
        // TOML inline value syntax requires quoted strings
        let tv: toml_edit::Value = "\"world\"".parse().unwrap();
        let jv = toml_value_to_json(&tv).expect("string must convert");
        assert_eq!(jv.as_str(), Some("world"));
    }

    #[test]
    fn toml_value_integer_roundtrip() {
        let tv: toml_edit::Value = "99".parse().unwrap();
        let jv = toml_value_to_json(&tv).expect("integer must convert");
        assert_eq!(jv.as_i64(), Some(99));
    }

    #[test]
    fn toml_value_bool_roundtrip() {
        let tv_true: toml_edit::Value = "true".parse().unwrap();
        let tv_false: toml_edit::Value = "false".parse().unwrap();
        assert!(toml_value_to_json(&tv_true)
            .expect("bool true")
            .as_bool()
            .unwrap());
        assert!(!toml_value_to_json(&tv_false)
            .expect("bool false")
            .as_bool()
            .unwrap());
    }

    #[test]
    fn toml_value_array_of_strings() {
        let doc: toml_edit::DocumentMut = "x = [\"alpha\", \"beta\"]".parse().unwrap();
        let tv = doc.get("x").unwrap().as_value().unwrap();
        let jv = toml_value_to_json(tv).expect("array must convert");
        let arr = jv.as_array().expect("must be array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("alpha"));
        assert_eq!(arr[1].as_str(), Some("beta"));
    }

    #[test]
    fn toml_value_inline_table() {
        let doc: toml_edit::DocumentMut = "x = {key = \"val\", count = 3}".parse().unwrap();
        let tv = doc.get("x").unwrap().as_value().unwrap();
        let jv = toml_value_to_json(tv).expect("inline table must convert");
        let obj = jv.as_object().expect("must be object");
        assert_eq!(obj["key"].as_str(), Some("val"));
        assert_eq!(obj["count"].as_i64(), Some(3));
    }

    // ── toml_table_to_json / toml_item_to_json ────────────────────────────

    #[test]
    fn toml_table_to_json_basic_fields() {
        let doc: toml_edit::DocumentMut = "type = \"command\"\ncommand = \"echo ok\"\ntimeout = 30"
            .parse()
            .unwrap();
        let tbl = doc.as_table();
        let map = toml_table_to_json(tbl);
        assert_eq!(map["type"].as_str(), Some("command"));
        assert_eq!(map["command"].as_str(), Some("echo ok"));
        assert_eq!(map["timeout"].as_i64(), Some(30));
    }

    #[test]
    fn toml_item_to_json_none_item() {
        let item = toml_edit::Item::None;
        assert!(
            toml_item_to_json(&item).is_none(),
            "Item::None must produce None"
        );
    }

    #[test]
    fn toml_item_to_json_table_item() {
        let doc: toml_edit::DocumentMut = "[sub]\nfoo = \"bar\"".parse().unwrap();
        let item = doc.get("sub").unwrap();
        let jv = toml_item_to_json(item).expect("table item must convert");
        let obj = jv.as_object().expect("must be object");
        assert_eq!(obj["foo"].as_str(), Some("bar"));
    }

    #[test]
    fn toml_item_to_json_array_of_tables() {
        let doc: toml_edit::DocumentMut =
            "[[entries]]\ntype = \"command\"\n[[entries]]\ntype = \"shell\""
                .parse()
                .unwrap();
        let item = doc.get("entries").unwrap();
        let jv = toml_item_to_json(item).expect("array-of-tables must convert");
        let arr = jv.as_array().expect("must be array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"].as_str(), Some("command"));
        assert_eq!(arr[1]["type"].as_str(), Some("shell"));
    }

    // ── parse_codex_hooks_toml ────────────────────────────────────────────

    fn write_toml(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_basic_hooks_section() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(
            &dir,
            "config.toml",
            r#"
[[hooks.Stop]]
matcher = ""

[[hooks.Stop.hooks]]
type = "command"
command = "echo done"
"#,
        );

        let val = parse_codex_hooks_toml(&path).unwrap();
        assert_eq!(
            val["path"].as_str().unwrap(),
            path.to_str().unwrap(),
            "path field must match the file path"
        );
        let hooks = &val["hooks"];
        let stop = hooks["Stop"].as_array().expect("Stop must be an array");
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["matcher"].as_str(), Some(""));
        let inner = stop[0]["hooks"]
            .as_array()
            .expect("inner hooks must be array");
        assert_eq!(inner[0]["type"].as_str(), Some("command"));
        assert_eq!(inner[0]["command"].as_str(), Some("echo done"));
    }

    #[test]
    fn parse_multiple_events() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(
            &dir,
            "config.toml",
            r#"
[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "pre-hook"

[[hooks.PostToolUse]]
matcher = "^Edit$"

[[hooks.PostToolUse.hooks]]
type = "command"
command = "post-hook"
"#,
        );

        let val = parse_codex_hooks_toml(&path).unwrap();
        let hooks = &val["hooks"];
        assert!(hooks["PreToolUse"].is_array(), "PreToolUse must be present");
        assert!(
            hooks["PostToolUse"].is_array(),
            "PostToolUse must be present"
        );
        assert_eq!(
            hooks["PreToolUse"][0]["hooks"][0]["command"].as_str(),
            Some("pre-hook")
        );
        assert_eq!(
            hooks["PostToolUse"][0]["hooks"][0]["command"].as_str(),
            Some("post-hook")
        );
    }

    #[test]
    fn parse_no_hooks_section_returns_empty_hooks_object() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(&dir, "config.toml", "[model]\nname = \"codex-mini\"\n");

        let val = parse_codex_hooks_toml(&path).unwrap();
        let hooks = &val["hooks"];
        assert!(
            hooks.is_object(),
            "hooks must be an object even when absent"
        );
        assert_eq!(
            hooks.as_object().unwrap().len(),
            0,
            "hooks object must be empty when [hooks] section is absent"
        );
    }

    #[test]
    fn parse_empty_file_returns_empty_hooks() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(&dir, "config.toml", "");

        let val = parse_codex_hooks_toml(&path).unwrap();
        assert!(val["hooks"].as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_nonexistent_file_returns_err() {
        let result = parse_codex_hooks_toml(std::path::Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err(), "missing file must return an error");
    }

    #[test]
    fn parse_invalid_toml_returns_err() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(&dir, "config.toml", "[[not valid toml %%");

        let result = parse_codex_hooks_toml(&path);
        assert!(result.is_err(), "invalid TOML must return an error");
    }

    #[test]
    fn parse_hook_with_timeout_and_bool_fields() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(
            &dir,
            "config.toml",
            r#"
[[hooks.Stop]]
matcher = ""

[[hooks.Stop.hooks]]
type = "command"
command = "echo ok"
timeout = 60
async = true
"#,
        );

        let val = parse_codex_hooks_toml(&path).unwrap();
        let inner = &val["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(inner["timeout"].as_i64(), Some(60));
        assert_eq!(inner["async"].as_bool(), Some(true));
    }

    // ── build_hooks_toml ─────────────────────────────────────────────────

    #[test]
    fn build_single_event_toml() {
        let entries = vec![(
            "Stop".to_string(),
            serde_json::json!([{
                "matcher": "",
                "hooks": [{ "type": "command", "command": "echo done" }]
            }]),
        )];

        let toml_str = build_hooks_toml(&entries).unwrap();
        assert!(
            toml_str.contains("[[hooks.Stop]]"),
            "must contain [[hooks.Stop]]: {}",
            toml_str
        );
        assert!(
            toml_str.contains("matcher"),
            "must contain matcher: {}",
            toml_str
        );
        assert!(
            toml_str.contains("echo done"),
            "must contain command: {}",
            toml_str
        );
    }

    #[test]
    fn build_multiple_events_toml() {
        let entries = vec![
            (
                "PreToolUse".to_string(),
                serde_json::json!([{
                    "matcher": "^Bash$",
                    "hooks": [{ "type": "command", "command": "pre" }]
                }]),
            ),
            (
                "PostToolUse".to_string(),
                serde_json::json!([{
                    "matcher": "^Edit$",
                    "hooks": [{ "type": "command", "command": "post" }]
                }]),
            ),
        ];

        let toml_str = build_hooks_toml(&entries).unwrap();
        assert!(toml_str.contains("[[hooks.PreToolUse]]"));
        assert!(toml_str.contains("[[hooks.PostToolUse]]"));
        assert!(toml_str.contains("pre"));
        assert!(toml_str.contains("post"));
    }

    #[test]
    fn build_empty_entries_produces_hooks_section() {
        let toml_str = build_hooks_toml(&[]).unwrap();
        // An empty build still produces a valid TOML document (at minimum the [hooks] header)
        assert!(
            toml_str.contains("[hooks]"),
            "empty entries must still produce a [hooks] table: {}",
            toml_str
        );
    }

    #[test]
    fn build_skips_non_array_entry_values() {
        // A non-array value for an event must be silently skipped (guarded by `match entries.as_array()`)
        let entries = vec![("Stop".to_string(), serde_json::json!("not-an-array"))];
        let toml_str = build_hooks_toml(&entries).unwrap();
        // Stop is not written because its value is not an array
        assert!(
            !toml_str.contains("[[hooks.Stop]]"),
            "non-array event value must be skipped: {}",
            toml_str
        );
    }

    // ── round-trip: JSON → TOML → JSON ────────────────────────────────────

    #[test]
    fn roundtrip_json_to_toml_and_back() {
        let entries = vec![(
            "Stop".to_string(),
            serde_json::json!([{
                "matcher": "^done$",
                "hooks": [{ "type": "command", "command": "echo roundtrip" }]
            }]),
        )];

        let toml_str = build_hooks_toml(&entries).unwrap();

        // Write the TOML to a temp file and parse it back
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, &toml_str).unwrap();

        let val = parse_codex_hooks_toml(&path).unwrap();
        let stop = val["hooks"]["Stop"].as_array().expect("Stop must be array");
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["matcher"].as_str(), Some("^done$"));
        let inner = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(inner[0]["type"].as_str(), Some("command"));
        assert_eq!(inner[0]["command"].as_str(), Some("echo roundtrip"));
    }

    #[test]
    fn roundtrip_preserves_numeric_and_bool_hook_fields() {
        let entries = vec![(
            "Stop".to_string(),
            serde_json::json!([{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": "echo ok",
                    "timeout": 120i64,
                    "async": true
                }]
            }]),
        )];

        let toml_str = build_hooks_toml(&entries).unwrap();

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, &toml_str).unwrap();

        let val = parse_codex_hooks_toml(&path).unwrap();
        let inner = &val["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(
            inner["timeout"].as_i64(),
            Some(120),
            "timeout must round-trip"
        );
        assert_eq!(
            inner["async"].as_bool(),
            Some(true),
            "async must round-trip"
        );
    }

    #[test]
    fn roundtrip_multiple_matcher_entries_for_same_event() {
        let entries = vec![(
            "PreToolUse".to_string(),
            serde_json::json!([
                {
                    "matcher": "^Bash$",
                    "hooks": [{ "type": "command", "command": "hook-a" }]
                },
                {
                    "matcher": "^Edit$",
                    "hooks": [{ "type": "command", "command": "hook-b" }]
                }
            ]),
        )];

        let toml_str = build_hooks_toml(&entries).unwrap();

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, &toml_str).unwrap();

        let val = parse_codex_hooks_toml(&path).unwrap();
        let pre = val["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse must be array");
        assert_eq!(pre.len(), 2, "both matcher entries must round-trip");
        let matchers: Vec<&str> = pre.iter().map(|e| e["matcher"].as_str().unwrap()).collect();
        assert!(matchers.contains(&"^Bash$"), "^Bash$ must be present");
        assert!(matchers.contains(&"^Edit$"), "^Edit$ must be present");
    }
}
