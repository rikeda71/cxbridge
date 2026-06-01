# ccx — Claude Code ⇄ Codex CLI Config Converter

[Japanese](README.ja.md)

**ccx** is a Rust CLI that bidirectionally converts configuration files between
[Claude Code](https://code.claude.com/docs) (`.claude/`, JSON) and
[OpenAI Codex CLI](https://developers.openai.com/codex) (`.codex/`, TOML).
It covers Skills, Plugins, Hooks, MCP servers, Memory files, Subagents, and Settings.
Conversion rules are declared in `mappings/*.yaml` (301 entries); the CLI is an engine
that interprets those declarations.

```
Claude Code  .claude/ (JSON)  ⇄  Codex CLI  .codex/ (TOML)
```

Every conversion always produces a **conversion report** that enumerates what was
lossless, lossy, degraded, dropped, and any body-scan warnings. Silent data loss is
prohibited.

---

## Install

**Prerequisites:** Rust 1.80+ (`cargo` available)

```sh
git clone https://github.com/rikeda71/ccx
cd ccx
cargo build --release
cp target/release/ccx ~/.local/bin/
```

---

## Usage

```sh
ccx c2x <path>    # Claude → Codex (one-way)
ccx x2c <path>    # Codex → Claude (one-way)
ccx check <path>  # Pre-conversion diagnosis (no writes)
```

`<path>` accepts a file or a directory (recursive detection).

### Options (shared by `c2x` / `x2c`)

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

### Examples

```sh
# Convert a Claude skill to Codex format
ccx c2x .claude/skills/deploy/SKILL.md

# Convert Codex config.toml to Claude format, report only
ccx x2c .codex/config.toml --dry-run --report

# Diagnose an MCP config before converting
ccx check .mcp.json

# CI: fail if any fields are dropped
ccx c2x .claude/skills/deploy/SKILL.md --strict

# Machine-readable JSON report
ccx c2x .mcp.json --dry-run --report=json
```

---

## Conversion Report

Every run prints a report. Example:

```
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

| Symbol | Meaning |
|---|---|
| ◎ | Lossless — fully equivalent |
| ○ | Lossy — meaning preserved but information partially reduced |
| △ | Degraded — moved to a broader scope (e.g., skill → session) |
| ✕ | Dropped — no conversion target; discarded |
| ⚠ | Body warning — requires manual review |

`--strict` causes exit code 2 when any dropped entries exist.

---

## Documentation

- **[docs/spec.md](docs/spec.md)** — full design & implementation specification (IR model,
  transform registry, domain handler contracts, degrade engine, CLI flags, exit codes,
  testing strategy, and more)
- **[mappings/](mappings/)** — canonical conversion table (301 entries across
  `skills.yaml`, `hooks.yaml`, `mcp.yaml`, `plugins.yaml`, `memory.yaml`,
  `subagents.yaml`, `settings-config.yaml`); schema defined in
  [mappings/SCHEMA.md](mappings/SCHEMA.md)
