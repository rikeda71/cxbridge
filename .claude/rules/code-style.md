---
paths:
  - "src/**"
  - "tests/**"
---

# Code Style

Conventions for writing and refactoring Rust code in this crate. Quality gates
(`cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`) and the mappings
relationship live in `rust.md`; this file is about how the code reads.

## Comments

- Comment **why**, not **what**. Explain non-obvious intent, invariants, edge
  cases, and surprising trade-offs. Do not narrate what the code plainly does.
- **No design-spec references in code.** Never cite the design document or its
  sections (no `docs/…` paths, no `§N` markers, no "see spec"). The code stands
  on its own; design rationale belongs in `docs/spec.md`, not in comments.
- Delete redundant, outdated, or placeholder comments. A comment that restates
  the function name or signature adds nothing — remove it.
- Doc comments (`///`) on public items state the item's purpose and contract
  (inputs, outputs, side effects, panics) in one or two lines — not spec history.

## Structure & simplicity

- One responsibility per function and per module. Keep the public surface small;
  prefer `pub(crate)`/private unless a wider scope is genuinely needed.
- Remove dead code and unused helpers rather than leaving them behind.
- Avoid needless indirection, premature abstraction, and redundant `clone()`.
  Prefer iterator combinators and `?` over manual loops and nested matches when
  they read more clearly.
- Do not change a public function's signature or observable behavior during a
  pure refactor; simplify the implementation, keep the contract.

## Errors

- Use `anyhow::Result` with `?` and `.context(...)`; no `unwrap()`/`expect()`/
  `panic!()` on runtime paths. Startup invariant checks that must abort may
  panic with a clear message. Tests may `unwrap()`.

## Naming & language

- `snake_case` for functions, variables, and modules; `UpperCamelCase` for types
  and traits; `SCREAMING_SNAKE_CASE` for consts.
- Names describe intent. Identifiers, comments, and messages are written in
  English.
