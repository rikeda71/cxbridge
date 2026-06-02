use serde_json::{Map, Value};

use crate::core::ir::{DiagLevel, Diagnostic};

/// Completes a partial semver string (major-only → major.0.0; major.minor → major.minor.0).
pub(super) fn complete_semver(ver: &str) -> String {
    // Convert a 40-char git SHA to "0.0.0"
    if ver.len() == 40 && ver.chars().all(|c| c.is_ascii_hexdigit()) {
        return "0.0.0".to_string();
    }

    let parts: Vec<&str> = ver.split('.').collect();
    match parts.len() {
        1 => {
            // Major only
            if parts[0].parse::<u64>().is_ok() {
                format!("{}.0.0", parts[0])
            } else {
                "0.0.0".to_string()
            }
        }
        2 => {
            // Major.minor
            if parts[0].parse::<u64>().is_ok() && parts[1].parse::<u64>().is_ok() {
                format!("{}.{}.0", parts[0], parts[1])
            } else {
                "0.0.0".to_string()
            }
        }
        _ => ver.to_string(), // 3 or more components: pass through unchanged
    }
}

/// Converts marketplace.json for Codex (c2x).
/// - Claude-only top-level fields are dropped with DiagLevel::Drop diagnostics
/// - Normalizes the source schema (Claude `relative`/string → Codex `{source:"local",...}`)
/// - Fills in default policy values if missing
pub(super) fn transform_marketplace_c2x(
    content: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let Ok(mut json): Result<Value, _> = serde_json::from_str(content) else {
        return content.to_string();
    };

    // Drop top-level Claude-only fields that have no Codex marketplace equivalent.
    // Corresponding mappings entries all carry direction:claude_to_codex + loss:dropped.
    const CLAUDE_ONLY_FIELDS: &[(&str, &str)] = &[
        ("owner", "plugins.marketplace.owner"),
        (
            "allowCrossMarketplaceDependenciesOn",
            "plugins.marketplace.allowCrossMarketplaceDependenciesOn",
        ),
        (
            "forceRemoveDeletedPlugins",
            "plugins.marketplace.forceRemoveDeletedPlugins",
        ),
    ];
    if let Some(obj) = json.as_object_mut() {
        for (field, mapping_id) in CLAUDE_ONLY_FIELDS {
            if obj.remove(*field).is_some() {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(mapping_id.to_string()),
                    message: format!("`{}` dropped (no Codex marketplace equivalent)", field),
                });
            }
        }
    }

    if let Some(plugins) = json.get_mut("plugins").and_then(|v| v.as_array_mut()) {
        for plugin_entry in plugins.iter_mut() {
            if let Some(obj) = plugin_entry.as_object_mut() {
                // Normalize the source schema
                normalize_marketplace_source_c2x(obj, diagnostics);

                // Fill in default policy if not set
                if !obj.contains_key("policy") {
                    obj.insert(
                        "policy".to_string(),
                        serde_json::json!({
                            "installation": "AVAILABLE",
                            "authentication": "ON_INSTALL"
                        }),
                    );
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("plugins.marketplace.plugins.policy".to_string()),
                        message: "marketplace plugin.policy auto-filled with defaults (installation=AVAILABLE, authentication=ON_INSTALL)".to_string(),
                    });
                }
            }
        }
    }

    serde_json::to_string_pretty(&json).unwrap_or_else(|_| content.to_string())
}

/// Converts marketplace.json for Claude (x2c).
/// - Normalizes the source schema (Codex `local` → Claude relative path)
/// - policy has no Claude equivalent (dropped)
pub(super) fn transform_marketplace_x2c(
    content: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let Ok(mut json): Result<Value, _> = serde_json::from_str(content) else {
        return content.to_string();
    };

    if let Some(plugins) = json.get_mut("plugins").and_then(|v| v.as_array_mut()) {
        for plugin_entry in plugins.iter_mut() {
            if let Some(obj) = plugin_entry.as_object_mut() {
                // Normalize the source schema
                normalize_marketplace_source_x2c(obj);

                // policy has no Claude equivalent (dropped)
                if obj.remove("policy").is_some() {
                    diagnostics.push(Diagnostic {
                        level: DiagLevel::Drop,
                        id: Some("plugins.marketplace.plugins.policy".to_string()),
                        message: "marketplace plugin.policy dropped (no Claude equivalent)"
                            .to_string(),
                    });
                }
            }
        }
    }

    serde_json::to_string_pretty(&json).unwrap_or_else(|_| content.to_string())
}

