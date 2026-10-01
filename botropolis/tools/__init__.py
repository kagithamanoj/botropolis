"""Botropolis builtin tools.

Tools are the hands of the agents. Each tool has a name, a description, a
parameter schema, and an execute() method. Agents declare which tools they
may actually call in the `toolkit` field of their spec YAML; the agent
runner executes them through a think-act-observe loop.

File and shell tools are sandboxed to the agent workspace. See
botropolis/tools/sandbox.py for the safety model.
"""

from botropolis.tools.builtin import (
    TOOLS,
    CalculatorTool,
    CurrentTimeTool,
    ListDirTool,
    ReadFileTool,
    ShellTool,
    Tool,
    WebFetchTool,
    WebSearchTool,
    WriteFileTool,
    get_tool,
)

__all__ = [
    "TOOLS",
    "Tool",
    "CalculatorTool",
    "CurrentTimeTool",
    "ListDirTool",
    "ReadFileTool",
    "ShellTool",
    "WebFetchTool",
    "WebSearchTool",
    "WriteFileTool",
    "get_tool",
]
