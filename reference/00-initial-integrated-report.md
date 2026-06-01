# Claude Code vs. Codex SKILL / PLUGIN Configuration Comparison Report

> Purpose: A detailed analysis of configuration-value-level correspondences, differences, and information-loss points to support building a future "Claude Code ⇄ OpenAI Codex CLI" configuration (Skills / Plugins / surrounding mechanisms) bidirectional conversion CLI.
>
> Created: 2026-05-30 / Scope: Claude Code (`code.claude.com/docs`), OpenAI Codex CLI (`developers.openai.com/codex`, `github.com/openai/codex`)
>
> Note: Codex's Skills / Plugins / Hooks are relatively new features added in the second half of 2025 through early 2026, and the specification is still evolving (see §7). This report is grounded in official documentation, schemas, and real repositories, but always re-verify against the specific version when implementing the CLI.

---

## 0. Executive Summary

| Dimension | Conclusion |
|---|---|
| **Conceptual mapping** | Skills, Plugins, Hooks, MCP, memory files (CLAUDE.md / AGENTS.md), and marketplace — **6 major concepts exist in both tools with near 1:1 correspondence**. Codex shows strong signs of following Anthropic's design; bidirectional conversion is structurally feasible. |
| **Biggest obstacle** | **The gap in SKILL.md frontmatter expressiveness and the absence of a "skill scope" concept**. Claude has 16 fields; Codex skills have only 2 (`name`/`description`). The missing fields are distributed across other Codex mechanisms (`agents/openai.yaml`, `.rules`, subagent `config_file`, hooks), and most can be approximated by **"degrading" skill scope to session/subagent** (§2.6). However, `user-invocable`, `paths` auto-trigger, built-in tool prohibition, and the argument mechanism are **confirmed losses**. |
| **Easier conversion direction** | **Codex → Claude Code**. Codex's minimal frontmatter has a receptacle on the Claude side for everything, so conversion is nearly lossless. |
| **Harder conversion direction** | **Claude Code → Codex**. `allowed-tools`/`model`/`effort`/`context:fork`/`hooks` cannot be written in a skill, but can be approximated by **degrading to `.rules`, subagent `config_file`, or session hooks** (§2.6). `user-invocable`, `paths` auto-trigger, the argument mechanism, and built-in tool prohibition are **discarded**. |
| **Config file formats** | Claude Code = **JSON** (`settings.json`, `plugin.json`, `.mcp.json`); Codex = **TOML** (`config.toml`) + JSON (`plugin.json`, `marketplace.json`). Format conversion is always required. |
| **Tailwind** | Codex's `marketplace.json` **reads `.claude-plugin/marketplace.json` as a compatible path**. MCP STDIO keys (`command`/`args`/`env`) are shared between both. The hierarchical merge philosophy for memory files is also identical. |

---

## 1. Overall Mapping (Concept Correspondence Table)

| Concept | Claude Code | OpenAI Codex CLI | Correspondence |
|---|---|---|---|
| Reusable instruction package | **Skills** (`SKILL.md`) | **Skills** (`SKILL.md`) | ◎ Even filename is identical |
| Local skill placement | `.claude/skills/<name>/` | `.agents/skills/<name>/` | ○ Path difference only |
| Global skill placement | `~/.claude/skills/<name>/` | `~/.agents/skills/<name>/` | ○ Path difference only |
| Distributable extension bundle | **Plugins** (`.claude-plugin/plugin.json`) | **Plugins** (`.codex-plugin/plugin.json`) | ◎ Structure nearly identical |
| Distribution catalog | `marketplace.json` | `marketplace.json` (with `.claude-plugin/` compatible read) | ◎ |
| Project instruction memory | `CLAUDE.md` | `AGENTS.md` (open standard) | ○ Same philosophy, different name |
| Explicit override | (CLAUDE.md hierarchy priority) | `AGENTS.override.md` | △ Codex has dedicated file |
| Slash commands (legacy) | `.claude/commands/*.md` | `~/.codex/prompts/*.md` (**deprecated**) | △ |
| Lifecycle hooks | **Hooks** (enabled by default, 30+ events) | **Hooks** (enabled by default, 10 events; `features.hooks = false` to disable) | ○ Claude is broader |
| MCP servers | `.mcp.json` (JSON) | `[mcp_servers.*]` (TOML) | ○ STDIO keys shared |
| Subagents | **Agents** (`agents/*.md`) | `[agents.*]` (config.toml) + `agents/openai.yaml` | △ Different design |
| Core settings | `settings.json` (JSON) | `config.toml` (TOML) | △ Format and granularity differ significantly |
| Permissions/sandbox | `permissions` (settings.json) | `approval_policy` + `sandbox_mode` + `[permissions.*]` | △ Codex is more fine-grained |
| Enterprise enforcement | managed settings | `requirements.toml` | ○ |

Legend: ◎ Nearly lossless conversion / ○ Meaning preserved with format conversion / △ Large design differences; partial or requires manual work

---

## 2. Skills Detailed Comparison

### 2.1 Directory Structure and Scope

**Claude Code**

```text
~/.claude/skills/<name>/SKILL.md         # personal (all projects)
.claude/skills/<name>/SKILL.md           # project
<plugin-root>/skills/<name>/SKILL.md     # within plugin (namespace plugin:name)
<plugin-root>/SKILL.md                   # single skill at plugin root
.claude/commands/<name>.md               # legacy command (backward compatible)
```
- Priority: Enterprise > Personal > Project
- Auxiliary files: `scripts/`, `references/`, arbitrary `.md`, `assets/` (convention, not mandatory structure)
- Auto-discovers from launch directory up to repository root + on-demand subdirectory discovery

**Codex**

```
$CWD/.agents/skills/<name>/SKILL.md      # repo (working directory)
$REPO_ROOT/.agents/skills/<name>/SKILL.md# repo root shared
~/.agents/skills/<name>/SKILL.md         # user
/etc/codex/skills/<name>/SKILL.md        # admin
(Codex bundled)                          # system / bundled
<plugin-root>/skills/<name>/SKILL.md     # within plugin
```
- Priority (highest first): REPO > USER > ADMIN > SYSTEM
- Auxiliary files: `scripts/`, `references/`, `assets/`, **`agents/openai.yaml`** (Codex-specific UI/policy configuration)
- Symbolic link following supported

