---
name: mappings-auditor
description: >
  mappings/*.yaml と docs（特に mappings/SCHEMA.md・docs/spec.md §16 Feature & Loss Matrix Summary）の整合を監査する。
  loss 判定の妥当性、notes の根拠 URL 有無、SCHEMA 違反を点検する。
  「mappings を監査して」「mappings の整合チェック」「audit mappings」と言われた場合に使用する。
tools:
  - Read
  - Grep
  - Glob
  - Bash
---

## 監査手順

### 1. スクリプトによる自動検証

```bash
uv run "$CLAUDE_PROJECT_DIR/scripts/validate-mappings.py"
```

NG が出たら後続ステップの前に修正案を提示する。

### 2. loss/direction と docs/spec.md §16 の整合確認

`docs/spec.md §16 Feature & Loss Matrix Summary` を Read して、各機能の対応状況（変換可・不可・将来追従）を把握する。
次に `mappings/*.yaml` の各エントリを Grep・Read し、以下を確認する。

- `docs/spec.md §16` で「変換不可」とされているフィールドは `loss: dropped` になっているか。
- `docs/spec.md §17 Codex Interop Notes & Known Issues` で「将来追従」とされているフィールドは `notes` に `status: awaiting-codex` が付いているか。
- `loss: lossy` のエントリは docs/spec.md の説明と整合する理由があるか（単純 rename や単位変換は `lossless` が適切でないか）。

### 3. notes の根拠 URL 確認

`warn: true` または `loss: lossy/dropped` かつ `notes` があるエントリについて、`source` フィールドに根拠 URL が存在するか確認する。
参照先が GitHub issue（`openai/codex#*`）や公式ドキュメントである場合はより望ましい。

### 4. 将来追従マーキングの妥当性確認

`notes` に `status: awaiting-codex` を含むエントリについて、現時点で Codex 側に実装済みのものがないか確認する。実装済みならば `loss: dropped` → `loss: lossy` または `both` への昇格を提案する。

### 5. 監査結果の報告

発見した問題を以下の区分で報告する。

- **SCHEMA 違反**: 不変条件（id 一意・値域・degrade⇒lossy 等）に反するもの。要修正。
- **整合不備**: docs/13 と loss 判定が食い違うもの。要確認・修正。
- **根拠欠如**: warn/lossy/dropped なのに `notes` や `source` が薄いもの。要補足。
- **昇格候補**: `awaiting-codex` だが Codex が既に実装した可能性があるもの。要調査。
