# mappings/*.yaml Schema Definition

`mappings/*.yaml` is a **machine-readable conversion table** loaded by the Claude Code ⇄ OpenAI Codex CLI bidirectional conversion engine. Human-oriented explanations live in `docs/`; this YAML captures the "field correspondence" subset in a machine-processable form.

## Top-Level Structure

```yaml
domain: skills                      # Domain identifier (matches the filename)
title: "Skills (SKILL.md)"          # Human-readable title
doc: ../docs/spec.md           # Corresponding documentation document (relative path)
files:                              # Configuration files covered by this domain
  claude:
    - ".claude/skills/<name>/SKILL.md"
    - "~/.claude/skills/<name>/SKILL.md"
  codex:
    - ".agents/skills/<name>/SKILL.md"
    - "~/.agents/skills/<name>/SKILL.md"
format:                            # File formats (list; a single domain may use multiple formats)
  claude: [markdown+yaml-frontmatter] # Write as a list even for a single format
  codex: [toml, json]                 # Example: Codex hooks use TOML or JSON
entries:                           # Array of field-correspondence entries (see below)
  - { ... }
notes:                             # Domain-wide annotations (optional)
  - "..."
```

## entries[] — Individual Entry

```yaml
- id: skills.allowed-tools          # Unique entry ID (domain.field)
  claude:                           # Claude-side counterpart (null if absent)
    field: "allowed-tools"          # Field name / key path (dot notation)
    type: "string|list"             # Type
    scope: skill                    # Scope where this setting takes effect
  codex:                            # Codex-side counterpart (null if absent)
    field: null                     # null if there is no direct counterpart
    type: null
    scope: null
  direction: claude_to_codex        # both | claude_to_codex | codex_to_claude
  loss: lossy                       # lossless | lossy | dropped
  degrade:                          # Scope-demotion info (only when demotion occurs)
    to: session                     # Demoted-to scope
    target: ".codex/rules/<skill>.rules (execpolicy allow)"  # Concrete write destination after demotion
  transform: null                   # Value transformation rule (transform vocabulary below; null if none)
  warn: true                        # Whether to emit a user warning during conversion
  notes: "skill-only pre-approval cannot be reproduced. Demoting to .rules allow makes it effective for the entire session"
  source: "https://..."             # Reference URL (optional)
```

## Field Vocabulary

### `scope` (Range where the setting is effective)
- `skill` / `command` / `agent` / `plugin` — only while that component is running
- `marketplace` — per marketplace manifest (marketplace.json)
- `session` — the entire running session
- `project` — per project (repository)
- `user` — per user (all projects)
- `profile` — per Codex named profile
- `subagent` — per Codex subagent (role/standalone TOML)
- `managed` — organization-enforced (managed settings / requirements.toml)
- `global` — tool-wide

### `direction` (Conversion direction)
- `both` — bidirectional conversion is possible
- `claude_to_codex` — meaningful only for Claude→Codex (not output for Codex→Claude / uses default)
- `codex_to_claude` — Codex→Claude only

### `loss` (Information-loss level)
- `lossless` — fully equivalent; value/format conversion only
- `lossy` — meaning is close but some information is lost / scope changes / values are rounded
- `dropped` — no counterpart; discarded (manual handling or warning only)

### `degrade` (Scope demotion)
Recorded when `loss: lossy` and the setting moves from a skill scope to a broader or different scope.
- `to`: demoted-to scope
- `target`: concrete setting at the demotion destination (file / key to write to)

### `transform` (Value transformation rules) — expressed as a string; parsed by the CLI implementation
Representative rules (intended to be implemented as functions in the CLI):
- `unit:ms_to_sec` / `unit:sec_to_ms` — unit conversion for timeouts, etc. (e.g., `60000`→`60.0`)
- `polarity:invert` — boolean polarity inversion (e.g., Claude `disabled:true` ⇔ Codex `enabled:false`)
- `enum_map:{a:b,...}` — enum value mapping (e.g., effort `max`→`xhigh`)
- `index_shift:+1` / `index_shift:-1` — argument index 0-based ⇔ 1-based shift (`$ARGUMENTS[0]`⇔`$1`)
- `str_to_list:space` / `list_to_str:space` — space-delimited string ⇔ array (OAuth scopes, etc.)
- `rename` — key name change only (e.g., `headers`⇔`http_headers`)
- `format:json_to_toml` / `format:toml_to_json` — serialization format conversion
- `extract:bearer_env` — extract environment variable name `VAR` from `"Bearer ${VAR}"` (MCP Bearer token)
- `path:remap` — path convention remapping (`.claude/`⇔`.agents/`, etc.)
- `inline_imports` — inline-expand `@import` references (CLAUDE.md→AGENTS.md)

Multiple rules are separated by `;` (e.g., `unit:ms_to_sec; rename`).

## Invariants the Conversion Engine Must Uphold
1. Entries with `loss: dropped` must always be enumerated in the conversion report.
2. Entries with `warn: true` must emit a user warning during conversion.
3. Entries with `degrade` must have the demoted-to scope (`to`) stated in the report.
4. Entries whose `direction` is one-way must be ignored (or have their default value restored) for the reverse direction.
5. The same `id` must be unique across all mappings.

## Notes
- This table represents "correspondence as documented in the current spec/schema." Items where actual binary behavior may differ are noted in `notes`.
- Codex-side specifications are fluid (new features in 2025-2026). Record version dependencies alongside `source` URLs in `notes`.
