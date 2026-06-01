# ccx — Claude Code ⇄ Codex CLI 設定双方向変換 CLI

[English](README.md)

**ccx** は [Claude Code](https://code.claude.com/docs)（`.claude/`、JSON）と
[OpenAI Codex CLI](https://developers.openai.com/codex)（`.codex/`、TOML）の設定ファイルを
双方向変換する Rust 製 CLI です。
Skills / Plugins / Hooks / MCP サーバー / メモリファイル / Subagents / Settings を対象とします。
変換ルールは `mappings/*.yaml`（301 エントリ）に宣言済みで、CLI はそれを解釈するエンジンです。

```
Claude Code  .claude/ (JSON)  ⇄  Codex CLI  .codex/ (TOML)
```

変換のたびに **conversion report** が必ず出力され、lossless / lossy / degrade / dropped /
本文スキャン警告が列挙されます。サイレントなデータ損失は禁止されています。

---

## インストール

**事前要件:** Rust 1.80 以上（`cargo` が使えること）

```sh
git clone https://github.com/rikeda71/ccx
cd ccx
cargo build --release
cp target/release/ccx ~/.local/bin/
```

---

## 使い方

```sh
ccx c2x <path>    # Claude → Codex（一方向）
ccx x2c <path>    # Codex → Claude（一方向）
ccx check <path>  # 変換可能性の事前診断（書き込まない）
```

`<path>` にはファイルまたはディレクトリ（再帰検出）を指定します。

### オプション（`c2x` / `x2c` 共通）

| フラグ | 既定値 | 説明 |
|---|---|---|
| `--out <dir>` | `<input>.converted/` | 出力先ディレクトリ |
| `--only <domains>` | 全ドメイン | 変換対象ドメインをカンマ区切りで限定（`skills,mcp` など） |
| `--scope <project\|user>` | `project` | 降格先スコープ（`.rules` / agents の配置） |
| `--skill-target <auto\|skill\|subagent>` | `auto` | Skill の変換先を強制指定 |
| `--interactive` | false | グレーケースを TTY 対話で確認する |
| `--rewrite-body` | false | 本文の変数/記法を自動書き換え（既定: 検出 + 警告のみ） |
| `--dual-manifest` | false | `.claude-plugin/` を残しつつ `.codex-plugin/` も生成 |
| `--hooks-target <user\|project>` | `user` | hooks の書き出し先 |
| `--report[=json]` | なし | 詳細レポートを出力（`=json` で機械可読 JSON） |
| `--dry-run` | false | 書き込まず report のみ出力 |
| `--strict` | false | dropped が 1 件でもあれば exit 2（CI 用） |
| `--keep-claude-frontmatter` | false | Claude 固有 frontmatter キーを Codex 出力に残置 |
| `--force` | false | 既存ファイルへの上書きを許可 |

### 使用例

```sh
# Claude の skill を Codex 形式に変換
ccx c2x .claude/skills/deploy/SKILL.md

# Codex の config.toml を Claude 形式に変換（レポートのみ確認）
ccx x2c .codex/config.toml --dry-run --report

# .mcp.json を変換前に診断
ccx check .mcp.json

# CI: dropped フィールドがあれば失敗
ccx c2x .claude/skills/deploy/SKILL.md --strict

# 機械可読な JSON レポートを出力
ccx c2x .mcp.json --dry-run --report=json
```

---

## Conversion Report

実行ごとにレポートが出力されます。例:

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

| 記号 | 意味 |
|---|---|
| ◎ | Lossless — 完全に等価 |
| ○ | Lossy — 意味は保持されるが情報が一部減少 |
| △ | Degraded — より広いスコープへ移動（例: skill → session） |
| ✕ | Dropped — 変換先なしで破棄 |
| ⚠ | Body warning — 手動確認が必要 |

`--strict` 使用時、dropped が 1 件以上あると exit code 2 で終了します。

---

## ドキュメント

- **[docs/spec.md](docs/spec.md)** — 設計・実装仕様の全文（IR モデル、transform レジストリ、
  ドメインハンドラ仕様、降格エンジン、CLI フラグ、終了コード、テスト戦略など）
- **[mappings/](mappings/)** — 変換テーブルの正本データ（301 エントリ、`skills.yaml` /
  `hooks.yaml` / `mcp.yaml` / `plugins.yaml` / `memory.yaml` / `subagents.yaml` /
  `settings-config.yaml`）、スキーマ定義は [mappings/SCHEMA.md](mappings/SCHEMA.md)
