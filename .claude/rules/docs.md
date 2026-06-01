---
paths:
  - "docs/**"
---

# Docs Editing Rules

`docs/spec.md` is the single source of truth for design and specifications. The legacy `docs/01`–`docs/13` files have been merged into `docs/spec.md` and must not be referenced.

## Alignment rules when changing design

- When changing the design, update **both** `docs/spec.md` and any related `mappings/*.yaml` to keep them in sync.
- The feature alignment and loss matrix (convertible / not convertible / future follow-up classifications) is maintained in `docs/spec.md §16 Feature & Loss Matrix Summary` and must match the `loss` distribution in `mappings/*.yaml` (counts and breakdown of lossless / lossy / dropped).
- If `docs/spec.md` and another document conflict, `docs/spec.md` takes precedence.

## Reference rules

- When there are questions about implementation flow, types, or interfaces, refer to the relevant section of `docs/spec.md` (referenced by section name).
- When there is uncertainty about Codex-side behavior, refer to `docs/spec.md §17 Codex Interop Notes & Known Issues` and the `notes` of individual entries.
