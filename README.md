# ccx — Claude Code ⇄ Codex config converter

[![CI](https://github.com/rikeda71/ccx/actions/workflows/ci.yml/badge.svg)](https://github.com/rikeda71/ccx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[日本語版 README](README.ja.md)

Move your agent setup between [Claude Code](https://code.claude.com/docs) and the
[OpenAI Codex CLI](https://developers.openai.com/codex) — without redoing it by hand,
and without silently losing settings.

```
Claude Code  .claude/ (JSON)   ⇄   Codex CLI  .codex/ (TOML)
```

## Why ccx?

If you use both Claude Code and Codex, your two setups drift apart. You've built up
skills, hooks, MCP servers, memory files, and subagents on one side and want them on
the other — but the two tools use different directory layouts, different file formats
(JSON vs TOML), and different feature sets.

ccx translates between them in either direction. The hard part isn't copying files;
it's knowing what *doesn't* translate cleanly. So every run prints a **conversion
report** that tells you exactly what came across losslessly, what was reshaped, what
got moved to a broader scope, and what had no equivalent and was dropped. **Nothing is
ever lost silently.**

The translation rules aren't hardcoded — they live in `mappings/*.yaml` (304 entries
across 8 domains). The CLI is just an engine that interprets them.

## At a glance

```sh
$ ccx c2x .claude/skills/deploy/SKILL.md

✔ skills/deploy/SKILL.md → .agents/skills/deploy/SKILL.md
  ◎ name, description                          lossless
  ○ when_to_use → description(concatenated)    lossy
  △ allowed-tools → .codex/rules/deploy.rules  lossy (degrade: skill→project)
  △ model: opus→gpt-5.x, effort: max→xhigh     lossy (degrade: skill→subagent)
  ✕ user-invocable                             dropped (no Codex equivalent)
  ✕ paths                                      dropped
  ⚠ body L42: !`git diff` not executed in Codex (literal residue risk)
  + generated: .codex/rules/deploy.rules, .codex/agents/deploy.toml
Summary: 2 lossless, 3 lossy (2 degraded), 2 dropped, 1 body-warning
```

## What it converts

| Domain | Examples |
|---|---|
| **Skills** | `SKILL.md` frontmatter, `allowed-tools`, model/effort, body scan |
| **Plugins** | plugin manifests, bundled `commands/` and `agents/` directories |
| **Hooks** | event hooks, matchers, command hooks |
| **MCP servers** | `.mcp.json` ⇄ Codex `[mcp_servers]` |
| **Memory** | `CLAUDE.md` / `AGENTS.md` and memory settings |
| **Subagents** | agent definitions and model tiers |
| **Settings / Config** | `settings.json` ⇄ `config.toml` |
| **Variables** | `${CLAUDE_*}` placeholders and Codex equivalents |

## Install

**Prerequisites:** Rust 1.80+ (stable `cargo`).

```sh
git clone https://github.com/rikeda71/ccx
cd ccx
cargo build --release
cp target/release/ccx ~/.local/bin/   # or anywhere on your PATH
```

Pre-built binaries are published on the [Releases](https://github.com/rikeda71/ccx/releases) page.

## Usage

```sh
ccx c2x <path>    # Claude → Codex
ccx x2c <path>    # Codex → Claude
ccx check <path>  # Diagnose convertibility without writing anything
```

`<path>` can be a single file or a directory (detected recursively).

```sh
# Convert one Claude skill to Codex format
ccx c2x .claude/skills/deploy/SKILL.md

# Preview a Codex → Claude conversion without touching disk
ccx x2c .codex/config.toml --dry-run --report

# Diagnose an MCP config before converting
ccx check .mcp.json

# Fail the build if anything would be dropped (good for CI)
ccx c2x .claude/skills/deploy/SKILL.md --strict

# Machine-readable JSON report
ccx c2x .mcp.json --dry-run --report=json
```

<details>
<summary><strong>All options</strong> (shared by <code>c2x</code> / <code>x2c</code>)</summary>

| Flag | Default | Description |
|---|---|---|
| `--out <dir>` | `<input>.converted/` | Output root directory |
| `--only <domains>` | all | Comma-separated domain filter (`skills,mcp`, …) |
| `--scope <project\|user>` | `project` | Degrade target scope (`.rules` / agents placement) |
| `--skill-target <auto\|skill\|subagent>` | `auto` | Force skill conversion target |
| `--interactive` | false | TTY confirmation for gray-case skills |
| `--rewrite-body` | false | Apply body substitutions (default: detect + warn only) |
| `--dual-manifest` | false | Keep `.claude-plugin/` and also generate `.codex-plugin/` |
| `--hooks-target <user\|project>` | `user` | Hooks write destination |
| `--report[=json]` | none | Emit detailed report (`=json` for machine-readable output) |
| `--dry-run` | false | Report only, no file writes |
| `--strict` | false | Exit 2 if any fields were dropped (for CI) |
| `--keep-claude-frontmatter` | false | Retain Claude-specific frontmatter keys in Codex output |
| `--force` | false | Allow overwriting existing files |

</details>

## Reading the report

Every run ends with a one-line summary; with `--report` you also get the per-field
detail shown above. Each line is tagged with one symbol:

| Symbol | Meaning |
|---|---|
| ◎ | **Lossless** — fully equivalent on the other side |
| ○ | **Lossy** — meaning preserved, but some information is reduced |
| △ | **Degraded** — moved to a broader scope (e.g. skill → project), which is named |
| ✕ | **Dropped** — no conversion target; discarded (and reported) |
| ⚠ | **Body warning** — a construct in the body needs manual review |

`--strict` turns any dropped field into a non-zero exit (code 2), so you can wire ccx
into CI and refuse conversions that would quietly lose data.

## Documentation

- **[docs/spec.md](docs/spec.md)** — the full design & implementation spec: IR model,
  transform registry, domain handler contracts, degrade engine, CLI flags, exit codes,
  and testing strategy.
- **[mappings/](mappings/)** — the canonical conversion table (304 entries across
  `skills`, `hooks`, `mcp`, `plugins`, `memory`, `subagents`, `settings-config`, and
  `variables`). The schema is documented in [mappings/SCHEMA.md](mappings/SCHEMA.md).

## License

[MIT](LICENSE) © Ryuya Ikeda
