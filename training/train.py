"""Training runner for Botropolis fine-tunes.

Validates a training config and its dataset, prints a concrete training
plan, and writes run_plan.json into the output directory. It does not run
gradient descent itself: there is no GPU trainer bundled here on purpose,
so this stays light and offline.

To plug in a real trainer, replace launch_trainer() with a call to your
trainer of choice (TRL SFTTrainer, Axolotl, Unsloth). Everything it needs
is already in the validated plan dict.

Usage:
    python training/train.py --config training/configs/lora_scout.yaml
    python training/train.py --config training/configs/lora_scout.yaml --dry-run
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List

import yaml

# Config keys we require. Keep this in sync with the example configs.
REQUIRED_TOP_LEVEL = ("model", "dataset", "training", "output")
REQUIRED_MODEL = ("base_model",)
REQUIRED_DATASET = ("path", "format")
REQUIRED_TRAINING = ("epochs", "learning_rate", "batch_size")
REQUIRED_OUTPUT = ("dir",)

VALID_FORMATS = ("chat",)


def repo_root() -> Path:
    """Repo root, so relative paths in configs resolve the same anywhere."""
    return Path(__file__).resolve().parents[1]


def load_config(config_path: Path) -> Dict[str, Any]:
    """Load a YAML training config."""
    with open(config_path, "r", encoding="utf-8") as fh:
        config = yaml.safe_load(fh)
    if not isinstance(config, dict):
        raise ValueError(f"{config_path} did not parse to a mapping")
    return config


def validate_config(config: Dict[str, Any]) -> List[str]:
    """Return a list of problems; empty means the config is valid."""
    problems: List[str] = []

    for section in REQUIRED_TOP_LEVEL:
        if section not in config:
            problems.append(f"missing top-level section: {section}")
    if problems:
        return problems

    # Each section must be a mapping with its required keys present.
    checks = (
        ("model", REQUIRED_MODEL),
        ("dataset", REQUIRED_DATASET),
        ("training", REQUIRED_TRAINING),
        ("output", REQUIRED_OUTPUT),
    )
    for section, required in checks:
        block = config.get(section)
        if not isinstance(block, dict):
            problems.append(f"section '{section}' must be a mapping")
            continue
        for key in required:
            if key not in block:
                problems.append(f"section '{section}' is missing key: {key}")

    dataset = config.get("dataset", {})
    if isinstance(dataset, dict) and dataset.get("format") not in VALID_FORMATS:
        problems.append(
            f"dataset.format must be one of {VALID_FORMATS}, got {dataset.get('format')!r}"
        )

    training = config.get("training", {})
    if isinstance(training, dict):
        epochs = training.get("epochs")
        if not isinstance(epochs, int) or epochs < 1:
            problems.append("training.epochs must be a positive integer")
        lr = training.get("learning_rate")
        if not isinstance(lr, (int, float)) or lr <= 0:
            problems.append("training.learning_rate must be a positive number")

    return problems


def validate_dataset(dataset_path: Path) -> Dict[str, Any]:
    """Check a JSONL chat dataset. Returns stats; raises on fatal problems."""
    if not dataset_path.exists():
        raise FileNotFoundError(f"dataset not found: {dataset_path}")
    rows = 0
    bad_rows: List[int] = []
    roles_seen = set()
    with open(dataset_path, "r", encoding="utf-8") as fh:
        for lineno, line in enumerate(fh, start=1):
            line = line.strip()
            if not line:
                continue
            rows += 1
            try:
                example = json.loads(line)
            except json.JSONDecodeError:
                bad_rows.append(lineno)
                continue
            messages = example.get("messages")
            if not isinstance(messages, list) or not messages:
                bad_rows.append(lineno)
                continue
            for msg in messages:
                if not isinstance(msg, dict) or "role" not in msg or "content" not in msg:
                    bad_rows.append(lineno)
                    break
                roles_seen.add(msg["role"])
    if rows == 0:
        raise ValueError(f"dataset is empty: {dataset_path}")
    if bad_rows:
        raise ValueError(f"dataset has malformed rows at lines: {bad_rows[:10]}")
    return {"rows": rows, "roles": sorted(roles_seen)}


def estimate_steps(rows: int, epochs: int, batch_size: int, grad_accum: int = 1) -> int:
    """Rough optimizer step count for the plan printout."""
    effective_batch = max(batch_size * grad_accum, 1)
    return max((rows * epochs) // effective_batch, 1)


def build_plan(config: Dict[str, Any], dataset_stats: Dict[str, Any]) -> Dict[str, Any]:
    """Assemble the validated, trainer-ready plan dict."""
    training = config["training"]
    grad_accum = training.get("gradient_accumulation_steps", 1)
    steps = estimate_steps(
        dataset_stats["rows"],
        training["epochs"],
        training["batch_size"],
        grad_accum,
    )
    peft = config.get("peft")
    return {
        "base_model": config["model"]["base_model"],
        "method": peft.get("method", "full") if isinstance(peft, dict) else "full",
        "peft": peft,
        "dataset": {
            "path": config["dataset"]["path"],
            "rows": dataset_stats["rows"],
            "eval_split": config["dataset"].get("eval_split", 0.1),
        },
        "epochs": training["epochs"],
        "learning_rate": training["learning_rate"],
        "effective_batch_size": training["batch_size"] * grad_accum,
        "estimated_steps": steps,
        "max_seq_length": training.get("max_seq_length", 2048),
        "seed": training.get("seed", 42),
        "output_dir": config["output"]["dir"],
    }


def launch_trainer(plan: Dict[str, Any]) -> None:
    """Hook point for a real trainer.

    Replace this body with your trainer call. Example with TRL:

        from trl import SFTTrainer, SFTConfig
        ...
    """
    # No trainer bundled; the plan below is what you feed yours.
    raise NotImplementedError(
        "No trainer bundled. Implement launch_trainer() with TRL, Axolotl, "
        "or Unsloth using the plan dict, or run --dry-run to just validate."
    )


def print_plan(plan: Dict[str, Any]) -> None:
    """Print a human readable summary of the training plan."""
    print("Training plan")
    print(f"  base model : {plan['base_model']}")
    print(f"  method     : {plan['method']}")
    print(
        f"  dataset    : {plan['dataset']['path']} "
        f"({plan['dataset']['rows']} rows, eval split {plan['dataset']['eval_split']})"
    )
    print(f"  epochs     : {plan['epochs']}")
    print(f"  learn rate : {plan['learning_rate']}")
    print(f"  eff. batch : {plan['effective_batch_size']}")
    print(f"  est. steps : {plan['estimated_steps']}")
    print(f"  output dir : {plan['output_dir']}")


def main(argv: List[str] | None = None) -> Dict[str, Any]:
    """CLI entry point. Returns the plan dict."""
    parser = argparse.ArgumentParser(description="Validate config and plan a fine-tune run.")
    parser.add_argument("--config", required=True, help="Path to a training config YAML")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Validate only; do not attempt to launch a trainer",
    )
    args = parser.parse_args(argv)

    root = repo_root()
    config_path = Path(args.config)
    if not config_path.is_absolute():
        config_path = root / config_path

    config = load_config(config_path)
    problems = validate_config(config)
    if problems:
        for problem in problems:
            print(f"config error: {problem}", file=sys.stderr)
        raise SystemExit(2)

    dataset_path = Path(config["dataset"]["path"])
    if not dataset_path.is_absolute():
        dataset_path = root / dataset_path
    dataset_stats = validate_dataset(dataset_path)

    plan = build_plan(config, dataset_stats)
    print_plan(plan)

    output_dir = root / plan["output_dir"]
    output_dir.mkdir(parents=True, exist_ok=True)
    plan_path = output_dir / "run_plan.json"
    with open(plan_path, "w", encoding="utf-8") as fh:
        json.dump(plan, fh, indent=2)
    print(f"wrote {plan_path}")

    if not args.dry_run:
        launch_trainer(plan)
    return plan


if __name__ == "__main__":
    main()
