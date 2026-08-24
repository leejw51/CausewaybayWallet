//! `cwbwallet` — the command line entry point.
//!
//! Everything the wallet *does* lives in `causewaybay-core`. What is left here
//! is what only a terminal has: argv, a tty to prompt on, stdin to read a
//! secret from, an exit status, and the TUI.

mod clipboard;
mod terminal;
mod tui;

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;

use causewaybay_core::app::App;
use causewaybay_core::command::{Cli, Command};
use causewaybay_core::error::{Code, Error};
use causewaybay_core::output;

use terminal::TerminalHost;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // clap prints its own message; --help and --version are not failures.
            let _ = e.print();
            return match e.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    ExitCode::SUCCESS
                }
                _ => ExitCode::from(2),
            };
        }
    };

    let json = cli.json;
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report(&e, json);
            ExitCode::from(if e.code == Code::Usage { 2 } else { 1 })
        }
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    // `--json` means nobody is watching to answer a prompt, so a confirmation
    // must be refused rather than asked for — which also stops an automated
    // caller from spending funds by accident.
    let host = Arc::new(TerminalHost::new(cli.yes, !cli.json));
    let app = App::new(cli.home, cli.network.as_deref(), cli.chain.as_deref(), host)?;

    // The TUI is the one command core will not run: it takes over the screen,
    // so it belongs to the front end that owns one.
    if matches!(cli.command, Command::Tui) {
        // It has already drawn its own farewell; do not repeat the banner.
        println!("{}", tui::run(app)?.human);
        return Ok(());
    }

    let output = app.run(cli.command)?;
    if cli.json {
        println!("{}", output::success_envelope(&output.data));
    } else {
        eprintln!("{}", output::WARNING);
        println!("{}", output.human);
    }
    Ok(())
}

fn report(error: &Error, json: bool) {
    let mut stderr = std::io::stderr();
    if json {
        // Machine callers read one envelope; stdout stays the single channel.
        println!("{}", output::error_envelope(error));
    } else {
        let _ = writeln!(stderr, "error [{}]: {}", error.code.as_str(), error.message);
    }
}
