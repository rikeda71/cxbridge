use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::core::mappings::MapEntry;

/// pipeline の実行方向（CLI サブコマンドに対応）。
/// mappings エントリの有効方向を示す MappingDirection とは別型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvDir {
    /// Claude → Codex
    C2x,
    /// Codex → Claude
    X2c,
}

/// モデルのティア（能力レベル）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    High,
    Mid,
    Low,
}

/// Claude モデル名 → Tier へのマッピング。
/// 未知モデル名は None を返し、呼び出し元が warn を出す。
pub fn claude_tier(m: &str) -> Option<Tier> {
    if m.contains("opus") {
        Some(Tier::High)
    } else if m.contains("sonnet") {
        Some(Tier::Mid)
    } else if m.contains("haiku") {
        Some(Tier::Low)
    } else {
        None
    }
}

/// Codex モデル名 → Tier へのマッピング。
/// 判定: -high/-xhigh → High、-mini → Low、その他 → Mid
pub fn codex_tier(m: &str) -> Option<Tier> {
    if m.ends_with("-high") || m.ends_with("-xhigh") {
        Some(Tier::High)
    } else if m.ends_with("-mini") {
        Some(Tier::Low)
    } else {
        Some(Tier::Mid)
    }
}

// Roundtrip invariant: tier_to_codex(t) must satisfy codex_tier(result) == Some(t),
// and tier_to_claude(t) must satisfy claude_tier(result) == Some(t).
const CODEX_LATEST: &[(Tier, &str)] = &[
    (Tier::High, "gpt-5-codex-high"), // ends_with("-high") → High ✓
    (Tier::Mid, "gpt-5-codex"),       // neither -high/-xhigh nor -mini → Mid ✓
    (Tier::Low, "gpt-5-codex-mini"),  // ends_with("-mini") → Low ✓
];

const CLAUDE_LATEST: &[(Tier, &str)] = &[
    (Tier::High, "claude-opus-4-8"),  // contains("opus") → High ✓
    (Tier::Mid, "claude-sonnet-4-6"), // contains("sonnet") → Mid ✓
    (Tier::Low, "claude-haiku-4-5"),  // contains("haiku") → Low ✓
];

/// Tier → Codex モデル名。CODEX_LATEST から検索する。
pub fn tier_to_codex(t: Tier) -> &'static str {
    CODEX_LATEST
        .iter()
        .find(|(tier, _)| *tier == t)
        .map(|(_, name)| *name)
        .expect("CODEX_LATEST は全 Tier を網羅している必要があります")
}

/// Tier → Claude モデル名。CLAUDE_LATEST から検索する。
pub fn tier_to_claude(t: Tier) -> &'static str {
    CLAUDE_LATEST
        .iter()
        .find(|(tier, _)| *tier == t)
        .map(|(_, name)| *name)
        .expect("CLAUDE_LATEST は全 Tier を網羅している必要があります")
}

/// transform 関数の型シグネチャ。
pub type TransformFn = fn(&Value, &TransformCtx) -> Value;

/// transform 実行時のコンテキスト情報。
pub struct TransformCtx<'a> {
    /// pipeline の実行方向
    pub direction: ConvDir,
    /// enum_map 等の引数（TransformSpec.args から詰める）
    pub args: Option<HashMap<String, String>>,
    /// 対応する mappings エントリ
    pub field: &'a MapEntry,
}

/// transform 指定文字列を分解した結果。
pub struct TransformSpec {
    /// transform 名（例: "enum_map", "unit:ms_to_sec"）
    pub name: String,
    /// 引数（`enum_map:{max:xhigh}` の `{...}` 部分）
    pub args: Option<HashMap<String, String>>,
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

/// 文字列 "true"/"false" は Value::Bool に変換する。それ以外は Value::String のまま。
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
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect();
            Value::String(parts.join(" "))
        }
        _ => v.clone(),
    }
}

fn tf_rename(v: &Value, _ctx: &TransformCtx) -> Value {
    // rename: 値はそのまま（キー差は lower 側で解決）
    v.clone()
}

/// Bearer 環境変数抽出の正規表現（静的初期化）。
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
    // no-op: serializer が処理する
    v.clone()
}

fn tf_format_toml_to_json(v: &Value, _ctx: &TransformCtx) -> Value {
    // no-op: serializer が処理する
    v.clone()
}