> **Conversion note**: Directory structure and auxiliary file conventions are nearly identical. **Conversion is essentially "path remapping: `.claude/skills/` ⇄ `.agents/skills/`"**. However, Codex's `agents/openai.yaml` has no receptacle in Claude Code (see §5.1).

### 2.2 SKILL.md Frontmatter Field Correspondence (Core of This Report)

Both sides share the same structure of YAML frontmatter + Markdown body in SKILL.md. However, the frontmatter vocabulary differs decisively.

| Claude Code field | Type | Codex counterpart | Convertible? | Notes |
|---|---|---|---|---|
| `name` | string (≤64, lowercase alphanumeric-hyphen, "claude"/"anthropic" reserved) | `name` (required) | ◎ Bidirectional | Codex naming constraints appear looser; Claude-side requires lowercase and reserved-word avoidance |
| `description` | string (≤1024) | `description` (required) | ◎ Bidirectional | Both serve as auto-trigger; semantically fully equivalent |
| `when_to_use` | string | (included in `description`) | ○ → Codex: concatenate to description | No standalone field in Codex; merge into description |
| `argument-hint` | string | (only in Custom Prompts `argument-hint`; absent from Skills) | △ | Lost in skill-to-skill conversion; preserved in prompt conversion |
| `arguments` | string/list (`$name` named positional args) | (absent from Skills; Custom Prompts use `$1`–`$9`/`$ARGUMENTS`) | △ | Lost in skill→skill; `$name` substitutions also unresolved on Codex side |
| `disable-model-invocation` | boolean | `agents/openai.yaml` `policy.allow_implicit_invocation` (inverted) | **△ nearly equivalent** (§2.5a) | Equivalent up to implicit-trigger disable + description exclusion; behavioral difference in explicit invocations (Codex is cleaner) |
| `user-invocable` | boolean | **No corresponding field** (`policy` has only `allow_implicit_invocation`/`products`; confirmed in source) | **✕ cannot reproduce** (§2.5b) | "Model-only, hidden from user" concept does not exist in Codex |
| `allowed-tools` | string/list (pre-approve, wildcards allowed) | **No skill-scope equivalent**; MCP tools only can be degraded to `approval_mode=auto` | **✕ not at skill scope / △ session degrade** (§2.5c) | `SkillConfig` has only `enabled`/`name`/`path`; degrade expands to full session scope (warning required) |
| `disallowed-tools` | string/list | **No skill-scope equivalent**; MCP only can degrade to `disabled_tools` | **✕ not at skill scope / △ session degrade** (§2.5c) | Prohibition of built-in tools (`AskUserQuestion` etc.) is a complete loss |
| `model` | string (`/model` value, `inherit`) | **subagent (agent TOML) / profile `model`** | △ subagent/profile degrade (§2.6B) | Impossible at skill scope but fully substitutable at subagent scope |
| `effort` | enum(low/medium/high/xhigh/max) | **`model_reasoning_effort` (subagent/profile)** | △ degrade (§2.6B) | `max` rounded to Codex maximum `xhigh` (Codex has no `max`) |
| `context: fork` | enum(fork) | **standalone agent TOML + `spawn_agent`** | △ partial (§2.6B) | No auto-fork; requires explicit spawn; needs `features.multi_agent=true`; default `max_depth` is 1 |
| `agent` | string (subagent type when forking) | **subagent name (agent TOML / `[agents.*]`)** | △ (§2.6B) | Maps to subagent |
| `hooks` | object (skill-scoped hooks) | **Codex hooks (session/project scope)** | △ scope degrade (§2.6C) | Codex hooks are not limited to skill scope; enabled by default (`features.hooks = false` to disable) |
| `paths` | string/list (glob auto-trigger conditions) | **No equivalent** (AGENTS.md dirname hierarchy placement is the closest) | ✕ no equivalent (§2.6C) | File-operation-event-driven auto-trigger does not exist in Codex |
| `shell` | enum(bash/powershell) | hooks `commandWindows` (Windows override) | △ partial (§2.6C) | Not a shell selection mechanism per se |
| `${CLAUDE_SKILL_DIR}` etc. | — | Codex side has equivalent variables with different names | △ | Variable substitutions in body require mapping |

