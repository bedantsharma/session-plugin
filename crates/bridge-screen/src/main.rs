mod cli;
mod commands;
mod hook;
mod install;
mod mcp;
mod report;
mod status;
mod ui;
mod usage;
mod watch;
mod words;

use std::process::ExitCode;

use clap::Parser;
use clap::error::ErrorKind;

fn main() -> ExitCode {
    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let is_hook = std::env::args_os().nth(1).is_some_and(|a| a == "hook");
            if is_hook
                && !matches!(
                    err.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                )
            {
                // A hook must never fail a Claude session, even when called with an event
                // this binary doesn't know.
                hook::bad_command_line(&err.to_string());
                return ExitCode::SUCCESS;
            }
            err.exit()
        }
    };
    if let cli::Command::Hook { event } = cli.command {
        // A hook must never fail a Claude session: it always exits 0.
        hook::main_entry(event);
        return ExitCode::SUCCESS;
    }
    match commands::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
