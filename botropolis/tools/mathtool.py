"""Symbolic math tool backed by sympy.

The calculator tool does arithmetic. This one does algebra and
calculus: solve equations in one variable, simplify, factor, expand,
differentiate, integrate (indefinite and definite), limits, series,
and numeric evaluation of expressions with constants.

Parsing is defensive, in two layers. The raw string is first checked
against a character whitelist and an identifier whitelist, so nothing
but numbers, operators, and known names can reach the parser at all.
It is then parsed with sympy's parse_expr against a namespace holding
only the allowed symbols, constants, and functions. Nothing here evals
Python, and there is no attribute access: the characters it needs are
not on the whitelist.

Errors are loud and specific: an unsupported operation names the
supported ones, a bad expression says what was rejected and why.
"""

from __future__ import annotations

import re
from typing import Any, Dict, List, Optional

from botropolis.tools.builtin import Tool

try:
    import sympy
    from sympy.parsing.sympy_parser import (
        convert_xor,
        implicit_multiplication_application,
        parse_expr,
        standard_transformations,
    )

    _HAS_SYMPY = True
except ImportError:  # pragma: no cover - sympy is a declared dependency
    _HAS_SYMPY = False

# Cap input size so one expression cannot turn into an unbounded
# symbolic computation.
MAX_EXPR_CHARS = 200
MAX_EXPR_NODES = 1000

# Variables an expression may use. `variable` arguments must name one.
_VARIABLES = ("x", "y", "z", "t", "n")

# Functions an expression may call.
_FUNCTIONS = (
    "sin", "cos", "tan",
    "asin", "acos", "atan",
    "sinh", "cosh", "tanh",
    "exp", "log", "sqrt", "Abs",
)

# Named constants. `oo` is sympy's spelling of infinity, for limits
# and improper integrals. `e` is the spelling everyone actually types
# for Euler's number, so it counts too.
_CONSTANTS = ("pi", "E", "e", "oo")

_ALLOWED_NAMES = set(_VARIABLES) | set(_FUNCTIONS) | set(_CONSTANTS)

# Only these characters may appear in an expression at all. Notably
# absent: quotes, underscores, brackets, semicolons. That is what makes
# attribute access and statement tricks impossible before parsing.
_ALLOWED_CHARS = re.compile(r"^[A-Za-z0-9\s+\-*/^(),.=]+$")
_IDENTIFIER = re.compile(r"[A-Za-z]+")

_TRANSFORMATIONS = (
    standard_transformations
    + (implicit_multiplication_application, convert_xor)
)

OPERATIONS = (
    "solve", "simplify", "factor", "expand",
    "diff", "integrate", "limit", "series", "eval",
)


def _namespace() -> Dict[str, Any]:
    """The only names the parser may resolve."""
    ns: Dict[str, Any] = {name: sympy.Symbol(name) for name in _VARIABLES}
    for name in _FUNCTIONS:
        ns[name] = getattr(sympy, name)
    ns["pi"] = sympy.pi
    ns["E"] = sympy.E
    ns["e"] = sympy.E
    ns["oo"] = sympy.oo
    return ns


def _validate(text: str, what: str) -> str:
    """Check one raw math string against the whitelists."""
    cleaned = (text or "").strip()
    if not cleaned:
        raise ValueError(f"{what} is empty")
    if len(cleaned) > MAX_EXPR_CHARS:
        raise ValueError(
            f"{what} is too long ({len(cleaned)} chars, max {MAX_EXPR_CHARS})"
        )
    if not _ALLOWED_CHARS.match(cleaned):
        bad = sorted({ch for ch in cleaned if not _ALLOWED_CHARS.match(ch)})
        raise ValueError(
            f"{what} contains characters that are not allowed: {bad}. "
            "Use digits, + - * / ^ ( ) , . = and the allowed names."
        )
    for name in _IDENTIFIER.findall(cleaned):
        if name not in _ALLOWED_NAMES:
            raise ValueError(
                f"{what} uses the name '{name}', which is not allowed. "
                f"Variables: {', '.join(_VARIABLES)}. "
                f"Functions: {', '.join(_FUNCTIONS)}. "
                f"Constants: {', '.join(_CONSTANTS)}."
            )
    return cleaned


def _parse(text: str, what: str = "expression"):
    """Validate and parse one math string into a sympy expression."""
    cleaned = _validate(text, what)
    try:
        expr = parse_expr(
            cleaned,
            local_dict=_namespace(),
            # The parser rewrites numbers into Integer()/Float() calls,
            # so those constructors must resolve. They are parser
            # internals, not user-writable names: the identifier
            # whitelist above still governs what users may type.
            global_dict={
                "__builtins__": {},
                "Integer": sympy.Integer,
                "Float": sympy.Float,
                "Rational": sympy.Rational,
            },
            transformations=_TRANSFORMATIONS,
            evaluate=True,
        )
    except Exception as exc:
        raise ValueError(f"could not parse {what} {text!r}: {exc}") from exc
    nodes = sum(1 for _ in sympy.preorder_traversal(expr))
    if nodes > MAX_EXPR_NODES:
        raise ValueError(
            f"{what} is too complex ({nodes} nodes, max {MAX_EXPR_NODES})"
        )
    return expr


def _numeric(expr) -> Optional[float]:
    """Float value of a sympy result, when it is a plain real number."""
    try:
        value = complex(expr.evalf())
    except Exception:
        return None
    if abs(value.imag) > 1e-12:
        return None
    return float(value.real)


