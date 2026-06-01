---
name: mappings-auditor
description: >
  Audits the alignment between mappings/*.yaml and the docs (especially mappings/SCHEMA.md and docs/spec.md §16 Feature & Loss Matrix Summary).
  Checks the validity of loss judgments, whether notes have source URLs, and SCHEMA violations.
  Use when asked to "audit mappings", "check mappings alignment", or "audit mappings".
tools:
  - Read
  - Grep
  - Glob
  - Bash
---

## Audit procedure

### 1. Automated validation via script

```bash
uv run "$CLAUDE_PROJECT_DIR/scripts/validate-mappings.py"
```

If any NG results appear, present fix proposals before proceeding to the next step.

### 2. Alignment check between loss/direction and docs/spec.md §16

Read `docs/spec.md §16 Feature & Loss Matrix Summary` to understand the conversion status of each feature (convertible / not convertible / future follow-up).
Then Grep/Read each entry in `mappings/*.yaml` and verify the following:

- Fields marked "not convertible" in `docs/spec.md §16` have `loss: dropped`.
- Fields marked "future follow-up" in `docs/spec.md §17 Codex Interop Notes & Known Issues` have `status: awaiting-codex` in their `notes`.
- Entries with `loss: lossy` have a valid reason consistent with the docs/spec.md description (check whether simple renames or unit conversions should instead be `lossless`).

### 3. Verify source URLs in notes

For entries with `warn: true` or `loss: lossy/dropped` that have `notes`, check whether the `source` field contains a supporting URL.
References to GitHub issues (`openai/codex#*`) or official documentation are preferred.

### 4. Validate future follow-up markings

For entries whose `notes` contain `status: awaiting-codex`, check whether any of them have already been implemented on the Codex side. If implemented, propose promoting from `loss: dropped` to `loss: lossy` or `both`.

### 5. Reporting audit findings

Report findings under the following categories:

- **SCHEMA violation**: Items that violate invariants (id uniqueness, value domains, degrade⇒lossy, etc.). Must be fixed.
- **Alignment issue**: Items where the loss judgment conflicts with docs/13. Needs review and fix.
- **Missing rationale**: Items that are warn/lossy/dropped but have thin `notes` or no `source`. Needs supplementation.
- **Promotion candidate**: Items marked `awaiting-codex` that may already be implemented in Codex. Needs investigation.
