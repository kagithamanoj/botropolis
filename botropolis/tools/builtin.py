"""Builtin tools shipped with Botropolis.

Each tool has a name, a description, a JSON-schema-style parameter map,
and an execute() method. Agents declare which tools they may actually
call in the `toolkit` field of their spec YAML.

File and shell tools are sandboxed: see botropolis/tools/sandbox.py.
web_search uses DuckDuckGo with no API key and degrades to an empty
result set (never an exception) when the network is unavailable.
"""

from __future__ import annotations

import ast
import html
import operator
import re
import subprocess
import urllib.parse
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

import requests

from botropolis.tools.sandbox import (
    check_shell_command,
    resolve_workspace_path,
    workspace_relative,
    workspace_root,
)

_BROWSER_UA = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
    "AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/126.0 Safari/537.36"
)

# Cap raw download size so one huge page cannot eat memory.
_MAX_FETCH_BYTES = 2_000_000


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


def _strip_tags(fragment: str) -> str:
    """Remove HTML tags and collapse whitespace."""
    text = re.sub(r"<[^>]+>", "", fragment)
    text = html.unescape(text)
    return re.sub(r"\s+", " ", text).strip()


def _ddg_real_url(href: str) -> str:
    """Unwrap DuckDuckGo's redirect link to the real destination URL."""
    match = re.search(r"uddg=([^&]+)", href)
    if match:
        return urllib.parse.unquote(match.group(1))
    if href.startswith("//"):
        return "https:" + href
    return href


class WebSearchTool(Tool):
    """Web search via DuckDuckGo. No API key needed."""

    name = "web_search"
    description = (
        "Search the web for current information. "
        "Returns ranked results with titles, urls, and snippets."
    )
    parameters = {
        "query": {"type": "string", "description": "The search query", "required": True},
        "num_results": {"type": "integer", "description": "Max results", "default": 5},
    }

    def execute(self, query: str, num_results: int = 5) -> Dict[str, Any]:
        try:
            resp = requests.get(
                "https://html.duckduckgo.com/html/",
                params={"q": query},
                headers={"User-Agent": _BROWSER_UA},
                timeout=15,
            )
            resp.raise_for_status()
        except requests.RequestException as exc:
            # Offline or blocked: report it, don't blow up the agent loop.
            return {"query": query, "results": [], "error": f"search failed: {exc}"}
        links = re.findall(
            r'<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>',
            resp.text,
            re.DOTALL,
        )
        snippets = re.findall(
            r'<a[^>]*class="result__snippet"[^>]*>(.*?)</a>', resp.text, re.DOTALL
        )
        results = []
        for (href, title_html), snippet_html in zip(links, snippets):
            results.append(
                {
                    "title": _strip_tags(title_html),
                    "url": _ddg_real_url(href),
                    "snippet": _strip_tags(snippet_html),
                }
            )
            if len(results) >= max(1, num_results):
                break
        return {"query": query, "results": results}


class WebFetchTool(Tool):
    """Fetch a web page and return readable text."""

    name = "web_fetch"
    description = (
        "Fetch a URL and return its readable text with HTML stripped. "
        "Use after web_search to read a promising result in full."
    )
    parameters = {
        "url": {"type": "string", "description": "The URL to fetch", "required": True},
        "max_chars": {
            "type": "integer",
            "description": "Truncate text after N chars",
            "default": 8000,
        },
    }

    def execute(self, url: str, max_chars: int = 8000) -> Dict[str, Any]:
        try:
            resp = requests.get(
                url,
                headers={"User-Agent": _BROWSER_UA},
                timeout=20,
                stream=True,
            )
            resp.raise_for_status()
        except requests.RequestException as exc:
            return {"url": url, "text": "", "error": f"fetch failed: {exc}"}
        content_type = resp.headers.get("content-type", "")
        if "html" not in content_type and "text" not in content_type:
            return {"url": url, "text": "", "error": f"not a text page: {content_type}"}
        chunks: List[str] = []
        size = 0
        try:
            for chunk in resp.iter_content(65536, decode_unicode=True):
                if not chunk:
                    continue
                chunks.append(chunk)
                size += len(chunk)
                if size > _MAX_FETCH_BYTES:
                    break
        finally:
            resp.close()
        page = "".join(chunks)
        page = re.sub(r"<script.*?</script>", " ", page, flags=re.DOTALL | re.IGNORECASE)
        page = re.sub(r"<style.*?</style>", " ", page, flags=re.DOTALL | re.IGNORECASE)
        text = _strip_tags(page)
        truncated = len(text) > max_chars
        return {"url": url, "text": text[:max_chars], "truncated": truncated}


