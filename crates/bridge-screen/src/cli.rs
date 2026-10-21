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
    /// The glance as a live full-terminal view (q quits; prints `status` when not a TTY).
    Watch,
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
        /// continue: why, in your words (at least 8 characters and 2 words).
        #[arg(long)]
        reason: Option<String>,
    },
    /// Set a unit's label: on-task, detour, requested (you asked for it) or drift
    /// (overrides the agent and heuristic).
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
    /// The owner's verdict on a task (spec §8.5 L4): it works, `--broken "<note>"`, or
    /// `--withdraw` to take your last verdict back.
    Verify {
        /// The task or subtask id, e.g. 2.3.
        task: String,
        /// It doesn't work: what's wrong. The task goes back to failed.
        #[arg(long, value_name = "NOTE", conflicts_with = "withdraw")]
        broken: Option<String>,
        /// Take back your last sign-off (or "broken") on the task: it goes back to where it
        /// was before.
        #[arg(long)]
        withdraw: bool,
        /// The level reached (the owner's own check is L4).
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(i64).range(1..=4))]
        level: i64,
        /// Any directory inside the worktree whose HEAD was checked.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
    },
    /// The repo's decisions (spec §8.6); superseded ones only with --all.
    Decisions {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// Only the decisions on this task.
        #[arg(long)]
        task: Option<String>,
        /// Include superseded decisions.
        #[arg(long)]
        all: bool,
    },
    /// Tokens and cost per project, task, session, agent or model (spec §9.2).
    Usage {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// Only this session (a unique prefix of its id).
        #[arg(long)]
        session: Option<String>,
        /// Only this task (its usage includes detours opened under it).
        #[arg(long)]
        task: Option<String>,
        /// How to split the totals.
        #[arg(long, value_enum, default_value_t = UsageBy::Task)]
        by: UsageBy,
        /// Read every transcript of this repo's sessions to the end first and record
        /// what's new (hooks read at most 512 KiB at a time).
        #[arg(long)]
        catch_up: bool,
    },
    /// When the project will likely be done, and what the forecast is based on (spec §9.3).
    Forecast {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
    },
    /// Import the repo's superpowers plan files (docs/superpowers/plans/*.md) into the plan.
    /// Files are only read. With no FILE, every new or changed file is imported.
    Import {
        /// One plan file to import.
        file: Option<PathBuf>,
        /// Link FILE to this existing goal or milestone (its tasks are matched by title).
        #[arg(long, requires = "file", value_name = "NODE")]
        into: Option<String>,
        /// Show what would change; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
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
    /// Open the local dashboard (127.0.0.1 only); prints its URL. One instance at a time.
    Ui {
        /// Port on 127.0.0.1 (default: `ui_port` in config.toml; 0 = any free port).
        #[arg(long)]
        port: Option<u16>,
        /// Don't open the browser.
        #[arg(long)]
        no_open: bool,
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

impl From<DriftAnswer> for bridge_screen_core::owner::DriftAnswer {
    fn from(a: DriftAnswer) -> Self {
        match a {
            DriftAnswer::Continue => Self::Continue,
            DriftAnswer::MakeTask => Self::MakeTask,
            DriftAnswer::Stop => Self::Stop,
        }
    }
}

/// How `usage` splits its totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum UsageBy {
    Model,
    Task,
    Session,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LabelArg {
    OnTask,
    Detour,
    Drift,
    /// You asked for this work in the session: never drift.
    #[value(alias = "asked", alias = "asked-for")]
    Requested,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Put this repo's auto-tuned drift thresholds back to the config defaults.
    ResetDrift,
    /// Set a per-repo setting: `test-command "<cmd>"` (re-run at verify level 2),
    /// `test-timeout <secs>` or `publish-reports never|ask`.
    Set { key: SettingArg, value: String },
    /// Unset a per-repo setting.
    Unset { key: SettingArg },
}

/// Per-repo settings only the owner sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SettingArg {
    /// The command bridge-screen re-runs (`sh -c`, in the session's worktree) at L2.
    TestCommand,
    /// Seconds the re-run may take (default: `verify_timeout_s` in config.toml).
    TestTimeout,
    /// `never` (the agent refuses to make progress reports here) or `ask` (the default).
    PublishReports,
}

impl SettingArg {
    pub fn key(self) -> &'static str {
        match self {
            Self::TestCommand => "test_command",
            Self::TestTimeout => "test_timeout_s",
            Self::PublishReports => "publish_reports",
        }
    }
}
