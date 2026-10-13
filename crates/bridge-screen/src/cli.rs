use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::hook::HookEvent;

/// Keeps track of where your Claude Code agents are: progress, tangents, and handoffs.
#[derive(Debug, Parser)]
#[command(name = "bridge-screen", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set up the data folder: database and default config.
    Install,
    /// Show or change a repo's plan.
    Plan {
        #[command(subcommand)]
        action: PlanCommand,
    },
    /// Rebuild all views from the event log.
    Rebuild,
    /// Print the effective configuration.
    Config,
    /// Serve the agent's tools over MCP (stdio). Claude Code starts this.
    Mcp,
    /// Handle a Claude Code hook event (reads the hook JSON on stdin). Always exits 0.
    Hook {
        #[arg(value_enum)]
        event: HookEvent,
    },
}

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    /// Print the plan tree.
    Show {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// Include dropped nodes.
        #[arg(long)]
        all: bool,
    },
    /// Apply a JSON array of plan operations (from --file or stdin).
    Edit {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// Why the plan is changing.
        #[arg(long)]
        reason: Option<String>,
        /// Record the batch as one deliberate re-plan (needs --reason).
        #[arg(long)]
        replan: bool,
        /// Read operations from this file instead of stdin.
        #[arg(long)]
        file: Option<PathBuf>,
    },
}
