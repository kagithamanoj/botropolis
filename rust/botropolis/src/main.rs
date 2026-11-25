//! `botropolis` command line interface.
//!
//! The full command surface (roster, ask, team, evals, serve, notebook,
//! resume, map, trace) lands across the Rust port; this binary starts
//! with `roster`, which only needs the registry.

use botropolis::registry::AgentRegistry;
use clap::{Parser, Subcommand};

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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Roster { department } => {
            let registry = match AgentRegistry::load() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Could not load the agent registry: {e}");
                    std::process::exit(1);
                }
            };
            let agents = match &department {
                Some(dept) => {
                    let found = registry.by_department(dept);
                    if found.is_empty() {
                        println!("No agents in department {dept:?}.");
                        std::process::exit(1);
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
        }
    }
}
