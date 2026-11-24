"""The architecture diagrams must match the code they describe.

docs/architecture.drawio is the picture version of docs/architecture.md.
When the system gains a subsystem, the diagram has to gain it too, and
the agent names and toolkit marks have to match the YAML specs.
"""

import html
import re
import xml.etree.ElementTree as ET
from pathlib import Path

import yaml

ROOT = Path(__file__).parent.parent
DRAWIO = ROOT / "docs" / "architecture.drawio"
AGENTS_DIR = ROOT / "botropolis" / "agents"

EXPECTED_PAGES = [
    "System overview",
    "Request lifecycle",
    "Teaming",
    "Training pipeline",
]

# Words the diagram must contain now that the harness, memory,
# consult, jobs, and eval work has landed.
REQUIRED_TERMS = [
    "Harness",
    "Company memory",
    "contract",
    "consult",
    "ledger",
    "evidence",
    "approval",
    "resume",
    "PLAN",
    "8000",
    "40 chat turns",
    "48,000",
    "memory_save",
    "memory_recall",
    "qwen2.5:1.5b",
    "cloud architect",
]


def diagram_text():
    tree = ET.parse(DRAWIO)
    pages = []
    text = []
    for diagram in tree.getroot():
        pages.append(diagram.get("name"))
        root = diagram.find("mxGraphModel").find("root")
        for cell in root:
            value = cell.get("value")
            if value:
                plain = re.sub(r"<[^>]+>", " ", html.unescape(value))
                text.append(re.sub(r"\s+", " ", plain).strip())
    return pages, "\n".join(text)


def test_drawio_parses_and_has_four_pages():
    pages, _ = diagram_text()
    assert pages == EXPECTED_PAGES


def test_drawio_cell_ids_unique_per_page():
    tree = ET.parse(DRAWIO)
    for diagram in tree.getroot():
        ids = [
            cell.get("id")
            for cell in diagram.find("mxGraphModel").find("root")
        ]
        assert len(ids) == len(set(ids)), diagram.get("name")


def test_drawio_covers_current_subsystems():
    _, text = diagram_text()
    lowered = text.lower()
    for term in REQUIRED_TERMS:
        assert term.lower() in lowered, f"diagram is missing {term!r}"


def agent_specs():
    specs = {}
    for path in AGENTS_DIR.glob("*/*.yaml"):
        spec = yaml.safe_load(path.read_text())
        specs[spec["name"]] = spec
    return specs


def test_diagram_agent_names_match_specs():
    _, text = diagram_text()
    specs = agent_specs()
    assert len(specs) == 20
    lowered = text.lower()
    for name in specs:
        assert name.lower() in lowered, name


def test_diagram_toolkit_marks_match_specs():
    # A "*" after an agent name on the System overview page means the
    # agent has a toolkit in its spec. The marks must match the specs.
    tree = ET.parse(DRAWIO)
    overview = next(
        d for d in tree.getroot() if d.get("name") == "System overview"
    )
    root = overview.find("mxGraphModel").find("root")
    specs = agent_specs()
    marked = set()
    unmarked = set()
    for cell in root:
        value = html.unescape(cell.get("value") or "")
        if not any(f"<b>{dept}</b>" in value for dept in
                   ("research", "health", "finance", "code", "data", "legal",
                    "marketing", "ops", "security", "support")):
            continue
        plain = re.sub(r"<br\s*/?>", "\n", value)
        plain = re.sub(r"<[^>]+>", "", plain)
        for line in plain.split("\n"):
                line = line.strip()
                if ":" not in line:
                    continue
                name = line.split(":")[0].strip().lower()
                if name in specs:
                    (marked if "*" in line else unmarked).add(name)
    assert marked | unmarked == set(specs), "diagram does not list all 20 agents"
    for name in marked:
        assert specs[name].get("toolkit"), f"{name} is starred but has no toolkit"
    for name in unmarked:
        assert not specs[name].get("toolkit"), f"{name} has a toolkit but no star"
