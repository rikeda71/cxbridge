---
paths:
  - "src/**"
  - "tests/**"
---

# Rust Implementation Rules

## Formatting & quality

- Pass `cargo fmt` (do not commit formatting violations)
- Pass `cargo clippy -- -D warnings` (treat warnings as errors)
- Pass `cargo test`

## Error handling

- Use `anyhow` for errors (`anyhow::Error` / `anyhow::Result` / `anyhow::bail!` / `anyhow::Context`)
- Do not leave panics or `unwrap()` in production logic

## Source of truth for types and flow

- `docs/spec.md` is the source of truth for type definitions and processing flow
- If code and the design document diverge, align to `docs/spec.md`
- Do not leave `todo!()` in production logic (implement stubs incrementally)

## Relationship with mappings

- `mappings/*.yaml` is the authoritative data for conversions. Code is the engine that drives it.
- Do not alter the semantics of YAML in code. When in doubt, consult `mappings/SCHEMA.md` and `notes`.
- Do not conflate `MappingDirection` (for mappings) with `ConvDir` (for the pipeline).

## Conversion implementation principles (see `docs/spec.md §6 IR Data Model` through `docs/spec.md §10 Degrade Engine` for details)

- **Model as tier const** (opus/sonnet/haiku ⇄ high/mid/low). `model-map.yaml` does not exist. Tier definitions are in `docs/spec.md §8 Transform Registry` (Model Tier Mapping section).
- **skill→skill vs skill→subagent** is determined by `--skill-target` / `--interactive` / `decide_skill_target` (`docs/spec.md §9.1 Skills`). Use subagent if `model`/`effort`/skill-scoped permissions are present; use skill for pure instructions.
- **Demotion (skill→session/subagent) changes scope.** Always record this explicitly in the conversion report. Also enumerate `dropped` entries — do not discard them silently (`docs/spec.md §10 Degrade Engine`, `docs/spec.md §12 Conversion Report`).
- **The Codex side is fluid** (plugin-bundled hooks may not be loaded `openai/codex#16430`; the skill loader silently ignores unknown frontmatter fields in fail-open mode; etc.). Refer to `docs/spec.md §17 Codex Interop Notes & Known Issues` and each mapping's `notes`. Verify demotion results against a real Codex instance.

## Test strategy (`docs/spec.md §18 Testing Strategy`)

- `insta` snapshots: use `tests/fixtures/` as golden files
- Round-trip tests: `c2x → x2c` must produce exact matches for `lossless` entries; only known diffs are permitted for `lossy`/`dropped`
- Mappings invariant tests: see `rules/mappings.md`
