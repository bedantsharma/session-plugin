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
    /// Set up the data folder (database, default config) and register the Claude Code plugin.
    Install {
        /// Register the plugin from this checkout instead of GitHub.
        #[arg(long)]
        dev: bool,
        /// Only set up the data folder; don't touch Claude Code.
        #[arg(long, conflicts_with = "dev")]
        no_plugin: bool,
    },
    /// Show or change a repo's plan.
    Plan {
        #[command(subcommand)]
        action: PlanCommand,
    },
    /// Live sessions per project: state, active task, units and active minutes.
    Status,
    /// Answer a drift question about a session (spec §7.3).
    Drift {
        /// A unique prefix of a live session id (as `status` shows it).
        session: String,
        answer: DriftAnswer,
        /// make-task: the new task's parent (milestone, goal, or task for a subtask).
        #[arg(long, required_if_eq("answer", "make-task"))]
        parent: Option<String>,
        /// make-task: the new task's title.
        #[arg(long, required_if_eq("answer", "make-task"))]
        title: Option<String>,
        /// make-task: estimate in agent-minutes.
        #[arg(long)]
        estimate: Option<u32>,
    },
    /// Set a unit's label: on-task, detour or drift (overrides the agent and heuristic).
    Label {
        /// A unique prefix of a live session id.
        session: String,
        unit_no: i64,
        label: LabelArg,
    },
    /// Resolve leftovers of dead sessions: `resolve <leftover-id|prefix>… adopt|resume|undo`.
    Resolve {
        /// Leftover ids (or unique prefixes), then the action: adopt, resume or undo.
        #[arg(required = true, num_args = 2..)]
        args: Vec<String>,
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
    },
    /// Squash the branch's wip(bridge) checkpoint commits into one (keeps a backup ref).
    Squash {
        /// Any directory inside the worktree.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// The base branch (default: origin's default branch, else main, else master).
        #[arg(long)]
        onto: Option<String>,
        /// Show what would happen; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rebuild all views from the event log.
    Rebuild,
    /// Print the effective configuration (and this repo's tuned drift thresholds).
    Config {
        #[command(subcommand)]
        action: Option<ConfigCommand>,
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".", global = true)]
        dir: PathBuf,
    },
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

/// The owner's answer to a drift question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DriftAnswer {
    /// Keep going: the away time so far is acknowledged.
    Continue,
    /// Promote the current detour to a task (needs --parent and --title).
    MakeTask,
    /// Stop the detour and return to the task.
    Stop,
}

impl DriftAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::MakeTask => "make_task",
            Self::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LabelArg {
    OnTask,
    Detour,
    Drift,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Put this repo's auto-tuned drift thresholds back to the config defaults.
    ResetDrift,
}
