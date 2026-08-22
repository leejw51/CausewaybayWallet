//! Causewaybay Wallet — an educational Cronos/EVM wallet library.
//!
//! ⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk. Do not use with
//! funds you are not prepared to lose. For real value use a hardware wallet.

pub mod app;
pub mod bip32;
pub mod bip39;
pub mod cli;
pub mod clipboard;
pub mod erc20;
pub mod error;
pub mod export;
pub mod network;
pub mod output;
pub mod paths;
pub mod rlp;
pub mod rpc;
pub mod store;
pub mod tui;
pub mod tx;
pub mod units;
pub mod wallet;