class MathTool(Tool):
    """Symbolic math: algebra and calculus via sympy."""

    name = "math"
    description = (
        "Symbolic math with sympy. Operations: solve (equation in one "
        "variable, '=' optional), simplify, factor, expand, diff "
        "(derivative), integrate (set lower and upper for a definite "
        "integral), limit (needs point), series (Taylor expansion), "
        "eval (numeric value). Expressions use variables x, y, z, t, n, "
        "functions sin, cos, tan, exp, log, sqrt, Abs and their inverse "
        "and hyperbolic forms, and constants pi, E, oo. Use ^ or ** "
        "for powers."
    )
    parameters = {
        "operation": {
            "type": "string",
            "description": "One of: " + ", ".join(OPERATIONS),
            "required": True,
        },
        "expression": {
            "type": "string",
            "description": "e.g. 'x^2 - 5*x + 6 = 0' for solve, 'sin(x)' to integrate",
            "required": True,
        },
        "variable": {
            "type": "string",
            "description": "The variable to work in",
            "default": "x",
        },
        "point": {
            "type": "string",
            "description": "Point for limit and series, e.g. '0' or 'oo'",
        },
        "lower": {
            "type": "string",
            "description": "Lower bound for a definite integral",
        },
        "upper": {
            "type": "string",
            "description": "Upper bound for a definite integral",
        },
        "terms": {
            "type": "integer",
            "description": "Number of terms for series",
            "default": 6,
        },
    }

    def execute(
        self,
        operation: str,
        expression: str,
        variable: str = "x",
        point: Optional[str] = None,
        lower: Optional[str] = None,
        upper: Optional[str] = None,
        terms: int = 6,
    ) -> Dict[str, Any]:
        if not _HAS_SYMPY:
            raise RuntimeError(
                "the math tool needs sympy, which is not installed. "
                "Install the project dependencies (pip install -e .)."
            )
        op = (operation or "").strip().lower()
        if op not in OPERATIONS:
            raise ValueError(
                f"unsupported operation {operation!r}. "
                f"Supported operations: {', '.join(OPERATIONS)}"
            )
        var = (variable or "x").strip()
        if var not in _VARIABLES:
            raise ValueError(
                f"unknown variable {variable!r}. Use one of: "
                f"{', '.join(_VARIABLES)}"
            )
        symbol = sympy.Symbol(var)
        handler = getattr(self, f"_op_{op}")
        try:
            return handler(expression, symbol, point, lower, upper, terms)
        except ValueError:
            raise
        except Exception as exc:
            raise ValueError(
                f"math {op} failed for {expression!r}: {exc}"
            ) from exc

    # -- operations ---------------------------------------------------

    def _single(self, op: str, result: Any) -> Dict[str, Any]:
        out: Dict[str, Any] = {"operation": op, "result": str(result)}
        value = _numeric(result)
        if value is not None:
            out["value"] = value
        return out

    def _body(self, expression: str):
        """Parse an expression, rejecting equations outside solve."""
        if "=" in (expression or ""):
            raise ValueError(
                "this operation takes a plain expression, not an "
                "equation. Use operation 'solve' for equations."
            )
        return _parse(expression)

    def _op_solve(self, expression, symbol, point, lower, upper, terms):
        text = _validate(expression, "expression")
        if text.count("=") > 1:
            raise ValueError("solve takes one equation with a single '='")
        if "=" in text:
            lhs_text, rhs_text = text.split("=", 1)
            equation = sympy.Eq(
                _parse(lhs_text, "left side"), _parse(rhs_text, "right side")
            )
        else:
            equation = sympy.Eq(_parse(text), 0)
        solutions = sympy.solve(equation, symbol)
        rendered = [str(sol) for sol in solutions]
        out: Dict[str, Any] = {
            "operation": "solve",
            "variable": str(symbol),
            "solutions": rendered,
            "result": ", ".join(f"{symbol} = {sol}" for sol in rendered)
            if rendered
            else "no solutions",
        }
        values = [_numeric(sol) for sol in solutions]
        if solutions and all(v is not None for v in values):
            out["values"] = values
        return out

    def _op_simplify(self, expression, symbol, point, lower, upper, terms):
        return self._single("simplify", sympy.simplify(self._body(expression)))

    def _op_factor(self, expression, symbol, point, lower, upper, terms):
        return self._single("factor", sympy.factor(self._body(expression)))

    def _op_expand(self, expression, symbol, point, lower, upper, terms):
        return self._single("expand", sympy.expand(self._body(expression)))

    def _op_diff(self, expression, symbol, point, lower, upper, terms):
        return self._single("diff", sympy.diff(self._body(expression), symbol))

    def _op_integrate(self, expression, symbol, point, lower, upper, terms):
        expr = self._body(expression)
        if (lower is None) != (upper is None):
            raise ValueError(
                "a definite integral needs both lower and upper bounds"
            )
        if lower is not None:
            lo = _parse(lower, "lower bound")
            hi = _parse(upper, "upper bound")
            result = sympy.integrate(expr, (symbol, lo, hi))
        else:
            result = sympy.integrate(expr, symbol)
        return self._single("integrate", result)

    def _op_limit(self, expression, symbol, point, lower, upper, terms):
        if point is None:
            raise ValueError("limit needs a point, e.g. point='0' or point='oo'")
        at = _parse(point, "point")
        return self._single("limit", sympy.limit(self._body(expression), symbol, at))

    def _op_series(self, expression, symbol, point, lower, upper, terms):
        at = _parse(point, "point") if point is not None else sympy.Integer(0)
        count = max(1, min(int(terms), 20))
        expansion = sympy.series(
            self._body(expression), symbol, at, n=count
        ).removeO()
        return self._single("series", expansion)

    def _op_eval(self, expression, symbol, point, lower, upper, terms):
        expr = self._body(expression)
        result = expr.evalf()
        out: Dict[str, Any] = {"operation": "eval", "result": str(result)}
        value = _numeric(result)
        if value is not None:
            out["value"] = value
        return out
