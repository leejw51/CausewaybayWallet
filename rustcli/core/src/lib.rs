//! Causewaybay Wallet — an educational Cronos/EVM wallet library.
//!
//! ⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk. Do not use with
//! funds you are not prepared to lose. For real value use a hardware wallet.
//!
//! # What lives here
//!
//! Everything a wallet does and nothing a terminal does. A command is a
//! function from a [`command::Command`] to a [`output::CommandOutput`]; it
//! never prints, never reads stdin, never exits the process. The two things a
//! command genuinely needs from its surroundings — a line of standard input,
//! and a yes/no answer — arrive through the [`host::Host`] trait, so the same
//! code serves the `cwbwallet` binary, the TUI, and a foreign caller reaching
//! in over the C ABI.
//!
//! # Two ways in
//!
//! ```no_run
//! use causewaybay_core::{App, Command, Request};
//!
//! # fn main() -> causewaybay_core::error::Result<()> {
//! // Typed: build an App and hand it a command.
//! let app = App::open(&Request::new(["info"]))?;
//! let output = app.run(Command::Info)?;
//! println!("{}", output.human);
//!
//! // Or from arguments, which is what a front end has:
//! let output = App::execute(&Request::new(["account", "list"]))?;
//! println!("{}", output.human);
//!
//! // Or untyped, the shape the C ABI and Lua use: JSON in, JSON out.
//! let envelope = causewaybay_core::api::execute_json(r#"{"argv":["info"]}"#);
//! # let _ = envelope;
//! # Ok(())
//! # }
//! ```

pub mod api;
pub mod app;
pub mod bip32;
pub mod bip39;
pub mod chain;
pub mod command;
pub mod erc20;
pub mod error;
pub mod export;
pub mod host;
pub mod network;
pub mod output;
pub mod paths;
pub mod request;
pub mod rlp;
pub mod rpc;
pub mod runtime;
pub mod search;
pub mod store;
pub mod token;
pub mod tx;
pub mod units;
pub mod wallet;

pub use app::App;
pub use chain::{Chain, ChainId};
pub use command::{Cli, Command};
pub use error::{Code, Error, Result};
pub use host::Host;
pub use output::CommandOutput;
pub use request::Request;

/// The version of the crate, and therefore of the wallet.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The C ABI / JSON request contract version.
///
/// Bumped whenever the shape of an [`api`] request or envelope changes in a way
/// a caller compiled against an older header would get wrong. Hosts that load
/// the shared library at runtime — Lua, LÖVE — check this before trusting it.
///
/// Version 2 added the request's `chain` field and the `chain` key on account
/// records, alongside the Solana, Cardano and Midnight chains. eCash arrived
/// later and did not bump it: a new value for a field that already exists is
/// additive, and a caller compiled against the version 2 header reads an
/// eCash account correctly without knowing what `ecash` is.
pub const ABI_VERSION: u32 = 2;
