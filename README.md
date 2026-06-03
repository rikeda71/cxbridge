# cxbridge

[![CI](https://github.com/rikeda71/cxbridge/actions/workflows/ci.yml/badge.svg)](https://github.com/rikeda71/cxbridge/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/cxbridge.svg)](https://crates.io/crates/cxbridge)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**[日本語版 README はこちら](README.ja.md)**

Bidirectional config converter between [Claude Code](https://code.claude.com/docs) (`.claude/`, JSON) and the [OpenAI Codex CLI](https://developers.openai.com/codex) (`.codex/`, TOML).

Convert skills, hooks, MCP servers, memory, subagents, plugins, and settings in either direction — and get a report of exactly what converted, what was reshaped, and what had no equivalent. Nothing is ever dropped silently.

```text
$ cxbridge c2x .claude/skills/deploy/SKILL.md --report

▸ skills: SKILL.md
  ◎ skills.name, skills.description  lossless
  △ skills.allowed-tools  degrade  skills.allowed-tools → .codex/rules/<skill>.rules (execpolicy allow)…
  ✕ skills.user-invocable  dropped  model-only / hidden-from-user flag has no Codex concept
  ⚠ 3 body warnings — run with --report=json for line-by-line
Summary: 2 lossless, 1 lossy(1 degraded), 1 dropped, 3 body-warning
```

Each converted file gets a `▸ <domain>: <source>` header, so a whole-directory
conversion stays readable. Repeated fields are grouped (`×N`), and body warnings
are summarized — the full line-by-line detail is always in `--report=json`.

## Keep Claude Code and Codex in sync

If you use both tools, you build up skills, hooks, and MCP servers on one side and want them on the other. cxbridge moves them across instead of making you redo the work by hand — and tells you where the two tools genuinely disagree.

```bash
# Bring a Claude skill into Codex
cxbridge c2x .claude/skills/deploy/SKILL.md

# Pull a Codex config back into Claude
cxbridge x2c .codex/config.toml

# See what would convert, before writing anything
cxbridge check .claude/
```

## Usage

```
cxbridge <c2x|x2c|check> <path> [options]
```

```bash
cxbridge c2x <path>    # Claude → Codex
cxbridge x2c <path>    # Codex → Claude
cxbridge check <path>  # Diagnose convertibility (no writes)

cxbridge --version     # Print version
cxbridge --help        # Show help
```

`<path>` is a file or a directory (scanned recursively).

```bash
# Convert one skill
cxbridge c2x .claude/skills/deploy/SKILL.md

# Preview a Codex → Claude conversion without touching disk
cxbridge x2c .codex/config.toml --dry-run --report

# Fail if anything would be dropped (use as a CI gate)
cxbridge c2x .claude/ --strict

# Machine-readable JSON report
cxbridge c2x .mcp.json --dry-run --report=json
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
| `--report[=json]` | none | Emit a detailed report (`=json` for machine-readable output) |
| `--dry-run` | false | Report only, no file writes |
| `--strict` | false | Exit 2 if any fields were dropped |
| `--keep-claude-frontmatter` | false | Retain Claude-specific frontmatter keys in Codex output |
| `--force` | false | Allow overwriting existing files |

</details>

## Installation

### Homebrew (macOS / Linux)

```bash
brew install rikeda71/tap/cxbridge
```

### curl (GitHub Releases)

```bash
# macOS (Apple Silicon)
curl -fsSL https://github.com/rikeda71/cxbridge/releases/latest/download/cxbridge-aarch64-apple-darwin.tar.gz | tar xz
sudo mv cxbridge /usr/local/bin/

# macOS (Intel)
curl -fsSL https://github.com/rikeda71/cxbridge/releases/latest/download/cxbridge-x86_64-apple-darwin.tar.gz | tar xz
sudo mv cxbridge /usr/local/bin/

# Linux (x86_64)
curl -fsSL https://github.com/rikeda71/cxbridge/releases/latest/download/cxbridge-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv cxbridge /usr/local/bin/
```

A statically linked `…-x86_64-unknown-linux-musl.tar.gz` and a Windows `…-x86_64-pc-windows-msvc.zip` are attached to each [release](https://github.com/rikeda71/cxbridge/releases) as well.

### Cargo

```bash
cargo install cxbridge
```

### From source

```bash
git clone https://github.com/rikeda71/cxbridge.git
cd cxbridge
cargo install --path .
```

## What it converts

| Domain | Examples |
|---|---|
| **Skills** | `SKILL.md` frontmatter, `allowed-tools`, model/effort, body scan |
| **Plugins** | plugin manifests, bundled `commands/` and `agents/` directories |
| **Hooks** | event hooks, matchers, command hooks |
| **MCP servers** | `.mcp.json` ⇄ Codex `[mcp_servers]` |
| **Memory** | `CLAUDE.md` ⇄ `AGENTS.md` and memory settings |
| **Subagents** | agent definitions and model tiers |
| **Settings / Config** | `settings.json` ⇄ `config.toml` |
| **Variables** | `${CLAUDE_*}` placeholders and Codex equivalents |

For a per-domain breakdown of what converts cleanly, what is reshaped, and what is dropped, see **[docs/conversion-coverage.md](docs/conversion-coverage.md)**.

## The conversion report

Every run ends with a one-line `Summary:`; with `--report` you also get the per-field detail. Each converted file is introduced by a `▸ <domain>: <source>` header (so directory conversions stay legible), and each field line is tagged with one symbol:

| Symbol | Meaning |
|---|---|
| ◎ | **Lossless** — fully equivalent on the other side |
| ○ | **Lossy** — meaning preserved, but some information is reduced |
| △ | **Degraded** — moved to a broader scope (e.g. skill → project), which is named |
| ✕ | **Dropped** — no conversion target; discarded (and always reported) |
| ⚠ | **Body warning** — a construct in the body needs manual review |

To keep the output scannable, repeated fields are grouped with a `×N` count and long messages are truncated; body warnings are collapsed to a single count line. The **`--report=json`** form is exhaustive — every dropped/degraded/lossy entry and every body warning line, plus the `source` and `domain` of each file. Dropped and degraded fields are *always* enumerated either way — nothing is lost silently.

`--strict` turns any dropped field into a non-zero exit (code 2), so you can refuse conversions that would quietly lose data in CI.

## Documentation

- **[docs/conversion-coverage.md](docs/conversion-coverage.md)** — what converts, what degrades, and what is dropped, per domain.
- **[docs/spec.md](docs/spec.md)** — full design & implementation spec (IR model, transform registry, degrade engine, CLI flags, exit codes).
- **[mappings/](mappings/)** — the canonical conversion table (304 entries across 8 domains); schema in [mappings/SCHEMA.md](mappings/SCHEMA.md).

## License

[MIT](LICENSE) © Ryuya Ikeda
