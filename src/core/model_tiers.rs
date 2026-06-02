/// Model capability tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    High,
    Mid,
    Low,
}

/// Maps a Claude model name to a Tier.
/// Returns None for unknown model names; the caller is responsible for emitting a warning.
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

/// Maps a Codex model name to a Tier.
///
/// Performs an explicit lookup against `CODEX_LATEST` first; this ensures the roundtrip
/// invariant `codex_tier(tier_to_codex(t)) == Some(t)` holds for all three tiers.
/// For model names not in `CODEX_LATEST` (e.g. a user's custom config), falls back to
/// a name-based heuristic: names containing `"mini"` map to `Low`, all others to `Mid`.
pub fn codex_tier(m: &str) -> Option<Tier> {
    // Explicit lookup first — covers all canonical names and preserves the roundtrip invariant.
    if let Some(&(tier, _)) = CODEX_LATEST.iter().find(|(_, name)| *name == m) {
        return Some(tier);
    }
    // Heuristic fallback for user-supplied or future model names not in the table.
    if m.contains("mini") {
        Some(Tier::Low)
    } else {
        Some(Tier::Mid)
    }
}

/// Canonical Codex model names per tier, tracking the current Codex frontier defaults.
/// Update these when OpenAI releases new Codex model versions.
/// High = most capable, Mid = balanced, Low = fast/lightweight.
pub(crate) const CODEX_LATEST: &[(Tier, &str)] = &[
    (Tier::High, "gpt-5.5"),     // current frontier flagship
    (Tier::Mid, "gpt-5.4"),      // balanced default
    (Tier::Low, "gpt-5.4-mini"), // fast/lightweight
];

pub(crate) const CLAUDE_LATEST: &[(Tier, &str)] = &[
    (Tier::High, "claude-opus-4-8"),  // contains("opus") → High ✓
    (Tier::Mid, "claude-sonnet-4-6"), // contains("sonnet") → Mid ✓
    (Tier::Low, "claude-haiku-4-5"),  // contains("haiku") → Low ✓
];

/// Returns the Codex model name for a given Tier, looked up from CODEX_LATEST.
pub fn tier_to_codex(t: Tier) -> &'static str {
    CODEX_LATEST
        .iter()
        .find(|(tier, _)| *tier == t)
        .map(|(_, name)| *name)
        .expect("CODEX_LATEST must cover all Tier variants")
}

/// Returns the Claude model name for a given Tier, looked up from CLAUDE_LATEST.
pub fn tier_to_claude(t: Tier) -> &'static str {
    CLAUDE_LATEST
        .iter()
        .find(|(tier, _)| *tier == t)
        .map(|(_, name)| *name)
        .expect("CLAUDE_LATEST must cover all Tier variants")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_roundtrip_codex() {
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
}
