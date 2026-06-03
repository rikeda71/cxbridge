# cxbridge

[![CI](https://github.com/rikeda71/cxbridge/actions/workflows/ci.yml/badge.svg)](https://github.com/rikeda71/cxbridge/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/cxbridge.svg)](https://crates.io/crates/cxbridge)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**[English README here](README.md)**

[Claude Code](https://code.claude.com/docs)（`.claude/`、JSON）と [OpenAI Codex CLI](https://developers.openai.com/codex)（`.codex/`、TOML）の設定を双方向に変換する CLI です。

skills / hooks / MCP サーバー / memory / subagents / plugins / settings をどちらの向きにも変換し、「何が完全に移せて、何が形を変え、何が移せなかったか」をレポートで示します。サイレントな損失は起こしません。

```text
$ cxbridge c2x .claude/skills/deploy/SKILL.md --report

▸ skills: SKILL.md
  ◎ skills.name, skills.description  lossless
  △ skills.allowed-tools  degrade  skills.allowed-tools → .codex/rules/<skill>.rules (execpolicy allow)…
  ✕ skills.user-invocable  dropped  model-only / hidden-from-user flag has no Codex concept
  ⚠ 3 body warnings — run with --report=json for line-by-line
Summary: 2 lossless, 1 lossy(1 degraded), 1 dropped, 3 body-warning
```

変換した各ファイルは `▸ <ドメイン>: <ソース>` のヘッダーで始まるので、ディレクトリ
一括変換でも読みやすいままです。同一フィールドは `×N` で集約、body warning は件数に
集約され、行単位の全詳細は常に `--report=json` で得られます。

## Claude Code と Codex の設定を揃える

両方のツールを使っていると、片方で作り込んだ skills・hooks・MCP サーバーをもう片方でも使いたくなります。cxbridge は手作業でやり直す代わりにそれらを変換し、2 つのツールで本当に食い違う箇所を教えてくれます。

```bash
# Claude の skill を Codex に持っていく
cxbridge c2x .claude/skills/deploy/SKILL.md

# Codex の設定を Claude に戻す
cxbridge x2c .codex/config.toml

# 書き込む前に、何が変換されるか確認する
cxbridge check .claude/
```

## 使い方

```
cxbridge <c2x|x2c|check> <path> [options]
```

```bash
cxbridge c2x <path>    # Claude → Codex
cxbridge x2c <path>    # Codex → Claude
cxbridge check <path>  # 変換可能性を診断（書き込まない）

cxbridge --version     # バージョン表示
cxbridge --help        # ヘルプ表示
```

`<path>` にはファイルまたはディレクトリ（再帰スキャン）を指定します。

```bash
# 1 つの skill を変換
cxbridge c2x .claude/skills/deploy/SKILL.md

# Codex → Claude をディスクに触れずプレビュー
cxbridge x2c .codex/config.toml --dry-run --report

# dropped が出たら失敗させる（CI ゲートとして）
cxbridge c2x .claude/ --strict

# 機械可読な JSON レポート
cxbridge c2x .mcp.json --dry-run --report=json
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
| `--strict` | false | dropped が 1 件でもあれば exit 2 |
| `--keep-claude-frontmatter` | false | Claude 固有 frontmatter キーを Codex 出力に残置 |
| `--force` | false | 既存ファイルへの上書きを許可 |

</details>

## インストール

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

静的リンクの `…-x86_64-unknown-linux-musl.tar.gz` と Windows 用 `…-x86_64-pc-windows-msvc.zip` も各 [release](https://github.com/rikeda71/cxbridge/releases) に添付されています。

### Cargo

```bash
cargo install cxbridge
```

### ソースから

```bash
git clone https://github.com/rikeda71/cxbridge.git
cd cxbridge
cargo install --path .
```

## 変換対象

| ドメイン | 例 |
|---|---|
| **Skills** | `SKILL.md` frontmatter、`allowed-tools`、model/effort、本文スキャン |
| **Plugins** | plugin マニフェスト、同梱の `commands/`・`agents/` ディレクトリ |
| **Hooks** | イベント hook、matcher、command hook |
| **MCP サーバー** | `.mcp.json` ⇄ Codex `[mcp_servers]` |
| **Memory** | `CLAUDE.md` ⇄ `AGENTS.md` とメモリ設定 |
| **Subagents** | エージェント定義とモデル tier |
| **Settings / Config** | `settings.json` ⇄ `config.toml` |
| **Variables** | `${CLAUDE_*}` プレースホルダと Codex の対応物 |

ドメインごとに「何がきれいに変換され、何が形を変え、何が dropped になるか」の詳細は **[docs/conversion-coverage.md](docs/conversion-coverage.md)** を参照してください。

## conversion report の読み方

実行ごとに必ず 1 行 `Summary:` が出ます。`--report` を付けるとフィールド単位の詳細も得られます。各ファイルは `▸ <ドメイン>: <ソース>` のヘッダーで始まり（ディレクトリ変換でも識別可能）、各フィールド行に記号が 1 つ付きます:

| 記号 | 意味 |
|---|---|
| ◎ | **Lossless** — 反対側でも完全に等価 |
| ○ | **Lossy** — 意味は保持されるが情報が一部減少 |
| △ | **Degraded** — より広いスコープへ移動（例: skill → project）。移動先を明示 |
| ✕ | **Dropped** — 変換先なしで破棄（必ず報告される） |
| ⚠ | **Body warning** — 本文中の構文に手動確認が必要 |

読みやすさのため、同一フィールドは `×N` で集約し長いメッセージは短縮、body warning は件数 1 行に集約されます。**`--report=json`** は網羅的で（全 dropped/degraded/lossy と body warning の全行＋各ファイルの `source`/`domain`）、dropped・degraded はどちらの形式でも必ず列挙されます（サイレントな損失なし）。

`--strict` を付けると dropped が 1 件でもあれば非ゼロ（exit code 2）で終了するので、データを黙って失う変換を CI で拒否できます。

## ドキュメント

- **[docs/conversion-coverage.md](docs/conversion-coverage.md)** — ドメインごとの変換可否・degrade・dropped の一覧。
- **[docs/spec.md](docs/spec.md)** — 設計・実装仕様の全文（IR モデル、transform レジストリ、degrade エンジン、CLI フラグ、終了コード）。
- **[mappings/](mappings/)** — 変換テーブルの正本（8 ドメイン・304 エントリ）。スキーマは [mappings/SCHEMA.md](mappings/SCHEMA.md)。

## ライセンス

[MIT](LICENSE) © Ryuya Ikeda