fn tf_inline_imports(v: &Value, _ctx: &TransformCtx) -> Value {
    // no-op: handler の lower が処理する
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

/// transform 名から TransformFn を引く（静的レジストリ）。
pub fn get_transform(name: &str) -> Option<TransformFn> {
    TRANSFORM_REGISTRY.get(name).copied()
}

/// `"unit:ms_to_sec; enum_map:{max:xhigh,high:high}"` を分解し Vec<TransformSpec> を返す。
/// `{...}` ブロックは key:value ペアに分解して TransformSpec.args に格納する。
pub fn parse_transform(spec: &str) -> Vec<TransformSpec> {
    let mut results = Vec::new();

    for part in spec.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // `{...}` ブロックを探す
        if let Some(brace_start) = part.find('{') {
            // trailing ':' or whitespace を除いた名前を取得（例: "enum_map:{...}" → "enum_map"）
            let name = part[..brace_start]
                .trim()
                .trim_end_matches(':')
                .trim()
                .to_string();
            let rest = &part[brace_start..];

            // `{key:val,key2:val2}` をパース
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

/// 方向依存の transform 名を x2c 方向用に反転する。
/// 例: "unit:ms_to_sec" → x2c では "unit:sec_to_ms"
/// 例: "str_to_list:space" → x2c では "list_to_str:space"（mcp.oauth.scopes 等）
fn invert_transform_for_x2c(name: &str) -> &str {
    match name {
        "unit:ms_to_sec" => "unit:sec_to_ms",
        "unit:sec_to_ms" => "unit:ms_to_sec",
        "str_to_list:space" => "list_to_str:space",
        "list_to_str:space" => "str_to_list:space",
        other => other,
    }
}

/// 各 TransformSpec を順に適用する。
/// enum_map 等の引数は TransformSpec.args を TransformCtx.args に詰めてから TransformFn を呼ぶ。
///
/// X2c 方向では unit:ms_to_sec / unit:sec_to_ms を自動反転する。
///
/// # 返値
/// `(変換後の Value, 適用した transform 名の一覧)`
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
        // x2c 方向では方向依存 transform を反転する
        let effective_name = if ctx.direction == ConvDir::X2c {
            invert_transform_for_x2c(&ts.name)
        } else {
            &ts.name
        };

        if let Some(tf_fn) = get_transform(effective_name) {
            // TransformSpec.args を TransformCtx.args に注入
            let ctx_with_args = TransformCtx {
                direction: ctx.direction,
                args: ts.args.clone(),
                field: ctx.field,
            };
            current = tf_fn(&current, &ctx_with_args);
            applied.push(effective_name.to_string());
        } else if let Some(tf_fn) = get_transform(&ts.name) {
            // フォールバック: 元の名前で試みる
            let ctx_with_args = TransformCtx {
                direction: ctx.direction,
                args: ts.args.clone(),
                field: ctx.field,
            };
            current = tf_fn(&current, &ctx_with_args);
            applied.push(ts.name.clone());
        }
        // 未知 transform はスキップ（warn は呼び出し元で出す）
    }

    (current, applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::path::Path;

    fn dummy_entry() -> MapEntry {
        // テスト用の最小 MapEntry（load_mappings でロードしたものを使うのが本来だが、
        // transform テストでは field の内容はほぼ参照しないのでダミーを使う）
        let maps = load_mappings(Path::new("mappings"));
        maps["mcp"].entries[0].clone()
    }

    #[test]
    fn test_tier_roundtrip_codex() {
        let _entry = dummy_entry();
        for tier in [Tier::High, Tier::Mid, Tier::Low] {
            let model = tier_to_codex(tier);
            let back = codex_tier(model);
            assert_eq!(
                back,
                Some(tier),
                "codex_tier(tier_to_codex({tier:?})) should be Some({tier:?}), model={model}"
            );
        }
    }

    #[test]
    fn test_tier_roundtrip_claude() {
        for tier in [Tier::High, Tier::Mid, Tier::Low] {
            let model = tier_to_claude(tier);
            let back = claude_tier(model);
            assert_eq!(
                back,
                Some(tier),
                "claude_tier(tier_to_claude({tier:?})) should be Some({tier:?}), model={model}"
            );
        }
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
        let maps = load_mappings(Path::new("mappings"));
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
        let maps = load_mappings(Path::new("mappings"));
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
