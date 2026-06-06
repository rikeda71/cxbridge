use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::core::mappings::MapEntry;

/// Pipeline execution direction (corresponding to the CLI subcommand).
/// A separate type from MappingDirection, which indicates the effective direction of a mappings entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvDir {
    /// Claude → Codex
    C2x,
    /// Codex → Claude
    X2c,
}

/// Type signature for transform functions.
pub(crate) type TransformFn = fn(&Value, &TransformCtx) -> Value;

/// Context information available during transform execution.
pub struct TransformCtx<'a> {
    /// Pipeline execution direction
    pub direction: ConvDir,
    /// Arguments for transforms such as enum_map (populated from TransformSpec.args)
    pub args: Option<HashMap<String, String>>,
    /// The corresponding mappings entry
    pub field: &'a MapEntry,
}

/// Result of parsing a transform specification string.
pub(crate) struct TransformSpec {
    /// Transform name (e.g. "enum_map", "unit:ms_to_sec")
    pub(crate) name: String,
    /// Arguments from the `{...}` block (e.g. `enum_map:{max:xhigh}`)
    pub(crate) args: Option<HashMap<String, String>>,
}

fn tf_ms_to_sec(v: &Value, _ctx: &TransformCtx) -> Value {
    match v {
        Value::Number(n) => {
            let ms = n.as_f64().unwrap_or(0.0);
            let sec = ms / 1000.0;
            Value::Number(
                serde_json::Number::from_f64(sec).unwrap_or_else(|| serde_json::Number::from(0)),
            )
        }
        _ => v.clone(),
    }
}

fn tf_sec_to_ms(v: &Value, _ctx: &TransformCtx) -> Value {
    match v {
        Value::Number(n) => {
            let sec = n.as_f64().unwrap_or(0.0);
            let ms = (sec * 1000.0).round() as i64;
            Value::Number(serde_json::Number::from(ms))
        }
        _ => v.clone(),
    }
}

fn tf_polarity_invert(v: &Value, _ctx: &TransformCtx) -> Value {
    match v {
        Value::Bool(b) => Value::Bool(!b),
        _ => v.clone(),
    }
}

fn tf_enum_map(v: &Value, ctx: &TransformCtx) -> Value {
    let Some(args) = &ctx.args else {
        return v.clone();
    };

    // C2x: direct lookup by string key
    if ctx.direction == ConvDir::C2x {
        if let Value::String(s) = v {
            if let Some(mapped) = args.get(s.as_str()) {
                // mapped value may be a bool literal ("true"/"false") or a string
                return parse_mapped_value(mapped);
            }
        }
        return v.clone();
    }

    // X2c: invert the map (swap keys and values), then match against the actual value
    // e.g. enum_map:{vim:true,normal:false} → X2c: true→"vim", false→"normal"
    for (k, mapped_v) in args {
        let mapped_as_value = parse_mapped_value(mapped_v);
        if &mapped_as_value == v {
            return Value::String(k.clone());
        }
    }
    v.clone()
}

/// Converts the strings "true"/"false" to Value::Bool; all other strings remain Value::String.
fn parse_mapped_value(s: &str) -> Value {
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        other => Value::String(other.to_string()),
    }
}

fn tf_index_shift(v: &Value, ctx: &TransformCtx) -> Value {
    // index_shift: $ARGUMENTS[0]→$1 (C2x: +1) / $1→$ARGUMENTS[0] (X2c: -1)
    // This transform is applied on string values containing argument references
    match v {
        Value::Number(n) => {
            let idx = n.as_i64().unwrap_or(0);
            let shifted = match ctx.direction {
                ConvDir::C2x => idx + 1,
                ConvDir::X2c => idx - 1,
            };
            Value::Number(serde_json::Number::from(shifted))
        }
        _ => v.clone(),
    }
}

fn tf_str_to_list_space(v: &Value, _ctx: &TransformCtx) -> Value {
    match v {
        Value::String(s) => {
            let items: Vec<Value> = s
                .split_whitespace()
                .map(|p| Value::String(p.to_string()))
                .collect();
            Value::Array(items)
        }
        _ => v.clone(),
    }
}

fn tf_list_to_str_space(v: &Value, _ctx: &TransformCtx) -> Value {
    match v {
        Value::Array(arr) => {
            let mut out = String::new();
            for (i, s) in arr.iter().filter_map(|x| x.as_str()).enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(s);
            }
            Value::String(out)
        }
        _ => v.clone(),
    }
}

