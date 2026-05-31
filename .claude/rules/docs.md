---
paths:
  - "docs/**"
---

# docs 編集ルール

`docs/spec.md` は設計・仕様の唯一の正本。旧 `docs/01`〜`docs/13` は `docs/spec.md` に統合済みであり、参照してはならない。

## 設計変更時の整合ルール

- 設計を変更する場合は `docs/spec.md` と関連する `mappings/*.yaml` を**両方**更新し整合を保つ
- 機能対応・損失マトリクス（変換可/不可/将来追従の分類）は `docs/spec.md §16 Feature & Loss Matrix Summary` に記載されており、`mappings/*.yaml` の `loss` 分布（lossless / lossy / dropped の件数・内訳）と一致させる
- `docs/spec.md` と他のドキュメントに矛盾が生じた場合は `docs/spec.md` を優先する

## 参照ルール

- 実装のフロー・型・インタフェースに不明点がある場合は `docs/spec.md` の該当セクション（セクション名で参照）を参照する
- Codex 側の挙動に不確実性がある場合は `docs/spec.md §17 Codex Interop Notes & Known Issues` と各エントリの `notes` を参照する
