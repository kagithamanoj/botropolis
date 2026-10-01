#!/usr/bin/env python3
"""Validate training/datasets/scout_large.jsonl.

Checks: every line parses as JSON, schema fields present, roles in order
(system/user/assistant), non-empty prompt and response, eval_keywords sane,
no duplicate prompts. Prints stats: total rows, per-type counts, average
prompt/response length in chars and words.

Run: python training/validate_dataset.py
Exits non-zero on any failure.
"""

import json
import sys
from collections import Counter
from pathlib import Path

PATH = Path(__file__).resolve().parent / "datasets" / "scout_large.jsonl"
TYPES = {"explain", "fact_check", "research_plan", "summarize", "compare"}

errors = []


def fail(msg):
    errors.append(msg)


def main():
    lines = PATH.read_text().splitlines()
    rows = []
    for i, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            rows.append((i, json.loads(line)))
        except json.JSONDecodeError as exc:
            fail(f"line {i}: invalid JSON: {exc}")

    seen = set()
    n_prompt_chars, n_resp_chars = 0, 0
    n_prompt_words, n_resp_words = 0, 0
    type_counts = Counter()

    for i, row in rows:
        msgs = row.get("messages")
        if not isinstance(msgs, list) or len(msgs) != 3:
            fail(f"line {i}: messages must be a list of 3")
            continue
        roles = [m.get("role") for m in msgs]
        if roles != ["system", "user", "assistant"]:
            fail(f"line {i}: roles out of order: {roles}")
        prompt = msgs[1].get("content", "")
        resp = msgs[2].get("content", "")
        if not isinstance(prompt, str) or not prompt.strip():
            fail(f"line {i}: empty user prompt")
        if not isinstance(resp, str) or not resp.strip():
            fail(f"line {i}: empty assistant response")
        key = prompt.strip().lower()
        if key in seen:
            fail(f"line {i}: duplicate prompt: {prompt[:60]}")
        seen.add(key)

        kws = row.get("eval_keywords")
        if not isinstance(kws, list) or not kws or not all(
                isinstance(k, str) and k.strip() for k in kws):
            fail(f"line {i}: eval_keywords must be a non-empty string list")

        etype = row.get("type")
        if etype not in TYPES:
            fail(f"line {i}: bad type: {etype!r}")
        else:
            type_counts[etype] += 1

        n_prompt_chars += len(prompt)
        n_resp_chars += len(resp)
        n_prompt_words += len(prompt.split())
        n_resp_words += len(resp.split())

    n = len(rows)
    print(f"rows: {n}")
    print("by type:", dict(sorted(type_counts.items())))
    if n:
        print(f"avg prompt: {n_prompt_chars / n:.0f} chars, "
              f"{n_prompt_words / n:.0f} words")
        print(f"avg response: {n_resp_chars / n:.0f} chars, "
              f"{n_resp_words / n:.0f} words")
        print(f"unique prompts: {len(seen)}")

    if errors:
        print(f"\n{len(errors)} ERRORS:")
        for e in errors[:20]:
            print(" -", e)
        sys.exit(1)
    print("\nOK: dataset is clean")


if __name__ == "__main__":
    main()
