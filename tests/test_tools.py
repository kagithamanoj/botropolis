"""Tool tests: calculator and current_time."""

from datetime import datetime

import pytest

from botropolis.tools import get_tool


def test_calculator_basic_arithmetic():
    calc = get_tool("calculator")
    assert calc.execute(expression="2 + 3 * 4") == 14
    assert calc.execute(expression="(10 - 2) / 4") == 2.0
    assert calc.execute(expression="2 ** 10") == 1024


def test_calculator_safe_functions():
    calc = get_tool("calculator")
    assert calc.execute(expression="abs(-7) + round(2.5)") == 9


def test_calculator_rejects_unsafe_code():
    calc = get_tool("calculator")
    with pytest.raises(ValueError):
        calc.execute(expression="__import__('os').system('echo hi')")
    with pytest.raises(ValueError):
        calc.execute(expression="open('/etc/passwd').read()")


def test_calculator_rejects_syntax_errors():
    calc = get_tool("calculator")
    with pytest.raises(ValueError):
        calc.execute(expression="2 +")


def test_current_time_is_iso8601():
    tool = get_tool("current_time")
    stamp = tool.execute()
    parsed = datetime.fromisoformat(stamp)
    assert parsed.tzinfo is not None


def test_unknown_tool_raises():
    with pytest.raises(KeyError):
        get_tool("teleporter")
