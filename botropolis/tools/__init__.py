"""Botropolis builtin tools.

Tools are the hands of the agents. Each tool has a name, a description, a
parameter schema, and an execute() method. Agents declare which tools they
may use in their spec.yaml; the CEO can wire execution later.
"""

from botropolis.tools.builtin import (
    TOOLS,
    CalculatorTool,
    CurrentTimeTool,
    ReadFileTool,
    Tool,
    WebSearchTool,
    WriteFileTool,
    get_tool,
)

__all__ = [
    "TOOLS",
    "Tool",
    "CalculatorTool",
    "CurrentTimeTool",
    "ReadFileTool",
    "WebSearchTool",
    "WriteFileTool",
    "get_tool",
]
