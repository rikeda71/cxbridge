---
name: validate-mappings
description: >
  mappings/*.yaml を編集したとき、またはコミット前に、変換テーブルの不変条件（id 一意・direction/loss
  の値域・degrade⇒lossy・dropped に transform なし）を検証する。mappings を変更したら必ず使う。
  「mappings を確認して」「mappings を検証して」「validate mappings」と言われた場合に使用する。
allowed-tools:
  - Bash(python3 *)
---

## 手順

1. バリデーションスクリプトを実行する。

   ```bash
   uv run "$CLAUDE_PROJECT_DIR/scripts/validate-mappings.py"
   ```

2. 出力が全件 OK であればそのまま完了を報告する。

3. NG が出た場合は、`mappings/SCHEMA.md` の語彙定義に従ってエラーを修正する。
   - `id 一意` 違反 → 重複 id を持つエントリを探し、一方を別の id に変更する。
   - `direction` 値域違反 → `both` / `claude_to_codex` / `codex_to_claude` のいずれかに修正する。
   - `loss` 値域違反 → `lossless` / `lossy` / `dropped` のいずれかに修正する。
   - `degrade⇒lossy` 違反 → `degrade` が設定されているエントリは `loss: lossy` でなければならない。
   - `dropped に transform` 違反 → `loss: dropped` のエントリに `transform` を設定してはならない（`null` に戻す）。

4. 修正後に再度スクリプトを実行して全件 OK を確認する。