fn tf_rename(v: &Value, _ctx: &TransformCtx) -> Value {
    // rename: value is passed through unchanged (key differences are resolved in the lower step)
    v.clone()
}

/// Compiled regular expression for extracting the Bearer environment variable (statically initialized).
static RE_BEARER_ENV: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Bearer \$\{([A-Z_][A-Z0-9_]*)\}"#).unwrap());

fn tf_extract_bearer_env(v: &Value, _ctx: &TransformCtx) -> Value {
    match v {
        Value::String(s) => {
            if let Some(caps) = RE_BEARER_ENV.captures(s) {
                Value::String(caps[1].to_string())
            } else {
                v.clone()
            }
        }
        _ => v.clone(),
    }
}

fn tf_path_remap(v: &Value, ctx: &TransformCtx) -> Value {
    match v {
        Value::String(s) => {
            let remapped = match ctx.direction {
                ConvDir::C2x => s
                    .replace(".claude/skills/", ".agents/skills/")
                    .replace("~/.claude/skills/", "~/.agents/skills/"),
                ConvDir::X2c => s
                    .replace(".agents/skills/", ".claude/skills/")
                    .replace("~/.agents/skills/", "~/.claude/skills/"),
            };
            Value::String(remapped)
        }
        _ => v.clone(),
    }
}

fn tf_format_json_to_toml(v: &Value, _ctx: &TransformCtx) -> Value {
    // no-op: handled by the serializer
    v.clone()
}

fn tf_format_toml_to_json(v: &Value, _ctx: &TransformCtx) -> Value {
    // no-op: handled by the serializer
    v.clone()
}

fn tf_inline_imports(v: &Value, _ctx: &TransformCtx) -> Value {
    // no-op: handled by the handler's lower step
    v.clone()
}

static TRANSFORM_REGISTRY: Lazy<HashMap<&'static str, TransformFn>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, TransformFn> = HashMap::new();
    m.insert("unit:ms_to_sec", tf_ms_to_sec);
    m.insert("unit:sec_to_ms", tf_sec_to_ms);
    m.insert("polarity:invert", tf_polarity_invert);
    m.insert("enum_map", tf_enum_map);
    m.insert("index_shift", tf_index_shift);
    // colon-argument alias: "index_shift:+1" in variables.yaml
    m.insert("index_shift:+1", tf_index_shift);
    m.insert("str_to_list:space", tf_str_to_list_space);
    m.insert("list_to_str:space", tf_list_to_str_space);
    m.insert("rename", tf_rename);
    m.insert("extract:bearer_env", tf_extract_bearer_env);
    m.insert("path:remap", tf_path_remap);
    m.insert("format:json_to_toml", tf_format_json_to_toml);
    m.insert("format:toml_to_json", tf_format_toml_to_json);
    m.insert("inline_imports", tf_inline_imports);
    m
});

/// Looks up a TransformFn by name from the static registry.
pub(crate) fn get_transform(name: &str) -> Option<TransformFn> {
    TRANSFORM_REGISTRY.get(name).copied()
}

/// Parses `"unit:ms_to_sec; enum_map:{max:xhigh,high:high}"` into a Vec<TransformSpec>.
/// `{...}` blocks are split into key:value pairs and stored in TransformSpec.args.
pub(crate) fn parse_transform(spec: &str) -> Vec<TransformSpec> {
    let mut results = Vec::new();

    for part in spec.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Look for a `{...}` block
        if let Some(brace_start) = part.find('{') {
            // Extract the name by stripping trailing ':' or whitespace (e.g. "enum_map:{...}" → "enum_map")
            let name = part[..brace_start]
                .trim()
                .trim_end_matches(':')
                .trim()
                .to_string();
            let rest = &part[brace_start..];

            // Parse `{key:val,key2:val2}`
            let args = if let (Some(start), Some(end)) = (rest.find('{'), rest.rfind('}')) {
                let inner = &rest[start + 1..end];
                let mut map = HashMap::new();
                for kv in inner.split(',') {
                    let kv = kv.trim();
                    if let Some(colon) = kv.find(':') {
                        let k = kv[..colon].trim().to_string();
                        let v = kv[colon + 1..].trim().to_string();
                        if !k.is_empty() {
                            map.insert(k, v);
                        }
                    }
                }
                if map.is_empty() {
                    None
                } else {
                    Some(map)
                }
            } else {
                None
            };

            results.push(TransformSpec { name, args });
        } else {
            results.push(TransformSpec {
                name: part.to_string(),
                args: None,
            });
        }
    }

    results
}

