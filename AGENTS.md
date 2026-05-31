# ccx — Contributor & Agent Guide

**ccx** is a Rust CLI that bidirectionally converts configuration files between
Claude Code (`.claude/`, JSON) and OpenAI Codex CLI (`.codex/`, TOML).
Conversion rules live in `mappings/*.yaml`; the CLI is an engine that interprets them.

## Repo Layout

```
src/        Rust implementation
mappings/   Conversion table YAML (287 entries) — canonical data
docs/       Design & specification documents
tests/      Integration tests and fixtures
```

## Source of Truth

- **Design & spec:** [`docs/spec.md`](docs/spec.md) — single authoritative document for
  the CLI design, IR model, transform registry, domain handler contracts, degrade engine,
  CLI flags, exit codes, and implementation phases. Supersedes any older per-file docs.
- **Conversion table:** [`mappings/*.yaml`](mappings/) + [`mappings/SCHEMA.md`](mappings/SCHEMA.md) —
  canonical field-level mapping data (287 entries). When code and spec diverge, follow `docs/spec.md`.

## Dev Commands

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
cargo run -- check <path>
cargo run -- c2x <path>
python3 scripts/validate-mappings.py   # validate mappings invariants
```

## Per-Area Work Rules

Fine-grained rules are under `.claude/rules/` and are auto-loaded by context:

| Area | Rule file |
|---|---|
| `src/**`, `tests/**` | `.claude/rules/rust.md` |
| `mappings/**` | `.claude/rules/mappings.md` |
| `docs/**` | `.claude/rules/docs.md` |

Read the relevant rule file before touching files in that area.

## Key Principles

- **Dropped fields are never silent.** Every `dropped` entry must appear in the conversion report.
- **Degrade scope must be recorded.** When a field moves to a broader scope (skill → session/project),
  the report must name the target scope.
- **Codex spec is fluid.** Check `mappings/*.yaml` `notes` fields and `docs/spec.md §17`
  for known issues and awaiting-codex annotations before assuming behavior.

## Language

Project content (code, docs, comments, commit messages, issues, PRs, README) is written
in **English**. The assistant replies to the maintainer in **Japanese** during chat.
