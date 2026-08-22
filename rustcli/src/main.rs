//! `cwbwallet` — the command line entry point.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use causewaybay_wallet::app::App;
use causewaybay_wallet::cli::{Cli, Command};
use causewaybay_wallet::error::{Code, Error};
use causewaybay_wallet::output;

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
    let interactive = matches!(cli.command, Command::Tui);
    let app = App::new(cli.home, cli.network.as_deref(), cli.json, cli.yes)?;
    let output = app.run(cli.command)?;

    if cli.json {
        println!("{}", output::success_envelope(&output.data));
    } else {
        // The TUI has already drawn its own farewell; do not repeat the banner.
        if !interactive {
            eprintln!("{}", output::WARNING);
        }
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
