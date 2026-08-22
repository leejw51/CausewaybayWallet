//! Command line surface. Kept in one place so it stays in step with the Python CLI.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "cwbwallet",
    version,
    about = "Educational Cronos/EVM wallet — CLI and TUI over an append-only JSONL store",
    long_about = "Causewaybay Wallet manages EVM accounts for Cronos testnet and mainnet.\n\
                  State lives in ~/.causewaybaywallet as append-only JSONL files.\n\
                  Pass --json for a machine-readable envelope on every command.",
    propagate_version = true
)]
pub struct Cli {
    /// Emit a single-line JSON envelope instead of human text.
    #[arg(long, global = true)]
    pub json: bool,

    /// Override the wallet home directory.
    #[arg(long, global = true, value_name = "PATH", env = "CAUSEWAYBAY_HOME")]
    pub home: Option<PathBuf>,

    /// Use this network for one invocation, without changing the stored default.
    #[arg(long, short = 'n', global = true, value_name = "NETWORK")]
    pub network: Option<String>,

    /// Skip interactive confirmation prompts.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create, import, inspect and select accounts.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Recall mnemonics and private keys this wallet has used before.
    Recent {
        #[command(subcommand)]
        command: RecentCommand,
    },
    /// Inspect and switch networks.
    Network {
        #[command(subcommand)]
        command: NetworkCommand,
    },
    /// Show the native token balance.
    Balance(TargetArgs),
    /// Show the next transaction nonce.
    Nonce(TargetArgs),
    /// Show the current gas price.
    GasPrice,
    /// Show the network, chain id and latest block as reported by the RPC node.
    ChainInfo,
    /// Send native CRO/TCRO.
    #[command(allow_negative_numbers = true)]
    Send(SendArgs),
    /// Look a transaction up on chain.
    Tx {
        /// Transaction hash.
        hash: String,
    },
    /// List transactions this wallet has sent.
    History {
        /// Show at most this many entries, newest first.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only show transactions on this network.
        #[arg(long)]
        network: Option<String>,
    },
    /// Sign a message with EIP-191.
    Sign {
        /// The message to sign; `-` reads it from stdin.
        message: String,
        /// Sign with this account instead of the active one.
        #[arg(long)]
        account: Option<String>,
    },
    /// Verify an EIP-191 message signature.
    Verify {
        /// The message that was signed; `-` reads it from stdin.
        #[arg(long)]
        message: String,
        /// The 65-byte signature, hex encoded.
        #[arg(long)]
        signature: String,
        /// The address the signature is expected to belong to.
        #[arg(long)]
        address: Option<String>,
    },
    /// Read and transfer ERC-20 tokens.
    Erc20 {
        #[command(subcommand)]
        command: Erc20Command,
    },
    /// Offline helpers that need no network access.
    Utils {
        #[command(subcommand)]
        command: UtilsCommand,
    },
    /// Launch the interactive terminal UI.
    Tui,
    /// Report where state lives and what is configured.
    Info,
}

