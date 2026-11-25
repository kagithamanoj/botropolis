//! Command-line interface for Botropolis.
//!
//! Subcommands:
//!     roster              List every agent, optionally filtered by department.
//!     ask AGENT "task"    Ask one agent directly and print the answer.
//!     team "task" A [B]  Run agents as a team in collaboration rounds.
//!     resume RUN_ID       Continue an interrupted tool run from its saved state.
//!     map RUN_ID          Print a run's task map: goal, plan, progress, blockers.
//!     trace RUN_ID        Print a run's trace: every step in order.
//!     evals               Run the scenario evals.
//!     serve               Start the web server.
//!     notebook            Print the shared company notebook.
//!
//! Mirrors `botropolis/cli.py`.

use clap::{Parser, Subcommand};
use std::sync::Arc;

use crate::agent::RunOpts;
use crate::eval::run_evals;
use crate::memory::{default_memory_store, set_memory_store};
use crate::orchestrator::CEO;
use crate::registry::AgentRegistry;
use crate::runstate::{task_from_state, RunStore};
use crate::schemas::new_run_id;
use crate::schemas::Task;
use crate::taskmap::read_map;
use crate::tools::builtin::notebook_path;
use crate::tools::consult::set_consult_registry;
use crate::trace::{build_trace, render_text};

#[derive(Parser)]
#[command(name = "botropolis", version, about = "Botropolis: a company of bots.")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List every agent, optionally filtered by department.
    Roster {
        /// Filter by department.
        #[arg(long)]
        department: Option<String>,
    },
    /// Ask one agent a question.
    Ask {
        /// Agent name, e.g. Coder.
        agent: String,
        /// The task or question.
        task: String,
        /// Show tool calls.
        #[arg(long)]
        verbose: bool,
    },
    /// Run a team of agents in rounds.
    Team {
        /// The task for the team.
        task: String,
        /// Agent names, in run order.
        agents: Vec<String>,
        /// Collaboration rounds (1-3).
        #[arg(long, default_value_t = 2)]
        rounds: u32,
    },
    /// Continue an interrupted tool run.
    Resume {
        /// Run id printed by a previous ask.
        run_id: String,
    },
    /// Print a run's task map.
    Map {
        /// Run id printed by a previous ask.
        run_id: String,
    },
    /// Print a run's trace.
    Trace {
        /// Run id printed by a previous ask.
        run_id: String,
        /// Dump the trace as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run the scenario evals.
    Evals {
        /// Scenarios directory.
        directory: Option<String>,
    },
    /// Start the web server.
    Serve {
        /// Port to listen on.
        #[arg(long, default_value_t = 8000)]
        port: u16,
    },
    /// Print the shared company notebook.
    Notebook,
}

fn load_registry() -> Result<Arc<AgentRegistry>, String> {
    AgentRegistry::load()
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

/// Point the memory tools at the shared company memory store, so the
/// CEO recalls from the same store agents save to.
fn wire_memory() {
    let store = default_memory_store();
    set_memory_store(Some(store));
}

/// Point the consult tool at this registry, so consults reach the
/// same agents the command is running.
fn wire_consult(registry: &Arc<AgentRegistry>) {
    set_consult_registry(Some(Arc::clone(registry)));
}

fn cmd_roster(department: Option<String>) -> i32 {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("Could not load the agent registry: {e}");
            return 1;
        }
    };
    let agents = match &department {
        Some(dept) => {
            let found = registry.by_department(dept);
            if found.is_empty() {
                println!("No agents in department {dept:?}.");
                return 1;
            }
            found
        }
        None => registry.list_all(),
    };
    for agent in agents {
        let tools = if agent.toolkit.is_empty() {
            String::new()
        } else {
            format!(" [{}]", agent.toolkit.join(", "))
        };
        println!(
            "{:12} {:38} {}{}",
            agent.name, agent.title, agent.department, tools
        );
    }
    0
}

fn cmd_ask(agent_name: String, task_text: String, verbose: bool) -> i32 {
    wire_memory();
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("Could not load the agent registry: {e}");
            return 1;
        }
    };
    wire_consult(&registry);
    let agent = match registry.get(&agent_name) {
        Ok(agent) => agent,
        Err(_) => {
            println!("Unknown agent: {agent_name}");
            return 1;
        }
    };
    let run_id = new_run_id();
    let store = RunStore::new(None);
    let result = {
        // Tool starts are collected on a shared slot so the event
        // closure can move into the run.
        let started: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let slot = Arc::clone(&started);
        let opts = RunOpts {
            run_id: Some(run_id.clone()),
            store: Some(store.clone()),
            on_event: Some(Box::new(move |kind: &str, payload: serde_json::Value| {
                if let Ok(mut events) = slot.lock() {
                    events.push((kind.to_string(), payload));
                }
            })),
            ..Default::default()
        };
        let result = agent.run(Task::new(task_text), opts);
        for (kind, payload) in started.lock().map(|e| e.clone()).unwrap_or_default() {
            if kind == "tool_started" {
                println!(
                    "[tool] {} {}",
                    payload.get("tool").and_then(|v| v.as_str()).unwrap_or(""),
                    payload
                        .get("args")
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                );
            }
        }
        result
    };
    if !result.success {
        println!(
            "Failed: {}",
            result.error.as_deref().unwrap_or("unknown error")
        );
        if store.exists(&run_id) {
            println!("Run state saved as {run_id}. Resume it with: botropolis resume {run_id}");
        }
        return 1;
    }
    println!("{}", result.output);
    if store.exists(&run_id) {
        println!("run id: {run_id}");
    }
    if verbose && !result.tool_calls.is_empty() {
        println!();
        for call in &result.tool_calls {
            let status = if call.success { "ok" } else { "FAILED" };
            println!("  {} {} -> {status}", call.tool, call.args);
        }
    }
    0
}

