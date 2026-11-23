"""Long-term memory tools: memory_save and memory_recall.

The notebook is a page of notes the whole company shares. These tools
are the searchable layer under it: individual facts, saved by the
agent that learned them, that any later run can pull back by keyword.
The store is botropolis/core/memory.py; the tools resolve the shared
store at call time, so the server, the CLI, and tests can pin their
own instance with set_memory_store().

memory_save is agent-aware: the tool loop passes the calling agent's
name in at dispatch (see execute_tool_call in botropolis/core/loop.py),
so a memory always records who learned it. The model never supplies
the name itself.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional, Union

from botropolis.core.memory import get_memory_store
from botropolis.tools.builtin import Tool


def _tag_list(tags: Optional[Union[str, List[str]]]) -> List[str]:
    """Accept tags as a list or a comma-separated string."""
    if tags is None:
        return []
    if isinstance(tags, str):
        return [part.strip() for part in tags.split(",") if part.strip()]
    return [str(tag).strip() for tag in tags if str(tag).strip()]


class MemorySaveTool(Tool):
    """Save one durable fact to the company memory."""

    name = "memory_save"
    description = (
        "Save one durable fact to the company memory so future tasks "
        "can find it: a decision, a person's name, a number, a "
        "preference, or a fact you learned during this task. Save "
        "facts as you learn them, one fact per call, in plain words "
        "a teammate could search for later. importance runs 1 to 5: "
        "5 for things that must not be forgotten (names, commitments, "
        "hard numbers), 1 for minor details."
    )
    parameters = {
        "fact": {
            "type": "string",
            "description": "The fact to remember, one or two sentences",
            "required": True,
        },
        "importance": {
            "type": "integer",
            "description": "1 (minor) to 5 (must not forget)",
            "default": 3,
        },
        "tags": {
            "type": "string",
            "description": "Comma-separated search tags, e.g. 'billing, acme'",
            "default": "",
        },
    }

    # The loop fills `agent` in at dispatch; it is not a model argument.
    agent_aware = True

    def execute(
        self,
        fact: str,
        importance: int = 3,
        tags: Optional[Union[str, List[str]]] = None,
        agent: str = "",
    ) -> Dict[str, Any]:
        fact = (fact or "").strip()
        if not fact:
            return {"error": "Nothing to save: fact was empty."}
        try:
            importance = int(importance)
        except (TypeError, ValueError):
            return {"error": f"importance must be a number 1 to 5, got {importance!r}."}
        memory = get_memory_store().save(
            fact,
            agent=agent,
            importance=importance,
            tags=_tag_list(tags),
        )
        return {"saved": True, "memory": memory.to_dict()}


class MemoryRecallTool(Tool):
    """Search the company memory for facts from earlier work."""

    name = "memory_recall"
    description = (
        "Search the company memory for facts saved during earlier "
        "work. Call this before answering anything about past work, "
        "earlier decisions, or things the company already learned, "
        "instead of guessing. Returns the best matches with who saved "
        "them and how important they were marked."
    )
    parameters = {
        "query": {
            "type": "string",
            "description": "Words describing the fact you need",
            "required": True,
        },
        "limit": {
            "type": "integer",
            "description": "Max memories to return",
            "default": 5,
        },
    }

    def execute(self, query: str, limit: int = 5) -> Dict[str, Any]:
        query = (query or "").strip()
        if not query:
            return {"error": "Nothing to search for: query was empty."}
        try:
            limit = int(limit)
        except (TypeError, ValueError):
            return {"error": f"limit must be a number, got {limit!r}."}
        found = get_memory_store().recall(query, limit=max(1, min(limit, 20)))
        return {
            "query": query,
            "count": len(found),
            "memories": [memory.to_dict() for memory in found],
        }
