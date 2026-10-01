"""Tests for the botropolis command-line interface."""

import pytest

from botropolis.cli import build_parser, main


def test_roster_lists_all_twenty(capsys):
    assert main(["roster"]) == 0
    out = capsys.readouterr().out
    assert "Liam" in out
    assert "Anya" in out
    assert len(out.strip().splitlines()) == 20


def test_roster_filters_by_department(capsys):
    assert main(["roster", "--department", "ops"]) == 0
    out = capsys.readouterr().out
    assert "Anya" in out and "Tyler" in out
    assert "Liam" not in out


def test_roster_unknown_department_fails(capsys):
    assert main(["roster", "--department", "nope"]) == 1


def test_ask_unknown_agent_fails(capsys):
    assert main(["ask", "Nobody", "hi"]) == 1
    assert "Unknown agent" in capsys.readouterr().out


def test_ask_runs_agent(capsys):
    assert main(["ask", "Liam", "say hi"]) == 0
    assert capsys.readouterr().out.strip() != ""


def test_team_runs_and_prints_synthesis(capsys):
    assert main(["team", "say hi", "Liam", "Sofia", "--rounds", "1"]) == 0
    out = capsys.readouterr().out
    assert "Team request: say hi" in out


def test_team_unknown_agent_fails(capsys):
    assert main(["team", "hi", "Nobody"]) == 1


def test_evals_subcommand_passes(capsys):
    assert main(["evals"]) == 0
    assert "scenarios passed" in capsys.readouterr().out


def test_parser_requires_subcommand():
    with pytest.raises(SystemExit):
        build_parser().parse_args([])
