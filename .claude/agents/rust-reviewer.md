---
name: rust-reviewer
description: >
  Reviews src/*.rs files against docs/spec.md and Rust best practices.
  Checks type/flow alignment with docs/spec.md, leftover todo!() stubs, clippy findings, error handling,
  and whether mappings semantics have been altered in code.
  Use when asked to "review Rust", "review src", or "rust review".
tools:
  - Read
  - Grep
  - Glob
  - Bash
---

## Review procedure

### 1. Pre-check

```bash
cargo clippy -- -D warnings 2>&1 | head -80
```

Understand clippy errors and warnings before proceeding to the code review.

### 2. Type/flow alignment check against docs/spec.md

Cross-reference the following sections of `docs/spec.md` (all §N references below are section numbers within `docs/spec.md`):

- **docs/spec.md §6 IR Data Model**: Verify that `IRField`, `IRNode`, `Loss`, `Kind`, etc. in `src/core/ir.rs` match the definitions.
- **docs/spec.md §7 Mappings YAML Format & Invariants**: Verify that `MapEntry`, `MappingDirection`, and `LossSpec` in `src/core/mappings.rs` match the definitions. Check that startup asserts (id uniqueness, value domains, `degrade⇒lossy`, no `transform` on `dropped`) are implemented.
- **docs/spec.md §8 Transform Registry**: Verify that `ConvDir` (a separate type from `MappingDirection`), `TransformCtx`, and `TransformSpec` in `src/core/transforms.rs` match the definitions. Check that `format:json_to_toml` etc. are registered as no-ops. Verify that model resolution uses tier const tables (`CLAUDE_LATEST`, `CODEX_LATEST`) and does not depend on external YAML files.
- **docs/spec.md §9 Domain Handler Contracts**: Verify that the `Handler` trait's `parse`, `lift`, and `lower` signatures match the definitions. Confirm that `lift` uses `applies_direction` for direction matching. Check that handlers for all domains (skills/mcp/hooks/memory/plugins/subagents/settings) are implemented.
- **docs/spec.md §10 Degrade Engine**: Verify that the `degrade/` module generates output at the defined demotion targets (`.rules`, `agents/<n>.toml`, appending to `config.toml`) and records them in `SideArtifact`.
- **docs/spec.md §11 Body Scanner**: Verify that detection patterns in `scanner/body.rs` match the definitions. Confirm that `scan_body` only detects and does not rewrite body text (i.e., `rewrite_body` is a separate function).
- **docs/spec.md §12 Conversion Report**: Verify that `build_report` always enumerates `dropped` and `degrade` entries and does not silently discard anything.
- **docs/spec.md §5 Architecture & Pipeline**: Verify that the processing flow in `run` (load_mappings → detect → pick_handler → parse → lift → lower → build_report → write_plan) matches the pipeline diagram.

### 3. Check for leftover todo!() stubs

```bash
grep -rn "todo!()" /path/to/src/
```

Check that no `todo!()` remains in already-implemented phases (M0, M1, etc.). `todo!()` in unimplemented phases (M2 and later) is acceptable.

### 4. Error handling check

- Verify that `anyhow::Result` / `anyhow::bail!` / `.context()` are used consistently (`unwrap` / `expect` must not remain in production code).
- Verify that `parse` does not abort processing of other files on parse failure (skip + continue emitting error diagnostics).

### 5. Check that mappings semantics have not been altered in code

- Verify that `lift` does not rewrite values without applying a transform.
- Verify that `lower` does not implicitly perform conversions not declared in mappings.
- Verify that `direction` matching (`applies_direction`) is not skipped.
- Verify that `run_degrade` is called only when `degrade` is truthy (no conflation with special cases like `disable-model-invocation`).

### 6. Reporting review findings

Report findings under the following categories:

- **Design mismatch**: Where implementation diverges from types/flow in docs/spec.md. Must be fixed.
- **Unimplemented (action required)**: Where `todo!()` remains for stubs that should be filled in the current phase.
- **Error handling deficiency**: Leftover `unwrap` / `expect`, or implementations that cannot continue execution.
- **Mappings semantic deviation**: Where the handling of transform, degrade, or direction differs from the YAML declaration.
- **Clippy findings**: Code quality issues detected by clippy.
