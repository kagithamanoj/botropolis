"""Builtin tools agents can call."""
import ast
import operator


def web_search(query: str, max_results: int = 5) -> list:
    """Placeholder: real web search wired up later."""
    raise NotImplementedError("web_search is not wired up yet")


def calculator(expression: str) -> float:
    """Safely evaluate a basic arithmetic expression."""
    ops = {ast.Add: operator.add, ast.Sub: operator.sub,
           ast.Mult: operator.mul, ast.Div: operator.truediv}
    node = ast.parse(expression, mode="eval").body

    def _eval(n):
        if isinstance(n, ast.Constant):
            return n.value
        return ops[type(n.op)](_eval(n.left), _eval(n.right))
    return _eval(node)
