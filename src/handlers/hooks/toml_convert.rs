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
