//! One serialisable description of "run this command".
//!
//! The CLI has argv and the ambient environment. A GUI has a network picker, a
//! home directory it chose itself, and a confirmation dialog it has already
//! shown. A [`Request`] is the shape both collapse to: the argument vector plus
//! the few things that would otherwise have to be global state.
//!
//! Fields set on the request are *defaults*: a flag inside `argv` wins, so
//! `{"network": "cronos-mainnet", "argv": ["-n", "testnet", "balance"]}` uses
//! testnet. That ordering is what lets a GUI hold a network in its own state
//! and still honour a one-off override typed into a command box.

use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::command::Cli;
use crate::error::{self, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The arguments, *without* the program name: `["account", "list"]`.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Wallet home, when the caller does not want `CAUSEWAYBAY_HOME`.
    #[serde(default)]
    pub home: Option<PathBuf>,
    /// Network key or alias for this call only.
    #[serde(default)]
    pub network: Option<String>,
    /// Answer every confirmation with yes. A GUI sets this once its own
    /// dialog has been accepted.
    #[serde(default)]
    pub yes: bool,
    /// What an argument written as `-` stands for. There is no stdin to read
    /// from inside a shared library, so it is passed in instead.
    #[serde(default)]
    pub stdin: Option<String>,
}

/// What clap made of the arguments.
#[derive(Debug)]
pub enum Parsed {
    /// A command to run.
    Command(Box<Cli>),
    /// clap wants to say something and stop: `--help`, `--version`. Not a
    /// failure, so it is not an `Err` — the caller prints it and exits 0.
    Message(String),
}

impl Request {
    /// A request that only carries arguments.
    pub fn new<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Request {
            argv: argv.into_iter().map(Into::into).collect(),
            ..Request::default()
        }
    }

    pub fn home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    pub fn network(mut self, network: Option<String>) -> Self {
        self.network = network;
        self
    }

    pub fn yes(mut self, yes: bool) -> Self {
        self.yes = yes;
        self
    }

    pub fn stdin(mut self, stdin: Option<String>) -> Self {
        self.stdin = stdin;
        self
    }

    /// Run the arguments through clap, folding this request's defaults in.
    ///
    /// clap's own failures become `usage` errors carrying its rendered message,
    /// which is the most useful thing a caller with no terminal can be handed.
    pub fn parse(&self) -> Result<Parsed> {
        let argv = std::iter::once("cwbwallet".to_string()).chain(self.argv.iter().cloned());
        match Cli::try_parse_from(argv) {
            Ok(mut cli) => {
                cli.home = cli.home.take().or_else(|| self.home.clone());
                cli.network = cli.network.take().or_else(|| self.network.clone());
                cli.yes = cli.yes || self.yes;
                Ok(Parsed::Command(Box::new(cli)))
            }
            Err(e) => match e.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    Ok(Parsed::Message(e.render().to_string()))
                }
                // `render()` rather than `to_string()`: it is the message a
                // user would have seen on the terminal, usage line and all.
                _ => Err(error::usage(e.render().to_string().trim_end().to_string())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;

    #[test]
    fn argv_flags_beat_request_defaults() {
        let request = Request::new(["-n", "cronos-mainnet", "balance"])
            .network(Some("cronos-testnet".into()))
            .yes(false);
        let Parsed::Command(cli) = request.parse().unwrap() else {
            panic!("expected a command");
        };
        assert_eq!(cli.network.as_deref(), Some("cronos-mainnet"));
    }

    #[test]
    fn request_defaults_fill_in_what_argv_omits() {
        let request = Request::new(["balance"])
            .network(Some("cronos-mainnet".into()))
            .home(Some(PathBuf::from("/tmp/w")))
            .yes(true);
        let Parsed::Command(cli) = request.parse().unwrap() else {
            panic!("expected a command");
        };
        assert_eq!(cli.network.as_deref(), Some("cronos-mainnet"));
        assert_eq!(cli.home.as_deref(), Some(std::path::Path::new("/tmp/w")));
        assert!(cli.yes);
    }

    #[test]
    fn yes_is_a_floor_not_an_override() {
        // A GUI that has already asked passes yes=true; no flag can unset it.
        let Parsed::Command(cli) = Request::new(["send", "--to", "0x0", "--amount", "1"])
            .yes(true)
            .parse()
            .unwrap()
        else {
            panic!("expected a command");
        };
        assert!(cli.yes);
    }

    #[test]
    fn help_and_version_are_messages_not_errors() {
        for flag in ["--help", "--version"] {
            let Parsed::Message(text) = Request::new([flag]).parse().unwrap() else {
                panic!("{flag} should be a message");
            };
            assert!(!text.is_empty());
        }
        // So is an empty argv: clap prints the top-level help.
        assert!(matches!(
            Request::new(Vec::<String>::new()).parse().unwrap(),
            Parsed::Message(_)
        ));
    }

    #[test]
    fn a_bad_command_is_a_usage_error_carrying_claps_message() {
        let e = Request::new(["teleport"]).parse().unwrap_err();
        assert_eq!(e.code, error::Code::Usage);
        assert!(e.message.contains("teleport"), "{}", e.message);
    }

    #[test]
    fn round_trips_through_json() {
        let json = r#"{"argv":["utils","keccak","abc"],"yes":true,"stdin":"x"}"#;
        let request: Request = serde_json::from_str(json).unwrap();
        assert_eq!(request.argv, ["utils", "keccak", "abc"]);
        assert!(request.yes);
        assert_eq!(request.stdin.as_deref(), Some("x"));

        let again: Request =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(again.argv, request.argv);
    }

    #[test]
    fn an_unknown_field_is_rejected_rather_than_ignored() {
        // A caller that misspells `yes` must hear about it, not silently send
        // funds without the confirmation it thought it had disabled.
        assert!(serde_json::from_str::<Request>(r#"{"argv":[],"yess":true}"#).is_err());
    }

    #[test]
    fn parses_into_the_expected_command() {
        let Parsed::Command(cli) = Request::new(["info"]).parse().unwrap() else {
            panic!("expected a command");
        };
        assert!(matches!(cli.command, Command::Info));
    }
}
