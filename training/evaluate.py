"""Small eval harness for Botropolis fine-tunes.

Runs each dataset row through a model (or the offline stub when no API key
is set) and checks whether the expected keywords show up in the response.
Prints an accuracy style report. Runs anywhere, no GPU needed.

Note on the sample dataset: its eval_keywords are topic words from the
prompt, because the offline stub restates the task. That exercises the full
pipeline with no API key. A real eval set for a trained model should use
substantive content keywords from the reference answers instead.

Usage:
    python training/evaluate.py --dataset training/datasets/sample.jsonl
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from botropolis.core.models import ModelClient  # noqa: E402


def load_dataset(path: Path) -> List[Dict[str, Any]]:
    rows = []
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def last_user_message(example: Dict[str, Any]) -> str:
    for msg in reversed(example.get("messages", [])):
        if msg.get("role") == "user":
            return msg.get("content", "")
    return ""


def score_response(response: str, keywords: List[str]) -> Dict[str, Any]:
    lowered = response.lower()
    hits = [kw for kw in keywords if kw.lower() in lowered]
    score = len(hits) / len(keywords) if keywords else 1.0
    return {"score": score, "hits": hits, "missed": [k for k in keywords if k not in hits]}


def run_eval(
    dataset_path: Path,
    model_id: str = "stub",
    max_examples: int | None = None,
) -> Dict[str, Any]:
    examples = load_dataset(dataset_path)
    if max_examples:
        examples = examples[:max_examples]
    client = ModelClient()
    per_example = []
    for i, example in enumerate(examples):
        prompt = last_user_message(example)
        response = client.chat(model_id, [{"role": "user", "content": prompt}])
        keywords = example.get("eval_keywords", [])
        result = score_response(response, keywords)
        result["index"] = i
        per_example.append(result)
    avg = sum(r["score"] for r in per_example) / len(per_example) if per_example else 0.0
    passed = sum(1 for r in per_example if r["score"] == 1.0)
    return {
        "model": model_id,
        "total": len(per_example),
        "passed": passed,
        "avg_score": round(avg, 3),
        "examples": per_example,
    }


def print_report(report: Dict[str, Any]) -> None:
    print(f"Eval report: model={report['model']}")
    for ex in report["examples"]:
        print(f"  ex {ex['index']}: score={ex['score']:.2f} missed={ex['missed']}")
    print(f"total={report['total']} passed={report['passed']} avg_score={report['avg_score']}")


def main(argv: List[str] | None = None) -> Dict[str, Any]:
    parser = argparse.ArgumentParser(description="Evaluate a model on a keyword dataset.")
    parser.add_argument("--dataset", required=True, help="Path to eval JSONL")
    parser.add_argument("--model", default="stub", help="Model id from models/registry.yaml")
    parser.add_argument("--max-examples", type=int, default=None)
    args = parser.parse_args(argv)

    root = Path(__file__).resolve().parents[1]
    dataset_path = Path(args.dataset)
    if not dataset_path.is_absolute():
        dataset_path = root / dataset_path
    if not dataset_path.exists():
        raise SystemExit(f"dataset not found: {dataset_path}")

    report = run_eval(dataset_path, model_id=args.model, max_examples=args.max_examples)
    print_report(report)
    return report


if __name__ == "__main__":
    main()
