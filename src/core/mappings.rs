use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::core::transforms::ConvDir;

/// mappings YAML の1エントリ。
#[derive(Debug, Clone, Deserialize)]
pub struct MapEntry {
    pub id: String,
    pub claude: Option<FieldSpec>,
    pub codex: Option<FieldSpec>,
    /// mappings YAML 上の有効方向宣言（Both/ClaudeToCodex/CodexToClaude）。
    /// pipeline 方向（ConvDir）とは別型。
    pub direction: MappingDirection,
    pub loss: LossSpec,
    pub degrade: Option<DegradeSpec>,
    /// transform 指定文字列（例: "unit:ms_to_sec; rename"、セミコロン区切り）
    pub transform: Option<String>,
    pub warn: Option<bool>,
    pub notes: Option<String>,
}

/// フィールドのスキーマ情報（claude 側 / codex 側それぞれ）。
#[derive(Debug, Clone, Deserialize)]
pub struct FieldSpec {
    pub field: Option<String>,
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    pub scope: Option<String>,
}

/// mappings YAML 上のエントリが有効な変換方向。
/// pipeline 方向（ConvDir）とは別型。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingDirection {
    Both,
    ClaudeToCodex,
    CodexToClaude,
}

/// 損失レベルの宣言（mappings YAML 上の値）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LossSpec {
    Lossless,
    Lossy,
    Dropped,
}

/// 降格仕様。
#[derive(Debug, Clone, Deserialize)]
pub struct DegradeSpec {
    /// 降格先の種別
    pub to: String,
    /// 降格先ターゲット
    pub target: String,
}

/// ドメインが扱うファイル形式。`claude` / `codex` はそれぞれ複数形式を取りうる
/// （例: Codex hooks は TOML または JSON）。スカラー文字列・リストの両方を受け付ける。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FormatSpec {
    #[serde(default, deserialize_with = "string_or_seq")]
    pub claude: Vec<String>,
    #[serde(default, deserialize_with = "string_or_seq")]
    pub codex: Vec<String>,
}

/// 単一文字列とリストのどちらでも `Vec<String>` として受け付ける。
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

/// ドメイン単位のマッピング（例: skills, mcp, hooks）。
#[derive(Debug, Clone, Deserialize)]
pub struct DomainMap {
    pub domain: String,
    #[serde(default)]
    pub format: Option<FormatSpec>,
    pub entries: Vec<MapEntry>,
}

// 全 8 ファイルをコンパイル時に埋め込む
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

/// 全 YAML ファイルを読み込み、domain → DomainMap の HashMap を返す。
///
/// 起動時不変条件を assert する:
/// - id 一意性
/// - direction/loss の値域
/// - degrade⇒loss:lossy
/// - loss:dropped に transform なし
pub fn load_mappings(_dir: &Path) -> HashMap<String, DomainMap> {
    let mut maps: HashMap<String, DomainMap> = HashMap::new();
    let mut all_ids: HashMap<String, String> = HashMap::new(); // id → filename

    for (filename, content) in EMBEDDED_MAPPINGS {
        let dm: DomainMap = serde_saphyr::from_str(content)
            .unwrap_or_else(|e| panic!("Failed to parse mappings file {filename}: {e}"));

        // id 一意性の検証
        for entry in &dm.entries {
            if let Some(prev_file) = all_ids.get(&entry.id) {
                panic!(
                    "Duplicate mapping id '{}' found in both {} and {}",
                    entry.id, prev_file, filename
                );
            }
            all_ids.insert(entry.id.clone(), filename.to_string());

            // degrade => loss:lossy の検証
            if entry.degrade.is_some() && !matches!(entry.loss, LossSpec::Lossy) {
                panic!(
                    "Mapping id '{}' has degrade but loss is not 'lossy' (in {})",
                    entry.id, filename
                );
            }

            // loss:dropped に transform がないことの検証
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

/// 全角括弧（U+FF08）で始まるフィールド名はプレースホルダであり実際のキーではない。
/// インデックスからスキップする。
fn is_pseudo_field(field: &str) -> bool {
    field.starts_with('\u{FF08}') // '（'
}

/// lift 時に「このフィールドはどの id か」を引く索引を構築する。
/// ConvDir::C2x（Claude→Codex）なら claude フィールド名、
/// ConvDir::X2c（Codex→Claude）なら codex フィールド名で索引。
pub fn index_by_claude_field(dm: &DomainMap) -> HashMap<String, &MapEntry> {
    let mut idx = HashMap::new();
    for entry in &dm.entries {
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

pub fn index_by_codex_field(dm: &DomainMap) -> HashMap<String, &MapEntry> {
    let mut idx = HashMap::new();
    for entry in &dm.entries {
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

/// MappingDirection と実行方向 ConvDir を照合し、このエントリを適用すべきか判定する。
pub fn applies_direction(entry: &MapEntry, dir: ConvDir) -> bool {
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
        // load_mappings が不変条件を満たすことを検証（失敗したら panic）
        let maps = load_mappings(Path::new("mappings"));
        assert!(!maps.is_empty(), "mappings should not be empty");
    }

    #[test]
    fn test_id_uniqueness() {
        let maps = load_mappings(Path::new("mappings"));
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
        let maps = load_mappings(Path::new("mappings"));
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
        let maps = load_mappings(Path::new("mappings"));
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
        let maps = load_mappings(Path::new("mappings"));
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

    #[test]
    fn test_index_by_codex_field_skips_pseudo() {
        let maps = load_mappings(Path::new("mappings"));
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
        let maps = load_mappings(Path::new("mappings"));
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
        let maps = load_mappings(Path::new("mappings"));

        // すべてのドメインで format が非空のリストとして読める
        for (domain, dm) in &maps {
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

        // Codex hooks は TOML または JSON の複数形式
        let hooks = &maps["hooks"].format.as_ref().unwrap().codex;
        assert!(hooks.contains(&"toml".to_string()));
        assert!(hooks.contains(&"json".to_string()));
    }
}
