use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::Deserialize;

use crate::core::ir::Loss;
use crate::core::transforms::ConvDir;

/// One entry from the mappings YAML.
#[derive(Debug, Clone, Deserialize)]
pub struct MapEntry {
    pub id: String,
    pub(crate) claude: Option<FieldSpec>,
    pub(crate) codex: Option<FieldSpec>,
    /// Effective direction declared in mappings YAML (Both/ClaudeToCodex/CodexToClaude).
    /// A separate type from the pipeline direction (ConvDir).
    pub(crate) direction: MappingDirection,
    pub(crate) loss: LossSpec,
    pub(crate) degrade: Option<DegradeSpec>,
    /// Transform specification string (e.g. "unit:ms_to_sec; rename", semicolon-separated)
    pub transform: Option<String>,
    pub warn: Option<bool>,
    pub notes: Option<String>,
}

/// Schema information for a field (claude side / codex side respectively).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FieldSpec {
    pub(crate) field: Option<String>,
    /// Type annotation from the mappings YAML; parsed for schema completeness, used in tests.
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub(crate) r#type: Option<String>,
    pub(crate) scope: Option<String>,
}

/// Effective conversion direction for an entry in the mappings YAML.
/// A separate type from the pipeline direction (ConvDir).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MappingDirection {
    Both,
    ClaudeToCodex,
    CodexToClaude,
}

/// Loss level declaration (value in mappings YAML).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LossSpec {
    Lossless,
    Lossy,
    Dropped,
}

impl From<&LossSpec> for Loss {
    fn from(spec: &LossSpec) -> Self {
        match spec {
            LossSpec::Lossless => Loss::Lossless,
            LossSpec::Lossy => Loss::Lossy,
            LossSpec::Dropped => Loss::Dropped,
        }
    }
}

/// Degrade specification.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DegradeSpec {
    /// Kind of degrade destination
    pub(crate) to: String,
    /// Degrade destination target
    pub(crate) target: String,
}

/// File formats handled by a domain. Both `claude` and `codex` may support multiple formats
/// (e.g. Codex hooks accept either TOML or JSON). Accepts both scalar strings and lists.
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub(crate) struct FormatSpec {
    #[serde(default, deserialize_with = "string_or_seq")]
    pub(crate) claude: Vec<String>,
    #[serde(default, deserialize_with = "string_or_seq")]
    pub(crate) codex: Vec<String>,
}

/// Accepts either a single string or a list of strings as `Vec<String>`.
fn string_or_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrSeq;

    impl<'de> serde::de::Visitor<'de> for StringOrSeq {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_string()])
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                out.push(item);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(StringOrSeq)
}

/// Per-domain mapping (e.g. skills, mcp, hooks).
#[derive(Debug, Clone, Deserialize)]
pub struct DomainMap {
    pub domain: String,
    /// Format spec deserialized from YAML; validated in tests but not consumed in production code.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) format: Option<FormatSpec>,
    pub entries: Vec<MapEntry>,
}

// Embed all 8 files at compile time
const EMBEDDED_MAPPINGS: &[(&str, &str)] = &[
    ("hooks.yaml", include_str!("../../mappings/hooks.yaml")),
    ("mcp.yaml", include_str!("../../mappings/mcp.yaml")),
    ("memory.yaml", include_str!("../../mappings/memory.yaml")),
    ("plugins.yaml", include_str!("../../mappings/plugins.yaml")),
    (
        "settings-config.yaml",
        include_str!("../../mappings/settings-config.yaml"),
    ),
    ("skills.yaml", include_str!("../../mappings/skills.yaml")),
    (
        "subagents.yaml",
        include_str!("../../mappings/subagents.yaml"),
    ),
    (
        "variables.yaml",
        include_str!("../../mappings/variables.yaml"),
    ),
];

/// Domain map set parsed once from the embedded YAML on first access.
///
/// The mappings are static data that never change at runtime, so parsing and
/// validating them a single time avoids redoing that work on every conversion.
static MAPPINGS: Lazy<HashMap<String, DomainMap>> = Lazy::new(build_mappings);