| Codex-only (SKILL.md / auxiliary files) | Location | Claude Code counterpart | Convertible? |
|---|---|---|---|
| `interface.display_name` | `agents/openai.yaml` | (Skill-level display name concept is weak; closest is plugin `displayName`) | △ |
| `interface.short_description` | `agents/openai.yaml` | `description` (partial) | △ |
| `interface.icon_small/large` | `agents/openai.yaml` | (No icon at skill level; available at plugin level) | ✕ |
| `interface.brand_color` | `agents/openai.yaml` | (None) | ✕ |
| `interface.default_prompt` | `agents/openai.yaml` | (Preamble prompt when `$skill` is invoked; can be approximated by prepending to body in Claude) | △ |
| `policy.allow_implicit_invocation` | `agents/openai.yaml` | `disable-model-invocation` (inverted) | ○ |
| `dependencies.tools` (MCP dependency) | `agents/openai.yaml` | (Weak direct support at skill level; plugin's `mcpServers` is closer) | △ |

> **Conclusion (Skills conversion)**:
> - **Codex → Claude Code**: `name` / `description` transfer directly; `agents/openai.yaml`'s `policy` and `interface` can be incorporated into Claude frontmatter (`disable-model-invocation` etc.) and body for **near-lossless** conversion.
> - **Claude Code → Codex**: `name`/`description`/`when_to_use` (concatenated)/`disable-model-invocation` (→openai.yaml) are safe. **`allowed-tools`, `model`, `effort`, `context:fork`, `agent`, `paths`, `arguments`, `hooks` have no equivalent placement in Codex's skill mechanism; loss or approximation via inline text in the body is required**.

### 2.3 Body and Auxiliary Files

| Item | Claude Code | Codex |
|---|---|---|
| Recommended body line limit | 500 lines | (No explicit limit confirmed; controlled by context budget) |
| Auxiliary file reference depth | 1 level recommended | (Same convention assumed) |
| Bundled scripts | `scripts/` (not loaded into context; output only) | `scripts/` (same) |
| Dynamic injection | `` !`cmd` `` / `` ```! `` block embeds shell execution results | (Equivalent mechanism unconfirmed) |
| Variable substitution | `$ARGUMENTS`, `$N`, `$name`, `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}` | (Custom Prompts: `$1`–`$9`/`$ARGUMENTS`/`$UPPER`/`$$`; skill body variables require verification) |
| Skill list budget | ~1% of context (configurable) | ~2% of context or ~8000 characters |

> **Conversion note**: Body Markdown can be ported nearly as-is. However, **Claude-specific dynamic injection `` !`cmd` `` and `${CLAUDE_*}` variables are not expanded on the Codex side**, so they must be detected and warned at conversion time (risk of silent misbehavior if left as literal strings).

### 2.4 Trigger and Invocation Mechanisms

| Item | Claude Code | Codex |
|---|---|---|
| Automatic trigger | Semantic match on `description` (+`when_to_use`) | Semantic match on `description` |
| Explicit invocation | **`/skill-name`** (slash) | **`$skill-name`** (dollar) or `/skills` menu |
| Suppressing auto-trigger | `disable-model-invocation: true` | `policy.allow_implicit_invocation: false` (openai.yaml) |
| Disabling (config side) | `skillOverrides` (settings.json) | `[[skills.config]]` `enabled=false` (config.toml) |
| Plugin namespace | `plugin:skill` | `plugin:skill` (same) |

> **Conversion note**: When a user writes "run `/foo`" in the body or README, **Codex requires `$foo`** (invocation symbol differs: slash ⇄ dollar). Invocation notation in body text is also subject to conversion.

### 2.5 [Deep-Dive Verification] Reproducibility in Codex of Three Skill Behavior Control Fields (Confirmed at Source Level)

`user-invocable` / `disable-model-invocation` / `allowed-tools` (+`disallowed-tools`) are central to skill design and determine the success of the interop CLI. Their reproducibility has been confirmed by examining Codex's **implementation source** (`codex-rs/core-skills/` in Rust), **JSON schema** (`codex-rs/core/config.schema.json`), and **official skill authoring guide** (`openai/codex` and `openai/skills` references: `references/openai_yaml.md`, `validate_plugin.py`). Two independent verifications reached the same primary source and reached identical conclusions.

#### Conclusion Summary

| Claude Code field | Reproduction in Codex | One-line conclusion |
|---|---|---|
| `disable-model-invocation: true` | **△ nearly equivalent** (1 behavioral difference) | Reproducible via `agents/openai.yaml` `policy.allow_implicit_invocation: false` |
| `user-invocable: false` | **✕ does not exist at all** | "Model-only, hidden from user" concept does not exist in Codex (confirmed in source) |
| `allowed-tools` | **✕ not at skill scope / △ degrade fallback available** (§2.6A) | No per-skill field, but session-level pre-approve is possible via user/project `.rules` `allow` |
| `disallowed-tools` | **✕ not at skill scope / △ degrade fallback available** (§2.6A) | Session-level prohibition is possible via execpolicy `forbidden` / MCP `disabled_tools`; built-in tool prohibition is not possible |

#### (a) `disable-model-invocation` → `policy.allow_implicit_invocation: false` [△ nearly equivalent; 1 behavioral difference]

- **Can be reproduced**. Writing `policy.allow_implicit_invocation: false` in the Codex skill auxiliary file `agents/openai.yaml` prohibits implicit (automatic) model invocation, allowing only explicit `$skill-name` invocation by the user.
- Evidence (implementation): `build_available_skills()` in `codex-rs/core-skills/src/render.rs` filters via `allowed_skills_for_implicit_invocation()` and **excludes the description entirely from the "## Skills" block shown to the model** for skills with `false`. Explicit invocations remain functional because `collect_explicit_skill_mentions()` in `codex-rs/core/src/session/turn.rs` targets **all skills** (`skills_outcome.skills`).
- Evidence (official): `openai_yaml.md` states: "When false, the skill is not injected into the model context by default, but can still be invoked explicitly via `$skill`. Defaults to true."
- **Equivalent aspects**: Suppression of implicit invocation + exclusion of description from model context.
- **Non-equivalent aspect (important)**: In Claude Code, setting `disable-model-invocation: true` makes the description disappear completely, potentially causing **the model to fail to recognize the skill's existence and fail routing** even when the user types `/skill-name` explicitly (known issue: `openai/codex-plugin-cc#211`; one flag conflates "suppress implicit invocation" with "break explicit routing"). Codex separates these two concerns in its implementation, so explicit invocations work normally. **Codex's design is cleaner in this respect**.
- Conversion policy: Claude→Codex conversion is feasible. However, the difference that "explicit invocations work more reliably on the Codex side" should be noted.

#### (b) `user-invocable` → No corresponding field [✕ does not exist at all]

- **Cannot be reproduced**. The only officially recognized fields in the `policy` section of `agents/openai.yaml` are **`allow_implicit_invocation` and `products`**. This is confirmed at source level via the `validate_plugin.py` validation implementation: `reject_skill_agent_unknown_fields(policy, {"allow_implicit_invocation"}, ...)` (unknown fields are rejected).
- The very **concept of one-sided control** — "hidden from user / not callable by user, but model can auto-invoke" — **does not exist in Codex skill design**. `config.toml`'s `skills.config[].enabled = false` simply **disables the skill entirely**; it cannot block only one side.
- Conversion policy: Claude→Codex is **not convertible**. The CLI must discard this field and explicitly warn: "**In Codex, users will be able to invoke this skill via `$name`** (cannot be made model-only)".

#### (c) `allowed-tools` / `disallowed-tools` → Not reproducible at skill scope [✕ (session degrade only △)]

