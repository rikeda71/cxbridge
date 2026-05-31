# mappings/*.yaml スキーマ定義

`mappings/*.yaml` は Claude Code ⇄ OpenAI Codex CLI の相互変換 CLI が読み込む**機械可読な変換テーブル**である。人間向け解説は `docs/` 側にあり、本 YAML はそのうち「フィールド対応」を機械処理可能な形に落としたもの。

## トップレベル構造

```yaml
domain: skills                      # 領域識別子（ファイル名と一致）
title: "Skills (SKILL.md)"          # 人間向けタイトル
doc: ../docs/spec.md           # 対応する解説ドキュメント（相対パス）
files:                              # この領域が扱う設定ファイル
  claude:
    - ".claude/skills/<name>/SKILL.md"
    - "~/.claude/skills/<name>/SKILL.md"
  codex:
    - ".agents/skills/<name>/SKILL.md"
    - "~/.agents/skills/<name>/SKILL.md"
format:                            # ファイル形式（リスト。1領域が複数形式を取りうる）
  claude: [markdown+yaml-frontmatter] # 単一形式でもリストで書く
  codex: [toml, json]                 # 例: Codex hooks は TOML または JSON
entries:                           # フィールド対応エントリの配列（下記）
  - { ... }
notes:                             # 領域全体にかかる注記（任意）
  - "..."
```

## entries[] の各エントリ

```yaml
- id: skills.allowed-tools          # 一意なエントリ ID（領域.フィールド）
  claude:                           # Claude 側の対応物（無い場合は null）
    field: "allowed-tools"          # フィールド名/キーパス（ドット記法）
    type: "string|list"             # 型
    scope: skill                    # この設定が効くスコープ
  codex:                            # Codex 側の対応物（無い場合は null）
    field: null                     # 直接対応がなければ null
    type: null
    scope: null
  direction: claude_to_codex        # both | claude_to_codex | codex_to_claude
  loss: lossy                       # lossless | lossy | dropped
  degrade:                          # スコープ降格情報（降格が起きる場合のみ）
    to: session                     # 降格先スコープ
    target: ".codex/rules/<skill>.rules (execpolicy allow)"  # 降格先の具体的な書き込み先
  transform: null                   # 値の変換規則（下記の transform 語彙、無ければ null）
  warn: true                        # 変換時にユーザー警告を出すべきか
  notes: "skill 実行中だけの pre-approve は再現不可。.rules の allow に降格するとセッション全体に効く"
  source: "https://..."             # 根拠 URL（任意）
```

## フィールド語彙

### `scope`（設定が効く範囲）
- `skill` / `command` / `agent` / `plugin` — そのコンポーネント実行中だけ
- `session` — 起動中のセッション全体
- `project` — プロジェクト（リポジトリ）単位
- `user` — ユーザー（全プロジェクト）単位
- `profile` — Codex の名前付きプロファイル単位
- `subagent` — Codex の subagent（role/standalone TOML）単位
- `managed` — 組織強制（managed settings / requirements.toml）
- `global` — ツール全体

### `direction`（変換方向）
- `both` — 双方向に変換可能
- `claude_to_codex` — Claude→Codex のみ意味を持つ（Codex→Claude では出力されない / 既定値）
- `codex_to_claude` — Codex→Claude のみ

### `loss`（情報損失レベル）
- `lossless` — 完全に等価。値・書式変換のみ
- `lossy` — 意味は近いが情報の一部が失われる / スコープが変わる / 値が丸まる
- `dropped` — 対応物がなく破棄（手動対応 or 警告のみ）

### `degrade`（スコープ降格）
`loss: lossy` で「skill スコープ → より広い/別スコープ」へ移る場合に記載。
- `to`: 降格先 scope
- `target`: 降格先の具体的な設定（書き込み先ファイル・キー）

### `transform`（値変換規則）— 文字列で記述、CLI 実装側でパース
代表的な規則（CLI 実装で関数化する想定）:
- `unit:ms_to_sec` / `unit:sec_to_ms` — タイムアウト等の単位変換（例: `60000`→`60.0`）
- `polarity:invert` — 真偽の極性反転（例: Claude `disabled:true` ⇔ Codex `enabled:false`）
- `enum_map:{a:b,...}` — enum 値の対応（例: effort `max`→`xhigh`）
- `index_shift:+1` / `index_shift:-1` — 引数インデックスの 0基点⇔1基点シフト（`$ARGUMENTS[0]`⇔`$1`）
- `str_to_list:space` / `list_to_str:space` — スペース区切り文字列 ⇔ 配列（OAuth scopes 等）
- `rename` — キー名のみ変更（例: `headers`⇔`http_headers`）
- `format:json_to_toml` / `format:toml_to_json` — シリアライズ形式変換
- `extract:bearer_env` — `"Bearer ${VAR}"` から環境変数名 `VAR` を抽出（MCP Bearer token）
- `path:remap` — パス規約の付け替え（`.claude/`⇔`.agents/` 等）
- `inline_imports` — `@import` 参照をインライン展開（CLAUDE.md→AGENTS.md）

複数規則は `;` 区切り（例: `unit:ms_to_sec; rename`）。

## 変換エンジンが守るべき不変条件
1. `loss: dropped` のエントリは、変換時に必ず conversion report に列挙する。
2. `warn: true` のエントリは、変換実行時にユーザー警告を出す。
3. `degrade` のあるエントリは、降格先スコープ（`to`）を report に明記する。
4. `direction` が片方向のエントリは、逆方向変換では無視（または既定値復元）する。
5. 同一 `id` は全 mappings を通じて一意。

## 補足
- 本テーブルは「現行ドキュメント/スキーマ上の対応」を表す。実バイナリ挙動と差異がありうる項目は `notes` に明記する。
- Codex 側仕様は流動的（2025-2026 の新機能）。`source` URL とともにバージョン依存性を `notes` に残す。