class ShellTool(Tool):
    """Run a shell command inside the agent workspace."""

    name = "shell"
    description = (
        "Run a shell command inside the agent workspace directory and "
        "capture stdout/stderr. Destructive commands are refused."
    )
    parameters = {
        "command": {"type": "string", "description": "Shell command to run", "required": True},
        "timeout": {
            "type": "integer",
            "description": "Kill the command after N seconds",
            "default": 30,
        },
    }

    def execute(self, command: str, timeout: int = 30) -> Dict[str, Any]:
        check_shell_command(command)
        try:
            proc = subprocess.run(
                command,
                shell=True,
                cwd=str(workspace_root()),
                capture_output=True,
                text=True,
                timeout=max(1, min(int(timeout), 60)),
            )
        except subprocess.TimeoutExpired as exc:
            return {
                "returncode": None,
                "stdout": (exc.stdout or "")[-4000:],
                "stderr": (exc.stderr or "")[-4000:],
                "error": f"timed out after {timeout}s",
            }
        return {
            "returncode": proc.returncode,
            "stdout": proc.stdout[-8000:],
            "stderr": proc.stderr[-4000:],
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
    """Read a text file inside the agent workspace."""

    name = "read_file"
    description = "Read a UTF-8 text file inside the agent workspace."
    parameters = {
        "path": {
            "type": "string",
            "description": "Path inside the workspace, e.g. 'notes/todo.txt'",
            "required": True,
        },
        "max_chars": {"type": "integer", "description": "Truncate after N chars", "default": 20000},
    }

    def execute(self, path: str, max_chars: int = 20000) -> str:
        target = resolve_workspace_path(path)
        if not target.is_file():
            raise FileNotFoundError(f"No such file in workspace: {path}")
        return target.read_text(encoding="utf-8")[:max_chars]


class WriteFileTool(Tool):
    """Write a text file inside the agent workspace."""

    name = "write_file"
    description = "Write text to a file inside the agent workspace, creating parents."
    parameters = {
        "path": {
            "type": "string",
            "description": "Path inside the workspace, e.g. 'notes/todo.txt'",
            "required": True,
        },
        "content": {"type": "string", "description": "Text to write", "required": True},
    }

    def execute(self, path: str, content: str) -> Dict[str, Any]:
        target = resolve_workspace_path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        return {
            "path": workspace_relative(target),
            "bytes": len(content.encode("utf-8")),
        }


class ListDirTool(Tool):
    """List a directory inside the agent workspace."""

    name = "list_dir"
    description = "List entries of a directory inside the agent workspace."
    parameters = {
        "path": {
            "type": "string",
            "description": "Directory inside the workspace",
            "default": ".",
        },
    }

    def execute(self, path: str = ".") -> Dict[str, Any]:
        target = resolve_workspace_path(path)
        if not target.exists():
            raise FileNotFoundError(f"No such path in workspace: {path}")
        if not target.is_dir():
            raise ValueError(f"Not a directory: {path}")
        entries = []
        for child in sorted(target.iterdir()):
            entries.append(child.name + ("/" if child.is_dir() else ""))
        return {"path": workspace_relative(target), "entries": entries}


class CurrentTimeTool(Tool):
    """Current UTC time."""

    name = "current_time"
    description = "Return the current UTC time as an ISO 8601 string."
    parameters = {}

    def execute(self) -> str:
        return datetime.now(timezone.utc).isoformat()


TOOLS: Dict[str, Tool] = {
    "web_search": WebSearchTool(),
    "web_fetch": WebFetchTool(),
    "shell": ShellTool(),
    "calculator": CalculatorTool(),
    "read_file": ReadFileTool(),
    "write_file": WriteFileTool(),
    "list_dir": ListDirTool(),
    "current_time": CurrentTimeTool(),
}


def get_tool(name: str) -> Tool:
    """Return the builtin tool with this name."""
    tool = TOOLS.get(name)
    if tool is None:
        raise KeyError(f"Unknown tool: {name}")
    return tool
