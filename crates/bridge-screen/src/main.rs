mod cli;
mod commands;
mod hook;
mod mcp;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
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
