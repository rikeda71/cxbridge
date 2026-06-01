---
name: run-checks
description: >
  Runs fmt, clippy, and tests after modifying the Rust implementation, or before committing.
  Use when asked to "run checks", "cargo check", "run checks", or "make CI pass".
allowed-tools:
  - Bash(cargo *)
---

## Steps

Execute the following in order. If a step fails, fix the issue before proceeding to the next step.

### 1. Format check

```bash
cargo fmt --check
```

If this fails, run `cargo fmt` and then re-verify.

### 2. Clippy (warnings as errors)

```bash
cargo clippy -- -D warnings
```

If this fails, fix each clippy finding.
When suppressing with `#[allow(...)]`, always include a reason comment.

### 3. Tests

```bash
cargo test
```

Fix any failing tests. If snapshots (`insta`) were updated, run `cargo insta review` to inspect the diff before accepting.

### Completion condition

Report completion once all 3 steps pass with exit code 0.
