#!/usr/bin/env python3
"""Generate scout_large.jsonl: ~3000 Scout training examples from curated facts.

Reads training/datasets/_scout_facts.json (210 topics of verified, stable
facts) and emits varied examples in five types: explain, fact_check,
research_plan, summarize, compare. 14 examples per topic = 2940 rows.

Row schema matches training/datasets/sample.jsonl:
  {"messages": [{"role": "system", ...}, {"role": "user", ...},
                {"role": "assistant", ...}],
   "eval_keywords": [...], "type": "<one of the five>"}

Run: python training/make_scout_large_dataset.py
"""

import hashlib
import json
import random
import re
from pathlib import Path

random.seed(20260930)

ROOT = Path(__file__).resolve().parent
FACTS = json.load(open(ROOT / "datasets" / "_scout_facts.json"))["topics"]

SYSTEM = "You are Scout, the web research specialist at Botropolis."

ANGLES = {
    "cs": ("complexity analysis", "a worked example"),
    "databases": ("consistency guarantees", "operational costs"),
    "networking": ("failure modes", "debugging tools"),
    "security": ("the threat model", "common misconfigurations"),
    "ml": ("how it is evaluated", "typical failure cases"),
    "math": ("worked examples", "where the assumptions break"),
    "tech": ("adoption trade-offs", "migration costs"),
    "science": ("the key evidence", "what is still unknown"),
    "history": ("causes", "consequences"),
    "geography": ("scale", "human impact"),
}

GAPS = {
    "cs": "exact performance numbers, which depend on the workload; measure on yours.",
    "databases": "which product fits a given workload; that changes as products evolve.",
    "networking": "deployment-specific behavior; lab results and production differ.",
    "security": "the threat landscape, which shifts constantly; today's advice needs periodic review.",
    "ml": "which method wins on a new dataset; that is always empirical.",
    "math": "nothing here; the math is settled, only the applications are new.",
    "tech": "tooling specifics, which churn fast; check current docs.",
    "science": "active research frontiers; textbooks lag the frontier by years.",
    "history": "emphasis and interpretation, which historians still debate.",
    "geography": "exact measurements, which get refined; and human impacts, which change.",
}