/// Returns the shared domain map set (parsed lazily on first access).
pub fn load_mappings() -> &'static HashMap<String, DomainMap> {
    &MAPPINGS
}

/// Parses and validates all domain maps from the YAML embedded at compile time.
///
/// Asserts startup invariants:
/// - id uniqueness
/// - valid values for direction/loss
/// - degrade implies loss:lossy
/// - loss:dropped must have no transform
fn build_mappings() -> HashMap<String, DomainMap> {
    let mut maps: HashMap<String, DomainMap> = HashMap::new();
    let mut all_ids: HashMap<String, String> = HashMap::new(); // id → filename

    for (filename, content) in EMBEDDED_MAPPINGS {
        let dm: DomainMap = serde_saphyr::from_str(content)
            .unwrap_or_else(|e| panic!("Failed to parse mappings file {filename}: {e}"));

        // Verify id uniqueness
        for entry in &dm.entries {
            if let Some(prev_file) = all_ids.get(&entry.id) {
                panic!(
                    "Duplicate mapping id '{}' found in both {} and {}",
                    entry.id, prev_file, filename
                );
            }
            all_ids.insert(entry.id.clone(), filename.to_string());

            // Verify degrade => loss:lossy
            if entry.degrade.is_some() && !matches!(entry.loss, LossSpec::Lossy) {
                panic!(
                    "Mapping id '{}' has degrade but loss is not 'lossy' (in {})",
                    entry.id, filename
                );
            }

            // Verify loss:dropped has no transform
            if matches!(entry.loss, LossSpec::Dropped) && entry.transform.is_some() {
                panic!(
                    "Mapping id '{}' has loss:dropped but also has transform (in {})",
                    entry.id, filename
                );
            }
        }

        maps.insert(dm.domain.clone(), dm);
    }

    maps
}

/// Field names that start with an ASCII "(" or a fullwidth left parenthesis (U+FF08) are
/// placeholders, not real field keys, and must be skipped when building lookup indexes.
pub(crate) fn is_pseudo_field(field: &str) -> bool {
    field.starts_with('(') || field.starts_with('\u{FF08}')
}

/// Builds a lookup index from claude field name → MapEntry for the C2x lift direction.
///
/// Entries whose direction excludes C2x are skipped so they cannot shadow
/// direction-compatible entries that share the same claude field name.
pub(crate) fn index_by_claude_field(dm: &DomainMap) -> HashMap<String, &MapEntry> {
    let mut idx = HashMap::new();
    for entry in &dm.entries {
        if !applies_direction(entry, ConvDir::C2x) {
            continue;
        }
        if let Some(spec) = &entry.claude {
            if let Some(field) = &spec.field {
                if !is_pseudo_field(field) {
                    idx.insert(field.clone(), entry);
                }
            }
        }
    }
    idx
}

/// Builds a lookup index from codex field name → MapEntry for the X2c lift direction.
///
/// Entries whose direction excludes X2c are skipped so they cannot shadow
/// direction-compatible entries that share the same codex field name.
pub(crate) fn index_by_codex_field(dm: &DomainMap) -> HashMap<String, &MapEntry> {
    let mut idx = HashMap::new();
    for entry in &dm.entries {
        if !applies_direction(entry, ConvDir::X2c) {
            continue;
        }
        if let Some(spec) = &entry.codex {
            if let Some(field) = &spec.field {
                if !is_pseudo_field(field) {
                    idx.insert(field.clone(), entry);
                }
            }
        }
    }
    idx
}

