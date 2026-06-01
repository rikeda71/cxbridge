// TOML reads/writes use toml_edit::DocumentMut directly; no custom emitter needed.

pub mod frontmatter;
pub mod json;

/// Converts a `toml::Value` to a `serde_json::Value`.
///
/// All TOML types map 1:1 to JSON types; `Datetime` is stringified.
/// Non-finite TOML floats (NaN, infinity) are not representable in JSON and
/// fall back to 0.0 via `from_f64` returning `None`.
pub fn toml_to_json(v: &toml::Value) -> anyhow::Result<serde_json::Value> {
    match v {
        toml::Value::String(s) => Ok(serde_json::Value::String(s.clone())),
        toml::Value::Integer(i) => Ok(serde_json::Value::Number(serde_json::Number::from(*i))),
        toml::Value::Float(f) => Ok(serde_json::Value::Number(
            serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)),
        )),
        toml::Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        toml::Value::Array(arr) => {
            let items: anyhow::Result<Vec<serde_json::Value>> =
                arr.iter().map(toml_to_json).collect();
            Ok(serde_json::Value::Array(items?))
        }
        toml::Value::Table(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, val) in tbl {
                map.insert(k.clone(), toml_to_json(val)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        toml::Value::Datetime(dt) => Ok(serde_json::Value::String(dt.to_string())),
    }
}
