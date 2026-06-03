# cxbridge — Design & Implementation Specification

> **Status: Implemented.** This document is the single source of truth for the `cxbridge` CLI design and implementation. It supersedes docs/01 through docs/13 in cases of conflict. Field-level detail is summarized here; precise, machine-readable values are deferred to `mappings/*.yaml`.
>
> Codex-side specification is fluid (2025–2026). Where `awaiting-codex` is noted, the relevant `mappings/*.yaml` entry carries a `notes: "status: awaiting-codex"` annotation.

---

## Table of Contents

1. [Project Goal & Scope](#1-project-goal--scope)
2. [Non-Goals & Constraints](#2-non-goals--constraints)
3. [Concept Mapping: Claude ↔ Codex](#3-concept-mapping-claude--codex)
4. [Three Fundamental Structural Differences](#4-three-fundamental-structural-differences)
5. [Architecture & Pipeline](#5-architecture--pipeline)
6. [IR Data Model](#6-ir-data-model)
7. [Mappings YAML Format & Invariants](#7-mappings-yaml-format--invariants)
8. [Transform Registry](#8-transform-registry)
9. [Domain Handler Contracts](#9-domain-handler-contracts)
   - 9.1 [Skills](#91-skills)
   - 9.2 [Hooks](#92-hooks)
   - 9.3 [MCP](#93-mcp)
   - 9.4 [Plugins](#94-plugins)
   - 9.5 [Memory](#95-memory)
   - 9.6 [Subagents](#96-subagents)
   - 9.7 [Settings / Config](#97-settings--config)
10. [Degrade Engine](#10-degrade-engine)
11. [Body Scanner](#11-body-scanner)
12. [Conversion Report](#12-conversion-report)
13. [CLI Commands, Flags, and Exit Codes](#13-cli-commands-flags-and-exit-codes)
14. [Error Handling & Fail-Open Policy](#14-error-handling--fail-open-policy)
15. [config.toml Non-Destructive Merge](#15-configtoml-non-destructive-merge)
16. [Feature & Loss Matrix Summary](#16-feature--loss-matrix-summary)
17. [Codex Interop Notes & Known Issues](#17-codex-interop-notes--known-issues)
18. [Testing Strategy](#18-testing-strategy)
19. [Extensibility: Hub-and-Spoke / Standard-Core Layering](#19-extensibility-hub-and-spoke--standard-core-layering)
20. [Technology Stack](#20-technology-stack)

---

## 1. Project Goal & Scope

**cxbridge** is a Rust CLI that bidirectionally converts configuration files between Claude Code (`.claude/`, JSON) and OpenAI Codex CLI (`.codex/`, TOML). It covers Skills, Plugins, Hooks, MCP servers, Memory files, Subagents, and Settings.

Conversion rules are declared in `mappings/*.yaml` (304 entries). The CLI is an engine that interprets those declarations. New field support requires only YAML edits, not code changes (mappings-driven design).

Every conversion produces a **conversion report** that enumerates what was lossless, lossy, degraded, dropped, and any body-scan warnings. Silent data loss is prohibited.

**Scope by version:**

| Domain | v1 | v2 | v3 | v4 |
|---|---|---|---|---|
| Skills (body scanner included) | ● | | | |
| MCP (plugin component) | ● | | | |
| Hooks (JSON ↔ TOML) | | ● | | |
| Memory (CLAUDE.md ↔ AGENTS.md) | | ● | | |
| Plugins (recursive, marketplace) | | | ● | |
| Subagents + Settings subset | | | | ● |

MVP = v1 (Skills + MCP roundtrip + report).

---

## 2. Non-Goals & Constraints

- **No full-auto settings conversion.** The permission model axis difference (tool-axis vs resource-axis) makes complete machine translation infeasible. Only a subset (permissions/env/model) is attempted.
- **Body rewrite is opt-in.** Default behavior is detection + warning only. `--rewrite-body` enables actual substitution.
- **No automatic model-name inference.** Model names are resolved via a three-tier (High/Mid/Low) constant table in code, not a user-editable file. Unknown model names are passed through with a warning.
- **Roundtrip losslessness only for `lossless` entries.** `lossy`/`dropped` differences are known and accepted.
- **No bidirectional sync.** `c2x` and `x2c` are independent one-way conversions.
- **No comment preservation in YAML.** `serde-saphyr` preserves key order but not comments. TOML comments and order are preserved via `toml_edit`.

---

## 3. Concept Mapping: Claude ↔ Codex

| Layer | Claude Code | Codex CLI | Format | Loss direction |
|---|---|---|---|---|
| Reusable instructions | `.claude/skills/<n>/SKILL.md` | `.agents/skills/<n>/SKILL.md` | MD + YAML frontmatter | Claude→Codex: lossy/degraded |
| Distribution bundle | `.claude-plugin/plugin.json` | `.codex-plugin/plugin.json` | JSON | Claude→Codex: several fields dropped |
| Distribution catalog | `.claude-plugin/marketplace.json` | `.agents/plugins/marketplace.json` | JSON | Near-identical |
| Sub-agents | `.claude/agents/<n>.md` | `[agents.<n>]` + `~/.codex/agents/<n>.toml` | MD / TOML | Structural divergence |
| Lifecycle hooks | `settings.json` / `hooks.json` `hooks` key | `config.toml` `[hooks.*]` | JSON / TOML | Claude: 30 events; Codex: 10 |
| MCP servers | `.mcp.json` | `[mcp_servers.*]` in `config.toml` | JSON / TOML | Claude→Codex: sse/ws dropped |
| Core settings | `settings.json` | `config.toml` | JSON / TOML | Axis divergence; partial subset only |
| Instruction memory | `CLAUDE.md` | `AGENTS.md` | Markdown | File rename + @import inline expansion |

**Invocation syntax:** `/skill-name` (Claude) ↔ `$skill-name` (Codex).

**File format differences requiring mechanical transforms:**

| Dimension | Claude | Codex | Transform |
|---|---|---|---|
| Core config format | JSON | TOML | `format:json_to_toml` |
| Skill directory | `.claude/` | `.agents/` | `path:remap` |
| Plugin manifest dir | `.claude-plugin/` | `.codex-plugin/` | `path:remap` |
| Boolean polarity (MCP) | `disabled: true` | `enabled: false` | `polarity:invert` |
| Timeout unit | ms | seconds float | `unit:ms_to_sec` |
| Argument index base | 0 (`$ARGUMENTS[0]`) | 1 (`$1`) | `index_shift:+1` |

---

## 4. Three Fundamental Structural Differences

### 4.1 Skill-scope dynamic control does not exist in Codex

Claude Code has a **dynamic scope**: many controls apply only "while this skill is executing." Examples: `allowed-tools`, `disallowed-tools`, `model`, `effort`, skill-scoped `hooks`. Codex skill `SKILL.md` frontmatter recognizes only `name` and `description` (plus `metadata.short-description`). All other Claude-side fields are silently ignored (fail-open) by Codex's `core-skills/loader.rs`, which does not use `deny_unknown_fields`.

This is not a simple missing field — Codex skill design intentionally uses a minimal model (name + description + body). The CLI resolves this via **scope degradation** (see §10).

### 4.2 Permission model axis: tool-axis vs resource-axis

- **Claude Code = tool-axis:** `permissions.allow/ask/deny` with patterns like `Bash(npm run *)`, `Read(~/.env)`. Evaluation order: deny → ask → allow.
- **Codex = resource-axis + phase separation:** `approval_policy` (when to confirm) + `sandbox_mode` (technical boundary) + `[permissions.<n>]` (filesystem paths with read/write/deny, network domains) + `.rules` (execpolicy: allow/prompt/forbidden Starlark).

Most permission conversions are `lossy`. The axis change means Claude's `Read`/`Write`/`Edit` boundaries are lost when converting to Codex filesystem permissions.

### 4.3 File format and placement divergence

See the transform table in §3. The most important mechanical differences are argument index (0 vs 1 based) and timeout units (ms vs seconds).

---

## 5. Architecture & Pipeline

```
Input path(s)
    │
    ▼ detect           — file kind from path pattern + first bytes
    │
    ▼ parse            — JSON / TOML / MD+YAML-frontmatter → serde_json::Value
    │                    (parse contract: {"frontmatter":{...}, "body":"...", "path":"..."})
    │
    ▼ lift(IR)         — domain handler maps source fields to IRNode
    │                    (index_by_*_field lookup → applies_direction → apply_transforms → IRField)
    │                    body → scan_body(dir) → BodySegment
    │                    (x2c also calls scan_body: detects $$ escape sequences in Codex bodies → Rewrite → $)
    │                    degrade entries → run_degrade() → SideArtifact + Diagnostic
    │
    ▼ lower(IR)        — domain handler emits EmitPlan (files + diagnostics)
    │                    opts: out dir, scope, dual_manifest, hooks_target, skill_target, interactive
    │                    (serialization happens inside each handler's lower():
    │                     toml_edit::DocumentMut for TOML, serde_json for JSON, serde-saphyr for YAML frontmatter)
    │
    ▼ build_report     — always produced; --report[=json] prints detail
    │
    ▼ write_plan       — writes EmitFile list + SideArtifacts; skipped on --dry-run; --force to overwrite
```

Key design principles:
- **Mappings-driven:** Field correspondence, loss level, transform, and degrade rules are declared in `mappings/*.yaml`. CLI code is an interpreter. New fields require only YAML edits.
- **Domain handlers:** Each handler owns parse/lift/lower for its domain. Plugins handler is the integration point — it delegates to skills/hooks/mcp handlers recursively.
- **SideArtifacts:** Degraded outputs (`.rules` files, `.codex/agents/*.toml`, `config.toml` patches) are generated as SideArtifact items alongside the primary output.

**Project layout:**

```
cxbridge/
├── Cargo.toml
├── mappings/           # ← YAML truth tables (304 entries)
│   ├── SCHEMA.md
│   └── *.yaml
├── src/
│   ├── lib.rs          # crate root; re-exports cli/core/degrade/handlers/scanner
│   ├── main.rs         # clap derive entry point
│   ├── cli.rs          # CLI definition + dispatch
│   ├── core/
│   │   ├── ir.rs
│   │   ├── mappings.rs
│   │   ├── transforms.rs
│   │   ├── report.rs
│   │   ├── detect.rs
│   │   └── serialize/  # json.rs, frontmatter.rs wrappers
│   ├── handlers/
│   │   ├── mod.rs      # Handler trait + LowerOpts + SkillTargetMode
│   │   ├── skills.rs
│   │   ├── hooks.rs
│   │   ├── mcp.rs
│   │   ├── plugins.rs
│   │   ├── memory.rs
│   │   ├── settings.rs
│   │   └── subagents.rs
│   ├── degrade/
│   │   ├── rules.rs    # allowed-tools → .rules (execpolicy)
│   │   ├── subagent.rs # skill(model/effort) → .codex/agents/*.toml
│   │   └── hooks_scope.rs
│   └── scanner/
│       └── body.rs
└── tests/
    ├── fixtures/
    ├── snapshots/      # insta golden snapshots
    ├── common/
    │   └── mod.rs      # shared test helpers
    ├── cli.rs
    ├── hooks.rs
    ├── mcp.rs
    ├── memory.rs
    ├── plugins.rs
    ├── settings.rs
    ├── skills.rs
    └── subagents.rs
```

---

## 6. IR Data Model

The IR is the normalized representation that both source and target tools are lifted into and lowered from. It enables bidirectional handling with a single model and structured diagnostics (aggregatable by tooling).

```rust
// core/ir.rs
use std::collections::HashMap;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool { Claude, Codex }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loss { Lossless, Lossy, Dropped }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind { Skill, Plugin, Subagent, Hooks, Mcp, Memory, Settings }

#[derive(Debug, Clone)]
pub struct IRField {
    pub id: String,                       // mappings entry id (e.g. "mcp.timeout")
    pub value: Value,                     // normalized value after lift
    pub loss: Loss,
    pub transforms_applied: Vec<String>,  // for report
    pub degrade: Option<DegradeInfo>,
    pub warning: Option<String>,          // from warn:true entries
    pub dropped: Option<DroppedInfo>,
}

#[derive(Debug, Clone)]
pub struct DegradeInfo { pub to: String, pub target: String }

#[derive(Debug, Clone)]
pub struct DroppedInfo { pub reason: String }

#[derive(Debug, Clone)]
pub struct BodySegment {
    pub raw: String,
    pub findings: Vec<BodyFinding>,       // from body scanner (§11)
}

#[derive(Debug, Clone)]
pub struct IRNode {
    pub kind: Kind,
    pub source_tool: Tool,
    pub source_path: String,
    pub fields: HashMap<String, IRField>, // id → field
    pub body: Option<BodySegment>,
    pub children: Vec<IRNode>,            // for plugins (recursive)
    pub side_artifacts: Vec<SideArtifact>,// degrade-generated extra files
    pub diagnostics: Vec<Diagnostic>,
    pub raw_frontmatter: Option<serde_json::Map<String, Value>>, // used for --keep-claude-frontmatter
}

#[derive(Debug, Clone)]
pub struct SideArtifact { pub path: String, pub content: String, pub note: String }

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagLevel { Info, Warn, Drop }
```

### Emit Types

`Handler::lower()` returns an `EmitPlan`. `write_plan` iterates its `files` list and writes each `EmitFile` to disk (root-relative paths are joined to absolute paths at write time).

```rust
// handlers/mod.rs
pub struct EmitPlan {
    pub files: Vec<EmitFile>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct EmitFile { pub path: String, pub content: String }
```

### CLI Opts and Skill Resolution Types

```rust
// src/cli.rs (clap derive)
#[derive(Parser)]
pub struct ConvertOpts {
    pub out: Option<String>,
    pub scope: Option<String>,
    pub hooks_target: Option<String>,
    pub rewrite_body: bool,
    pub dual_manifest: bool,
    pub report: Option<Option<String>>,
    pub dry_run: bool,
    pub strict: bool,
    pub force: bool,
    pub skill_target: Option<String>,  // auto|skill|subagent
    pub interactive: bool,
    pub only: Vec<String>,
    pub keep_claude_frontmatter: bool,
}

// handlers/mod.rs
/// Skill-target selection mode (from --skill-target flag).
/// Distinct from `SkillTarget`, the resolved decision enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTargetMode { Auto, Skill, Subagent }
```

```rust
// degrade/subagent.rs
/// The resolved skill conversion target after `decide_skill_target` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTarget { Skill, Subagent }
```

Key functions defined in the skeleton (`src/main.rs` / `handlers/skills.rs` / `core/`):

| Function | Module | Purpose |
|---|---|---|
| `pick_handler(kind, maps)` | `src/main.rs` | Dispatch `Kind` → `Box<dyn Handler>` |
| `decide_skill_target(ir, opts)` | `degrade/subagent.rs` | Resolve `SkillTargetMode` → `SkillTarget`; explicit → auto → gray-case |
| `ask_user_skill_target(ir)` | `degrade/subagent.rs` | TTY prompt (dialoguer) for gray-case interactive mode |
| `rewrite_body(raw, findings)` | `scanner/body.rs` | Apply `Action::Rewrite` findings; called only when `opts.rewrite_body == true` |
| `build_report(ir, plan)` | `core/report.rs` | Aggregate IR diagnostics + EmitPlan into `Report` |
| `upsert_agent_config(path, name, toml_path)` | `degrade/subagent.rs` | Non-destructive `toml_edit` merge for `[agents.*]` / `[features]` |
| `to_field(entry, value, applied)` | `handlers/mod.rs` | Construct `IRField` from a `MapEntry` + transformed value |
| `warn_for(entry)` | `handlers/mod.rs` | Construct `Diagnostic { level: Warn, ... }` for `warn: true` entries |
| `new_node(kind, source_tool, source_path)` | `core/ir.rs` | Construct a default `IRNode` |

### `parse()` Contract Shape

Every handler's `parse(path) -> anyhow::Result<serde_json::Value>` returns a `Value` of shape:

```json
{
  "frontmatter": { "name": "...", "description": "..." },
  "body": "...",
  "path": "/absolute/path/to/file"
}
```

- For TOML/JSON sources, all top-level fields go into `"frontmatter"` and `"body"` is empty.
- Frontmatter output key order follows the `mappings` entry definition order (leveraging `serde-saphyr` key-order preservation).
- **Lenient frontmatter fallback (`core/serialize/frontmatter.rs`):** Markdown frontmatter is parsed with strict YAML first. Real Claude agent/skill files often contain frontmatter that strict YAML rejects — most commonly an unquoted colon inside a value (e.g. a `description` containing `例: …`). When strict parse fails, a flat line-based fallback parser runs instead of skipping the file: a non-indented line whose key is an ASCII identifier starts a new string field (value = everything after the first colon); `key:` followed by indented `- item` lines becomes a string array; any other line is appended to the previous value, so wrapped multi-line descriptions survive without spawning spurious keys. Strict parse is unchanged when it succeeds (typed values and lists preserved).

---

## 7. Mappings YAML Format & Invariants

Each `mappings/<domain>.yaml` file has the structure:

```yaml
domain: skills
title: "Skills (SKILL.md)"
doc: ../docs/spec.md
files:
  claude: [".claude/skills/<name>/SKILL.md", "~/.claude/skills/<name>/SKILL.md"]
  codex:  [".agents/skills/<name>/SKILL.md", "~/.agents/skills/<name>/SKILL.md"]
format:
  claude: markdown+yaml-frontmatter
  codex:  markdown+yaml-frontmatter
entries:
  - id: skills.name
    claude: { field: "name", type: "string", scope: "skill" }
    codex:  { field: "name", type: "string", scope: "skill" }
    direction: both
    loss: lossless
    transform: null
    warn: false
    notes: "..."
    source: "https://..."
notes: ["..."]
```

### Entry Fields

| Field | Values | Meaning |
|---|---|---|
| `id` | `domain.field` string | Globally unique across all mappings files |
| `direction` | `both` / `claude_to_codex` / `codex_to_claude` | Which conversion direction(s) this entry applies to |
| `loss` | `lossless` / `lossy` / `dropped` | Information loss level |
| `degrade` | `{to: scope, target: "path/key"}` | Present when the value moves to a different (broader) scope |
| `transform` | `"unit:ms_to_sec; rename"` (`;`-separated) | Value transformation rules; see §8 |
| `warn` | `true` / `false` | Whether to emit a user warning at conversion time |

### The Invariants

`load_mappings()` asserts the following three invariants at startup via panics (failing at runtime if violated):

1. **`id` values are globally unique** across all `mappings/*.yaml` files.
2. **`degrade` implies `loss: lossy`** — a `degrade` block may only appear on entries with `loss: lossy`.
3. **`loss: dropped` entries must not carry a `transform`** — dropped fields have no output, so a transform would be meaningless.

The following are behavioral contracts enforced in handlers and tests (not startup panics):

- **`loss: dropped` entries are always listed in the conversion report.** Silent discard is prohibited.
- **`warn: true` entries always emit a user-visible warning** at conversion time.
- **`degrade` entries always record the target scope** (`to` field) in the report.
- **`direction`-scoped entries are ignored in the reverse direction** (not silently applied).

Note: the `source` field appears in `mappings/*.yaml` as documentation metadata but is **not** deserialized into `MapEntry` and is **not** validated by `load_mappings()`.

---

## 8. Transform Registry

Transforms are value-level functions applied during lift. The `transform` field in a mappings entry is a `;`-separated string of one or more transform specs, parsed by `parse_transform()` into `Vec<TransformSpec>`.

```rust
// core/transforms.rs
pub struct TransformCtx<'a> {
    pub direction: ConvDir,
    pub args: Option<HashMap<String, String>>,  // injected from TransformSpec.args (e.g. enum_map dict)
    pub field: &'a MapEntry,
}

pub type TransformFn = fn(&Value, &TransformCtx) -> Value;

/// One parsed transform directive from the `;`-separated `transform` string.
pub struct TransformSpec {
    pub name: String,                           // e.g. "enum_map", "unit:ms_to_sec"
    pub args: Option<HashMap<String, String>>,  // parsed from `{key:val,...}` suffix
}
```

### Transform Vocabulary

| Transform | Behavior |
|---|---|
| `unit:ms_to_sec` | `v / 1000.0` |
| `unit:sec_to_ms` | `(v * 1000.0).round()` |
| `polarity:invert` | `!v` (bool flip; e.g. `disabled:true` ↔ `enabled:false`) |
| `enum_map:{a:b,...}` | Map enum value; `args` injects the mapping dictionary |
| `index_shift` | Direction-aware: c2x adds +1, x2c subtracts 1 (`$ARGUMENTS[0]` ↔ `$1`). **Exception: index 0 is never auto-rewritten** — `$ARGUMENTS[0]` → `$1` would conflict with `$0` (the bash script name). Index 0 is warn+propose only; auto-rewrite applies only to indices ≥ 1. |
| `index_shift:+1` | Colon-arg alias for `index_shift`; used in `variables.yaml` to explicitly signal the +1 shift direction. |
| `str_to_list:space` | Split on whitespace → array |
| `list_to_str:space` | Join array with space |
| `rename` | Key rename only; value passes through |
| `extract:bearer_env` | Extracts `VAR` from `"Bearer ${VAR}"` |
| `path:remap` | Replaces path prefix (`.claude/` ↔ `.agents/`, `.claude-plugin/` ↔ `.codex-plugin/`) |
| `format:json_to_toml` | **No-op in transform registry.** Serializer handles it. |
| `format:toml_to_json` | **No-op in transform registry.** Serializer handles it. |
| `inline_imports` | **No-op in transform registry.** Handler's lower handles @import expansion. |

> `format:*` and `inline_imports` transforms are declarations — they signal to reviewers that format/import handling occurs, but the actual work is done by the handler and serializer, not by a transform function.

### Model Tier Mapping

Model names are not automatically inferred from ID strings. Instead, models are bucketed into three tiers, and the CLI holds a `const` table of current canonical names per tier per tool. When CLI versions are released, these consts are updated.

```rust
pub enum Tier { High, Mid, Low }

// Claude: opus → High, sonnet → Mid, haiku → Low
// Codex: explicit lookup against CODEX_LATEST first (roundtrip invariant);
//        fallback heuristic for unknown names: "mini" in name → Low, else Mid.
//        No suffix-based rule (-high/-xhigh) — current Codex models do not use such suffixes.

const CODEX_LATEST: &[(Tier, &str)] = &[
    (Tier::High, "gpt-5.5"),      // current frontier flagship; update at Codex release
    (Tier::Mid,  "gpt-5.4"),      // balanced default
    (Tier::Low,  "gpt-5.4-mini"), // fast/lightweight
];
const CLAUDE_LATEST: &[(Tier, &str)] = &[
    (Tier::High, "claude-opus-4-8"),
    (Tier::Mid,  "claude-sonnet-4-6"),
    (Tier::Low,  "claude-haiku-4-5"),
];
```

**Roundtrip-consistency invariant:** `codex_tier(tier_to_codex(t)) == Some(t)` for all `t`, and likewise for Claude. The CI tier roundtrip test (§18) enforces this. Unknown model names are passed through with a `DiagLevel::Warn` diagnostic.

`effort` enum mapping (`max` → `xhigh`) is handled by `enum_map:{max:xhigh,high:high,medium:medium,low:low}` in the mappings entry.

### Version Detection (`min_version` / `max_version`)

`cxbridge` detects the target tool version at conversion time and checks it against the `min_version` and `max_version` fields on each mappings entry. This allows the same `mappings/*.yaml` to work correctly across multiple versions of Claude Code or Codex CLI — entries that require a minimum version are silently skipped (with a diagnostic) when converting for an older target, and entries marked `max_version` are excluded for newer targets. An implementer must support reading these optional fields from `MapEntry` and skipping entries that fall outside the detected version range.

### Pipeline Direction Type

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvDir { C2x, X2c }
```

`ConvDir` is the pipeline execution direction. `MappingDirection` (from mappings YAML: `Both`/`ClaudeToCodex`/`CodexToClaude`) is a separate type used only to decide whether an entry applies in the current `ConvDir`.

---

## 9. Domain Handler Contracts

The `Handler` trait:

```rust
pub trait Handler {
    fn kind(&self) -> Kind;
    fn detect(&self, path: &Path) -> bool;
    fn parse(&self, path: &Path) -> anyhow::Result<serde_json::Value>;
    fn lift(&self, parsed: &serde_json::Value, dir: ConvDir) -> anyhow::Result<IRNode>;
    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan>;
}
```

### File Detection (`core/detect.rs`)

**File input:** Pattern match on filename + first bytes:
- `SKILL.md` (under a skills directory) → `Kind::Skill`
- `.mcp.json` → `Kind::Mcp`
- `CLAUDE.md` / `AGENTS.md` → `Kind::Memory`
- `hooks.json` → `Kind::Hooks`
- `settings.json` → parse: if it has a `hooks` key → `Kind::Hooks`; otherwise → `Kind::Settings`
- `config.toml` → parse: both `[mcp_servers]` + `[hooks]` present → `Kind::Plugin`; `[mcp_servers]` only → `Kind::Mcp`; `[hooks]` only → `Kind::Hooks`; neither → `Kind::Settings`
- `*plugin.json` (filename ends with `plugin.json`) → `Kind::Plugin`
- `openai.yaml` → `Kind::Skill`
- Non-config `*.toml` files → `Kind::Subagent`
- Agent-style `*.md` files (not SKILL.md / CLAUDE.md / AGENTS.md) → `Kind::Subagent`

**Directory input:** Walk with glob patterns for all of the above.

**x2c additions:** `.agents/skills/<n>/agents/openai.yaml` detected as `Kind::Skill` and merged with its sibling `SKILL.md`. `.codex/agents/<n>.toml` detected as `Kind::Subagent`.

**c2x:** `agents/openai.yaml` is a lower-side SideArtifact, not a lift-side input.

### 9.1 Skills

**Source:** `mappings/skills.yaml` | **Detail:** see [§9.1](#91-skills) (this document)

**parse:** `gray_matter` splits YAML frontmatter + body. The Markdown body string is passed to `scan_body()` (§11).

**lift (both directions):**
- Map frontmatter keys via `index_by_*_field` → `applies_direction` → `apply_transforms` → `IRField`.
- Unknown/unmapped keys become `Drop` diagnostics.

**lower (c2x):**
- Output: `name`, `description` (with `when_to_use` concatenated to description end).
- `disable-model-invocation: true` → SideArtifact `.agents/skills/<n>/agents/openai.yaml` with `policy.allow_implicit_invocation: false`. This is a handler special-case, not processed by `run_degrade`.
- `allowed-tools`, `disallowed-tools`, `model`, `effort`, `context:fork` → degrade engine (§10).
- `argument-hint`, `arguments`, `paths`, `user-invocable` → **dropped**.
- `shell: powershell` → propose `commandWindows` in hooks (warn only, no auto-conversion).
- Non-`.md` auxiliary files under the skill directory (scripts/, references/, assets/) → path-remap only, content unchanged.
- `--keep-claude-frontmatter`: retains Claude-specific frontmatter keys in output (Codex ignores them via fail-open).

**lower (x2c):**
- `agents/openai.yaml` `policy.allow_implicit_invocation: false` → `disable-model-invocation: true` (polarity invert).
- `interface.*` fields — specific handling:
  - `interface.display_name`, `interface.short_description`, `interface.icon_small`, `interface.icon_large`, `interface.brand_color` → **warn + dropped** (no Claude skill receptacle)
  - `interface.default_prompt` → **lossy approximate**: prepended to skill body as a note
  - `dependencies.tools` → **warn** (no Claude skill-level tool dependency concept)

**Skill target decision (c2x):**

Two possible conversion targets:

| Target | Description | Trade-off |
|---|---|---|
| `skill→skill` | `.agents/skills/<n>/SKILL.md` | Auto-invocation preserved; `model`/`effort`/permissions lost or session-degraded |
| `skill→subagent` | `.codex/agents/<n>.toml` + `config.toml [agents.*]` | `model`/`effort`/permissions preserved; explicit `spawn_agent` required (no auto-fork) |

**Decision table (`decide_skill_target`):**

| Skill characteristics | Recommended target | Reason |
|---|---|---|
| Has `model` / `effort` / `context:fork` | **subagent** | Cannot preserve in skill frontmatter |
| Pure instructions (no control frontmatter) | **skill** | Preserves auto-invocation convenience |
| Has `allowed-tools`/`disallowed-tools` only | Gray case | Trade-off between access control preservation and auto-invocation |

Priority: explicit `--skill-target` > deterministic auto-detection > gray-case (interactive TTY if `--interactive`, else conservative default = subagent).

**Lossless:** path, name, description, invocation syntax  
**Lossy/degraded:** when_to_use, disable-model-invocation, allowed-tools, disallowed-tools, model, effort, context:fork, skill-scoped hooks  
**Dropped:** user-invocable, paths, argument-hint, arguments, `` !`cmd` `` (dynamic injection), `${CLAUDE_*}` variables

### 9.2 Hooks

**Source:** `mappings/hooks.yaml` | **Detail:** see [§9.2](#92-hooks) (this document)

**Format conversion:** Claude JSON (`{"hooks":{"Event":[{matcher, hooks:[{type,...}]}]}}`) ↔ Codex TOML (`[[hooks.Event]]`). This structural conversion is handled by the handler + `toml_edit`, not by a transform function (`format:json_to_toml` in mappings is a declaration only).

**Events:**
- **10 common events (both directions, lossless):** SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, PreCompact, PostCompact, SubagentStart, SubagentStop, Stop.
- **20 Claude-only events (c2x: dropped, ⏳ awaiting-codex):** Setup, UserPromptExpansion, PermissionDenied, PostToolUseFailure, PostToolBatch, Notification, MessageDisplay, TaskCreated, TaskCompleted, StopFailure, TeammateIdle, InstructionsLoaded, ConfigChange, CwdChanged, FileChanged, WorktreeCreate, WorktreeRemove, Elicitation, ElicitationResult, SessionEnd.

**Hook types:**
- `command` → both directions (lossless for core fields).
- `http`, `mcp_tool` → **dropped** (⏳ awaiting-codex).
- `prompt`, `agent` → **dropped** (Codex parses schema but has no execution engine).

**Matcher conversion:**
- Exact string (alphanumeric + `_` + `|`) → Codex is always regex: `"Bash"` → `"^Bash$"`, `"Edit|Write"` → `"^(Edit|Write)$"` (lossy + warn).
- Wildcard (`"*"` or `""`) → `""` (Codex `"*"` evaluates as a literal regex and does not match all).
- Existing regex patterns → pass through with warn.

**`args` (exec form) synthesis:** Before dropping `args`, the handler synthesizes a shell-form command string by joining `command` + `args` with shell escaping (`shlex::quote` equivalent, i.e. `shell_escape`). This ensures that command arguments containing spaces or special characters produce a correct shell-form string rather than a broken one. The synthesized string replaces the original `command` value in the output; `args` itself is then dropped.

**Dropped command fields (c2x):** `args` (exec form — synthesized into `command` before drop), `shell`, `if`, `once`, `asyncRewake`, `CLAUDE_PROJECT_DIR` env var, `CLAUDE_ENV_FILE`, `CLAUDE_EFFORT` env vars.

**Dropped output fields (c2x):** `terminalSequence`, `sessionTitle`, `watchPaths`, `reloadSkills`, `initialUserMessage`, `permissionDecision: "defer"` (Claude-only value for `PreToolUse`; Codex does not support `defer` — the field must be dropped, not passed through).

**Dropped output fields (x2c):** `updatedMCPToolOutput` (Codex PostToolUse only), `model`, `turn_id`, `tool_use_id` (Codex-only stdin fields).

**Plugin-bundled hooks (c2x — issue #16430):** Codex may not load `hooks/hooks.json` from installed plugin roots. The CLI emits a `DiagLevel::Warn` diagnostic with the exact message: `"Plugin-bundled hooks may not be loaded by Codex (#16430). Use --hooks-target=user|project to output hooks to ~/.codex/hooks.json or .codex/config.toml instead."` Using `--hooks-target=user|project` is the recommended workaround to ensure hooks are picked up.

**Codex-specific stdin fields (x2c handling):** Codex adds extra stdin fields not present in Claude. The x2c handler must drop these when converting Codex hook configurations toward Claude:
- `SessionStart` input: `source` field (`startup|resume|clear|compact`) — Codex-only, dropped on x2c
- `Stop`/`SubagentStop` input: `stop_hook_active` — present in both (lossless), `last_assistant_message` — Codex-only, dropped on x2c
- `SubagentStop` input: `agent_transcript_path` — Codex-only, dropped on x2c
- Tool events: `turn_id`, `tool_use_id` — Codex-only, dropped on x2c; `model` — Codex-only, dropped on x2c

**Output JSON nesting:** `hookSpecificOutput.*` nesting differences are the hook script's responsibility, not the CLI's.

**Lossless:** 10 common events + command core fields  
**Lossy:** matcher normalization  
**Dropped:** 20 Claude-only events, http/mcp_tool/prompt/agent hook types, several command sub-fields

### 9.3 MCP

**Source:** `mappings/mcp.yaml` | **Detail:** see [§9.3](#93-mcp) (this document)

Mostly mappings-driven mechanical transforms.

**Key transforms:**
- `timeout` (ms) ↔ `tool_timeout_sec` (sec): `unit:ms_to_sec`
- `disabled: true` ↔ `enabled: false`: `polarity:invert`
- `headers` ↔ `http_headers`: `rename`
- Bearer auth: `headers.Authorization: "Bearer ${VAR}"` ↔ `bearer_token_env_var: "VAR"`: `extract:bearer_env`
- OAuth `scopes`: space-delimited string ↔ array: `str_to_list:space`
- Transport: Claude explicit `type` ↔ Codex implicit (`command` present = stdio, `url` present = http)

**HTTP transport `env` restriction:** Codex `env` is stdio-only. For c2x with http transport:
1. `env` values in `${VAR}` form → convert to `env_http_headers` (header name = env key, value = bare `VAR` — Codex maps a header to an env-var *name*, e.g. `{ "X-Auth": "AUTH_ENV" }`).
2. Literal values → warn + request manual action.

**`env_http_headers` x2c direction:** Codex `env_http_headers` entries are converted back to Claude `headers` as `${VAR}` form (e.g. `{ "Authorization": "API_KEY" }` → `headers.Authorization: "${API_KEY}"`). This is the inverse of the c2x transform above.

**Dropped (c2x):** `alwaysLoad`, `headersHelper`, `sse`/`ws` transport, `oauth.authServerMetadataUrl`.  
**Dropped (x2c):** `enabled_tools`, `disabled_tools`, `default_tools_approval_mode`, `tools.<name>.approval_mode`, `startup_timeout_sec`, `env_vars`, `required`, `supports_parallel_tool_calls`, `environment_id`, `oauth_resource`.  
**`mcp.enabled: false` (x2c):** Codex MCP entries with `enabled: false` are **entirely excluded** from the Claude output — the whole server entry is omitted, not merely converted with the field dropped. This is a behavioral distinction: the handler must filter out disabled servers before emitting, not just strip the `enabled` field.

**Lossless:** command, args, env, cwd, url, headers/http_headers, OAuth client_id, OAuth scopes, timeout  
**Lossy:** transport type detection, Bearer extraction, OAuth callback_port (scope mismatch)  
**Dropped:** see above

### 9.4 Plugins

**Source:** `mappings/plugins.yaml` | **Detail:** see [§9.4](#94-plugins) (this document)

The Plugins handler is the integration point. It coordinates skills/hooks/mcp handlers recursively.

1. Transform `plugin.json` manifest fields (`.claude-plugin/` ↔ `.codex-plugin/` via `path:remap`).
2. Delegate `skills/`, `hooks/`, `.mcp.json` sub-directories to their respective handlers; store results in `IRNode.children`.
3. **c2x dropped fields:** `lspServers`, `outputStyles`, `experimental.themes`, `experimental.monitors`, `settings`, `channels`, `userConfig`, `dependencies`. All emit dropped + warn diagnostics.
   - `userConfig` carries extra warn: unresolved `${user_config.KEY}` references in MCP/hooks may silently break.
4. **c2x/x2c fields:** `commands` → lossless path-remap (`commands/` auto-discovered on both sides; no SKILL.md wrapper required). `agents` → lossy path-remap; per-file frontmatter converted via subagents domain rules. `defaultEnabled` → `policy.installation: INSTALLED_BY_DEFAULT` (lossy approximate — not fully equivalent).
5. `marketplace.json`: near-identical schema; `source` type normalization required (Claude `relative` → Codex `{source:"local",...}`); Claude `github` source → approximate with `git-subdir` or `url` (Codex has no `github` shorthand source type); `npm`-type sources dropped. Missing `policy` entries get default values (`installation=AVAILABLE`, `authentication=ON_INSTALL`) with report annotation. Additional dropped fields (c2x): `marketplace.owner`, `allowCrossMarketplaceDependenciesOn`, `forceRemoveDeletedPlugins`. `marketplace.plugins[].policy` is Codex-only and dropped on x2c.
6. **Dual manifest strategy (`--dual-manifest`):** Retain `.claude-plugin/` and generate `.codex-plugin/plugin.json` alongside. This is the only way to get native Codex recognition.

**Hook #16430 applies here too.** Plugin-bundled hooks may not be loaded by Codex. Use `--hooks-target=user|project` as the recommended workaround (see §9.2 and §17).

**x2c dropped/lossy — Codex-specific `interface.*` fields:** When converting Codex → Claude, the following Codex `interface.*` fields have no direct Claude plugin receptacle and are handled as follows:
- `interface.brandColor`, `interface.composerIcon`, `interface.logo`, `interface.capabilities`, `interface.screenshots`, `interface.privacyPolicyURL`, `interface.termsOfServiceURL` → **dropped** (x2c)
- `interface.websiteURL` → lossy approximate → `homepage`
- `interface.defaultPrompt` → lossy approximate → prepended to skill body
- `interface.developerName` → lossy approximate → `author.name`
- `interface.category` → lossy approximate → appended to `keywords` array
- `interface.longDescription` → lossy approximate ↔ `description`

**Lossless:** name, version, description, author/homepage/repository/license/keywords, displayName, marketplace core fields, commands (path-remap)  
**Lossy (c2x/x2c):** skills path (multi-path array cannot be fully represented in Codex manifest), short-description, version strict-semver, mcpServers inline→file, hooks (event/type limits), agents (per-file frontmatter via subagents rules), defaultEnabled  
**Dropped (c2x):** see item 3 above (lspServers, outputStyles, experimental.themes, experimental.monitors, settings, channels, userConfig, dependencies)  
**Dropped/lossy (x2c):** Codex `interface.*` fields as above  
**marketplace `source`:** Codex supports only `local`, `url`, `git-subdir` — there is no `github` shorthand; Claude `github` sources approximate to `git-subdir`/`url`.

### 9.5 Memory

**Source:** `mappings/memory.yaml` | **Detail:** see [§9.5](#95-memory) (this document)

**Core conversion:** `CLAUDE.md` ↔ `AGENTS.md` with path remap (`~/.claude/CLAUDE.md` ↔ `~/.codex/AGENTS.md`).

**`@import` expansion (c2x):** Codex has no `@import` equivalent. The `inline_imports` transform (declared in mappings, executed by the handler's lower) expands all `@import path` references inline. Warn if result exceeds 28 KiB (Codex `project_doc_max_bytes` is 32 KiB; 28 KiB is the warning threshold).

**Subdirectory on-demand load (lossy, c2x):** Claude scans subdirectories on-demand for `CLAUDE.md` files. Codex does not walk deeper than CWD when scanning for `AGENTS.md`. This behavioral difference is classified as lossy: memory files in subdirectories that Claude would pick up may be invisible to Codex. Emit a diagnostic when subdirectory `CLAUDE.md` files are detected during conversion.

**Dropped (c2x):** `CLAUDE.local.md` (no non-committed personal file concept in Codex), managed policy `/etc/` CLAUDE.md, `claudeMdExcludes`, `rules/*.md` paths frontmatter, Auto memory (`MEMORY.md`).  
**Dropped (x2c):** `AGENTS.override.md` (Codex-only complete-replacement concept), `features.child_agents_md`, `project_doc_fallback_filenames`.

### 9.6 Subagents

**Source:** `mappings/subagents.yaml` | **Detail:** see [§9.6](#96-subagents) (this document)

The structural divergence is significant. Key architectural difference: Claude subagents are auto-dispatched via description semantic matching; Codex subagents require explicit `spawn_agent` invocation.

**Lossless:** file path (format conversion: MD ↔ TOML), name, description, body/developer_instructions (content lossless, format converts).  
**Lossy (c2x):** model (different provider; tier mapping applied), effort (enum_map), mcpServers (rename + format), skills (meaning differs: Claude injects content, Codex overrides enabled state), permissionMode (`bypassPermissions` → `danger-full-access`, `plan` → `read-only`, etc. — approximate), tools (lossy: no per-tool allow-list concept → `sandbox_mode` approximation), hooks (lossy: agent scope not supported → session/project hooks degrade), memory (lossy: 3-scope → global boolean), initialPrompt (lossy: auto-submit behavior dropped → appended to `developer_instructions`).  
**Dropped (c2x):** disallowedTools, maxTurns, background, isolation:worktree, color, full auto-dispatch behavior.  
**Dropped (x2c):** nickname_candidates, config_file, agents.max_threads/max_depth/job_max_runtime_seconds.

### 9.7 Settings / Config

**Source:** `mappings/settings-config.yaml` | **Detail:** see [§9.7](#97-settings--config) (this document)

Full automatic conversion is not attempted. Only a subset is converted.

**Partial-subset converted (lossy):** model (tier mapping), effortLevel/model_reasoning_effort, autoMemoryEnabled→use_memories+generate_memories, cleanupPeriodDays, attribution/commit_attribution, editorMode/vim_mode_default, sandbox.network.allowAllUnixSockets.

**`defaultMode` approximate conversions (lossy, c2x):**
- `defaultMode: acceptEdits` → `approval_policy = "untrusted"`
- `defaultMode: auto` → `approval_policy = "on-request"` (Codex default)
- `defaultMode: bypassPermissions` → `approval_policy = "never"` + `sandbox_mode = "danger-full-access"`
- `defaultMode: plan` → **dropped** (no Codex equivalent)

**Permission conversion (lossy, degrade):**
- `permissions.allow(Bash(...))` → `.codex/rules/<n>.rules` (prefix_rule allow)
- `permissions.deny(Bash(...))` → `.codex/rules/<n>.rules` (prefix_rule forbidden)
- `permissions.allow(Read/Write)` → `[permissions.<n>].filesystem` (tool-axis → resource-axis; Read/Write boundary lost)
- `permissions.allow/deny(WebFetch)` → `[permissions.<n>].network = true|false` (boolean)

**Dropped (c2x):** viewMode, worktree, autoUpdatesChannel, spinnerTips, voice/voiceEnabled, maxSkillDescriptionChars, defaultMode:plan.  
**Dropped (x2c):** profiles, permissions.extends, approval_policy.granular.*, agents.max_threads/max_depth, tui.keymap.*, model_verbosity, web_search, features.child_agents_md, project_doc_fallback_filenames, developer_instructions (→ CLAUDE.md approximation only).

**Lossless count: 2 out of 49 entries (4%).**

---

## 10. Degrade Engine

When Claude skill-scoped control has no Codex skill-scope equivalent, the degrade engine places the equivalent setting in a broader scope, generating SideArtifacts and diagnostics.

### 10.1 `allowed-tools` / `disallowed-tools` → Tool-type dispatch

```
allowed-tools: ["Bash(git add *)", "Write(**/*.py)"]
        ↓ c2x
```

| Claude pattern | Codex target |
|---|---|
| `Bash(<cmd> <args>)` | `.codex/rules/<skill>.rules` — Starlark `prefix_rule(pattern=[...], decision="allow", justification="from skill <name>")` |
| `Write(<glob>)` / `Edit(<glob>)` | `[permissions.<skill>].filesystem.<glob> = "write"` |
| `Read(<glob>)` | `[permissions.<skill>].filesystem.<glob> = "read"` |
| `WebFetch` | `[permissions.<skill>].network = true\|false` (boolean) |
| `WebSearch` | `[features].web_search = true\|false` (boolean) |
| `mcp__<server>__<tool>` | `[mcp_servers.<server>].enabled_tools` / `disabled_tools` |
| Built-in tools (e.g. `AskUserQuestion`) | **Dropped** — no equivalent |

**`.rules` scope requirement:** Project-scoped `.rules` files require `trust_level='trusted'` on the project. The generated Starlark uses `prefix_rule(pattern=[...], decision="allow", justification="from skill <name>")` — the `justification` parameter is required for audit-trail purposes.

**`disallowed-tools` dispatch:** `disallowed-tools` entries follow the same tool-type dispatch as `allowed-tools` but produce `decision="forbidden"` rules. Specifically, `Bash(...)` patterns → `.rules` with `decision="forbidden"`; `Write`/`Read`/`Edit` → `filesystem` deny; `mcp__*` → `disabled_tools`.

**Scope note (important):** `.rules` and `[permissions.*]` are session/project scope — not skill scope. To approximate skill-scoped permissions, bundle them inside a subagent's `config_file` (see §10.2). The CLI emits a scope-expansion warning when degrading to session/project.

**Bash wildcard handling:** Trailing wildcards → prefix match. Mid-string wildcards (e.g. `git add *.py`) → prefix truncation + warn.

**`.rules` generation:** Starlark format generated via `format!` macro. `[permissions.*]` appended via `toml_edit::DocumentMut` non-destructive merge.

### 10.2 `skill(model/effort/context:fork)` → subagent

```
model: opus, effort: max, context: fork
        ↓ c2x → .codex/agents/<skill>.toml
```

Generated file content:

```toml
name = "<skill>"
description = "<when_to_use or description>"
developer_instructions = "<skill body>"
model = "<tier_to_codex(claude_tier(model))>"
model_reasoning_effort = "xhigh"  # max→xhigh via enum_map
```

`config.toml` receives a non-destructive append:

```toml
[agents.<skill>]
config_file = ".codex/agents/<skill>.toml"

[features]
multi_agent = true
```

**Diagnostic:** Emits a note: `skill '<name>' degraded to subagent`. The degrade writes `.codex/agents/<name>.toml` and sets `[features].multi_agent = true` in `config.toml`. The Claude auto-fork behavior is replaced by an explicit `spawn_agent` call — callers must invoke the subagent explicitly.

### 10.3 Skill-scoped hooks → session/project hooks

Skill frontmatter `hooks` are moved to `[[hooks.<Event>]]` in `.codex/config.toml` (project scope, `--hooks-target=project`) or `~/.codex/hooks.json` (user scope, `--hooks-target=user`).

**Diagnostic:** "Skill-scoped hooks degraded to session/project scope. They will fire for all sessions, not only when this skill runs."

Only `command` hook type is portable. `http`, `mcp_tool`, `prompt`, `agent` are dropped.

---

## 11. Body Scanner

The body scanner detects variables, invocation syntax, and dynamic injection in Markdown bodies. It is read-only by default; `--rewrite-body` enables actual substitution.

```rust
pub struct BodyFinding {
    pub kind: FindingKind,
    pub matched: String,
    pub line: usize,
    pub action: Action,
    pub rewrite: Option<String>,  // populated when action == Rewrite
    pub note: String,
}

pub enum FindingKind {
    ArgIndexed, ArgNamed, EnvVar, DynamicInline, DynamicBlock, InvokeSlash, InvokeNamespaced,
}

pub enum Action { Rewrite, Warn, Drop }

/// Controls context-sensitive env-var handling (see Detection Table below).
pub enum BodyContext {
    SkillBody,   // default: all ${CLAUDE_*} variables are dropped
    PluginHook,  // ${CLAUDE_PLUGIN_ROOT} and ${CLAUDE_PLUGIN_DATA} are lossless
}
```

### Detection Table (c2x direction)

| Pattern (regex sketch) | Context | Kind | Action | Notes |
|---|---|---|---|---|
| `\$ARGUMENTS\[(\d+)\]` | any | ArgIndexed | **Rewrite** (index+1) | `$ARGUMENTS[0]` → `$1`. Exception: `[0]` is warn+propose only (conflicts with `$0` = shell script name). |
| `\$([1-9][0-9]*)` (positional, x2c only) | any | ArgIndexed | **Rewrite** (index−1) | x2c direction: `$1` → `$ARGUMENTS[0]` etc. (index shift −1). |
| `\$ARGUMENTS(?!\[)` (bare, c2x only) | any | ArgIndexed | **Warn** | Bare `$ARGUMENTS` without `[N]` — Codex supports this only in Custom Prompts, not in Skill bodies. Do not rewrite. |
| `\$\$` (x2c only) | any | — | Rewrite → `$` | Codex Custom Prompts escape |
| `\$([a-z][a-z0-9_]*)` | any | ArgNamed | Warn | Invocation syntax changes to `KEY=value` in Codex |
| `${CLAUDE_PLUGIN_ROOT}` or `${CLAUDE_PLUGIN_DATA}` | PluginHook | EnvVar | *(no finding)* | Codex injects these in plugin-sourced hook commands → lossless in that context |
| `${CLAUDE_PLUGIN_ROOT}` or `${CLAUDE_PLUGIN_DATA}` | SkillBody | EnvVar | **Drop** | Not injected in skill bodies |
| other `\$\{CLAUDE_[A-Z_]+\}` | any | EnvVar | **Drop** | No Codex equivalent; literal residue causes misoperation |
| `` (^|\s)!`[^`]+` `` | any | DynamicInline | Warn | Codex issue #5019: "not planned". Literal residue is high-risk misoperation. |
| ` ```! ` | any | DynamicBlock | Warn | Same |
| `/[\w-]+` (in body prose) | any | InvokeSlash | Warn → propose `$name` | False-positive risk; detection+proposal only |
| `/[\w-]+:[\w-]+` | any | InvokeNamespaced | Drop | No namespace concept in Codex |

**`scan_body(body, dir, context)` is detection-only.** When `opts.rewrite_body == true`, the handler's lower additionally calls `rewrite_body(raw, findings)` which applies only `Action::Rewrite` findings and returns the rewritten string. The default (rewrite_body=false) emits `Warn` diagnostics only; the original body is emitted unchanged. Skill-body scans always pass `BodyContext::SkillBody`; plugin-hook-command scans pass `BodyContext::PluginHook`.

**False-positive mitigation:** `\$([a-z][a-z0-9_]*)` may match shell script variables (`$HOME`, `$PATH`). Code blocks should be excluded or flagged for manual review.

---

## 12. Conversion Report

The report is always produced. It is built from `IRNode.diagnostics` and `IRField` metadata.

```rust
/// A single diagnostic entry used in every `Vec<DiagEntry>` field of `Report`.
pub struct DiagEntry {
    pub id:      Option<String>,  // mappings entry id, e.g. "skill.allowed-tools"
    pub message: String,
}

pub struct Report {
    pub lossless:      Vec<String>,     // entry ids
    pub lossy:         Vec<DiagEntry>,
    pub dropped:       Vec<DiagEntry>,  // ALWAYS listed — silent discard prohibited
    pub degraded:      Vec<DiagEntry>,  // ALWAYS listed with target scope
    pub body_warnings: Vec<DiagEntry>,
}
```

**Invariant:** `dropped` and `degraded` are always enumerated. Silent discard is prohibited.

### Report Format (human-readable)

`print_report` delegates the text formatting to the pure, testable
`format_report_text(report) -> String`. Each converted file's report is preceded
by a `▸ <domain>: <source>` header (the source path relative to the input root, or
the file name for single-file inputs), so a directory conversion's per-file reports
are distinguishable. Within a file, lossy/degraded/dropped entries are **grouped by
id** with a `(×N)` count, long messages are truncated to ~100 chars, and body
warnings are collapsed to a single count line (`--report=json` keeps the full
line-by-line detail). Lossless ids are listed when few, counted when many. Dropped
and degraded ids are always enumerated.

```
$ cxbridge c2x .claude/skills/deploy/SKILL.md --report

▸ skills: SKILL.md
  ◎ skills.name, skills.description  lossless
  ○ skills.allowed-tools (×2)  lossy  Per-tool pre-approval control at skill scope does not exist in Codex…
  △ skills.allowed-tools  degrade  skills.allowed-tools → .codex/rules/<skill>.rules (execpolicy allow)…
  ✕ skills.user-invocable  dropped  model-only / hidden-from-user flag has no Codex concept
  ⚠ 3 body warnings — run with --report=json for line-by-line
Summary: 2 lossless, 1 lossy(1 degraded), 1 dropped, 3 body-warning
```

| Symbol | Meaning |
|---|---|
| ◎ | Lossless: fully equivalent |
| ○ | Lossy: meaning preserved but information partially lost or scope changed |
| △ | Degraded: moved to a different (broader) scope |
| ✕ | Dropped: no conversion target; discarded |
| ⚠ | Body warning: requires manual review |

**`--report=json`:** machine-readable, exhaustive — every lossy/degraded/dropped
entry and every body-warning line, plus the `source` and `domain` of each file.
**`--dry-run`:** produces the report without writing any files.

---

## 13. CLI Commands, Flags, and Exit Codes

### Commands

```
cxbridge c2x <path> [options]    # Claude → Codex (one-way)
cxbridge x2c <path> [options]    # Codex → Claude (one-way)
cxbridge check <path>            # Pre-conversion diagnosis (dropped count estimate, no writes)
cxbridge --version               # Print version and exit
```

`<path>` accepts a file or directory (recursive detection).

### Options (shared by `c2x` / `x2c`)

| Flag | Default | Description |
|---|---|---|
| `--out <dir>` | See Output Directory Structure (three distinct defaults by input type) | Output root directory |
| `--only <domains>` | all | Comma-separated domain filter (e.g. `skills,mcp`) |
| `--scope <project\|user>` | `project` | Degrade target scope (`.rules` / agents placement) |
| `--skill-target <auto\|skill\|subagent>` | `auto` | Force skill conversion target |
| `--interactive` | false | TTY confirmation for gray-case skills |
| `--rewrite-body` | false | Apply body substitutions (default: detect-only) |
| `--dual-manifest` | false | Keep `.claude-plugin/` and generate `.codex-plugin/` alongside |
| `--hooks-target <user\|project>` | `user` | Hooks write destination; `user` → `~/.codex/hooks.json`, `project` → `.codex/config.toml`; recommended workaround for #16430 |
| `--report[=json]` | none | Emit detailed report (`=json` for machine-readable) |
| `--dry-run` | false | Report only, no file writes |
| `--strict` | false | Exit 2 if any dropped entries exist (CI use) |
| `--keep-claude-frontmatter` | false | Retain Claude-specific frontmatter keys in Codex output (Codex ignores them via fail-open) |
| `--force` | false | Allow overwriting existing files |

### Output Directory Structure

| Input type | Default output |
|---|---|
| Single skill directory | `<skill_dir>.converted/` |
| `.mcp.json` file | `<parent>/<stem>.converted/` (e.g. `.mcp.json` → `.mcp.converted/`), a directory |
| Project root | `./.codex-converted/` |

Generated files within the output root:

| Artifact | Path |
|---|---|
| Converted SKILL.md | `<root>/.agents/skills/<n>/SKILL.md` |
| Converted .mcp.json | `<root>/.mcp.json` |
| execpolicy `.rules` | `<root>/.codex/rules/<skill>.rules` |
| Subagent TOML | `<root>/.codex/agents/<skill>.toml` |
| config.toml patch | `<root>/config.toml` (non-destructive merge or new) |
| openai.yaml SideArtifact | `<root>/.agents/skills/<n>/agents/openai.yaml` |

All `SideArtifact.path` and `EmitFile.path` are stored as root-relative; `write_plan` joins them to absolute paths.

### `--skill-target` Priority

1. Explicit `--skill-target skill|subagent` → always honored.
2. `auto` + deterministic case → auto-detect (model/effort/context:fork present → subagent; pure instructions → skill).
3. Gray case (permissions only; acceptable scope undetermined):
   - `--interactive` present → TTY prompt.
   - Non-interactive → conservative default (subagent) + report annotation with reason.

### Exit Codes

| Code | Condition |
|---|---|
| 0 | Success (dropped entries allowed) |
| 1 | Input error or parse failure |
| 2 | `--strict` mode with one or more dropped entries |

---

## 14. Error Handling & Fail-Open Policy

- Parse failures (invalid JSON/TOML/frontmatter) → **skip that file + error diagnostic**; other files continue. A bundle conversion is not halted by a single file's failure.
- Unknown fields → **drop + diagnostic**, processing continues. This mirrors Codex's own fail-open loader philosophy.
- Untranslatable fields → always emit diagnostic, never silently discard.
- **Existing file overwrite is blocked by default.** Use `--force` to allow overwrite. `.rules` and `config.toml` are append/merge only (never overwrite).
- Existing non-table keys in `config.toml` are not overwritten — they are kept silently. If `[features].multi_agent` already exists, the CLI keeps the existing value without warning (see §15 for the full merge algorithm).

---

## 15. config.toml Non-Destructive Merge

All writes to `config.toml` use `toml_edit::DocumentMut`. No string-patching (sed-style replacement) is used.

**Algorithm (`merge_config_toml` in `src/cli.rs`):**

1. Read existing `config.toml` as `DocumentMut` (or start with empty `DocumentMut` if file absent).
2. For each key in the patch document:
   - If the key is absent in the base: insert it.
   - If both sides have a table for the key: recurse into that table and apply the same logic.
   - If the key already exists in the base as a non-table value: **keep the existing value silently** (no warning emitted).
3. Write back with `doc.to_string()` (preserves comments and key order).

**Array-of-tables:** `[[array-of-tables]]` sections are **not** specially handled — there is no `ArrayOfTables::push`. Do not rely on this function to append `[[hooks.*]]` array entries.

**Caveat:** Scattered `[[array-of-tables]]` sections in the source file may be reordered to contiguous positions when `toml_edit` parses them (comments and values are preserved).

---

## 16. Feature & Loss Matrix Summary

Total entries across all `mappings/*.yaml`: **304**

| Loss level | Count | % |
|---|---|---|
| lossless | 73 | 24% |
| lossy | 89 | 29% |
| dropped | 142 | 47% |

**Directional asymmetry:**
- **Codex → Claude:** Near-lossless. Codex vocabulary is smaller; Claude has receptacles for most concepts.
- **Claude → Codex:** High loss. Claude has richer skill-scope control, argument machinery, dynamic injection, and many hook events that Codex does not implement.

### Per-Domain Summary

| Domain | Entries | Lossless | Lossy | Dropped | Notes |
|---|---|---|---|---|---|
| Skills | 23 | 5 | 13 | 5 | Core — degrade engine + body scanner |
| Hooks | 83 | 34 | 6 | 43 | Core — JSON↔TOML structural conversion |
| MCP | 32 | 10 | 4 | 18 | Lightweight mechanical transforms |
| Plugins | 48 | 13 | 15 | 20 | Integration point; recursive |
| Memory | 18 | 3 | 5 | 10 | File rename + @import expansion |
| Subagents | 25 | 4 | 10 | 11 | Large structural divergence |
| Settings/Config | 60 | 2 (3%) | 31 | 27 | Hardest; permission axis mismatch |
| Variables | 15 | 2 | 5 | 8 | No standalone handler. All variable-related transformations are performed by the body scanner within the Skills handler. |

**Implementation value/complexity concentration:**
- **Skills** = new logic (degrade engine, body scanner).
- **Hooks** = non-trivial structural conversion (array-of-tables, 30↔10 events).
- **Plugins** = integration point; calls all other handlers recursively.
- **MCP / Memory** = lightweight; mechanical transforms only.

### Confirmed Dropped Fields (representative)

Fields with no Codex equivalent, always dropped on c2x:

- `user-invocable`, `paths` (glob auto-trigger), `argument-hint`, `arguments` — skill invocation machinery
- `` !`cmd` `` (inline dynamic injection), `${CLAUDE_*}` variables — body preprocessing
- `lspServers`, `outputStyles`, `experimental.themes`, `experimental.monitors`, `bin`, `userConfig`, `channels` — plugin features
- Claude-only hook events (20): Setup, UserPromptExpansion, PermissionDenied, PostToolUseFailure, PostToolBatch, Notification, MessageDisplay, TaskCreated, TaskCompleted, StopFailure, TeammateIdle, InstructionsLoaded, ConfigChange, CwdChanged, FileChanged, WorktreeCreate, WorktreeRemove, Elicitation, ElicitationResult, SessionEnd
- `http`, `mcp_tool` hook types

---

## 17. Codex Interop Notes & Known Issues

### Fail-Open Loader

Codex's `core-skills/loader.rs` (`SkillFrontmatter` struct) does **not** use `deny_unknown_fields`. Claude-specific frontmatter in a SKILL.md does not cause errors — `name` and `description` are extracted; all other fields are silently ignored. This means:

- Static-text-only skills can be brought to Codex directly with ~90% functionality.
- Tool permissions (`allowed-tools`), model selection, and invocation control (`disable-model-invocation`) silently have no effect without explicit degradation artifacts.

The CLI mirrors this philosophy: unknown fields produce drop diagnostics but processing continues.

### Known Codex Issues and Follow-Up Policy

| Issue | Description | CLI Behavior |
|---|---|---|
| **#16430** | Plugin-bundled `hooks.json` may not be loaded by Codex. | Emit `DiagLevel::Warn`: `"Plugin-bundled hooks may not be loaded by Codex (#16430). Use --hooks-target=user\|project to output hooks to ~/.codex/hooks.json or .codex/config.toml instead."` `--hooks-target` is the recommended workaround. |
| **#14161** | `[[skills.config]]` per-skill override: `enabled` and `path` fields inside `SkillConfig` were silently ignored at runtime — **fixed 2026-03 (PR #14806)**. Per-skill config overrides are now stable. | No longer needs a degradation warning for this bug specifically; existing diagnostics for scope expansion remain |
| **#21753** | Hook event parity tracker (still open) — `SubagentStart`/`SubagentStop` are listed as "Missing" in the tracker matrix, but **both ARE implemented** in the current Codex source (`HookEventName` enum). Source is authoritative; tracker is stale for these two events. Other Claude-only events remain unimplemented. | Treat `SubagentStart`/`SubagentStop` as common events (both/lossless). Mark remaining 20 Claude-only events as `⏳ awaiting-codex`; drop + warn. |
| **#5019** | Dynamic injection `` !`cmd` `` — "not planned" | Body scanner detects + warns; no auto-removal (too destructive); user must manually handle |

**Awaiting-Codex follow-up policy:** Fields currently `dropped` due to missing Codex implementation carry `notes: "status: awaiting-codex"` in their mappings entry. When Codex ships the feature, update the entry (`loss: dropped` → `both`/`lossy`, add `codex.field` and any `transform`). No CLI engine code change is needed — the mappings-driven design handles it automatically.

Current `awaiting-codex` fields include: `user-invocable`, `paths` (auto-trigger), `http`/`mcp_tool` hook types, `prompt`/`agent` hook types (Codex parses but does not execute), Claude-only hook events (20), `once`, `if`, `asyncRewake`, and SessionStart output fields `sessionTitle`/`watchPaths`/`reloadSkills`/`initialUserMessage`.

### Codex Invocation Model Difference

Claude uses description semantic matching for automatic subagent dispatch. Codex requires explicit `spawn_agent` call. There is no mechanical workaround for this behavioral difference — it is always noted as a `lossy` diagnostic.

### Dual Manifest Strategy

`.claude-plugin/marketplace.json` is read by Codex as a "legacy-compatible marketplace." However, `.claude-plugin/plugin.json` is **not** natively interpreted by Codex — only `.codex-plugin/plugin.json` is. Use `--dual-manifest` (or create both manually) to be recognized by both tools.

`codex-plugin-cc` is a one-way bridge (Codex → Claude Code), not a compatibility layer. Do not conflate it with cxbridge.

---

## 18. Testing Strategy

1. **Mappings invariant tests** (at startup + CI): assert globally unique `id`, `degrade` implies `loss:lossy`, `loss:dropped` has no `transform`. 304 entries; 0 issues confirmed. (Note: `source` field is not validated — it is documentation metadata only.)

2. **Unit tests:**
   - Each transform function (`unit:ms_to_sec`, `polarity:invert`, `enum_map`, `index_shift`, etc.)
   - `scan_body()` for each detection kind
   - `toml_edit` non-destructive merge scenarios

3. **Golden / snapshot tests** (`insta`): Fixed input fixtures → snapshot comparison. `cargo insta review` for snapshot updates. Location: `tests/fixtures/` (claude/ and codex/ inputs), `tests/snapshots/`.

4. **Roundtrip tests** (domain-grouped test files: `tests/cli.rs`, `tests/hooks.rs`, `tests/mcp.rs`, `tests/memory.rs`, `tests/plugins.rs`, `tests/settings.rs`, `tests/skills.rs`, `tests/subagents.rs`; shared helpers in `tests/common/mod.rs`): `c2x → x2c` IR diff must contain only known `lossy`/`dropped` differences. `lossless` entries must produce identical values.

   > **Degrade roundtrip special case (§18, item 4):** Degrade paths (e.g. session hooks → skill hooks) are structurally non-invertible. For degraded fields, test that `side_artifacts` are regenerated identically rather than requiring IR field equality.

5. **Tier roundtrip tests** (enforced before merging const updates):
   - `codex_tier(tier_to_codex(t)) == Some(t)` for all `Tier` variants.
   - `claude_tier(tier_to_claude(t)) == Some(t)` for all `Tier` variants.

6. **Property tests** (not yet implemented — `proptest` is not a dependency): Exhaustive verification of mappings invariants and roundtrip invariants is planned but not currently in the test suite.

---

## 19. Extensibility: Hub-and-Spoke / Standard-Core Layering

### IR Two-Layer Model

| Layer | Content | Examples |
|---|---|---|
| **Standard core** | Cross-tool minimum common denominator | `name`/`description` (agentskills.io), `AGENTS.md` (open standard, 30+ agents including GitHub Copilot, Gemini CLI, Aider, Cursor, Zed) |
| **Tool-specific extension** | Rich per-tool features | Claude: `allowed-tools`/`user-invocable`/`hooks`. Codex: `policy`/standalone agents TOML |

Standard core follows agentskills.io `name`/`description` as minimum. Rich features remain as tool-specific extensions and become `dropped` on cross-tool conversion. This avoids designing for the lowest common denominator.

### Hub-and-Spoke Architecture

Currently Claude ↔ Codex only, but the `Handler` trait and IR design support N-tool extension:

```
Cursor ──────┐
Claude ───────┼──▶  Standard IR (standard core + tool-specific extension fields)  ◀──── Gemini CLI
Codex ────────┤
Zed ──────────┘
              ↑
        Add handler → new tool supported without engine changes
```

**Extension principles:**

1. **New tool = new handler file** (`handlers/<tool>.rs`) + new mappings YAML. No engine modifications.
2. **`AGENTS.md` as memory hub:** It is the open standard memory format (Agentic AI Foundation, 30+ agent adoption including GitHub Copilot, Gemini CLI, Aider, Cursor, Zed). Claude's `CLAUDE.md` converts to `AGENTS.md`; new tools should target AGENTS.md.
3. **No forced standard convergence:** Tool-specific extension fields are preserved in the IR and appear as `dropped` on the other side. Standardization is emergent, not enforced.

---

## 20. Technology Stack

**Language:** Rust. Rationale: `toml_edit` provides non-destructive TOML merge (the only language/library combination that satisfies this requirement), single binary distribution is straightforward.

| Crate | Purpose |
|---|---|
| `toml_edit` (0.25) | config.toml read/write with comment+order preservation; `DocumentMut` API |
| `toml` (v1.1) | Typed TOML deserialization via `toml::from_str` (used alongside `toml_edit`) |
| `serde-saphyr` (0.0.27) | YAML read/write with key-order preservation (0.0.x — API unstable) |
| `gray_matter` (0.3) | Splits YAML frontmatter from Markdown body; `Matter::parse` returns `Result<ParsedEntity, Error>` |
| `serde_json` | JSON serialization; generic `Value` for IR |
| `serde` (1) | Derive macros for serialization |
| `clap` (4, derive) | CLI subcommands and flags |
| `regex` (1) + `once_cell` (1) | Body scanner regex patterns (statically initialized) |
| `anyhow` (1) | `anyhow::Result`, `bail!`, `Context` for unified error handling |
| `dialoguer` (0.12) | TTY interactive prompts for `--interactive` skill-target confirmation |
| `walkdir` (2) | Recursive directory traversal for directory-input mode |
| `shlex` (2) | Shell-quoting for args synthesis (exec-form `args` → shell-form `command`) |
| `insta` (1, dev) | Golden/snapshot test comparison |
| `tempfile` (3.27, dev) | Temp directories in integration tests |

**Codex-rs:** Not used. cxbridge defines its own TOML types locally; there is no `codex-rs`/`codex-config` crate dependency. `toml_edit::DocumentMut` handles all non-destructive `config.toml` writes.

**Distribution:** A custom version-driven GitHub Actions workflow (`.github/workflows/release.yml`) triggered by a `version` bump to `Cargo.toml` on `main`. Builds 5 targets: macOS aarch64 + x86_64, Linux gnu + musl, Windows msvc. Publishes to GitHub Releases; optionally pushes to crates.io (requires `CARGO_REGISTRY_TOKEN`); dispatches a Homebrew tap update (`rikeda71/homebrew-tap`, `cxbridge.rb`). `cargo dist` is **not** used.

**Known weaknesses:**
- `serde-saphyr` 0.0.27 — API is in the 0.0.x range and may have breaking changes in future releases.
- `toml_edit` reorders scattered `[[array-of-tables]]` to contiguous positions at parse time (values/comments preserved; emit a warning when this occurs).
- First-build compile time is long with many dependency crates (mitigated by `sccache`).

---

*Source references for all entries: see `source` and `notes` fields in `mappings/*.yaml`. Codex-side primary source URLs: `developers.openai.com/codex`, `github.com/openai/codex`. Claude-side: `code.claude.com/docs`, SchemaStore.*