/// Inverts a direction-dependent transform name for the x2c direction.
/// Example: "unit:ms_to_sec" → "unit:sec_to_ms" in x2c
/// Example: "str_to_list:space" → "list_to_str:space" in x2c (e.g. mcp.oauth.scopes)
fn invert_transform_for_x2c(name: &str) -> &str {
    match name {
        "unit:ms_to_sec" => "unit:sec_to_ms",
        "unit:sec_to_ms" => "unit:ms_to_sec",
        "str_to_list:space" => "list_to_str:space",
        "list_to_str:space" => "str_to_list:space",
        other => other,
    }
}

/// Applies each TransformSpec in order.
/// Arguments for transforms such as enum_map are injected into TransformCtx.args from TransformSpec.args
/// before calling the TransformFn.
///
/// In the X2c direction, unit:ms_to_sec / unit:sec_to_ms are automatically inverted.
///
/// # Returns
/// `(transformed Value, list of applied transform names)`
pub fn apply_transforms(
    value: &Value,
    spec: Option<&str>,
    ctx: &TransformCtx,
) -> (Value, Vec<String>) {
    let Some(spec_str) = spec else {
        return (value.clone(), Vec::new());
    };

    let specs = parse_transform(spec_str);
    if specs.is_empty() {
        return (value.clone(), Vec::new());
    }

    let mut current = value.clone();
    let mut applied = Vec::new();

    for ts in &specs {
        // In the x2c direction, invert direction-dependent transforms
        let effective_name = if ctx.direction == ConvDir::X2c {
            invert_transform_for_x2c(&ts.name)
        } else {
            &ts.name
        };

        if let Some(tf_fn) = get_transform(effective_name) {
            // Inject TransformSpec.args into TransformCtx.args
            let ctx_with_args = TransformCtx {
                direction: ctx.direction,
                args: ts.args.clone(),
                field: ctx.field,
            };
            current = tf_fn(&current, &ctx_with_args);
            applied.push(effective_name.to_string());
        } else if let Some(tf_fn) = get_transform(&ts.name) {
            // The direction-inverted name was not registered; retry with the original
            // (un-inverted) name so direction-neutral transforms work in both directions
            // without requiring a separate inverted alias.
            let ctx_with_args = TransformCtx {
                direction: ctx.direction,
                args: ts.args.clone(),
                field: ctx.field,
            };
            current = tf_fn(&current, &ctx_with_args);
            applied.push(ts.name.clone());
        }
        // If neither the inverted nor the original name is registered, the transform
        // is silently skipped.
    }

    (current, applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;

    fn dummy_entry() -> MapEntry {
        // Minimal MapEntry for tests (ideally loaded via load_mappings, but transform tests
        // rarely inspect the field contents, so a dummy entry is used here)
        let maps = load_mappings();
        maps["mcp"].entries[0].clone()
    }

    #[test]
    fn test_ms_to_sec() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: &e,
        };
        let result = tf_ms_to_sec(&Value::Number(serde_json::Number::from(60000)), &ctx_val);
        assert_eq!(result.as_f64().unwrap(), 60.0);
    }

    #[test]
    fn test_sec_to_ms() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::X2c,
            args: None,
            field: &e,
        };
        let result = tf_sec_to_ms(
            &Value::Number(serde_json::Number::from_f64(60.0).unwrap()),
            &ctx_val,
        );
        assert_eq!(result.as_i64().unwrap(), 60000);
    }

    #[test]
    fn test_polarity_invert() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: &e,
        };
        let result = tf_polarity_invert(&Value::Bool(true), &ctx_val);
        assert_eq!(result, Value::Bool(false));
        let result2 = tf_polarity_invert(&Value::Bool(false), &ctx_val);
        assert_eq!(result2, Value::Bool(true));
    }

    #[test]
    fn test_enum_map() {
        let e = dummy_entry();
        let mut args = HashMap::new();
        args.insert("max".to_string(), "xhigh".to_string());
        args.insert("high".to_string(), "high".to_string());
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: Some(args),
            field: &e,
        };
        let result = tf_enum_map(&Value::String("max".to_string()), &ctx_val);
        assert_eq!(result, Value::String("xhigh".to_string()));
        // unknown value → passthrough
        let result2 = tf_enum_map(&Value::String("unknown".to_string()), &ctx_val);
        assert_eq!(result2, Value::String("unknown".to_string()));
    }

    #[test]
    fn test_str_to_list_space() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: &e,
        };
        let result = tf_str_to_list_space(
            &Value::String("channels:read chat:write".to_string()),
            &ctx_val,
        );
        assert_eq!(
            result,
            Value::Array(vec![
                Value::String("channels:read".to_string()),
                Value::String("chat:write".to_string()),
            ])
        );
    }

    #[test]
    fn test_list_to_str_space() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::X2c,
            args: None,
            field: &e,
        };
        let result = tf_list_to_str_space(
            &Value::Array(vec![
                Value::String("channels:read".to_string()),
                Value::String("chat:write".to_string()),
            ]),
            &ctx_val,
        );
        assert_eq!(
            result,
            Value::String("channels:read chat:write".to_string())
        );
    }

    #[test]
    fn test_extract_bearer_env() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: &e,
        };
        let result =
            tf_extract_bearer_env(&Value::String("Bearer ${MY_TOKEN}".to_string()), &ctx_val);
        assert_eq!(result, Value::String("MY_TOKEN".to_string()));
        // non-matching string → passthrough
        let result2 = tf_extract_bearer_env(&Value::String("Token ${OTHER}".to_string()), &ctx_val);
        assert_eq!(result2, Value::String("Token ${OTHER}".to_string()));
    }

    #[test]
    fn test_parse_transform_basic() {
        let specs = parse_transform("unit:ms_to_sec");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "unit:ms_to_sec");
        assert!(specs[0].args.is_none());
    }

    #[test]
    fn test_parse_transform_with_args() {
        let specs = parse_transform("enum_map:{max:xhigh,high:high}");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "enum_map");
        let args = specs[0].args.as_ref().unwrap();
        assert_eq!(args.get("max"), Some(&"xhigh".to_string()));
        assert_eq!(args.get("high"), Some(&"high".to_string()));
    }

    #[test]
    fn test_parse_transform_multiple() {
        let specs = parse_transform("unit:ms_to_sec; rename");
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "unit:ms_to_sec");
        assert_eq!(specs[1].name, "rename");
    }

    #[test]
    fn test_apply_transforms_ms_to_sec() {
        let maps = load_mappings();
        // use mcp.timeout entry which has transform: "unit:ms_to_sec"
        let entry = maps["mcp"]
            .entries
            .iter()
            .find(|e| e.id == "mcp.timeout")
            .unwrap();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: entry,
        };
        let (result, applied) = apply_transforms(
            &Value::Number(serde_json::Number::from(60000)),
            entry.transform.as_deref(),
            &ctx_val,
        );
        assert_eq!(result.as_f64().unwrap(), 60.0);
        assert!(applied.contains(&"unit:ms_to_sec".to_string()));
    }

    #[test]
    fn test_apply_transforms_enum_map() {
        let maps = load_mappings();
        // use skills.effort entry which has transform: "enum_map:{max:xhigh}"
        let entry = maps["skills"]
            .entries
            .iter()
            .find(|e| e.id == "skills.effort")
            .unwrap();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: entry,
        };
        let (result, applied) = apply_transforms(
            &Value::String("max".to_string()),
            entry.transform.as_deref(),
            &ctx_val,
        );
        assert_eq!(result, Value::String("xhigh".to_string()));
        assert!(applied.contains(&"enum_map".to_string()));
    }

    #[test]
    fn test_path_remap_c2x() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::C2x,
            args: None,
            field: &e,
        };
        let result = tf_path_remap(
            &Value::String(".claude/skills/deploy/SKILL.md".to_string()),
            &ctx_val,
        );
        assert_eq!(
            result,
            Value::String(".agents/skills/deploy/SKILL.md".to_string())
        );
    }

    #[test]
    fn test_path_remap_x2c() {
        let e = dummy_entry();
        let ctx_val = TransformCtx {
            direction: ConvDir::X2c,
            args: None,
            field: &e,
        };
        let result = tf_path_remap(
            &Value::String(".agents/skills/deploy/SKILL.md".to_string()),
            &ctx_val,
        );
        assert_eq!(
            result,
            Value::String(".claude/skills/deploy/SKILL.md".to_string())
        );
    }

    #[test]
    fn test_get_transform_all_registered() {
        let names = [
            "unit:ms_to_sec",
            "unit:sec_to_ms",
            "polarity:invert",
            "enum_map",
            "index_shift",
            "str_to_list:space",
            "list_to_str:space",
            "rename",
            "extract:bearer_env",
            "path:remap",
            "format:json_to_toml",
            "format:toml_to_json",
            "inline_imports",
        ];
        for name in &names {
            assert!(
                get_transform(name).is_some(),
                "Transform '{}' should be registered",
                name
            );
        }
    }
}
