use std::collections::HashMap;

use crate::core::mappings::{is_pseudo_field, DomainMap, MapEntry};
use crate::core::transforms::ConvDir;

/// Indexes only scope:"plugin" entries to avoid collisions with same-named fields in marketplace etc.
/// Indexes by the claude field for c2x, or the codex field for x2c.
pub(super) fn build_plugin_scope_index(map: &DomainMap, dir: ConvDir) -> HashMap<String, MapEntry> {
    let mut idx = HashMap::new();
    for entry in &map.entries {
        let spec = match dir {
            ConvDir::C2x => entry.claude.as_ref(),
            ConvDir::X2c => entry.codex.as_ref(),
        };
        let Some(spec) = spec else { continue };
        // Only include scope:"plugin" entries (exclude marketplace / null)
        if spec.scope.as_deref() != Some("plugin") {
            continue;
        }
        let Some(field) = spec.field.as_ref() else {
            continue;
        };
        if is_pseudo_field(field) {
            continue;
        }
        // First-registered entry wins; later duplicates for the same field are ignored
        idx.entry(field.clone()).or_insert_with(|| entry.clone());
    }
    idx
}
