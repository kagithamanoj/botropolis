"""Training tests: config validation and the eval harness run offline."""

from pathlib import Path

import train
import evaluate

ROOT = Path(__file__).resolve().parents[1]


def test_lora_config_validates():
    config = train.load_config(ROOT / "training" / "configs" / "lora_scout.yaml")
    assert train.validate_config(config) == []


def test_sft_config_validates():
    config = train.load_config(ROOT / "training" / "configs" / "sft_base.yaml")
    assert train.validate_config(config) == []


def test_config_missing_section_flagged():
    assert train.validate_config({"model": {}}) != []


def test_sample_dataset_validates():
    stats = train.validate_dataset(ROOT / "training" / "datasets" / "sample.jsonl")
    assert stats["rows"] == 6
    assert "user" in stats["roles"] and "assistant" in stats["roles"]


def test_train_dry_run_writes_plan(tmp_path, monkeypatch):
    # Keep the test from writing into the real checkpoints dir.
    monkeypatch.chdir(tmp_path)
    plan = train.main(
        ["--config", str(ROOT / "training" / "configs" / "lora_scout.yaml"), "--dry-run"]
    )
    assert plan["estimated_steps"] > 0
    assert plan["method"] == "lora"


def test_evaluate_runs_offline_on_sample():
    report = evaluate.run_eval(ROOT / "training" / "datasets" / "sample.jsonl")
    assert report["total"] == 6
    assert report["passed"] == 6
    assert report["avg_score"] == 1.0
