---
paths:
  - "mappings/**"
---

# Mappings Editing Rules

`mappings/*.yaml` is the **authoritative data** for Claude Code ⇄ Codex CLI conversions. The following rules must be strictly observed.

## Schema compliance (see `mappings/SCHEMA.md`)

- `id` must be unique across all files
- `direction` must be one of `both` / `claude_to_codex` / `codex_to_claude`
- `loss` must be one of `lossless` / `lossy` / `dropped`
- If `degrade` is set, `loss: lossy` is required
- Do not add `transform` to entries with `loss: dropped`

## Validation after editing

Always run `scripts/validate-mappings.py` after editing to verify invariants.
The Claude Code PostToolUse hook runs it automatically, but you can also run it manually:

```bash
uv run scripts/validate-mappings.py
```

## Preserving semantics and rationale

- Leave a source URL in `notes` for each entry (via the `source:` field or within `notes`)
- Do not make changes that contradict `docs/`. If a contradiction arises, update **both** `docs/spec.md` and the relevant `mappings/*.yaml` to keep them in sync.
- Do not silently change the meaning of existing entries. When in doubt, consult `docs/` and `notes`.

## Mappings invariant tests (`docs/spec.md §18 Testing Strategy`)

Implement the following tests on the implementation side (`src/**`, `tests/**`):

- `id` is unique across all files
- `direction` only takes values `both` / `claude_to_codex` / `codex_to_claude`
- `loss` only takes values `lossless` / `lossy` / `dropped`
- Entries with `degrade` must have `loss: lossy`
- Entries with `loss: dropped` must not have `transform`
