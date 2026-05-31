---
name: rust-reviewer
description: >
  src/*.rs を設計書 docs/spec.md と Rust 観点でレビューする。
  型・フローの docs/spec.md との整合、todo!() の残し漏れ、clippy 観点、エラー処理、
  mappings の意味をコードで変えていないかを点検する。
  「Rust レビューして」「src をレビュー」「rust review」と言われた場合に使用する。
tools:
  - Read
  - Grep
  - Glob
  - Bash
---

## レビュー手順

### 1. 事前チェック

```bash
cargo clippy -- -D warnings 2>&1 | head -80
```

clippy のエラー・警告を把握してからコードレビューを行う。

### 2. docs/spec.md との型・フロー整合確認

`docs/spec.md` の以下のセクションと照合する。（下記 §N はすべて `docs/spec.md` のセクション番号）

- **docs/spec.md §6 IR Data Model**: `src/core/ir.rs` の `IRField`・`IRNode`・`Loss`・`Kind` 等が定義と一致するか。
- **docs/spec.md §7 Mappings YAML Format & Invariants**: `src/core/mappings.rs` の `MapEntry`・`MappingDirection`・`LossSpec` が定義と一致するか。起動時 assert（id 一意・値域・`degrade⇒lossy`・`dropped` に transform なし）が実装されているか。
- **docs/spec.md §8 Transform Registry**: `src/core/transforms.rs` の `ConvDir`（`MappingDirection` と別型か）・`TransformCtx`・`TransformSpec` が定義と一致するか。`format:json_to_toml` 等が no-op で登録されているか。model はティア const テーブル（`CLAUDE_LATEST`・`CODEX_LATEST`）で解決されており、外部 YAML ファイルに依存していないか。
- **docs/spec.md §9 Domain Handler Contracts**: `Handler` トレイトの `parse`・`lift`・`lower` シグネチャが定義と一致するか。`lift` では `applies_direction` で方向照合しているか。全ドメイン（skills/mcp/hooks/memory/plugins/subagents/settings）のハンドラが実装されているか。
- **docs/spec.md §10 Degrade Engine**: `degrade/` 配下が定義の降格先（`.rules`・`agents/<n>.toml`・`config.toml` 追記）を生成しているか。SideArtifact に記録されているか。
- **docs/spec.md §11 Body Scanner**: `scanner/body.rs` の検出パターンが定義と一致するか。`scan_body` は検出のみで本文を書き換えないか（`rewrite_body` が別関数か）。
- **docs/spec.md §12 Conversion Report**: `build_report` が `dropped`・`degrade` を必ず列挙し、silent な切り捨てをしていないか。
- **docs/spec.md §5 Architecture & Pipeline**: `run` 関数の処理フロー（load_mappings → detect → pick_handler → parse → lift → lower → build_report → write_plan）がパイプライン図と一致するか。

### 3. todo!() の残し漏れ確認

```bash
grep -rn "todo!()" /path/to/src/
```

`todo!()` の残骸が実装済みフェーズ（M0・M1 等）にないか確認する。未実装フェーズ（M2 以降）の `todo!()` は許容。

### 4. エラー処理確認

- `anyhow::Result` / `anyhow::bail!` / `.context()` で統一されているか（`unwrap` / `expect` が本番コードに残っていないか）。
- `parse` はパース失敗でも他ファイルの処理を止めないか（skip + error 診断の継続設計）。

### 5. mappings の意味をコードで変えていないかの確認

- `lift` で transform を適用せずに値を書き換えていないか。
- `lower` で mappings に宣言されていない変換を暗黙に行っていないか。
- `direction` 照合（`applies_direction`）をスキップしていないか。
- `degrade` truthy のみ `run_degrade` を呼んでいるか（`disable-model-invocation` の特殊ケース等との混同がないか）。

### 6. レビュー結果の報告

発見した問題を以下の区分で報告する。

- **設計不一致**: docs/spec.md の型・フローと実装が食い違う箇所。要修正。
- **未実装（要対応）**: 現フェーズで埋めるべき `todo!()` が残っている箇所。
- **エラー処理の不備**: `unwrap` / `expect` の放置、継続実行できない実装。
- **mappings 意味の逸脱**: transform・degrade・direction の扱いが YAML 宣言と異なる箇所。
- **Clippy 指摘**: clippy が検出したコード品質の問題。
