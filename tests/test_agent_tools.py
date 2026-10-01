"""Agent tool tests: registry, sandbox, and the think-act-observe loop."""

import pytest
import yaml

from botropolis.core.agent import Agent
from botropolis.core.loop import DEFAULT_MAX_STEPS, parse_model_reply
from botropolis.tools import TOOLS, get_tool


class FakeClient:
    """Scripted stand-in for ModelClient. Replies play in order."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


def make_agent(tmp_path, toolkit=None, max_steps=None, replies=None):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
    }
    if toolkit is not None:
        spec["toolkit"] = toolkit
    if max_steps is not None:
        spec["max_steps"] = max_steps
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=FakeClient(replies or ["FINAL: done"]))


def test_registry_tools_have_valid_schemas():
    assert len(TOOLS) >= 6
    for name, tool in TOOLS.items():
        assert tool.name == name
        assert isinstance(tool.description, str) and tool.description
        assert isinstance(tool.parameters, dict)


def test_unknown_tool_in_toolkit_raises_at_load(tmp_path):
    with pytest.raises(ValueError, match="unknown tools"):
        make_agent(tmp_path, toolkit=["teleporter"])


def test_toolkit_defaults_to_empty_and_single_shot(tmp_path):
    agent = make_agent(tmp_path, replies=["FINAL: plain answer"])
    assert agent.toolkit == []
    result = agent.run("do a thing")
    assert result.success
    # Same answer semantics as the tool loop: the FINAL: marker is stripped.
    assert result.output == "plain answer"
    assert result.tool_calls == []


def test_react_loop_runs_tools_in_order(tmp_path):
    replies = [
        'ACTION: {"tool": "calculator", "args": {"expression": "2 + 3"}}',
        'ACTION: {"tool": "calculator", "args": {"expression": "5 * 2"}}',
        "FINAL: The answer is 10",
    ]
    agent = make_agent(tmp_path, toolkit=["calculator"], replies=replies)
    result = agent.run("multiply two plus three by two")
    assert result.success
    assert result.output == "The answer is 10"
    assert [c.tool for c in result.tool_calls] == ["calculator", "calculator"]
    assert all(c.success for c in result.tool_calls)
    assert "5" in result.tool_calls[0].result_preview
    assert agent.client.calls == 3


def test_max_steps_stops_runaway_loop(tmp_path):
    agent = make_agent(
        tmp_path,
        toolkit=["calculator"],
        max_steps=3,
        replies=['ACTION: {"tool": "calculator", "args": {"expression": "1+1"}}'],
    )
    result = agent.run("loop forever")
    assert len(result.tool_calls) == 3
    assert agent.client.calls == 3
    assert result.success


def test_default_max_steps_is_sane():
    assert DEFAULT_MAX_STEPS == 8


def test_malformed_action_is_handled_gracefully(tmp_path):
    replies = [
        "ACTION: {this is not json",
        "FINAL: recovered",
    ]
    agent = make_agent(tmp_path, toolkit=["calculator"], replies=replies)
    result = agent.run("broken action first")
    assert result.success
    assert result.output == "recovered"
    assert result.tool_calls == []


def test_unknown_tool_action_becomes_failed_call(tmp_path):
    replies = [
        'ACTION: {"tool": "teleporter", "args": {}}',
        "FINAL: moving on",
    ]
    agent = make_agent(tmp_path, toolkit=["calculator"], replies=replies)
    result = agent.run("use a wrong tool")
    assert result.success
    assert len(result.tool_calls) == 1
    assert result.tool_calls[0].success is False
    assert "Unknown tool" in result.tool_calls[0].result_preview


def test_parse_model_reply_variants():
    kind, payload = parse_model_reply("FINAL: hello there")
    assert (kind, payload) == ("final", "hello there")
    kind, payload = parse_model_reply('ACTION: {"tool": "shell", "args": {"command": "ls"}}')
    assert kind == "action"
    assert payload == ("shell", {"command": "ls"})
    kind, _ = parse_model_reply("ACTION: nope")
    assert kind == "malformed"
    kind, payload = parse_model_reply("Just a plain answer.")
    assert (kind, payload) == ("final", "Just a plain answer.")


def test_read_file_cannot_escape_workspace(tmp_path, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_WORKSPACE", str(tmp_path / "ws"))
    tool = get_tool("read_file")
    with pytest.raises(ValueError, match="escapes the workspace"):
        tool.execute(path="../../etc/passwd")
    with pytest.raises(ValueError, match="escapes the workspace"):
        tool.execute(path="/etc/passwd")


def test_write_then_read_roundtrip_in_workspace(tmp_path, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_WORKSPACE", str(tmp_path / "ws"))
    get_tool("write_file").execute(path="notes/todo.txt", content="buy milk")
    assert get_tool("read_file").execute(path="notes/todo.txt") == "buy milk"
    listing = get_tool("list_dir").execute(path="notes")
    assert "todo.txt" in listing["entries"]


def test_shell_blocks_destructive_commands():
    tool = get_tool("shell")
    with pytest.raises(ValueError, match="Blocked"):
        tool.execute(command="rm -rf /")
    with pytest.raises(ValueError, match="Blocked"):
        tool.execute(command="rm -r ~")
    with pytest.raises(ValueError, match="Blocked"):
        tool.execute(command="mkfs.ext4 /dev/sda1")


def test_shell_runs_inside_workspace(tmp_path, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_WORKSPACE", str(tmp_path / "ws"))
    tool = get_tool("shell")
    out = tool.execute(command="pwd")
    assert out["returncode"] == 0
    assert str(tmp_path / "ws") in out["stdout"]


def test_builtin_agents_with_toolkits_load():
    from botropolis.core.registry import AgentRegistry

    registry = AgentRegistry()
    assert registry.get("Ethan").toolkit == ["web_search", "web_fetch"]
    assert "shell" in registry.get("Liam").toolkit
    assert registry.get("Tara").toolkit == []
