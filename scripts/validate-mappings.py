#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml"]
# ///
"""Validates invariants of mappings/*.yaml (per mappings/SCHEMA.md).

Called via `uv run` from Claude Code hooks and the validate-mappings skill.
Dependencies (PyYAML) are declared in the PEP 723 metadata above and resolved by uv.
Exits non-zero and prints violations to stderr if any invariant is violated.

Invariants checked:
  - `id` is unique across all yaml files
  - `id` / `direction` / `loss` fields are present
  - `direction` ∈ {both, claude_to_codex, codex_to_claude}
  - `loss` ∈ {lossless, lossy, dropped}
  - if `degrade` is set, `loss` must be lossy
  - entries with `loss` == dropped must not have `transform`
"""
import sys
import os
import glob

import yaml

VALID_DIRECTION = {"both", "claude_to_codex", "codex_to_claude"}
VALID_LOSS = {"lossless", "lossy", "dropped"}


def main() -> int:
    mappings_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "mappings")
    files = sorted(glob.glob(os.path.join(mappings_dir, "*.yaml")))
    if not files:
        print(f"No mappings/*.yaml files found: {mappings_dir}", file=sys.stderr)
        return 2

    seen_ids: dict[str, str] = {}
    errors: list[str] = []
    total = 0

    for path in files:
        name = os.path.basename(path)
        try:
            with open(path, encoding="utf-8") as fp:
                doc = yaml.safe_load(fp)
        except Exception as exc:  # noqa: BLE001
            errors.append(f"{name}: YAML parse failed: {exc}")
            continue
        if not isinstance(doc, dict):
            errors.append(f"{name}: top-level is not a map")
            continue
        for entry in doc.get("entries", []) or []:
            total += 1
            eid = entry.get("id")
            if not eid:
                errors.append(f"{name}: entry missing id: {entry!r:.80}")
                continue
            if eid in seen_ids:
                errors.append(f"duplicate id: '{eid}' (in both {seen_ids[eid]} and {name})")
            seen_ids[eid] = name
            direction = entry.get("direction")
            loss = entry.get("loss")
            if direction not in VALID_DIRECTION:
                errors.append(f"{eid}: invalid direction: {direction!r}")
            if loss not in VALID_LOSS:
                errors.append(f"{eid}: invalid loss: {loss!r}")
            if entry.get("degrade") and loss != "lossy":
                errors.append(f"{eid}: has degrade but loss != lossy (loss={loss!r})")
            if loss == "dropped" and entry.get("transform"):
                errors.append(f"{eid}: loss=dropped but has transform: {entry.get('transform')!r}")

    if errors:
        print(f"✗ mappings validation FAILED: {len(errors)} error(s) ({total} entries / {len(files)} files)", file=sys.stderr)
        for err in errors:
            print(f"  - {err}", file=sys.stderr)
        return 1

    print(f"✓ mappings validation OK ({total} entries / {len(files)} files / unique IDs / invariants satisfied)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