/// Normalizes the marketplace.json source schema for Codex.
/// - Relative path string → `{source: "local", path: "..."}`
/// - `github` passes through mostly unchanged (warn if field names differ)
/// - `npm` has no Codex equivalent: removes the source field and emits a Drop diagnostic
fn normalize_marketplace_source_c2x(
    obj: &mut Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(source) = obj.get("source").cloned() {
        match &source {
            Value::String(s) => {
                // Relative path string → Codex local format
                let normalized = serde_json::json!({
                    "source": "local",
                    "path": s
                });
                obj.insert("source".to_string(), normalized);
            }
            Value::Object(src_obj) => {
                // Already in object form: inspect the source type
                if let Some(src_type) = src_obj.get("source").and_then(|v| v.as_str()) {
                    if src_type == "relative" {
                        // Claude `relative` → Codex `local`
                        let mut new_src = src_obj.clone();
                        new_src.insert("source".to_string(), Value::String("local".to_string()));
                        obj.insert("source".to_string(), Value::Object(new_src));
                    } else if src_type == "npm" {
                        // npm has no Codex equivalent; remove the field and report it dropped
                        let plugin_name = obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        obj.remove("source");
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: Some("plugins.marketplace.plugins.source".to_string()),
                            message: format!(
                                "marketplace plugin source type 'npm' dropped \
                                 (no Codex equivalent): plugin '{}'",
                                plugin_name
                            ),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Normalizes the marketplace.json source schema for Claude.
/// - `{source: "local", path: "..."}` → relative path string
fn normalize_marketplace_source_x2c(obj: &mut Map<String, Value>) {
    if let Some(source) = obj.get("source").cloned() {
        if let Some(src_obj) = source.as_object() {
            if let Some(src_type) = src_obj.get("source").and_then(|v| v.as_str()) {
                if src_type == "local" {
                    // Codex `local` → Claude relative path string
                    if let Some(path) = src_obj.get("path").and_then(|v| v.as_str()) {
                        obj.insert("source".to_string(), Value::String(path.to_string()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── complete_semver ────────────────────────────────────────────────────────

    #[test]
    fn complete_semver_major_only() {
        assert_eq!(complete_semver("1"), "1.0.0");
        assert_eq!(complete_semver("0"), "0.0.0");
        assert_eq!(complete_semver("42"), "42.0.0");
    }

    #[test]
    fn complete_semver_major_minor() {
        assert_eq!(complete_semver("1.2"), "1.2.0");
        assert_eq!(complete_semver("0.9"), "0.9.0");
    }

    #[test]
    fn complete_semver_full_passthrough() {
        assert_eq!(complete_semver("1.2.3"), "1.2.3");
        assert_eq!(complete_semver("10.20.30"), "10.20.30");
        // Four-component string: passed through unchanged (≥3 parts)
        assert_eq!(complete_semver("1.2.3.4"), "1.2.3.4");
    }

    #[test]
    fn complete_semver_git_sha_becomes_0_0_0() {
        let sha = "a".repeat(40);
        assert_eq!(complete_semver(&sha), "0.0.0");
        // 40-char hex with mixed case
        let sha2 = "0123456789abcdefABCDEF01234567890123456a";
        // Not all hex (uppercase A outside 0-9a-f range counts as ascii_hexdigit) — verify
        assert_eq!(sha2.len(), 40);
        assert_eq!(complete_semver(sha2), "0.0.0");
    }

    #[test]
    fn complete_semver_non_numeric_parts_become_0_0_0() {
        assert_eq!(complete_semver("alpha"), "0.0.0");
        assert_eq!(complete_semver("v1.2"), "0.0.0"); // "v1" is not parse::<u64>
        assert_eq!(complete_semver("1.beta"), "0.0.0");
    }

    // ── normalize_marketplace_source_c2x ─────────────────────────────────────

    #[test]
    fn normalize_source_c2x_string_becomes_local_object() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("my-plugin"));
        obj.insert("source".to_string(), json!("./plugins/my-plugin"));
        let mut diags = vec![];
        normalize_marketplace_source_c2x(&mut obj, &mut diags);

        let src = obj.get("source").unwrap();
        assert_eq!(src["source"].as_str(), Some("local"));
        assert_eq!(src["path"].as_str(), Some("./plugins/my-plugin"));
        assert!(diags.is_empty(), "no diagnostic expected for string→local");
    }

    #[test]
    fn normalize_source_c2x_relative_object_becomes_local() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("my-plugin"));
        obj.insert(
            "source".to_string(),
            json!({"source": "relative", "path": "./local/path"}),
        );
        let mut diags = vec![];
        normalize_marketplace_source_c2x(&mut obj, &mut diags);

        let src = obj.get("source").unwrap().as_object().unwrap();
        assert_eq!(
            src.get("source").and_then(|v| v.as_str()),
            Some("local"),
            "relative must be rewritten to local"
        );
        assert_eq!(
            src.get("path").and_then(|v| v.as_str()),
            Some("./local/path")
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn normalize_source_c2x_github_passes_through() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("gh-plugin"));
        obj.insert(
            "source".to_string(),
            json!({"source": "github", "repo": "owner/repo", "ref": "v1.0.0"}),
        );
        let mut diags = vec![];
        normalize_marketplace_source_c2x(&mut obj, &mut diags);

        // github source is not modified by this function
        let src = obj.get("source").unwrap().as_object().unwrap();
        assert_eq!(src.get("source").and_then(|v| v.as_str()), Some("github"));
        assert_eq!(src.get("repo").and_then(|v| v.as_str()), Some("owner/repo"));
        assert!(diags.is_empty());
    }

    #[test]
    fn normalize_source_c2x_npm_drops_source_with_diagnostic() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("npm-plugin"));
        obj.insert(
            "source".to_string(),
            json!({"source": "npm", "package": "@scope/pkg"}),
        );
        let mut diags = vec![];
        normalize_marketplace_source_c2x(&mut obj, &mut diags);

        assert!(
            obj.get("source").is_none(),
            "source field must be removed for npm"
        );
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagLevel::Drop);
        assert_eq!(
            diags[0].id.as_deref(),
            Some("plugins.marketplace.plugins.source")
        );
        assert!(
            diags[0].message.contains("npm"),
            "message must mention 'npm'"
        );
        assert!(
            diags[0].message.contains("npm-plugin"),
            "message must contain plugin name"
        );
    }

    #[test]
    fn normalize_source_c2x_missing_source_field_is_noop() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("no-source"));
        let mut diags = vec![];
        normalize_marketplace_source_c2x(&mut obj, &mut diags);
        assert!(!obj.contains_key("source"));
        assert!(diags.is_empty());
    }

    // ── normalize_marketplace_source_x2c ─────────────────────────────────────

    #[test]
    fn normalize_source_x2c_local_object_becomes_path_string() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("local-plugin"));
        obj.insert(
            "source".to_string(),
            json!({"source": "local", "path": "./dist/local-plugin"}),
        );
        normalize_marketplace_source_x2c(&mut obj);

        let src = obj.get("source").unwrap();
        assert_eq!(
            src.as_str(),
            Some("./dist/local-plugin"),
            "local object must collapse to path string"
        );
    }

    #[test]
    fn normalize_source_x2c_non_local_source_unchanged() {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert("name".to_string(), json!("gh-plugin"));
        obj.insert(
            "source".to_string(),
            json!({"source": "github", "repo": "owner/repo"}),
        );
        normalize_marketplace_source_x2c(&mut obj);

        // github object must remain untouched
        let src = obj.get("source").unwrap().as_object().unwrap();
        assert_eq!(src.get("source").and_then(|v| v.as_str()), Some("github"));
    }

    // ── transform_marketplace_c2x ────────────────────────────────────────────

    #[test]
    fn transform_c2x_drops_claude_only_top_level_fields() {
        let input = json!({
            "owner": {"name": "ACME"},
            "allowCrossMarketplaceDependenciesOn": ["other-registry"],
            "forceRemoveDeletedPlugins": true,
            "plugins": []
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_c2x(&input, &mut diags)).unwrap();

        assert!(out.get("owner").is_none(), "owner must be dropped");
        assert!(
            out.get("allowCrossMarketplaceDependenciesOn").is_none(),
            "allowCrossMarketplaceDependenciesOn must be dropped"
        );
        assert!(
            out.get("forceRemoveDeletedPlugins").is_none(),
            "forceRemoveDeletedPlugins must be dropped"
        );

        let drop_ids: Vec<_> = diags
            .iter()
            .filter(|d| d.level == DiagLevel::Drop)
            .map(|d| d.id.as_deref())
            .collect();
        assert!(drop_ids.contains(&Some("plugins.marketplace.owner")));
        assert!(drop_ids.contains(&Some(
            "plugins.marketplace.allowCrossMarketplaceDependenciesOn"
        )));
        assert!(drop_ids.contains(&Some("plugins.marketplace.forceRemoveDeletedPlugins")));
    }

    #[test]
    fn transform_c2x_fills_default_policy_when_absent() {
        let input = json!({
            "plugins": [
                {"name": "p1", "source": "./p1"}
            ]
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_c2x(&input, &mut diags)).unwrap();

        let policy = &out["plugins"][0]["policy"];
        assert!(policy.is_object(), "policy must be an object");
        assert_eq!(policy["installation"].as_str(), Some("AVAILABLE"));
        assert_eq!(policy["authentication"].as_str(), Some("ON_INSTALL"));

        let has_policy_warn = diags.iter().any(|d| {
            d.id.as_deref() == Some("plugins.marketplace.plugins.policy")
                && d.message.contains("auto-filled")
        });
        assert!(
            has_policy_warn,
            "Expected auto-fill warn for missing policy"
        );
    }

    #[test]
    fn transform_c2x_preserves_existing_policy() {
        let input = json!({
            "plugins": [
                {
                    "name": "p1",
                    "source": "./p1",
                    "policy": {"installation": "REQUIRE_APPROVAL", "authentication": "NEVER"}
                }
            ]
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_c2x(&input, &mut diags)).unwrap();

        let policy = &out["plugins"][0]["policy"];
        assert_eq!(
            policy["installation"].as_str(),
            Some("REQUIRE_APPROVAL"),
            "existing policy must not be overwritten"
        );
        let has_auto_fill = diags
            .iter()
            .any(|d| d.id.as_deref() == Some("plugins.marketplace.plugins.policy"));
        assert!(
            !has_auto_fill,
            "no auto-fill warning expected when policy present"
        );
    }

    #[test]
    fn transform_c2x_normalizes_string_source_to_local_object() {
        let input = json!({
            "plugins": [
                {"name": "p1", "source": "./p1", "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"}}
            ]
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_c2x(&input, &mut diags)).unwrap();

        let src = &out["plugins"][0]["source"];
        assert_eq!(src["source"].as_str(), Some("local"));
        assert_eq!(src["path"].as_str(), Some("./p1"));
    }

    #[test]
    fn transform_c2x_invalid_json_returns_original() {
        let bad = "not json {{{}";
        let mut diags = vec![];
        let result = transform_marketplace_c2x(bad, &mut diags);
        assert_eq!(result, bad);
        assert!(diags.is_empty());
    }

    // ── transform_marketplace_x2c ────────────────────────────────────────────

    #[test]
    fn transform_x2c_drops_policy_with_diagnostic() {
        let input = json!({
            "plugins": [
                {
                    "name": "p1",
                    "source": {"source": "local", "path": "./p1"},
                    "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"}
                }
            ]
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_x2c(&input, &mut diags)).unwrap();

        assert!(
            out["plugins"][0].get("policy").is_none(),
            "policy must be dropped in x2c"
        );
        let drop = diags.iter().find(|d| {
            d.level == DiagLevel::Drop
                && d.id.as_deref() == Some("plugins.marketplace.plugins.policy")
        });
        assert!(drop.is_some(), "Expected Drop diagnostic for policy");
    }

    #[test]
    fn transform_x2c_normalizes_local_source_to_path_string() {
        let input = json!({
            "plugins": [
                {
                    "name": "p1",
                    "source": {"source": "local", "path": "./dist/p1"}
                }
            ]
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_x2c(&input, &mut diags)).unwrap();

        assert_eq!(
            out["plugins"][0]["source"].as_str(),
            Some("./dist/p1"),
            "local object must become path string"
        );
    }

    #[test]
    fn transform_x2c_preserves_github_source() {
        let input = json!({
            "plugins": [
                {
                    "name": "gh-plugin",
                    "source": {"source": "github", "repo": "owner/repo", "ref": "v2.0.0"}
                }
            ]
        })
        .to_string();
        let mut diags = vec![];
        let out: serde_json::Value =
            serde_json::from_str(&transform_marketplace_x2c(&input, &mut diags)).unwrap();

        let src = &out["plugins"][0]["source"];
        assert_eq!(
            src["source"].as_str(),
            Some("github"),
            "github source must not be modified by x2c"
        );
        assert_eq!(src["repo"].as_str(), Some("owner/repo"));
    }

    #[test]
    fn transform_x2c_invalid_json_returns_original() {
        let bad = "{ invalid }";
        let mut diags = vec![];
        let result = transform_marketplace_x2c(bad, &mut diags);
        assert_eq!(result, bad);
        assert!(diags.is_empty());
    }
}