/// Checks whether an entry should be applied for the given pipeline direction.
pub(crate) fn applies_direction(entry: &MapEntry, dir: ConvDir) -> bool {
    matches!(
        (&entry.direction, dir),
        (MappingDirection::Both, _)
            | (MappingDirection::ClaudeToCodex, ConvDir::C2x)
            | (MappingDirection::CodexToClaude, ConvDir::X2c)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_mappings_invariants() {
        // Verify that load_mappings satisfies invariants (panics on failure)
        let maps = load_mappings();
        assert!(!maps.is_empty(), "mappings should not be empty");
    }

    #[test]
    fn test_id_uniqueness() {
        let maps = load_mappings();
        let mut all_ids = std::collections::HashSet::new();
        for dm in maps.values() {
            for entry in &dm.entries {
                assert!(
                    all_ids.insert(entry.id.clone()),
                    "Duplicate id: {}",
                    entry.id
                );
            }
        }
    }

    #[test]
    fn test_degrade_implies_lossy() {
        let maps = load_mappings();
        for dm in maps.values() {
            for entry in &dm.entries {
                if entry.degrade.is_some() {
                    assert!(
                        matches!(entry.loss, LossSpec::Lossy),
                        "Entry {} has degrade but loss is not lossy",
                        entry.id
                    );
                }
            }
        }
    }

    #[test]
    fn test_dropped_has_no_transform() {
        let maps = load_mappings();
        for dm in maps.values() {
            for entry in &dm.entries {
                if matches!(entry.loss, LossSpec::Dropped) {
                    assert!(
                        entry.transform.is_none(),
                        "Entry {} has loss:dropped but also has transform",
                        entry.id
                    );
                }
            }
        }
    }

    #[test]
    fn test_index_by_claude_field_skips_pseudo() {
        let maps = load_mappings();
        for dm in maps.values() {
            let idx = index_by_claude_field(dm);
            for key in idx.keys() {
                assert!(
                    !is_pseudo_field(key),
                    "Pseudo field {} should be skipped",
                    key
                );
            }
        }
    }

    /// When two entries share the same claude.field but have different directions
    /// (e.g. skills.disable-model-invocation=both and
    /// skills.openai-yaml.allow_implicit_invocation=codex_to_claude both map
    /// to claude field "disable-model-invocation"), index_by_claude_field must
    /// return the entry that applies to C2x, not the one that doesn't.
    #[test]
    fn test_index_by_claude_field_direction_collision_c2x() {
        let maps = load_mappings();
        let skills_dm = &maps["skills"];
        let idx = index_by_claude_field(skills_dm);
        let entry = idx
            .get("disable-model-invocation")
            .expect("disable-model-invocation must be in index");
        assert_eq!(
            entry.id, "skills.disable-model-invocation",
            "C2x index must resolve to the 'both' entry, not codex_to_claude"
        );
        assert!(
            applies_direction(entry, ConvDir::C2x),
            "Resolved entry must apply to C2x direction"
        );
    }

    #[test]
    fn test_index_by_codex_field_skips_pseudo() {
        let maps = load_mappings();
        for dm in maps.values() {
            let idx = index_by_codex_field(dm);
            for key in idx.keys() {
                assert!(
                    !is_pseudo_field(key),
                    "Pseudo field {} should be skipped",
                    key
                );
            }
        }
    }

    #[test]
    fn test_all_domains_loaded() {
        let maps = load_mappings();
        let expected = [
            "hooks",
            "mcp",
            "memory",
            "plugins",
            "settings-config",
            "skills",
            "subagents",
            "variables",
        ];
        for domain in &expected {
            assert!(maps.contains_key(*domain), "Missing domain: {}", domain);
        }
    }

    #[test]
    fn test_format_parsed_as_list() {
        let maps = load_mappings();

        // format must parse as a non-empty list for every domain
        for (domain, dm) in maps {
            let format = dm
                .format
                .as_ref()
                .unwrap_or_else(|| panic!("domain {domain} missing format"));
            assert!(
                !format.claude.is_empty(),
                "domain {domain} has empty claude format"
            );
            assert!(
                !format.codex.is_empty(),
                "domain {domain} has empty codex format"
            );
        }

        // Codex hooks supports multiple formats: TOML and JSON
        let hooks = &maps["hooks"].format.as_ref().unwrap().codex;
        assert!(hooks.contains(&"toml".to_string()));
        assert!(hooks.contains(&"json".to_string()));
    }
}
