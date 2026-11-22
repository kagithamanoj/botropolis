"""Math tool tests: real problems with known answers, plus the
safety rails on parsing."""

import pytest

from botropolis.core.registry import AgentRegistry
from botropolis.tools import get_tool


@pytest.fixture()
def math_tool():
    return get_tool("math")


def test_solve_quadratic(math_tool):
    out = math_tool.execute(operation="solve", expression="x^2 - 5*x + 6 = 0")
    assert out["solutions"] == ["2", "3"]
    assert out["values"] == [2.0, 3.0]


def test_solve_without_equals_sign_means_equals_zero(math_tool):
    out = math_tool.execute(operation="solve", expression="x^2 - 5*x + 6")
    assert out["solutions"] == ["2", "3"]


def test_solve_irrational_roots(math_tool):
    out = math_tool.execute(operation="solve", expression="x^2 - 2 = 0")
    assert out["solutions"] == ["-sqrt(2)", "sqrt(2)"]
    assert out["values"] == pytest.approx([-1.41421356, 1.41421356])


def test_derivative(math_tool):
    out = math_tool.execute(operation="diff", expression="x^3")
    assert out["result"] == "3*x**2"


def test_indefinite_integral(math_tool):
    out = math_tool.execute(operation="integrate", expression="sin(x)")
    assert out["result"] == "-cos(x)"


def test_definite_integral(math_tool):
    out = math_tool.execute(
        operation="integrate", expression="x^2", lower="0", upper="3"
    )
    assert out["result"] == "9"
    assert out["value"] == 9.0


def test_limit_at_zero(math_tool):
    out = math_tool.execute(operation="limit", expression="sin(x)/x", point="0")
    assert out["result"] == "1"


def test_limit_at_infinity(math_tool):
    out = math_tool.execute(operation="limit", expression="1/x", point="oo")
    assert out["result"] == "0"


def test_simplify_cancels(math_tool):
    out = math_tool.execute(operation="simplify", expression="(x^2 - 1)/(x - 1)")
    assert out["result"] == "x + 1"


def test_factor(math_tool):
    out = math_tool.execute(operation="factor", expression="x^2 - 5*x + 6")
    assert out["result"] == "(x - 3)*(x - 2)"


def test_expand(math_tool):
    out = math_tool.execute(operation="expand", expression="(x + 1)^2")
    assert out["result"] == "x**2 + 2*x + 1"


def test_series_expansion(math_tool):
    out = math_tool.execute(operation="series", expression="exp(x)", terms=4)
    assert out["result"] == "x**3/6 + x**2/2 + x + 1"


def test_eval_numeric(math_tool):
    out = math_tool.execute(operation="eval", expression="2^10 + sqrt(16)")
    assert out["value"] == 1028.0


def test_eval_in_another_variable(math_tool):
    out = math_tool.execute(
        operation="diff", expression="t^2 + 3*t", variable="t"
    )
    assert out["result"] == "2*t + 3"


def test_unsupported_operation_names_the_supported_ones(math_tool):
    with pytest.raises(ValueError, match="unsupported operation"):
        math_tool.execute(operation="transpose", expression="x")


def test_bad_expression_is_a_clear_error(math_tool):
    with pytest.raises(ValueError, match="could not parse"):
        math_tool.execute(operation="solve", expression="x +")


def test_python_code_is_refused(math_tool):
    with pytest.raises(ValueError, match="not allowed"):
        math_tool.execute(operation="eval", expression="__import__('os')")
    with pytest.raises(ValueError, match="not allowed"):
        math_tool.execute(operation="eval", expression="open('/etc/passwd')")


def test_unknown_names_are_refused(math_tool):
    with pytest.raises(ValueError, match="'foo'"):
        math_tool.execute(operation="eval", expression="foo + 1")
    with pytest.raises(ValueError, match="'real'"):
        math_tool.execute(operation="eval", expression="x.real")


def test_long_input_is_refused(math_tool):
    with pytest.raises(ValueError, match="too long"):
        math_tool.execute(operation="eval", expression="x" * 300)


def test_definite_integral_needs_both_bounds(math_tool):
    with pytest.raises(ValueError, match="both lower and upper"):
        math_tool.execute(operation="integrate", expression="x", lower="0")


def test_limit_needs_a_point(math_tool):
    with pytest.raises(ValueError, match="needs a point"):
        math_tool.execute(operation="limit", expression="1/x")


def test_equation_outside_solve_is_refused(math_tool):
    with pytest.raises(ValueError, match="not an equation"):
        math_tool.execute(operation="diff", expression="x = 3")


def test_data_agents_have_the_math_tool():
    registry = AgentRegistry()
    for name in ("pipelines", "trainer"):
        assert "math" in registry.get(name).toolkit
