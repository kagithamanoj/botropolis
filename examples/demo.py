"""End-to-end Botropolis demo. Runs fully offline.

Builds the CEO, asks two sample questions (one research, one code), and
prints the company report for each. With no API keys set, agents answer
with structured stub reasoning; set OPENAI_API_KEY, ANTHROPIC_API_KEY, or
GOOGLE_API_KEY (or run Ollama) for real model output.

Usage:
    python examples/demo.py
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry


def print_report(report) -> None:
    print("=" * 70)
    print(f"REQUEST: {report.request}")
    print(f"departments: {', '.join(report.departments_involved)}")
    print(f"elapsed: {report.elapsed_seconds:.2f}s")
    print("-" * 70)
    for result in report.results:
        print(f"[{result.department} / {result.agent_name}]")
        print(result.output)
        print("-" * 70)
    print("CEO SUMMARY:")
    print(report.summary)
    print("=" * 70)
    print()


def main() -> None:
    registry = AgentRegistry()
    print(f"Botropolis is open for business: {len(registry)} agents, "
          f"{len(registry.departments)} departments.\n")

    ceo = CEO(registry)

    questions = [
        "What are the key trends in agentic AI frameworks in 2026?",
        "Write a Python function that parses a CSV file into a list of dicts.",
    ]
    for question in questions:
        report = ceo.handle(question)
        print_report(report)


if __name__ == "__main__":
    main()