- **Cannot be reproduced at skill scope**. Codex's `SkillConfig` (`[[skills.config]]` in config.toml) has only **3 fields** — `enabled` / `name` / `path` — with `additionalProperties: false` (confirmed in `config.schema.json`). The `SKILL.md` frontmatter also has only `name` / `description`, and the official guide explicitly states: **"Do not include any other fields in YAML frontmatter."** **There are absolutely no fields to allow/deny/pre-approve tools at skill scope**.
- `dependencies.tools` in `agents/openai.yaml` is a **declaration of MCP dependencies** (for detecting missing dependencies and auto-installing them; controlled by `features.skill_mcp_dependency_install`), **not approval control for tool invocations**.
- Misleading neighbor: `approval_policy.granular.skill_approval` (config.toml) is a **global** setting for "whether to show an approval prompt when a skill script runs." ① It does not distinguish between skills ② It is the opposite direction of `allowed-tools` (requesting approval rather than skipping it). It is not a substitute for `allowed-tools`.
- **Limits of approximation (degrade)**: For MCP tools, `mcp_servers.<id>.tools.<name>.approval_mode = "auto"` (pre-approve) / `disabled_tools` (prohibition) can functionally approximate, but **both are fixed at full-session scope**, losing the essential "only during skill execution" quality (= security property changes). **Prohibition of Codex built-in tools (`AskUserQuestion` etc.) has no alternative mechanism and is a complete loss**.
- Note (bug): The `[[skills.config]]` override mechanism itself that governs per-skill enable/disable has a known bug where "overrides don't take effect in either direction" (`openai/codex#14161`, open as of 2026-03). Per-skill control is fundamentally unstable at present.
- Conversion policy: Claude→Codex involves **information loss**. MCP tools only can be "degraded" to session configuration (scope expansion **warning required**). Everything else (built-in tools, dynamic scopes like `Bash(git add *)` argument patterns) is discarded.

> **Summary of the 3 fields**: Of Claude Code's **dynamic, skill-scoped control** — "who can invoke (user/model)" and "what is permitted only during execution" — only (a) is equivalent at the Codex skill level. (b) `user-invocable` does not exist as a specification and no substitute is feasible (confirmed loss). (c) `allowed-tools`/`disallowed-tools` **do not exist at skill scope, but degrading to session / subagent scope allows significant functional substitution via surrounding mechanisms** (execpolicy `rules`, subagent `config_file`) (detailed in §2.6). The interop CLI should treat (b) as a confirmed loss and (c) as "partial substitution with scope degrade".

### 2.6 [Deep-Dive Verification 2] "Degrade Fallback" via `permissions` / `rules` / Subagent

§2.5 confirmed that skill-scope equivalence is impossible. However, by **giving up on skill scope and degrading to session / project / subagent scope**, Codex's surrounding mechanisms can substantially approximate `allowed-tools` etc. This is the practical conversion strategy for the interop CLI, so the confirmed fallback paths, quality, and limits are summarized below.

> **Important correction**: §2.5(c) stated "Codex rules only support `prompt`/`forbidden`", but this was a constraint **specific to the managed layer (`requirements.toml`)**. **User/project layer `.rules` files support the execpolicy `allow` decision (= skip approval = pre-approve)**, confirmed in `codex-rs/execpolicy/src/decision.rs` ("Command may run without further approval."). This enables session-level reproduction of `allowed-tools`.

#### (A) `allowed-tools` / `disallowed-tools` → execpolicy `rules` + MCP tool control [partial substitute at session/project scope]

Codex command approval is determined by a multi-stage process: "execpolicy (`allow`/`prompt`/`forbidden`) → `approval_policy` (never/on-request/untrusted/granular) → `sandbox_mode` → `permissions` (filesystem/network)". `allow` corresponds to pre-approve.

| Claude specification | Codex substitute | Scope | Substitute quality |
|---|---|---|---|
| `allowed-tools: Bash(git add *)` | `prefix_rule(pattern=["git","add"], decision="allow")` to `~/.codex/rules/default.rules` (user) or `.codex/rules/*.rules` (project, requires `trust_level="trusted"`) | Full session | **Medium** (skill scope lost) |
| `allowed-tools: Bash(git *)` (wildcard) | `prefix_rule(["git"], "allow")` (prefix match: "allow everything after git") | session | **Medium** (Codex supports prefix match only) |
| `allowed-tools: Bash(*)` (allow all) | `approval_policy="never"` or `sandbox_mode="danger-full-access"` | session | **Low** (indiscriminate allow) |
| `allowed-tools: <MCP tool>` | `[mcp_servers.X] enabled_tools=[...]` | user/project | **High** |
| `disallowed-tools: Bash(rm -rf *)` | `prefix_rule(["rm","-rf"], "forbidden")` | user/project/managed | **High** (most faithful) |
| `disallowed-tools: <MCP tool>` | `[mcp_servers.X] disabled_tools=[...]` | user/project | **High** |
| `disallowed-tools: AskUserQuestion` (built-in) | **No alternative** (undocumented official API) | — | **Not possible** |

- **Axis mismatch in `permissions` profiles**: Codex `[permissions.<name>]` is **resource-axis** (filesystem path → read/write/deny; network domain → allow/deny). Claude `allowed-tools` is **tool-axis** (command + arguments). `network.domains["evil.com"]="deny"` or `filesystem["~/.ssh"]="deny"` can partially supplement `disallowed-tools`, but this does not align with "whether a tool can be executed". Note also that `permissions` and `sandbox_mode` are **mutually exclusive**.
- **Conclusion**: Command-type (Bash) and MCP-type tools can be practically substituted at session/project scope. **Only prohibition of built-in tools (`AskUserQuestion` etc.) has no substitute**.

#### (B) `model` / `effort` / `context:fork` → subagent + `config_file` [substitute at subagent scope; no auto-fork]

