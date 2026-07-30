"""Builtin tools shipped with Botropolis.

web_search is stub-backed on purpose: it defines the interface and returns
a placeholder so everything runs offline. Plug in a real search API (Tavily,
Brave, Serper) by replacing WebSearchTool.execute internals; the schema
stays the same.
"""

from __future__ import annotations

import ast
import operator
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Optional


class Tool:
    """Base class for every Botropolis tool."""

    name: str = "tool"
    description: str = ""
    parameters: Dict[str, Any] = {}

    def execute(self, **kwargs: Any) -> Any:
        """Run the tool. Subclasses must implement this."""
        raise NotImplementedError

    def schema(self) -> Dict[str, Any]:
        """Return the tool's public schema."""
        return {
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        }


class WebSearchTool(Tool):
    """Web search. Stub-backed; wire a real provider here."""

    name = "web_search"
    description = "Search the web for current information. Returns ranked results."
    parameters = {
        "query": {"type": "string", "description": "The search query", "required": True},
        "num_results": {"type": "integer", "description": "Max results", "default": 5},
    }

    def execute(self, query: str, num_results: int = 5) -> Dict[str, Any]:
        # TODO: plug in Tavily/Brave/Serper here using SEARCH_API_KEY.
        return {
            "query": query,
            "results": [],
            "note": (
                "stub: no search provider configured. Set SEARCH_API_KEY and "
                "implement the provider call in WebSearchTool.execute."
            ),
        }


# Allowed operators for the safe calculator.
_SAFE_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
    ast.USub: operator.neg,
    ast.UAdd: operator.pos,
}

_SAFE_FUNCS = {"abs": abs, "round": round, "min": min, "max": max}


def _eval_node(node: ast.AST) -> Any:
    if isinstance(node, ast.Expression):
        return _eval_node(node.body)
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
        return node.value
    if isinstance(node, ast.BinOp) and type(node.op) in _SAFE_OPS:
        return _SAFE_OPS[type(node.op)](_eval_node(node.left), _eval_node(node.right))
    if isinstance(node, ast.UnaryOp) and type(node.op) in _SAFE_OPS:
        return _SAFE_OPS[type(node.op)](_eval_node(node.operand))
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
        func = _SAFE_FUNCS.get(node.func.id)
        if func is None:
            raise ValueError(f"Function not allowed: {node.func.id}")
        return func(*(_eval_node(a) for a in node.args))
    raise ValueError(f"Expression not allowed: {ast.dump(node)}")


class CalculatorTool(Tool):
    """Safe arithmetic evaluator. No imports, no attribute access."""

    name = "calculator"
    description = "Evaluate an arithmetic expression safely."
    parameters = {
        "expression": {"type": "string", "description": "e.g. '2 + 3 * 4'", "required": True},
    }

    def execute(self, expression: str) -> Any:
        try:
            tree = ast.parse(expression, mode="eval")
        except SyntaxError as exc:
            raise ValueError(f"Invalid expression: {exc}") from exc
        return _eval_node(tree)


class ReadFileTool(Tool):
    """Read a text file."""

    name = "read_file"
    description = "Read a UTF-8 text file and return its contents."
    parameters = {
        "path": {"type": "string", "description": "File path to read", "required": True},
        "max_chars": {"type": "integer", "description": "Truncate after N chars", "default": 20000},
    }

    def execute(self, path: str, max_chars: int = 20000) -> str:
        text = Path(path).read_text(encoding="utf-8")
        return text[:max_chars]


class WriteFileTool(Tool):
    """Write a text file, creating parent directories."""

    name = "write_file"
    description = "Write text to a file, creating parent directories as needed."
    parameters = {
        "path": {"type": "string", "description": "File path to write", "required": True},
        "content": {"type": "string", "description": "Text to write", "required": True},
    }

    def execute(self, path: str, content: str) -> Dict[str, Any]:
        target = Path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        return {"path": str(target), "bytes": len(content.encode("utf-8"))}


class CurrentTimeTool(Tool):
    """Current UTC time."""

    name = "current_time"
    description = "Return the current UTC time as an ISO 8601 string."
    parameters = {}

    def execute(self) -> str:
        return datetime.now(timezone.utc).isoformat()


TOOLS: Dict[str, Tool] = {
    "web_search": WebSearchTool(),
    "calculator": CalculatorTool(),
    "read_file": ReadFileTool(),
    "write_file": WriteFileTool(),
    "current_time": CurrentTimeTool(),
}


def get_tool(name: str) -> Tool:
    """Return the builtin tool with this name."""
    tool = TOOLS.get(name)
    if tool is None:
        raise KeyError(f"Unknown tool: {name}")
    return tool