fn cmd_resume(run_id: String) -> i32 {
    wire_memory();
    let store = RunStore::new(None);
    let state = match store.load(&run_id) {
        Ok(state) => state,
        Err(_) => {
            println!("No saved run with id {run_id}.");
            return 1;
        }
    };
    if state.get("status").and_then(|v| v.as_str()) == Some("finished") {
        println!("That run already finished.");
        if let Some(answer) = state.get("answer").and_then(|v| v.as_str()) {
            if !answer.is_empty() {
                println!("{answer}");
            }
        }
        return 0;
    }
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("Could not load the agent registry: {e}");
            return 1;
        }
    };
    wire_consult(&registry);
    let agent_name = state.get("agent").and_then(|v| v.as_str()).unwrap_or("");
    let agent = match registry.get(agent_name) {
        Ok(agent) => agent,
        Err(_) => {
            println!("Unknown agent in saved run: {agent_name}");
            return 1;
        }
    };
    let done = state
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|s| s.len())
        .unwrap_or(0);
    println!("Resuming run {run_id}: {done} step(s) already done, continuing.");
    let opts = RunOpts {
        run_id: Some(
            state
                .get("run_id")
                .and_then(|v| v.as_str())
                .unwrap_or(&run_id)
                .to_string(),
        ),
        store: Some(store),
        resume_state: Some(state.clone()),
        ..Default::default()
    };
    let result = agent.run(task_from_state(&state), opts);
    if !result.success {
        println!(
            "Failed: {}",
            result.error.as_deref().unwrap_or("unknown error")
        );
        return 1;
    }
    println!("{}", result.output);
    0
}

fn cmd_map(run_id: String) -> i32 {
    let store = RunStore::new(None);
    match read_map(&store, &run_id) {
        Ok(map) => {
            print!("{map}");
            0
        }
        Err(_) => {
            println!("No saved run with id {run_id}.");
            1
        }
    }
}

fn cmd_trace(run_id: String, as_json: bool) -> i32 {
    let store = RunStore::new(None);
    match build_trace(&store, &run_id, None) {
        Ok(trace) => {
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&trace).unwrap_or_default()
                );
            } else {
                print!("{}", render_text(&trace));
            }
            0
        }
        Err(_) => {
            println!("No saved run with id {run_id}.");
            1
        }
    }
}

fn cmd_team(task_text: String, agents: Vec<String>, rounds: u32) -> i32 {
    wire_memory();
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("Could not load the agent registry: {e}");
            return 1;
        }
    };
    wire_consult(&registry);
    let ceo = match CEO::new(Some(registry), None, Some(default_memory_store())) {
        Ok(ceo) => ceo,
        Err(e) => {
            eprintln!("Could not start the CEO: {e}");
            return 1;
        }
    };
    match ceo.team(&task_text, &agents, rounds, None, &[], false) {
        Ok(report) => {
            println!("{}", report.synthesis);
            if report.successful() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            println!("Error: {e}");
            1
        }
    }
}

fn cmd_evals(directory: Option<String>) -> i32 {
    run_evals(directory.as_deref().map(std::path::Path::new))
}

fn cmd_serve(port: u16) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Could not start the server runtime: {e}");
            return 1;
        }
    };
    match runtime.block_on(crate::server::serve(port)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Server stopped: {e}");
            1
        }
    }
}

fn cmd_notebook() -> i32 {
    let content = std::fs::read_to_string(notebook_path())
        .unwrap_or_default()
        .trim()
        .to_string();
    if content.is_empty() {
        println!("The notebook is empty. Agents with the notes tools write here.");
    } else {
        println!("{content}");
    }
    0
}

/// Run the CLI with the given arguments (including argv[0]).
/// Returns a process exit code.
pub fn run(argv: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    match cli.command {
        Commands::Roster { department } => cmd_roster(department),
        Commands::Ask {
            agent,
            task,
            verbose,
        } => cmd_ask(agent, task, verbose),
        Commands::Team {
            task,
            agents,
            rounds,
        } => cmd_team(task, agents, rounds),
        Commands::Resume { run_id } => cmd_resume(run_id),
        Commands::Map { run_id } => cmd_map(run_id),
        Commands::Trace { run_id, json } => cmd_trace(run_id, json),
        Commands::Evals { directory } => cmd_evals(directory),
        Commands::Serve { port } => cmd_serve(port),
        Commands::Notebook => cmd_notebook(),
    }
}
