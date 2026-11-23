"""Command-line interface for Botropolis.

Subcommands:
    roster              List every agent, optionally filtered by department.
    ask AGENT "task"    Ask one agent directly and print the answer.
    team "task" A [B]  Run agents as a team in collaboration rounds.
    resume RUN_ID       Continue an interrupted tool run from its saved state.
    evals               Run the scenario evals.
    serve               Start the web server.
    notebook            Print the shared company notebook.

Installed as the `botropolis` command (see pyproject.toml), or run as
python -m botropolis.cli.
"""

from __future__ import annotations

import argparse
import sys

from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry
from botropolis.eval.runner import main as evals_main


def cmd_roster(args: argparse.Namespace) -> int:
    registry = AgentRegistry()
    if args.department:
        agents = registry.by_department(args.department)
        if not agents:
            print(f"No agents in department {args.department!r}.")
            return 1
    else:
        agents = registry.list_all()
    for agent in agents:
        tools = f" [{', '.join(agent.toolkit)}]" if agent.toolkit else ""
        print(f"{agent.name:12} {agent.title:38} {agent.department}{tools}")
    return 0


def _wire_memory():
    """Point the memory tools at the shared company memory store and
    return it, so the CEO recalls from the same store agents save to."""
    from botropolis.core.memory import default_memory_store, set_memory_store

    store = default_memory_store()
    set_memory_store(store)
    return store


def cmd_ask(args: argparse.Namespace) -> int:
    from botropolis.core.ledger import new_run_id
    from botropolis.core.runstate import RunStore

    _wire_memory()
    registry = AgentRegistry()
    try:
        agent = registry.get(args.agent)
    except KeyError:
        print(f"Unknown agent: {args.agent}")
        return 1
    events = []
    run_id = new_run_id()
    store = RunStore()
    result = agent.run(
        args.task,
        on_event=lambda kind, payload: events.append((kind, payload)),
        run_id=run_id,
        store=store,
    )
    for kind, payload in events:
        if kind == "tool_started":
            print(f"[tool] {payload.get('tool')} {payload.get('args', {})}")
    if not result.success:
        print(f"Failed: {result.error or 'unknown error'}")
        if store.exists(run_id):
            print(f"Run state saved as {run_id}. Resume it with: botropolis resume {run_id}")
        return 1
    print(result.output)
    if store.exists(run_id):
        print(f"run id: {run_id}")
    if args.verbose and result.tool_calls:
        print()
        for call in result.tool_calls:
            status = "ok" if call.success else "FAILED"
            print(f"  {call.tool} {call.args} -> {status}")
    return 0


def cmd_resume(args: argparse.Namespace) -> int:
    from botropolis.core.runstate import RunStore, task_from_state

    _wire_memory()
    store = RunStore()
    try:
        state = store.load(args.run_id)
    except FileNotFoundError:
        print(f"No saved run with id {args.run_id}.")
        return 1
    if state.get("status") == "finished":
        print("That run already finished.")
        if state.get("answer"):
            print(state["answer"])
        return 0
    registry = AgentRegistry()
    try:
        agent = registry.get(state.get("agent", ""))
    except KeyError:
        print(f"Unknown agent in saved run: {state.get('agent')}")
        return 1
    done = len(state.get("steps") or [])
    print(f"Resuming run {args.run_id}: {done} step(s) already done, continuing.")
    result = agent.run(
        task_from_state(state),
        run_id=state["run_id"],
        store=store,
        resume_state=state,
    )
    if not result.success:
        print(f"Failed: {result.error or 'unknown error'}")
        return 1
    print(result.output)
    return 0


def cmd_team(args: argparse.Namespace) -> int:
    ceo = CEO(memory=_wire_memory())
    try:
        report = ceo.team(args.task, args.agents, rounds=args.rounds)
    except (KeyError, ValueError) as exc:
        print(f"Error: {exc}")
        return 1
    print(report.synthesis)
    return 0 if report.successful() else 1


def cmd_evals(args: argparse.Namespace) -> int:
    return evals_main([args.directory] if args.directory else [])


def cmd_serve(args: argparse.Namespace) -> int:
    import uvicorn

    uvicorn.run("botropolis.server:app", host="127.0.0.1", port=args.port)
    return 0


def cmd_notebook(args: argparse.Namespace) -> int:
    from botropolis.tools.builtin import notebook_path

    try:
        content = notebook_path().read_text(encoding="utf-8").strip()
    except OSError:
        content = ""
    if content:
        print(content)
    else:
        print("The notebook is empty. Agents with the notes tools write here.")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="botropolis", description="Botropolis: a company of bots."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("roster", help="List agents.")
    p.add_argument("--department", default=None, help="Filter by department.")
    p.set_defaults(func=cmd_roster)

    p = sub.add_parser("ask", help="Ask one agent a question.")
    p.add_argument("agent", help="Agent name, e.g. Coder.")
    p.add_argument("task", help="The task or question.")
    p.add_argument("--verbose", action="store_true", help="Show tool calls.")
    p.set_defaults(func=cmd_ask)

    p = sub.add_parser("team", help="Run a team of agents in rounds.")
    p.add_argument("task", help="The task for the team.")
    p.add_argument("agents", nargs="+", help="Agent names, in run order.")
    p.add_argument("--rounds", type=int, default=2, help="Collaboration rounds (1-3).")
    p.set_defaults(func=cmd_team)

    p = sub.add_parser("resume", help="Continue an interrupted tool run.")
    p.add_argument("run_id", help="Run id printed by a previous ask.")
    p.set_defaults(func=cmd_resume)

    p = sub.add_parser("evals", help="Run the scenario evals.")
    p.add_argument("directory", nargs="?", default=None, help="Scenarios directory.")
    p.set_defaults(func=cmd_evals)

    p = sub.add_parser("serve", help="Start the web server.")
    p.add_argument("--port", type=int, default=8000)
    p.set_defaults(func=cmd_serve)

    p = sub.add_parser("notebook", help="Print the shared company notebook.")
    p.set_defaults(func=cmd_notebook)

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
