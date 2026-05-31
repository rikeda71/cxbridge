---
paths:
  - "mappings/**"
---

# mappings 編集ルール

`mappings/*.yaml` は Claude Code ⇄ Codex CLI 変換の**正本データ**。以下を厳守すること。

## スキーマ準拠（`mappings/SCHEMA.md` を参照）

- `id` は全ファイルを通じて一意にする
- `direction` は `both` / `claude_to_codex` / `codex_to_claude` のいずれか
- `loss` は `lossless` / `lossy` / `dropped` のいずれか
- `degrade` を付けるなら `loss: lossy` であること
- `loss: dropped` のエントリに `transform` を付けない

## 編集後の検証

編集後は必ず `scripts/validate-mappings.py` を実行して不変条件を確認する。
Claude Code の PostToolUse hook が自動実行するが、手動でも確認できる:

```
python3 scripts/validate-mappings.py
```

## 意味・根拠の保全

- 各エントリの `notes` に根拠 source URL を残す（`source:` フィールドまたは `notes` 内）
- `docs/` の記述と矛盾する変更はしない。矛盾が生じる場合は `docs/spec.md` と該当 `mappings/*.yaml` を**両方**更新して整合を保つ
- 既存エントリの意味を黙って変えない。不明な場合は `docs/` および `notes` を確認する

## mappings 不変条件テスト（`docs/spec.md §18 Testing Strategy`）

実装側（`src/**`, `tests/**`）でテストすること:

- `id` が全ファイルを通じて一意であること
- `direction` は `both` / `claude_to_codex` / `codex_to_claude` のみ
- `loss` は `lossless` / `lossy` / `dropped` のみ
- `degrade` を持つエントリは `loss: lossy` であること
- `loss: dropped` のエントリに `transform` が付いていないこと
