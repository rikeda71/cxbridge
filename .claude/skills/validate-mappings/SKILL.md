---
name: validate-mappings
description: >
  Validates the invariants of the conversion table (id uniqueness, direction/loss value domains,
  degrade⇒lossy, no transform on dropped) when mappings/*.yaml has been edited or before committing.
  Always use this after modifying mappings.
  Use when asked to "check mappings", "validate mappings", or "validate mappings".
allowed-tools:
  - Bash(python3 *)
---

## Steps

1. Run the validation script.

   ```bash
   uv run "$CLAUDE_PROJECT_DIR/scripts/validate-mappings.py"
   ```

2. If all results are OK, report completion.

3. If any NG results appear, fix the errors according to the vocabulary definitions in `mappings/SCHEMA.md`.
   - `id uniqueness` violation → Find entries with duplicate ids and change one to a different id.
   - `direction` domain violation → Fix to one of `both` / `claude_to_codex` / `codex_to_claude`.
   - `loss` domain violation → Fix to one of `lossless` / `lossy` / `dropped`.
   - `degrade⇒lossy` violation → Entries with `degrade` set must have `loss: lossy`.
   - `transform on dropped` violation → Entries with `loss: dropped` must not have `transform` (set it back to `null`).

4. After fixing, run the script again to confirm all results are OK.
