# ccx — Claude Code ⇄ Codex 設定変換 CLI

[![CI](https://github.com/rikeda71/ccx/actions/workflows/ci.yml/badge.svg)](https://github.com/rikeda71/ccx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English README](README.md)

[Claude Code](https://code.claude.com/docs) と [OpenAI Codex CLI](https://developers.openai.com/codex)
のエージェント設定を、手作業でやり直すことなく、そして設定を黙って失うことなく、双方向に移行します。

```
Claude Code  .claude/ (JSON)   ⇄   Codex CLI  .codex/ (TOML)
```

## なぜ ccx か

Claude Code と Codex を両方使っていると、2 つの設定はだんだん食い違っていきます。片方で
skills・hooks・MCP サーバー・メモリファイル・subagents を作り込んだあと、それをもう片方でも
使いたい。しかし両者はディレクトリ構成もファイル形式（JSON / TOML）も対応機能も異なります。

ccx は両者をどちらの向きにも変換します。難しいのはファイルをコピーすることではなく、「何が
きれいには変換できないか」を把握することです。そこで実行のたびに **conversion report** を出力し、
完全等価で移せたもの・形を変えて移したもの・より広いスコープへ移したもの・対応先がなく破棄した
ものを正確に示します。**サイレントな損失は決して起こしません。**

変換ルールはハードコードされておらず `mappings/*.yaml`（8 ドメイン・304 エントリ）に宣言されています。
CLI はそれを解釈するエンジンに過ぎません。

## ひと目で

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

## 変換対象

| ドメイン | 例 |
|---|---|
| **Skills** | `SKILL.md` frontmatter、`allowed-tools`、model/effort、本文スキャン |
| **Plugins** | plugin マニフェスト、同梱の `commands/`・`agents/` ディレクトリ |
| **Hooks** | イベント hook、matcher、command hook |
| **MCP サーバー** | `.mcp.json` ⇄ Codex `[mcp_servers]` |
| **Memory** | `CLAUDE.md` / `AGENTS.md` とメモリ設定 |
| **Subagents** | エージェント定義とモデル tier |
| **Settings / Config** | `settings.json` ⇄ `config.toml` |
| **Variables** | `${CLAUDE_*}` プレースホルダと Codex の対応物 |

## インストール

**事前要件:** Rust 1.80 以上（stable の `cargo`）。

```sh
git clone https://github.com/rikeda71/ccx
cd ccx
cargo build --release
cp target/release/ccx ~/.local/bin/   # PATH の通った場所へ
```

ビルド済みバイナリは [Releases](https://github.com/rikeda71/ccx/releases) ページで配布しています。

## 使い方

```sh
ccx c2x <path>    # Claude → Codex
ccx x2c <path>    # Codex → Claude
ccx check <path>  # 変換可能性を診断（書き込まない）
```

`<path>` にはファイルまたはディレクトリ（再帰検出）を指定します。

```sh
# Claude の skill を Codex 形式へ変換
ccx c2x .claude/skills/deploy/SKILL.md

# Codex → Claude をディスクに触れずプレビュー
ccx x2c .codex/config.toml --dry-run --report

# .mcp.json を変換前に診断
ccx check .mcp.json

# dropped が出たらビルドを失敗させる（CI 向け）
ccx c2x .claude/skills/deploy/SKILL.md --strict

# 機械可読な JSON レポート
ccx c2x .mcp.json --dry-run --report=json
```

<details>
<summary><strong>全オプション</strong>（<code>c2x</code> / <code>x2c</code> 共通）</summary>

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

</details>

## レポートの読み方

実行ごとに必ず 1 行サマリが出ます。`--report` を付けると上記のフィールド単位の詳細も得られます。
各行には記号が 1 つ付きます:

| 記号 | 意味 |
|---|---|
| ◎ | **Lossless** — 反対側でも完全に等価 |
| ○ | **Lossy** — 意味は保持されるが情報が一部減少 |
| △ | **Degraded** — より広いスコープへ移動（例: skill → project）。移動先を明示 |
| ✕ | **Dropped** — 変換先なしで破棄（必ず報告される） |
| ⚠ | **Body warning** — 本文中の構文に手動確認が必要 |

`--strict` を付けると dropped が 1 件でもあれば非ゼロ（exit code 2）で終了します。これにより、
データを黙って失う変換を CI で拒否できます。

## ドキュメント

- **[docs/spec.md](docs/spec.md)** — 設計・実装仕様の全文: IR モデル、transform レジストリ、
  ドメインハンドラ仕様、降格エンジン、CLI フラグ、終了コード、テスト戦略。
- **[mappings/](mappings/)** — 変換テーブルの正本データ（8 ドメイン・304 エントリ:
  `skills` / `hooks` / `mcp` / `plugins` / `memory` / `subagents` / `settings-config` /
  `variables`）。スキーマは [mappings/SCHEMA.md](mappings/SCHEMA.md) を参照。

## ライセンス

[MIT](LICENSE) © Ryuya Ikeda
