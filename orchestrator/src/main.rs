use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use boruna_orchestrator::cli;
use boruna_orchestrator::engine::Role;

#[derive(Parser)]
#[command(name = "boruna-orch", about = "Boruna Multi-Agent Orchestrator")]
struct Cli {
    /// Workspace root directory (default: current directory)
    #[arg(long, default_value = ".")]
    workspace: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a DAG from a plan specification file.
    Plan {
        /// Path to the plan spec JSON file.
        spec: PathBuf,
    },
    /// Assign the next ready node for a given role.
    Next {
        /// Role: planner, implementer, reviewer, red-team
        #[arg(long)]
        role: String,
    },
    /// Apply a patch bundle and run deterministic gates.
    Apply {
        /// Path to the .patchbundle.json file.
        bundle: PathBuf,
    },
    /// Review a patch bundle: validate + gates + checklist.
    Review {
        /// Path to the .patchbundle.json file.
        bundle: PathBuf,
    },
    /// Show current work graph state.
    Status,
    /// Machine-readable JSON summary of graph + gates.
    Report {
        /// Output as JSON.
        #[arg(long, default_value_t = true)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let workspace = &cli.workspace;

    let result = match cli.command {
        Command::Plan { spec } => cli::cmd_plan(workspace, &spec),
        Command::Next { role } => {
            let role: Role = role.parse().unwrap_or_else(|e| {
                eprintln!("error: {e}");
                process::exit(1);
            });
            cli::cmd_next(workspace, role)
        }
        Command::Apply { bundle } => cli::cmd_apply(workspace, &bundle),
        Command::Review { bundle } => cli::cmd_review(workspace, &bundle),
        Command::Status => cli::cmd_status(workspace),
        Command::Report { .. } => cli::cmd_report(workspace),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
