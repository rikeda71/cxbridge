# Conversion Coverage Guide

This document gives a practical overview of what cxbridge converts cleanly, what
it reshapes with partial loss, and what it cannot carry across at all. It is
intended as a quick orientation for users and contributors.

**Canonical sources:**

- Field-level detail: [`mappings/*.yaml`](../mappings/)
- Full design and rationale: [`docs/spec.md §16`](spec.md#16-feature--loss-matrix-summary)

**Nothing is ever dropped silently.** Every `dropped` entry is enumerated in the
conversion report produced by `cxbridge c2x` / `cxbridge x2c`. Use `--report` for
the full list, or `--report=json` for machine-readable output.

---

## Loss Levels

| Symbol | Level | Meaning |
|--------|-------|---------|
| ◎ | **Lossless** | Fully equivalent; value or format conversion only. Roundtrip produces identical values. |
| ○ | **Lossy** | Meaning is close but some information is lost, scope changes, or values are rounded. |
| △ | **Degraded** | A sub-case of lossy: the setting moves to a broader scope (e.g. skill → session/project). The target scope is always named in the report. |
| ✕ | **Dropped** | No equivalent on the other side. The field is discarded and **always listed in the conversion report** — never silent. |

These symbols match the report output from `cxbridge --report`.

---

## Per-Domain Summary

Numbers are taken directly from `docs/spec.md §16` and confirmed against the
YAML source. They sum to 317.

| Domain | Total | ◎ Lossless | ○ Lossy | ✕ Dropped |
|--------|------:|----------:|--------:|----------:|
| Skills | 23 | 5 | 13 | 5 |
| Hooks | 83 | 34 | 6 | 43 |
| MCP | 32 | 10 | 4 | 18 |
| Plugins | 48 | 13 | 15 | 20 |
| Memory | 18 | 3 | 5 | 10 |
| Subagents | 25 | 4 | 10 | 11 |
| Settings/Config | 73 | 2 | 35 | 36 |
| Variables | 15 | 2 | 5 | 8 |
| **Total** | **317** | **73** | **93** | **151** |

**Directional note:** Codex → Claude conversion is generally lower-loss than
Claude → Codex. Codex has a smaller vocabulary; Claude has receptacles for most
concepts. The reverse direction loses heavily because Claude's richer skill-scope
controls, argument machinery, dynamic injection, and 20 additional hook events
have no Codex equivalent.

---

## Skills

**Files:** `.claude/skills/<name>/SKILL.md` ↔ `.agents/skills/<name>/SKILL.md`

◎ **Converts cleanly:** `name`, `description`, file path (directory remap),
invocation syntax (`/skill-name` ↔ `$skill-name`), `disable-model-invocation`
(polarity-inverted to `policy.allow_implicit_invocation`).

△ **Degrades:** `allowed-tools` and `disallowed-tools` expand from skill scope to
session/project scope — Bash patterns become `.codex/rules/<skill>.rules`
(Starlark `prefix_rule`), MCP tool names go to `enabled_tools`/`disabled_tools`.
`model`, `effort`, and `context:fork` degrade to a generated
`.codex/agents/<skill>.toml` subagent TOML (explicit `spawn_agent` invocation is
then required; automatic forking is lost). Skill-scoped `hooks` degrade to
session-scoped hooks in `config.toml`.

✕ **Dropped (notable examples):**
- `user-invocable` (`skills.user-invocable`) — "model-only, hidden from user" has no Codex concept.
- `paths` (`skills.paths`) — glob-triggered auto-invocation has no Codex equivalent.
- `argument-hint`, `arguments` (`skills.argument-hint`, `skills.arguments`) — Claude's named/positional argument machinery.

See [`mappings/skills.yaml`](../mappings/skills.yaml) for all 23 entries.

---

## Hooks

**Files:** `settings.json` / `hooks.json` ↔ `config.toml` / `hooks.json`

◎ **Converts cleanly:** 10 common events shared by both tools —
`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`,
`PostToolUse`, `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`,
`Stop` — plus core hook fields (`command`, `timeout`, `statusMessage`, `async`)
and most stdin/stdout JSON fields. Format conversion is JSON ↔ TOML
(`format:json_to_toml`).

○ **Lossy:** Matcher normalization — Claude exact strings like `"Bash"` become
anchored regexes `"^Bash$"` in Codex (which always evaluates matchers as regex).
`permissionDecision: "defer"` (Claude-only value) is dropped on c2x.

✕ **Dropped (notable examples):**
- 20 Claude-only events (`hooks.event.Setup`, `hooks.event.UserPromptExpansion`,
  `hooks.event.PermissionDenied`, `hooks.event.SessionEnd`, and 16 more) — all
  marked `awaiting-codex`.
- Hook types `http` and `mcp_tool` (`hooks.type.http`, `hooks.type.mcp_tool`) —
  no Codex execution engine.
- Hook types `prompt` and `agent` (`hooks.type.prompt`, `hooks.type.agent`) —
  Codex parses the schema but does not execute them.
- Command sub-fields `args`, `shell`, `if`, `once`, `asyncRewake`
  (`hooks.command.*`) — Codex does not support exec-form args, shell selection,
  or conditional/once semantics.

See [`mappings/hooks.yaml`](../mappings/hooks.yaml) for all 83 entries.

---

## MCP Servers

**Files:** `.mcp.json` ↔ `[mcp_servers.*]` in `config.toml`

◎ **Converts cleanly:** `command`, `args`, `env`, `cwd`, `url`,
`headers`/`http_headers` (rename), OAuth `client_id` (rename) and `scopes`
(string ↔ array via `str_to_list:space`), `timeout`/`tool_timeout_sec`
(ms ↔ seconds via `unit:ms_to_sec`).

○ **Lossy:** Transport type detection is implicit in Codex (presence of `command`
vs `url`) vs explicit `type` field in Claude. Bearer auth is extracted from
`headers.Authorization: "Bearer ${VAR}"` into `bearer_token_env_var`. OAuth
`callbackPort` is per-server in Claude but global (`mcp_oauth_callback_port`) in
Codex — a collision risk when multiple servers differ.

✕ **Dropped (notable examples):**
- `alwaysLoad` (`mcp.alwaysLoad`) — Claude lazy-loading control; no Codex equivalent.
- `headersHelper` (`mcp.headersHelper`) — dynamic header generation command.
- SSE (`mcp.transport_sse`) and WebSocket (`mcp.transport_ws`) transports.
- OAuth `authServerMetadataUrl` (`mcp.oauth.auth_server_metadata_url`).
- Codex-only fields on x2c: `enabled_tools`, `disabled_tools`,
  `default_tools_approval_mode`, `startup_timeout_sec`, `env_vars`,
  `environment_id`, `oauth_resource`.

**Note:** Codex entries with `enabled: false` are **excluded entirely** from
Claude output (Claude has no `enabled` field; presence equals enabled). The
excluded server names are listed in the conversion report.

See [`mappings/mcp.yaml`](../mappings/mcp.yaml) for all 32 entries.

---

## Plugins

**Files:** `.claude-plugin/plugin.json` + `marketplace.json` ↔
`.codex-plugin/plugin.json` + `.agents/plugins/marketplace.json`

The Plugins handler is the integration point: it delegates to the Skills, Hooks,
and MCP handlers recursively for sub-directories.

◎ **Converts cleanly:** Core metadata (`name`, `description`, `author`,
`homepage`, `repository`, `license`, `keywords`, `displayName`/`interface.displayName`
via rename), manifest path (directory remap), `commands/` path, marketplace `name`
and `category`.

○ **Lossy:** `version` (Claude optional; Codex requires strict semver — a
placeholder may be synthesized). `skills`, `mcpServers`, `hooks`, `agents` paths
(Codex manifest fields accept only a single path while Claude allows arrays).
`defaultEnabled` (approximated as `policy.installation: INSTALLED_BY_DEFAULT` in a
marketplace entry). Codex `interface.*` display fields (`websiteURL` → `homepage`,
`developerName` → `author.name`, `longDescription` ↔ `description`,
`defaultPrompt` prepended to skill body, `category` appended to `keywords`).

✕ **Dropped (notable examples, c2x):**
- `lspServers` (`plugins.lspServers`) — no LSP support in Codex.
- `outputStyles`, `experimental.themes`, `experimental.monitors`
  (`plugins.outputStyles`, `plugins.experimental.*`) — no counterpart.
- `userConfig` (`plugins.userConfig`) — Codex has no user-config prompt mechanism;
  unresolved `${user_config.KEY}` references will remain in MCP/hook configs.
- `channels` (`plugins.channels`), `settings` (`plugins.settings`),
  `dependencies` (`plugins.dependencies`).

**Dual manifest:** Use `--dual-manifest` to retain `.claude-plugin/` and emit
`.codex-plugin/plugin.json` alongside — required for native Codex recognition
(`.claude-plugin/plugin.json` is not read as a Codex manifest).

See [`mappings/plugins.yaml`](../mappings/plugins.yaml) for all 48 entries.

---

## Memory

**Files:** `CLAUDE.md` ↔ `AGENTS.md` (path remap; content carried through)

◎ **Converts cleanly:** File rename and path remap (`CLAUDE.md` →
`AGENTS.md`, `~/.claude/CLAUDE.md` → `~/.codex/AGENTS.md`). Content is
copied verbatim.

○ **Lossy:** `@import` expansion (`memory.import-syntax`) — Codex has no
`@import` mechanism; all imports are inlined into a single file. Post-expansion
size is checked: a warning is emitted if the result exceeds 28 KiB (Codex's
`project_doc_max_bytes` limit is 32 KiB). Subdirectory `CLAUDE.md` files deeper
than CWD (`memory.subdirectory-load`) may be invisible to Codex, which does not
scan below CWD. HTML comment stripping behavior (`memory.html-comments`) differs
between tools.

✕ **Dropped (notable examples):**
- `CLAUDE.local.md` (`memory.local-file`) — no uncommitted personal file concept in Codex.
- Managed policy `/etc/claude-code/CLAUDE.md` (`memory.managed-policy`) — no org-enforcement path.
- `.claude/rules/*.md` `paths:` frontmatter (`memory.rules-paths-frontmatter`) — path-conditional rule loading.
- Auto memory `MEMORY.md` (`memory.auto-memory`) — Claude's autonomous memory system.
- Codex-only x2c drops: `AGENTS.override.md` (`memory.override-file`),
  `project_doc_fallback_filenames` (`memory.fallback-filenames`),
  `features.child_agents_md` (`memory.child-agents-md-feature`).

See [`mappings/memory.yaml`](../mappings/memory.yaml) for all 18 entries.

---

## Subagents

**Files:** `.claude/agents/<name>.md` (Markdown + YAML frontmatter) ↔
`.codex/agents/<name>.toml` (TOML)

The structural divergence here is the most significant of any domain. **Claude
automatically delegates work via semantic matching of the agent description; Codex
requires an explicit `spawn_agent` tool call.** This behavioral difference cannot
be reproduced by conversion and is always stated in the conversion report
(`subagents.spawn-model`).

◎ **Converts cleanly:** File path and format (`.md` + frontmatter ↔ `.toml`),
`name`, `description`, body/`developer_instructions` (content lossless, format
converts).

○ **Lossy:** `model` (different providers; tier mapping applied),
`effort`/`model_reasoning_effort` (enum mapped; `max` rounds to `xhigh`),
`mcpServers`/`mcp_servers` (rename + format; named-reference form is hard to
express), `skills`/`skills.config` (semantics differ: Claude injects content,
Codex overrides enabled state), `permissionMode` → `sandbox_mode`
(approximate; `acceptEdits`/`auto`/`dontAsk` have no Codex counterpart),
`tools` → `sandbox_mode` (coarse approximation), `hooks` degrade from agent scope
to session scope, `memory` degrades from 3-scope to global boolean,
`initialPrompt` (appended to `developer_instructions`; auto-submit behavior lost),
`subagents.plugin-restrictions` (hooks/mcpServers/permissionMode are silently
ignored for Claude plugin agents).

✕ **Dropped (notable examples):**
- `disallowedTools` (`subagents.disallowedTools`) — no per-tool denylist in Codex.
- `maxTurns` (`subagents.maxTurns`) — turn-count limit; Codex has only
  time-based `job_max_runtime_seconds`.
- `background` (`subagents.background`) — persistent background agent mode.
- `isolation: worktree` (`subagents.isolation`) — git worktree isolation.
- `color` (`subagents.color`) — UI decoration.
- Codex-only x2c drops: `nickname_candidates`, `config_file`,
  `agents.max_threads`/`max_depth`/`job_max_runtime_seconds`.

See [`mappings/subagents.yaml`](../mappings/subagents.yaml) for all 25 entries.

---

## Settings / Config

**Files:** `settings.json` ↔ `config.toml`

Full automatic conversion is not attempted. The permission model axis mismatch
(Claude: tool-axis; Codex: resource-axis) makes complete machine translation
infeasible. **Only 2 of 73 entries (3%) are lossless.**

◎ **Converts cleanly:** `editorMode`/`tui.vim_mode_default` (enum → boolean
rename), `sandbox.network.allowAllUnixSockets`/`features.network_proxy.dangerously_allow_all_unix_sockets`
(rename).

△ **Degrades (lossy with scope change):**
- `permissions.allow(Bash(...))` → `.codex/rules/default.rules` Starlark
  `prefix_rule(decision="allow")` — expands to project scope
  (`settings.permissions.allow.bash`).
- `permissions.deny(Bash(...))` → `.rules` with `decision="forbidden"`
  (`settings.permissions.deny.bash`).
- `permissions.allow(Read/Write/Edit)` → `[permissions.<n>].filesystem`
  — tool-axis to resource-axis; Read/Write boundary lost
  (`settings.permissions.allow.read`, `settings.permissions.allow.write`).
- `permissions.allow(WebFetch)` → `[permissions.<n>].network.domains`
  (`settings.permissions.allow.webfetch`).
- Codex `developer_instructions` → appended to `CLAUDE.md`
  (`settings.codex.developer_instructions`).

○ **Lossy (notable converted subset):** `model` (tier mapping; different provider),
`effortLevel`/`model_reasoning_effort` (`max` rounds to `xhigh`),
`autoMemoryEnabled` → `memories.use_memories + memories.generate_memories`,
`cleanupPeriodDays` → `memories.max_rollout_age_days` (clamped to 0–90),
`attribution.commit`/`commit_attribution` (rename), `defaultMode` values (c2x):
`default`/`acceptEdits`/`auto` → `approval_policy="on-request" + sandbox_mode="workspace-write"`;
`plan` → `approval_policy="on-request" + sandbox_mode="read-only"`;
`dontAsk` → `approval_policy="never" + sandbox_mode="workspace-write"`;
`bypassPermissions` → `approval_policy="never" + sandbox_mode="danger-full-access"`.
Reverse (x2c): `sandbox_mode` × `approval_policy` jointly collapse to the nearest `defaultMode`
(`read-only` → `plan`; `danger-full-access` → `bypassPermissions`;
`workspace-write+never` → `dontAsk`; `workspace-write+other` → `default`). Both directions are lossy.

✕ **Dropped (notable examples, c2x):**
- `viewMode`, `worktree`, `autoUpdatesChannel`, `spinnerTips*`, `voice`,
  `maxSkillDescriptionChars`, `wheelScrollAccelerationEnabled` — Claude-only UI/behavior controls.
- `apiKeyHelper` (`settings.apiKeyHelper`) — shell-command-based API key helper.
- `alwaysThinkingEnabled`, `disableBypassPermissionsMode`.
- `fallbackModel` (`settings.fallbackModel`) — overload fallback chain; no Codex counterpart.
- `availableModels` / `enforceAvailableModels` (`settings.availableModels`) — managed model allowlist.
- `disableBundledSkills`, `requiredMinimumVersion` / `requiredMaximumVersion`.
- `agent` (`settings.agent`) — run the main thread as a named subagent; Codex agents are
  explicit `spawn_agent` targets only.

✕ **Dropped (notable examples, x2c):** `plan_mode_reasoning_effort`,
`approvals_reviewer` / `auto_review.policy`, `projects.<path>.trust_level`,
`tui.theme` / `tui.status_line` / `tui.terminal_title`.

See [`mappings/settings-config.yaml`](../mappings/settings-config.yaml) for all 73 entries.

---

## Variables

**Scope:** Skill body text in `.claude/skills/*/SKILL.md` and
`.agents/skills/*/SKILL.md`; handled by the body scanner within the Skills
handler. There is no standalone Variables handler.

◎ **Converts cleanly:** `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}`
(`variables.plugin-root`, `variables.plugin-data`) — lossless **only inside
plugin-sourced hook commands** (Codex sets compatible aliases `CLAUDE_PLUGIN_ROOT`
and `CLAUDE_PLUGIN_DATA`); these are treated as unsupported in general skill
bodies.

○ **Lossy:** `$ARGUMENTS` (bare, without index) — valid in Claude skill bodies;
likely unsupported in Codex skill bodies (`variables.arguments-all`). Indexed
arguments `$ARGUMENTS[N]` ↔ `$N+1` (0-indexed vs 1-indexed shift —
`variables.arguments-indexed`). Named arguments `$name` ↔ `$UPPERCASE_NAME`
(`variables.named`). Invocation syntax `/skill-name` ↔ `$skill-name`
(`variables.invocation-slash`; automatic rewrite is risky due to false positives,
so detect-and-propose only). `$$` Codex escape → `$` Claude (`variables.dollar-escape`).

✕ **Dropped (notable examples):**
- `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}`,
  `${CLAUDE_PROJECT_DIR}` (`variables.session-id`, `variables.effort-var`,
  `variables.skill-dir`, `variables.project-dir`) — no Codex equivalents.
- `` !`cmd` `` inline dynamic shell injection and ` ```! ` block injection
  (`variables.inline-injection`, `variables.block-injection`) — Codex explicitly
  declined implementation (issue #5019, "not planned"). Leaving these unconverted
  causes silent misbehavior; the body scanner flags them for manual handling.
- `/plugin:skill` namespaced invocation (`variables.invocation-namespaced`) —
  no namespace concept in Codex.

See [`mappings/variables.yaml`](../mappings/variables.yaml) for all 15 entries.

---

## Further Reading

- **Field-level detail:** [`mappings/<domain>.yaml`](../mappings/) — every entry
  has `id`, `loss`, `transform`, `degrade`, and `notes` fields with source URLs.
- **Full design and spec:** [`docs/spec.md`](spec.md) — architecture, IR model,
  transform registry, degrade engine, CLI flags, exit codes, and all domain
  handler contracts.
- **Mappings schema:** [`mappings/SCHEMA.md`](../mappings/SCHEMA.md) — field
  vocabulary and invariants.