Codex has **2 lineages** of per-subagent configuration, both implemented (Issue #11701 completed):
- **Lineage A**: `config.toml`'s `[agents.<name>]` with `config_file` (path to role-specific config layer) + `description` + `nickname_candidates`.
- **Lineage B**: `~/.codex/agents/<name>.toml` (standalone). Contains `name`/`description`/`developer_instructions` + **any `config.toml`-compatible keys** (`model`, `model_reasoning_effort`, `sandbox_mode`, `approval_policy`, `mcp_servers`, `skills.config` ...).

→ **Mapping one Claude skill to one Codex subagent** allows bundling the following as substitutes:

| Claude field | Codex substitute | Quality |
|---|---|---|
| `model` | agent TOML `model` | **Complete** |
| `effort` (low–xhigh) | `model_reasoning_effort` | **Complete** |
| `effort: max` | `model_reasoning_effort="xhigh"` | **Approximation** (Codex has no `max`; `xhigh` is maximum) |
| `context:fork` + `agent` | standalone agent TOML + explicit `spawn_agent` call by model (requires `features.multi_agent=true`, default `max_depth` 1) | **Partial** |
| `allowed-tools` (via B) | agent TOML `sandbox_mode`/`approval_policy`/`mcp_servers` | **Partial** (can be limited to while that subagent runs) |
| `when_to_use` | agent TOML `description` | **Complete** |

- **Fundamental limitation**: Codex subagents **do not auto-fork**. As the official description states: "Codex only spawns a new agent when you explicitly ask it to do so." The trigger mechanism differs qualitatively from Claude's "skill fires → auto-fork".
- However, the advantage of the subagent path, unlike the session-wide degrade in (A), is that **tool permissions/model can be limited to "only while that subagent is running"**. This creates the closest pseudo-scope to skill scope.

#### (C) `paths` / `arguments` / `hooks` / `shell` substitutes

| Claude field | Codex substitute | Scope | Quality |
|---|---|---|---|
| `paths` (glob auto-trigger) | **No equivalent**. Closest is AGENTS.md directory hierarchy scoping (place AGENTS.md in glob's dirname). `child_agents_md` is hierarchical guidance and is not a substitute | directory (cwd-dependent) | **Not possible** (not event-driven by file operations) |
| `arguments` / `argument-hint` | Custom Prompts `$1`-`$9`/`$ARGUMENTS`/`argument-hint` (**deprecated**); no argument mechanism in skill body | prompt | **Form only / discard** |
| `hooks` (skill scope) | Codex hooks (enabled by default, `[[hooks.*]]`). **Session/project scope; not limited to skill scope** | session/project | **Partial** (scope degrade; warning required) |
| `shell` (bash/powershell) | hooks `commandWindows` (Windows command override; not a shell selection mechanism) | hook handler | **Partial** |

#### Summary of Scope Degrade

Since Claude's skill scope does not exist in Codex, all substitutions **degrade to a broader or different scope**:
- Tool permissions → **session/project** (`.rules`) or **subagent** (`config_file` sandbox/approval)
- model/effort → **subagent** (agent TOML) or **profile**
- hooks → **session/project**

The fundamental cost of degrade is losing the dynamic, automatic limitation of "only during skill execution". Session degrade affects the entire session; subagent degrade requires explicit `spawn_agent` invocation. **The CLI must explicitly record in the loss report which field was degraded to which scope (session/project/subagent)** (§5, §6).

---

## 3. Plugins Detailed Comparison

### 3.1 Directory Structure and Manifest

**Claude Code**

```
my-plugin/
├── .claude-plugin/plugin.json   # manifest (only plugin.json goes here)
├── skills/<name>/SKILL.md
├── commands/<name>.md           # legacy
├── agents/<name>.md
├── hooks/hooks.json
├── .mcp.json
├── .lsp.json
├── output-styles/ themes/ monitors/ bin/ scripts/
└── settings.json
```

**Codex**

```
my-plugin/
├── .codex-plugin/plugin.json    # manifest
├── skills/<name>/SKILL.md
├── .mcp.json                    # bundled MCP
├── .app.json                    # app/connector (GitHub/Slack etc.)
├── hooks/hooks.json
└── assets/                      # icons, logos
```

> **Conversion note**: Only the manifest directory name differs — **`.claude-plugin/` ⇄ `.codex-plugin/`**; the placement philosophy for `skills/`, `hooks/`, and `.mcp.json` inside is shared. Codex's `.app.json` (connector) has no direct counterpart in Claude Code. Claude's `output-styles/`, `themes/`, `monitors/`, `lspServers`, and `bin/` have no confirmed counterparts in Codex plugins.

### 3.2 plugin.json Field Correspondence

| Claude Code | Codex | Convertible? | Notes |
|---|---|---|---|
| `name` (required, kebab-case) | `name` | ◎ | |
| `version` (semver, defaults to git SHA if omitted) | `version` | ◎ | |
| `description` | `description` | ◎ | |
| `author` (object) | `author` (object: name/email/url) | ◎ | |
| `homepage` / `repository` / `license` / `keywords` | (some similar under `interface`) | ○/△ | Codex tends to consolidate into `interface.category` etc. |
| `displayName` | `interface.displayName` | ○ | Different location |
| `skills` (path, added to skills/ by default) | `skills` (`"./skills/"`) | ◎ | |
| `commands` (path, replacement) | (Codex has thin commands concept / prompts are separate) | △ | |
| `agents` (path, replacement) | (config.toml `[agents.*]` side) | △ | |
| `hooks` (path/inline) | `hooks` (`"./hooks/hooks.json"`) | ○ | |
| `mcpServers` (path/inline) | `mcpServers` (`"./.mcp.json"`) | ○ | |
| `lspServers` | (unconfirmed) | ✕ | |
| `outputStyles` / `experimental.themes` / `experimental.monitors` | (unconfirmed) | ✕ | Claude-specific |
| `userConfig` (typed settings input UI) | (`interface.capabilities` etc.; equivalent typed input unconfirmed) | △ | Claude has stronger declarative config input |
| `defaultEnabled` | `policy.installation` (marketplace side) | △ | |
| `dependencies` (plugin dependency, semver) | (unconfirmed) | △ | |
| `channels` (message injection) | `.app.json` (connector) is philosophically similar | △ | |
| (not in Claude) | `interface.brandColor` / `composerIcon` / `logo` / `capabilities` | — | Codex-specific UI metadata |

### 3.3 marketplace.json Correspondence

| Claude Code | Codex | Convertible? |
|---|---|---|
| Placement: `.claude-plugin/marketplace.json` | `.agents/plugins/marketplace.json` (**also reads `.claude-plugin/marketplace.json` as compatible path**) | ◎ Codex absorbs Claude format |
| `name` (required, kebab-case, reserved names prohibited) | `name` (required) | ◎ |
| `owner` (required, name/email) | (unconfirmed but similar metadata) | ○ |
| `plugins[]` (required) | `plugins[]` (required) | ◎ |
| `plugins[].name` | `plugins[].name` | ◎ |
| `plugins[].source` (string / github / url / git-subdir / npm) | `plugins[].source` (`{source:"local"/"github"/..., path/repo}`) | ○ Source type format differs |
| (not in Claude) | `plugins[].policy` (`installation: AVAILABLE`, `authentication: ON_INSTALL`) | — (Codex-specific) |
| `metadata.pluginRoot` / `version` / `description` | (similar) | ○ |
| `allowCrossMarketplaceDependenciesOn` | (unconfirmed) | △ |
| `plugins[]` `category`/`tags`/`strict`/`defaultEnabled` etc. | (partial only) | △ |

> **Conversion note**: For marketplace, **Codex reads `.claude-plugin/marketplace.json` as a compatible path**, so Claude → Codex may work without changes just by placement. However, the `source` description format (Claude: string or typed object; Codex: `{source, path/repo}`) differs, so schema conversion is necessary.

### 3.4 Specification Differences for Bundled Components (commands / agents / hooks / mcp)

#### Slash commands / prompts
| | Claude Code | Codex |
|---|---|---|
| Format | `commands/<name>.md` (skill legacy) / skills recommended | `~/.codex/prompts/<name>.md` (**deprecated**, skills recommended) |
| Arguments | `$ARGUMENTS`, `$N`, `$name`, `argument-hint`, `arguments` | `$1`–`$9`, `$ARGUMENTS`, `$UPPER`, `$$`, `argument-hint`, `description` |
| Invocation | `/name` | `/prompts:name [args]` |
| Both sides | Recommend migrating to skills | |

> Argument templates are **closer to Codex Custom Prompts** (`$1`–`$9`). Claude's `arguments` (named) ⇄ Codex prompts `$UPPER` can be converted. However, both sides recommend skill migration, and skills have weak argument support, so transitional handling is required.

#### Agents (subagents)
| | Claude Code | Codex |
|---|---|---|
| Definition | `agents/<name>.md` (frontmatter: name, description, tools, model, permissionMode, maxTurns, skills, isolation, color, ~18 fields) | `config.toml` `[agents.<name>]` (config_file, description) + skill-internal `agents/openai.yaml` |
| Parallelism control | (session side) | `agents.max_threads`, `max_depth`, `job_max_runtime_seconds` |
| Conversion | △ Different design philosophies. Claude: markdown file per agent; Codex: config table + role-specific config file | |

> Agent bidirectional conversion has **the largest design divergence**. Claude's `agents/*.md` (self-contained markdown) and Codex's `[agents.*]` (config.toml reference table) have different structures. Some frontmatter like `tools`/`model`/`maxTurns` have no direct placement in Codex; manual work required.

#### Hooks
| Event | Claude Code | Codex |
|---|---|---|
| Shared | PreToolUse, PostToolUse, Stop, SessionStart, SubagentStart, SubagentStop, UserPromptSubmit, PreCompact, PostCompact, PermissionRequest | Supports the same 10 types |
| Claude-specific | Setup, UserPromptExpansion, PermissionDenied, PostToolUseFailure, PostToolBatch, Notification, MessageDisplay, TaskCreated, TaskCompleted, StopFailure, TeammateIdle, InstructionsLoaded, ConfigChange, CwdChanged, FileChanged, WorktreeCreate/Remove, Elicitation(Result), SessionEnd (30+ total) | Absent |
| Activation | Enabled by default | Enabled by default; set `features.hooks = false` to disable |
| Format | JSON (`hooks.json`), matcher is string/regex | TOML (`[[hooks.<Event>]]`), matcher is regex, `command_windows` override |
| Hook types | command / http / mcp_tool / prompt / agent | command (primary) |
| Output control | Exit code 0/2/other, JSON `permissionDecision` etc. | `continue`, `permissionDecision`, `updatedInput`, `decision:block` etc. |

> **Conversion note**: The 10 events present on both sides can be converted (JSON ⇄ TOML; matchers carry over mostly as-is). **Claude-specific 20+ events and non-command hook types (http/mcp_tool/prompt/agent) have no Codex counterpart and are lost**. Conversely, Codex → Claude is safe since Codex events are a subset.

#### MCP servers
| Key | Claude Code (.mcp.json / JSON) | Codex (config.toml / TOML) | Conversion |
|---|---|---|---|
| Launch command | `command` | `command` | ◎ |
| Arguments | `args` | `args` | ◎ |
| Environment variables | `env` (object) | `env` (table) / `env_vars` (forwarding name list) | ○ |
| Working dir | `cwd` | `cwd` | ◎ |
| HTTP URL | `url` (+ `type:"http"`) | `url` | ◎ |
| HTTP auth | `headers` / `oauth` | `bearer_token_env_var` / `http_headers` / `env_http_headers` | ○ |
| Enable/disable | `disabled: true` | `enabled: false` | ○ (inverted) |
| Timeout | `timeout` (ms) | `startup_timeout_sec` / `tool_timeout_sec` (seconds) | △ Unit/granularity difference |
| Per-tool control | (none in standard) | `enabled_tools` / `disabled_tools` / per-tool `approval_mode` | ✕ → Codex-specific |
| Always load | `alwaysLoad` | (unconfirmed) | △ |

> **Conversion note**: MCP **STDIO core (command/args/env/cwd) is fully compatible**. Differences are: (1) JSON ⇄ TOML format, (2) timeout unit (ms ⇄ seconds), (3) enable/disable flag polarity (`disabled` ⇄ `enabled`), (4) Codex per-tool approval (absent in Claude; information lost).

---

## 4. Surrounding Mechanism Comparison

### 4.1 Memory / Instruction Files (CLAUDE.md ⇄ AGENTS.md)

| Item | Claude Code | Codex |
|---|---|---|
| Project instructions | `CLAUDE.md` | `AGENTS.md` (Agentic AI Foundation open standard) |
| Global instructions | `~/.claude/CLAUDE.md` | `~/.codex/AGENTS.md` (relative to `$CODEX_HOME`) |
| Explicit override | (hierarchy proximity determines priority) | `AGENTS.override.md` (dedicated file) |
| Hierarchical merge | Root→CWD; closer file takes priority | Root→CWD concatenation; later file wins |
| Size limit | (not explicitly confirmed) | `project_doc_max_bytes` (default 32 KiB) |
| Fallback name | — | `project_doc_fallback_filenames` |

> **Conversion note**: Meaning can be preserved with just filename renaming (`CLAUDE.md` ⇄ `AGENTS.md`) and repositioning. **AGENTS.md is an open standard** (also adopted by Cursor, Jules, Amp, etc.), so a design centering AGENTS.md as the hub format for the conversion CLI is worth considering.

### 4.2 Core Settings (settings.json ⇄ config.toml)

| Item | Claude Code | Codex |
|---|---|---|
| Format | JSON (`settings.json`) | TOML (`config.toml`) |
| Scope | 4 layers: enterprise/user/project/local | Multi-layer: system(`/etc/codex`)/user(`~/.codex`)/project(`.codex`, requires trust)/profile/CLI |
| Profiles | (none; managed by scope) | `[profiles.<name>]` / separate file `~/.codex/<name>.config.toml` |
| Permissions | `permissions` (allow/deny rules) | `approval_policy` + `sandbox_mode` + `[permissions.<name>]` (filesystem/network granularity) |
| Enforced settings | managed settings | `requirements.toml` |
| Priority | enterprise > project > local > user | CLI > `-c` > project > profile > user > system > default |

> **Conversion note**: settings.json and config.toml differ **greatly in granularity and philosophy; full mechanical conversion is unrealistic**. The interop CLI should for now **restrict scope to Skills / Plugins / MCP / hooks / memory files, limiting settings ⇄ config conversion to the subset with clear correspondence (permissions, MCP, hooks)**.

### 4.3 / 4.4 MCP and Hooks are merged into §3.4.

---

## 5. Interoperability Conversion Matrix (Core of CLI Design)

Symbol: ◎ Lossless / ○ Format conversion only / △ Partial/approximate/distributed across files / ✕ Loss (discarded or manual)

### 5.1 Skills Conversion Loss Table

| Element | Claude → Codex | Codex → Claude |
|---|---|---|
| `name` | ◎ | ◎ (mind naming constraints) |
| `description` | ◎ | ◎ |
| `when_to_use` | ○ (concatenate to description) | — (absent in Codex) |
| Body Markdown | ○ (`` !`cmd` `` and `${CLAUDE_*}` require processing) | ◎ |
| `disable-model-invocation` | △ (→ openai.yaml `allow_implicit_invocation`; explicit invocation behavior differs / §2.5a) | ◎ (← openai.yaml) |
| `user-invocable` | ✕ (concept absent → discard + warn / §2.5b) | — (absent in Codex) |
| `argument-hint` / `arguments` | △ (loss in skill; possible via prompt) | △ |
| `allowed-tools` / `disallowed-tools` | ✕ at skill scope / △ degrade (`.rules` `allow`/`forbidden`, MCP, subagent / §2.6A) | — (absent in Codex) |
| `model` / `effort` | ✕ at skill scope / △ subagent/profile degrade (`max`→`xhigh` / §2.6B) | — (absent in Codex → default) |
| `context: fork` / `agent` | ✕ at skill scope / △ subagent degrade (no auto-fork; explicit spawn / §2.6B) | — (absent) |
| `paths` (glob trigger) | ✕ (no equivalent; AGENTS.md placement approximation only / §2.6C) | — (absent) |
| `hooks` (skill scope) | ✕ at skill scope / △ session/project degrade (§2.6C) | — (absent) |
| `shell` | △ (→ hooks `commandWindows`) | — (absent) |
| Codex `interface.*` (UI metadata) | — (Claude skill has weak receptacle) | △ (to plugin displayName etc.) |
| Codex `dependencies.tools` (MCP deps) | — (absent) | △ (to plugin mcpServers) |

**Summary**: Codex → Claude is **nearly lossless**. Claude → Codex has **14 / 16 frontmatter fields requiring attention**; in particular, `allowed-tools` / `model` / `effort` / `context:fork` / `paths` cannot be equivalently converted.

### 5.2 Plugins Conversion Loss Table

| Element | Claude → Codex | Codex → Claude |
|---|---|---|
| Manifest basics (name/version/description/author) | ◎ | ◎ |
| Manifest directory name | ○ (`.claude-plugin/` ⇄ `.codex-plugin/`) | ○ |
| Bundled `skills/` | ◎ | ◎ |
| Bundled `hooks/` | ○ (basic 10 events) | ○ |
| Bundled `.mcp.json` | ○ (JSON ⇄ TOML conversion may be needed) | ○ |
| `userConfig` (typed input) | △ (weak Codex equivalent) | △ |
| `lspServers` / `outputStyles` / `themes` / `monitors` / `bin` | ✕ (absent in Codex) | — (absent) |
| `dependencies` (plugin dependency) | △ | △ |
| Codex `.app.json` (connector) | — (absent) | ✕ (absent in Claude) |
| Codex `interface.*` (brandColor/logo/capabilities) | — (absent) | △ (partial → displayName) |
| marketplace.json | ◎ (Codex reads `.claude-plugin/` compatible) | ○ (source format conversion) |

### 5.3 Non-convertible / Manual-Only Representative Items (Items Where the CLI Should Emit Warnings)

- **Claude → Codex items that "can be partially substituted via scope degrade" (§2.6)**: `allowed-tools`/`disallowed-tools` (→ user/project `.rules` `allow`/`forbidden`, MCP `enabled_tools`/`disabled_tools`; except built-in tool prohibition like `AskUserQuestion`), `model`/`effort` (→ subagent agent TOML / profile; `max`→`xhigh`), `context:fork`+`agent` (→ subagent + explicit `spawn_agent`), skill-scoped `hooks` (→ session/project hooks). All involve scope degrade from skill→session/subagent and must be explicitly noted in the loss report.
- **Claude → Codex items that are completely discarded or require manual work**: `user-invocable` (concept absent), `disallowed-tools` for built-in tools (`AskUserQuestion` etc.), `paths` glob auto-trigger, `arguments`/`argument-hint` (no argument mechanism in skills), plugin `lspServers`/`outputStyles`/`themes`/`monitors`/`bin`, `userConfig`, body `` !`cmd` `` dynamic injection and `${CLAUDE_*}` variables, non-command hook types (http/mcp_tool/prompt/agent), Claude-specific hook events (20+).
- **Codex → Claude items that are discarded or require manual work**: `agents/openai.yaml` `interface.*` (icons, brand_color etc.), `.app.json` (connector), MCP per-tool `approval_mode`/`enabled_tools`, `config.toml` `[permissions.*]` fine-grained rules, `profiles`, `requirements.toml` enforced rules.
- **Invocation notation**: `/skill` ⇄ `$skill` rewriting in body and README (automatic replacement risks false positives; detect and propose replacements rather than auto-applying).

---

## 6. CLI Design Recommendations

1. **Narrow scope incrementally**: v1 limited to **Skills and MCP** gives the highest ROI (many ◎–○ correspondences, minimal loss). Follow with plugin manifest, hooks (basic 10 events), memory files. Full-auto settings ⇄ config conversion last (or declared out-of-scope). For preserving skill `model`/`effort`/tool permissions, **offer a "skill → subagent" conversion mode (§2.6B) as an option**.
2. **Use an intermediate representation (IR)**: Design where both Claude and Codex configs are projected into a **higher-level normalized schema (IR)** before output. The IR holds the union of fields from both sides; each field annotated with `origin` (claude/codex/both) and `lossiness` (lossless/lossy/dropped).
3. **Make loss reports mandatory output**: For each conversion run, output a **conversion report** (`--report` for details) listing "discarded fields", "approximated fields", and "items requiring manual action". The §5 tables become the inspection rules. **Also note which scope each field was degraded to (session/project/subagent)** (§2.6).
4. **Modularize the `agents/openai.yaml` ⇄ Claude frontmatter bridge**: `disable-model-invocation ⇔ policy.allow_implicit_invocation` and `interface ⇔ displayName/description` mappings should be encapsulated in a dedicated module.
5. **Add a body scanner**: Detect `` !`cmd` ``, `${CLAUDE_*}` / `$ARGUMENTS` / `$N` / `$name`, and `/skill` invocation notation; warn about and propose replacements for items that become invalid in the conversion target.
6. **Separate the format conversion layer**: JSON ⇄ TOML, timeout ms ⇄ seconds, `disabled` ⇄ `enabled` polarity inversion — keep these in a thin layer separate from semantic conversion.
7. **Leverage marketplace compatibility path**: Claude → Codex can likely place `.claude-plugin/marketplace.json` as-is. Only normalize the `source` schema.
8. **Add version detection**: Codex-side features are fluid (§7). Check `codex --version` / `claude --version` and skip + warn for unsupported features.
9. **Consider AGENTS.md as hub format**: Centering memory files around the open-standard AGENTS.md makes future expansion to Cursor etc. easier.
10. **Round-trip tests**: Validate in CI that `claude→codex→claude` produces diffs containing only "known loss items".

### Concrete Rules for the Degrade Mapping Engine (Implementing §2.6)

Implementation rules for preserving functionality at the cost of skill scope in Claude→Codex "degrade" conversions:

- **Tool pre-approve**: `allowed-tools: Bash(<cmd> <args>)` → generate `prefix_rule(pattern=[<cmd>, <args>...], decision="allow")` into `.codex/rules/<skill>.rules` (project) or `~/.codex/rules/default.rules` (user). `disallowed-tools: Bash(...)` → `decision="forbidden"`. MCP tools → `[mcp_servers.X] enabled_tools`/`disabled_tools`. **Built-in tool (`AskUserQuestion` etc.) prohibition has no conversion target → warn and discard**.
- **Skill → subagent**: Skills with `model`/`effort`/`context:fork` generate `.codex/agents/<skill>.toml` (`model` / `model_reasoning_effort` (`max`→`xhigh`) / `sandbox_mode` / `approval_policy` / `developer_instructions`=skill body) and reference it from `config.toml`'s `[agents.<skill>]` via `config_file`. `description`=`when_to_use`. Explicitly set `[features] multi_agent=true`.
- **Hooks**: Skill-scoped hooks → `[[hooks.<Event>]]` (session/project) + warning "no longer limited to skill scope". Only `command` type can be ported; other types are discarded.
- **Mandatory warning output**: ① Scope degrade (skill→session/subagent) ② Behavioral change from auto-fork → explicit `spawn_agent` ③ Loss of built-in tool prohibition, `paths` auto-trigger, and argument mechanism ④ Note that project-layer `.rules`/`.codex/agents` require `projects.<path>.trust_level="trusted"`.

---

## 7. Unconfirmed Items and Notes on Specification Fluidity

- Codex's **Skills / Plugins / Hooks are new features where documentation and schemas sometimes run ahead of the implementation**. Actual binary behavior (especially skill body variable substitution, absence of `user-invocable` equivalent, plugin `lspServers` support etc.) **requires real-instance verification**.
- Codex **Custom Prompts are deprecated**. Conversions relying on argument templates should be treated as transitional measures.
- Claude Code hooks events and plugin fields (`defaultEnabled`, `experimental.*`) also vary between versions. Tracking schema URLs (e.g., `json.schemastore.org/claude-code-plugin-manifest.json`) is recommended.
- The `settings.json` ⇄ `config.toml` full correspondence table in this report is **confirmed only for the subset of MCP/hooks/permissions**. Full field correspondence requires separate investigation.
- The ✕/△ judgments in this report are based on "the presence or absence of an equivalent field in current documentation". In practice, some cases can be functionally approximated by embedding instructions in the body (e.g., writing `allowed-tools` as instructional text in the body).

---

## 8. Reference URLs

**Claude Code**
- Skills: https://code.claude.com/docs/en/skills
- Agent Skills (API/overview): https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview
- Plugins: https://code.claude.com/docs/en/plugins
- Plugins Reference: https://code.claude.com/docs/en/plugins-reference
- Plugin Marketplaces: https://code.claude.com/docs/en/plugin-marketplaces
- Sub-agents: https://code.claude.com/docs/en/sub-agents
- Hooks: https://code.claude.com/docs/en/hooks
- MCP: https://code.claude.com/docs/en/mcp
- Official marketplace example: https://github.com/anthropics/claude-plugins-official

**OpenAI Codex**
- Skills: https://developers.openai.com/codex/skills
- Custom Prompts (deprecated): https://developers.openai.com/codex/custom-prompts
- AGENTS.md: https://developers.openai.com/codex/guides/agents-md
- Plugins: https://developers.openai.com/codex/plugins
- Build plugins: https://developers.openai.com/codex/plugins/build
- Hooks: https://developers.openai.com/codex/hooks
- Config Reference: https://developers.openai.com/codex/config-reference
- Config Sample: https://developers.openai.com/codex/config-sample
- MCP: https://developers.openai.com/codex/mcp
- Managed configuration: https://developers.openai.com/codex/enterprise/managed-configuration
- Repository: https://github.com/openai/codex (config.schema.json, docs/skills.md, .codex/skills/)
- AGENTS.md standard: https://agents.md/
