"""Tests for the shared company notebook tools."""

import pytest

from botropolis.tools import get_tool


@pytest.fixture
def notes_file(monkeypatch, tmp_path):
    """Point the notebook at a temp file via BOTROPOLIS_WORKSPACE."""
    monkeypatch.setenv("BOTROPOLIS_WORKSPACE", str(tmp_path))
    return tmp_path / "notes.md"


def test_notes_append_and_read_roundtrip(notes_file):
    append = get_tool("notes_append")
    result = append.execute(text="Manoj prefers morning standups.")
    assert result["noted"] is True

    read = get_tool("notes_read")
    out = read.execute()
    assert "Manoj prefers morning standups." in out["notes"]


def test_notes_read_empty_notebook(notes_file):
    out = get_tool("notes_read").execute()
    assert out["notes"] == ""
    assert "empty" in out["note"]


def test_notes_append_rejects_empty(notes_file):
    result = get_tool("notes_append").execute(text="   ")
    assert "error" in result


def test_notes_append_stamps_entries(notes_file):
    get_tool("notes_append").execute(text="first")
    get_tool("notes_append").execute(text="second")
    content = notes_file.read_text(encoding="utf-8")
    assert content.index("first") < content.index("second")
    assert "UTC" in content


def test_notes_tools_registered():
    assert get_tool("notes_append").name == "notes_append"
    assert get_tool("notes_read").name == "notes_read"