#[derive(Subcommand, Debug)]
pub enum AccountCommand {
    /// Add the next address of the wallet's mnemonic.
    ///
    /// A wallet holds one mnemonic and many addresses derived from it, so this
    /// continues the sequence: 0, 1, 2, … Use `--new-seed` to start a separate
    /// mnemonic instead, and `--index` to pick a specific one.
    New {
        /// Label for the new account.
        #[arg(long, short)]
        label: Option<String>,
        /// Generate a fresh mnemonic instead of using the wallet's.
        #[arg(long)]
        new_seed: bool,
        /// Mnemonic length. Only meaningful with --new-seed.
        #[arg(long, short, default_value_t = 12, value_parser = parse_word_count)]
        words: usize,
        /// BIP-44 address index. Defaults to the next free one.
        #[arg(long, short)]
        index: Option<u32>,
        /// Also print the mnemonic (it is stored either way).
        #[arg(long)]
        show_secret: bool,
    },
    /// Import an existing BIP-39 mnemonic.
    ImportMnemonic {
        /// The mnemonic; `-` reads it from stdin. Defaults to $CAUSEWAYBAY_MNEMONIC.
        #[arg(long, short, env = "CAUSEWAYBAY_MNEMONIC", hide_env_values = true)]
        mnemonic: Option<String>,
        /// BIP-44 address index to derive.
        #[arg(long, short, default_value_t = 0)]
        index: u32,
        /// Label for the imported account.
        #[arg(long, short)]
        label: Option<String>,
        /// Optional BIP-39 passphrase (the "25th word").
        #[arg(long, default_value = "")]
        passphrase: String,
    },
    /// Import a raw private key.
    ImportKey {
        /// The private key; `-` reads it from stdin. Defaults to $CAUSEWAYBAY_PRIVATE_KEY.
        #[arg(
            long,
            short = 'k',
            env = "CAUSEWAYBAY_PRIVATE_KEY",
            hide_env_values = true
        )]
        private_key: Option<String>,
        /// Label for the imported account.
        #[arg(long, short)]
        label: Option<String>,
    },
    /// List every account.
    List {
        /// Render the list as a file format instead of the usual output.
        #[arg(long, value_parser = ["jsonl", "csv", "txt", "md"])]
        format: Option<String>,
        /// Write to this file instead of stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Include private keys and mnemonics in the export.
        #[arg(long)]
        secret: bool,
    },
    /// Show one account.
    Show {
        /// Account id, label or address. Defaults to the active account.
        selector: Option<String>,
        /// Include the private key and mnemonic.
        #[arg(long)]
        secret: bool,
    },
    /// Make an account the default for later commands.
    Use {
        /// Account id, label or address.
        selector: String,
    },
    /// Derive another address from an existing mnemonic account.
    Derive {
        /// BIP-44 address index.
        #[arg(long, short)]
        index: u32,
        /// Label for the derived account.
        #[arg(long, short)]
        label: Option<String>,
        /// Derive from this account instead of the active one.
        #[arg(long)]
        from: Option<String>,
    },
    /// Change an account's label.
    Rename {
        /// Account id, label or address.
        selector: String,
        /// The new label.
        label: String,
    },
    /// Forget an account.
    Remove {
        /// Account id, label or address.
        selector: String,
    },
    /// Print an account's secrets.
    Export {
        /// Account id, label or address. Defaults to the active account.
        selector: Option<String>,
    },
    /// Create an account from key material the wallet already remembers.
    ImportRecent {
        /// Recall id, 1-based position, or address. Defaults to the newest entry.
        selector: Option<String>,
        /// BIP-44 address index, for remembered mnemonics.
        #[arg(long, short, default_value_t = 0)]
        index: u32,
        /// Label for the new account.
        #[arg(long, short)]
        label: Option<String>,
        /// The BIP-39 passphrase, when the entry was created with one.
        #[arg(long, default_value = "")]
        passphrase: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RecentCommand {
    /// List remembered key material, most recently used first.
    List {
        /// Only show one kind.
        #[arg(long, value_parser = ["mnemonic", "private-key"])]
        kind: Option<String>,
        /// Show at most this many entries.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one remembered entry.
    Show {
        /// Recall id, 1-based position, or address. Defaults to the newest entry.
        selector: Option<String>,
        /// Reveal the mnemonic or private key.
        #[arg(long)]
        secret: bool,
    },
    /// Drop one remembered entry.
    Forget {
        /// Recall id, 1-based position, or address.
        selector: String,
    },
    /// Drop every remembered entry.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum NetworkCommand {
    /// List the supported networks.
    List,
    /// Show the selected network.
    Current,
    /// Change the default network.
    Use {
        /// Network key or alias.
        network: String,
    },
    /// Override a network's RPC URL (an empty URL restores the default).
    SetRpc {
        /// Network key or alias.
        network: String,
        /// The RPC endpoint.
        url: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum Erc20Command {
    /// Show a token's name, symbol, decimals and total supply.
    Info {
        /// The token contract address.
        #[arg(long, short)]
        token: String,
    },
    /// Show a token balance.
    Balance {
        /// The token contract address.
        #[arg(long, short)]
        token: String,
        /// Check this address instead of the active account.
        #[arg(long, short)]
        address: Option<String>,
    },
    /// Transfer tokens.
    Send {
        /// The token contract address.
        #[arg(long, short)]
        token: String,
        /// The recipient.
        #[arg(long)]
        to: String,
        /// Amount in whole tokens, e.g. 1.5.
        #[arg(long)]
        amount: String,
        /// Wait for the receipt before returning.
        #[arg(long)]
        wait: bool,
        /// Send from this account instead of the active one.
        #[arg(long)]
        account: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum UtilsCommand {
    /// keccak256 of a UTF-8 string (or of hex bytes with --hex).
    Keccak {
        /// The input.
        input: String,
        /// Treat the input as hex-encoded bytes.
        #[arg(long)]
        hex: bool,
    },
    /// Apply the EIP-55 checksum to an address.
    Checksum {
        /// The address.
        address: String,
    },
    /// Convert a decimal amount into its smallest unit.
    #[command(allow_negative_numbers = true)]
    ToWei {
        /// The amount, e.g. 1.25.
        amount: String,
        /// Token decimals.
        #[arg(long, short, default_value_t = 18)]
        decimals: u8,
    },
    /// Convert a smallest-unit integer into a decimal amount.
    #[command(allow_negative_numbers = true)]
    FromWei {
        /// The integer value.
        value: String,
        /// Token decimals.
        #[arg(long, short, default_value_t = 18)]
        decimals: u8,
    },
    /// Generate a mnemonic without storing it.
    NewMnemonic {
        /// Mnemonic length.
        #[arg(long, short, default_value_t = 12)]
        words: usize,
    },
}

/// Accept only the five BIP-39 word counts, with a message that lists them.
fn parse_word_count(input: &str) -> Result<usize, String> {
    let value: usize = input
        .parse()
        .map_err(|_| format!("'{input}' is not a number"))?;
    if crate::bip39::WORD_COUNTS
        .iter()
        .any(|(words, _)| *words == value)
    {
        Ok(value)
    } else {
        Err("word count must be 12, 15, 18, 21 or 24".to_string())
    }
}

/// Shared "which address" arguments for read-only queries.
#[derive(Args, Debug)]
pub struct TargetArgs {
    /// Query this address instead of the active account.
    #[arg(long, short)]
    pub address: Option<String>,
    /// Query this account instead of the active one.
    #[arg(long)]
    pub account: Option<String>,
}

#[derive(Args, Debug)]
pub struct SendArgs {
    /// The recipient address.
    #[arg(long)]
    pub to: String,
    /// Amount in whole CRO/TCRO, e.g. 0.5.
    #[arg(long)]
    pub amount: String,
    /// Gas limit (default 21000 for a plain transfer).
    #[arg(long)]
    pub gas_limit: Option<u64>,
    /// Gas price in gwei (default: whatever the node reports).
    #[arg(long)]
    pub gas_price_gwei: Option<String>,
    /// Nonce (default: the account's pending nonce).
    #[arg(long)]
    pub nonce: Option<u64>,
    /// Hex call data to attach.
    #[arg(long)]
    pub data: Option<String>,
    /// Wait for the receipt before returning.
    #[arg(long)]
    pub wait: bool,
    /// Send from this account instead of the active one.
    #[arg(long)]
    pub account: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("cwbwallet").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn global_flags_work_before_and_after_the_subcommand() {
        assert!(parse(&["--json", "balance"]).json);
        assert!(parse(&["balance", "--json"]).json);
        assert_eq!(
            parse(&["balance", "--network", "mainnet"])
                .network
                .as_deref(),
            Some("mainnet")
        );
        assert!(parse(&["-y", "account", "list"]).yes);
    }

    #[test]
    fn account_new_defaults_to_twelve_words_at_index_zero() {
        let Command::Account {
            command:
                AccountCommand::New {
                    words,
                    index,
                    label,
                    ..
                },
        } = parse(&["account", "new"]).command
        else {
            panic!("expected account new");
        };
        assert_eq!(words, 12);
        // No index means "the next free one", which only the store can work out.
        assert_eq!(index, None);
        assert_eq!(label, None);
    }

    #[test]
    fn account_new_rejects_an_unsupported_word_count() {
        assert!(Cli::try_parse_from(["cwbwallet", "account", "new", "--words", "13"]).is_err());
        assert!(Cli::try_parse_from(["cwbwallet", "account", "new", "--words", "24"]).is_ok());
    }

    #[test]
    fn send_requires_a_recipient_and_an_amount() {
        assert!(Cli::try_parse_from(["cwbwallet", "send"]).is_err());
        assert!(Cli::try_parse_from(["cwbwallet", "send", "--to", "0xabc"]).is_err());
        assert!(
            Cli::try_parse_from(["cwbwallet", "send", "--to", "0xabc", "--amount", "1"]).is_ok()
        );
    }

    #[test]
    fn history_has_a_sensible_default_limit() {
        let Command::History { limit, .. } = parse(&["history"]).command else {
            panic!("expected history");
        };
        assert_eq!(limit, 20);
    }

    #[test]
    fn utils_defaults_to_eighteen_decimals() {
        let Command::Utils {
            command: UtilsCommand::ToWei { decimals, .. },
        } = parse(&["utils", "to-wei", "1.5"]).command
        else {
            panic!("expected utils to-wei");
        };
        assert_eq!(decimals, 18);
    }

    #[test]
    fn subcommands_are_kebab_case() {
        assert!(
            Cli::try_parse_from(["cwbwallet", "account", "import-mnemonic", "-m", "x"]).is_ok()
        );
        assert!(Cli::try_parse_from(["cwbwallet", "account", "import-key", "-k", "x"]).is_ok());
        assert!(Cli::try_parse_from(["cwbwallet", "gas-price"]).is_ok());
        assert!(Cli::try_parse_from(["cwbwallet", "chain-info"]).is_ok());
        assert!(
            Cli::try_parse_from(["cwbwallet", "network", "set-rpc", "testnet", "http://x"]).is_ok()
        );
        assert!(Cli::try_parse_from(["cwbwallet", "utils", "from-wei", "1"]).is_ok());
        assert!(Cli::try_parse_from(["cwbwallet", "utils", "new-mnemonic"]).is_ok());
    }

    #[test]
    fn unknown_commands_are_a_parse_error() {
        assert!(Cli::try_parse_from(["cwbwallet", "teleport"]).is_err());
        assert!(Cli::try_parse_from(["cwbwallet", "account", "obliterate"]).is_err());
    }
}