def words(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def short(fact):
    s = fact.strip()
    if s:
        s = s[0].lower() + s[1:]
    return s.rstrip(".").rstrip(";")


def pick_facts(topic, start, n=3):
    facts = topic["facts"]
    return [facts[(start + i) % len(facts)] for i in range(n)]


EXPLAIN_PROMPTS = [
    "What is {t}?",
    "How does {t} work?",
    "Explain {t} in plain terms.",
    "What should I know about {t}?",
]
EXPLAIN_OPENERS = [
    "{T} is one of those ideas worth getting straight.",
    "Here is {t} in plain terms.",
    "Let's break {t} down.",
    "{T}: what it is and why it matters.",
]
EXPLAIN_CLOSERS = [
    "That is the stable core; the details vary by implementation.",
    "Keep that mental model and the edge cases will make sense when you meet them.",
    "The rest is practice more than theory.",
    "Once you see it in one real example, the abstraction clicks.",
]


def explain(topic, slot):
    t = topic["topic"]
    prompt = EXPLAIN_PROMPTS[slot].format(t=t)
    f1, f2, f3 = pick_facts(topic, slot * 2)
    opener = EXPLAIN_OPENERS[slot].format(t=t, T=t)
    closer = EXPLAIN_CLOSERS[(slot + len(t)) % len(EXPLAIN_CLOSERS)]
    response = f"{opener} {f1} {f2} {f3} {closer}"
    return prompt, opener, response


FACTCHECK_PROMPTS = [
    "Is it true that {f}?",
    "I heard that {f}. Is that right?",
    "Fact-check this for me: {f}",
]
FACTCHECK_VERDICTS = [
    "Confirmed: {F}",
    "Yes, that checks out: {F}",
    "That is accurate: {F}",
]
FACTCHECK_TAILS = [
    "This is settled material, not a contested claim. For anything you will cite, check a primary source rather than taking my word for it.",
    "No real dispute here; it is textbook-level established. Still, cite the primary source, not me.",
    "Widely confirmed and uncontroversial. If you need a citable reference, go to the docs or a textbook.",
]


def fact_check(topic, slot):
    facts = topic["facts"]
    fact = facts[slot % len(facts)]
    support = facts[(slot + 1) % len(facts)]
    prompt = FACTCHECK_PROMPTS[slot].format(f=short(fact))
    verdict = FACTCHECK_VERDICTS[slot].format(F=fact)
    tail = FACTCHECK_TAILS[slot]
    response = f"{verdict} {support} {tail}"
    return prompt, verdict, response


RESEARCH_PROMPTS = [
    "Research {t} for me.",
    "I need to get up to speed on {t}. Where do I start?",
]
RESEARCH_OPENERS = [
    "Break {t} into sub-questions first.",
    "To research {t}, start by splitting the question.",
]


def research_plan(topic, slot):
    t = topic["topic"]
    prompt = RESEARCH_PROMPTS[slot].format(t=t)
    f1, f2 = pick_facts(topic, slot, 2)
    a1, a2 = ANGLES[topic["category"]]
    opener = RESEARCH_OPENERS[slot].format(t=t)
    response = (
        f"{opener} 1. What is {t} and what problem does it solve? "
        f"2. How does it compare to the alternatives? "
        f"3. What mistakes do people commonly make with it? "
        f"Then verify the basics: {f1} {f2} "
        f"Cover these angles too: {a1} and {a2}. "
        f"Most of this is stable material, but note the date on any benchmarks "
        f"or vendor claims you find, since those move fast. "
        f"Prefer primary docs over tutorials."
    )
    return prompt, opener, response


SUMMARIZE_PROMPTS = [
    "Summarize what is known about {t}.",
    "Give me the key points on {t}.",
    "Brief me on {t}.",
]
SUMMARIZE_OPENERS = [
    "Key points on {t} first.",
    "Here is the current picture on {t}.",
    "The essentials of {t}:",
]


def summarize(topic, slot):
    t = topic["topic"]
    prompt = SUMMARIZE_PROMPTS[slot].format(t=t)
    f1, f2, f3 = pick_facts(topic, slot * 2 + 1)
    opener = SUMMARIZE_OPENERS[slot].format(t=t)
    gap = GAPS[topic["category"]]
    response = (
        f"{opener} {f1} {f2} {f3} "
        f"What remains open or debated: {gap} "
        f"Treat this as a starting map; for anything you will act on, "
        f"verify against a primary source."
    )
    return prompt, opener, response


COMPARE_PROMPTS = [
    "Compare {a} and {b}.",
    "What is the difference between {a} and {b}?",
]
COMPARE_OPENERS = [
    "{A} and {b}, side by side:",
    "Putting {a} next to {b}:",
]


def compare(topic, partner, slot):
    a, b = topic["topic"], partner["topic"]
    prompt = COMPARE_PROMPTS[slot].format(a=a.lower(), b=b.lower())
    af1, af2 = pick_facts(topic, slot, 2)
    bf1, bf2 = pick_facts(partner, slot + 1, 2)
    opener = COMPARE_OPENERS[slot].format(a=a.lower(), b=b.lower(), A=a)
    response = (
        f"{opener} {a}: {af1} {af2} {b}: {bf1} {bf2} "
        f"The core difference is {short(af1)} versus {short(bf1)}. "
        f"I will not declare a winner; the right pick depends on your constraints. "
        f"Vendor specifics change fast, so check current docs and date any "
        f"benchmark before trusting it."
    )
    return prompt, opener, response


def partners_by_category():
    by_cat = {}
    for t in FACTS:
        by_cat.setdefault(t["category"], []).append(t)
    pairs = {}
    for cat, ts in by_cat.items():
        n = len(ts)
        for i, t in enumerate(ts):
            pairs[t["topic"]] = ts[(i + 1) % n]
    return pairs


def main():
    partners = partners_by_category()
    rows = []
    seen_prompts = set()
    seen_openers = set()

    def add(prompt, opener, response, etype, topic):
        h = hashlib.sha256(prompt.strip().lower().encode()).hexdigest()
        assert h not in seen_prompts, f"duplicate prompt: {prompt}"
        seen_prompts.add(h)
        assert opener not in seen_openers, f"duplicate opener: {opener}"
        seen_openers.add(opener)
        kws = words(topic["topic"])[:2] + [etype]
        rows.append({
            "messages": [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": prompt},
                {"role": "assistant", "content": response},
            ],
            "eval_keywords": kws,
            "type": etype,
        })

    for topic in FACTS:
        for s in range(4):
            p, o, r = explain(topic, s)
            add(p, o, r, "explain", topic)
        for s in range(3):
            p, o, r = fact_check(topic, s)
            add(p, o, r, "fact_check", topic)
        for s in range(2):
            p, o, r = research_plan(topic, s)
            add(p, o, r, "research_plan", topic)
        for s in range(3):
            p, o, r = summarize(topic, s)
            add(p, o, r, "summarize", topic)
        for s in range(2):
            p, o, r = compare(topic, partners[topic["topic"]], s)
            add(p, o, r, "compare", topic)

    random.shuffle(rows)
    out = ROOT / "datasets" / "scout_large.jsonl"
    with open(out, "w") as fh:
        for row in rows:
            fh.write(json.dumps(row) + "\n")

    from collections import Counter
    print(f"wrote {len(rows)} rows to {out}")
    print("by type:", dict(Counter(r["type"] for r in rows)))
    print("unique prompts:", len(seen_prompts), "unique openers:", len(seen_openers))


if __name__ == "__main__":
    main()
