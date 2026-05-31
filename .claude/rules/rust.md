---
paths:
  - "src/**"
  - "tests/**"
---

# Rust 実装規約

## フォーマット・品質

- `cargo fmt` を通す（フォーマット違反をコミットしない）
- `cargo clippy -- -D warnings` を通す（警告をエラーとして扱う）
- `cargo test` を通す

## エラーハンドリング

- エラーは `anyhow` で扱う（`anyhow::Error` / `anyhow::Result` / `anyhow::bail!` / `anyhow::Context`）
- パニックや `unwrap()` を本番ロジックに残さない

## 型・フローの正本

- 型定義・処理フローは `docs/spec.md` を正とする
- コードと設計書に齟齬があれば `docs/spec.md` に合わせる
- `todo!()` を本番ロジックに残さない（スタブは段階的に実装する）

## mappings との関係

- `mappings/*.yaml` が変換の正本データ。コードはそれを駆動するエンジン
- YAML の意味をコードで変えない。不明な場合は `mappings/SCHEMA.md` と `notes` を参照
- `MappingDirection`（mappings 用）と `ConvDir`（pipeline 用）を混同しない

## 変換実装の原則（`docs/spec.md §6 IR Data Model` 〜 `docs/spec.md §10 Degrade Engine` に詳細）

- **model はティア const**（opus/sonnet/haiku ⇄ high/mid/low）。`model-map.yaml` は存在しない。ティア定義は `docs/spec.md §8 Transform Registry`（Model Tier Mapping 節）
- **skill→skill か skill→subagent か**は `--skill-target`/`--interactive`/`decide_skill_target` で決定（`docs/spec.md §9.1 Skills`）。`model`/`effort`/skill 限定権限があれば subagent、純粋指示なら skill
- **降格（skill→session/subagent）はスコープが変わる**。conversion report に**必ず明記**。`dropped` も silent にせず列挙（`docs/spec.md §10 Degrade Engine`、`docs/spec.md §12 Conversion Report`）
- **Codex 側は流動的**（plugin 同梱 hooks は未ロードの可能性 `openai/codex#16430`、skill loader は未知 frontmatter を fail-open で無視、等）。`docs/spec.md §17 Codex Interop Notes & Known Issues` と各 `mappings` の `notes` を参照。降格結果は実機検証を推奨

## テスト戦略（`docs/spec.md §18 Testing Strategy`）

- `insta` スナップショット: `tests/fixtures/` をゴールデンとして使用
- 往復テスト: `c2x → x2c` で `lossless` エントリは完全一致、`lossy`/`dropped` は既知差分のみ許容
- mappings 不変条件テストは `rules/mappings.md` 参照
