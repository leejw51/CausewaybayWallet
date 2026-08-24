//! Command implementations. Every command returns a `CommandOutput` so the
//! caller decides between human text and the JSON envelope.
//!
//! Nothing here prints, reads stdin, or exits: a command is a function from a
//! [`Command`] to a [`CommandOutput`]. Where one genuinely needs the outside
//! world — a secret passed as `-`, a yes before spending — it goes through
//! [`Host`], so the same code backs the terminal, the TUI and the C ABI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{keccak256, Address, U256};
use serde_json::{json, Value};

use crate::bip39;
use crate::command::*;
use crate::erc20;
use crate::error::{self, Code, Error, Result};
use crate::host::{Headless, Host};
use crate::network::{self, Network};
use crate::output::{self, CommandOutput};
use crate::paths;
use crate::request::{Parsed, Request};
use crate::rpc::RpcClient;
use crate::store::{self, Account, Source, Store, TxRecord};
use crate::tx::LegacyTransaction;
use crate::units;
use crate::wallet::{self, Keypair};

/// How long `--wait` polls for a receipt before giving up.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(180);

/// A send that has passed every check and is waiting only on a yes.
///
/// Splitting the plan from the execution is what lets the TUI run its own
/// confirmation prompt while still going through the same validation, gas
/// resolution and funding check the CLI uses.
pub struct SendPlan {
    pub keypair: Keypair,
    pub to: Address,
    pub value: U256,
    pub nonce: u64,
    pub gas_price: U256,
    pub gas_limit: u64,
    pub data: Vec<u8>,
    /// The question a caller should put to the user.
    pub prompt: String,
}

pub struct App {
    pub store: Store,
    pub network: Network,
    /// Where a `-` argument and a confirmation prompt are answered.
    pub host: Arc<dyn Host>,
}

impl App {
    pub fn new(
        home: Option<PathBuf>,
        network_override: Option<&str>,
        host: Arc<dyn Host>,
    ) -> Result<Self> {
        let home = paths::resolve_home(home.as_deref())?;
        let store = Store::open(home)?;
        let network = match network_override {
            Some(key) => network::find(key)?,
            None => store.network()?,
        };
        Ok(App {
            store,
            network,
            host,
        })
    }

    /// Open the wallet a [`Request`] describes, with a host built from it.
    ///
    /// The one call a foreign front end needs: it turns `home`, `network`,
    /// `yes` and `stdin` into an `App` without the caller having to know that
    /// hosts exist.
    pub fn open(request: &Request) -> Result<App> {
        let host = Headless::new()
            .assume_yes(request.yes)
            .with_input(request.stdin.clone());
        App::new(
            request.home.clone(),
            request.network.as_deref(),
            Arc::new(host),
        )
    }

    /// Parse a request's arguments and run them, in one step.
    ///
    /// Returns `None` when clap had something to say instead — `--help` and
    /// `--version` are not commands and produce no wallet state.
    pub fn execute(request: &Request) -> Result<CommandOutput> {
        match request.parse()? {
            Parsed::Message(text) => Ok(CommandOutput::message(text)),
            Parsed::Command(cli) => {
                let host = Headless::new()
                    .assume_yes(cli.yes)
                    .with_input(request.stdin.clone());
                let app = App::new(cli.home.clone(), cli.network.as_deref(), Arc::new(host))?;
                app.run(cli.command)
            }
        }
    }

    /// The same wallet answering to a different host.
    ///
    /// The TUI uses it: it puts its own confirmation dialog on the screen, so
    /// by the time a command runs the question has already been asked.
    pub fn with_host(self, host: Arc<dyn Host>) -> App {
        App { host, ..self }
    }

    /// A second `App` over the same home, for when the network changes.
    pub fn reopen(&self) -> Result<App> {
        App::new(
            Some(self.store.home().to_path_buf()),
            Some(self.network.key),
            Arc::clone(&self.host),
        )
    }

    /// An RPC client pointed at the active network.
    pub fn rpc(&self) -> Result<RpcClient> {
        let configured = self.store.config_get(&self.network.rpc_config_key())?;
        RpcClient::new(self.network.resolve_rpc(configured.as_deref()))
    }

    /// The account a command should act on: `--account`, else the active one.
    fn pick_account(&self, selector: Option<&str>) -> Result<Account> {
        match selector {
            Some(s) => self.store.find_account(s),
            None => self.store.active_account(),
        }
    }

    /// The address a read-only query should use.
    fn pick_address(&self, target: &TargetArgs) -> Result<(Address, Option<Account>)> {
        if let Some(raw) = &target.address {
            return Ok((wallet::parse_address(raw)?, None));
        }
        let account = self.pick_account(target.account.as_deref())?;
        let address = wallet::parse_address(&account.address)?;
        Ok((address, Some(account)))
    }

    /// Ask before doing something irreversible.
    ///
    /// Whether that means a terminal prompt, a GUI dialog, or a flat refusal
    /// is the host's business; a command only needs the yes or the error.
    fn confirm(&self, prompt: &str) -> Result<()> {
        self.host.confirm(prompt)
    }

    // ================================================================ dispatch

    pub fn run(&self, command: Command) -> Result<CommandOutput> {
        match command {
            Command::Account { command } => self.account(command),
            Command::Recent { command } => self.recent(command),
            Command::Network { command } => self.network_cmd(command),
            Command::Balance(target) => self.balance(target),
            Command::Nonce(target) => self.nonce(target),
            Command::GasPrice => self.gas_price(),
            Command::ChainInfo => self.chain_info(),
            Command::Send(args) => self.send(args),
            Command::Tx { hash } => self.tx(&hash),
            Command::History { limit, network } => self.history(limit, network.as_deref()),
            Command::Sign { message, account } => self.sign(&message, account.as_deref()),
            Command::Verify {
                message,
                signature,
                address,
            } => self.verify(&message, &signature, address.as_deref()),
            Command::Erc20 { command } => self.erc20(command),
            Command::Utils { command } => self.utils(command),
            // The TUI is a terminal front end and lives in the CLI crate; a
            // library has no business seizing the screen. `cwbwallet`
            // intercepts this before it reaches here.
            Command::Tui => Err(error::usage(
                "the terminal UI is not available through this front end",
            )),
            Command::Info => self.info(),
        }
    }

    // ================================================================ accounts

    pub fn account(&self, command: AccountCommand) -> Result<CommandOutput> {
        match command {
            AccountCommand::New {
                label,
                new_seed,
                words,
                index,
                show_secret,
            } => {
                // A wallet holds one mnemonic and many addresses derived from
                // it, so a new address continues that sequence. Only an empty
                // wallet — or an explicit --new-seed — mints a fresh phrase.
                let existing = if new_seed {
                    None
                } else {
                    self.store.current_seed()?
                };
                let (mnemonic, minted) = match existing {
                    Some(phrase) => (phrase, false),
                    None => (bip39::generate(words)?, true),
                };
                let index = match index {
                    Some(chosen) => chosen,
                    None => self.store.next_address_index(&mnemonic)?,
                };

                let keypair = Keypair::from_mnemonic(&mnemonic, index, "")?;
                let account = self.store.create_account(
                    label.as_deref(),
                    &keypair.address().to_checksum(None),
                    Source::Mnemonic,
                    &keypair.private_key_hex(),
                    Some(&mnemonic),
                    Some(&crate::bip32::ethereum_path(index)),
                    Some(index),
                )?;
                self.activate_if_first(&account)?;
                self.remember_mnemonic(&mnemonic, "")?;

                let mut data = account.public_view();
                data["new_seed"] = json!(minted);
                if show_secret {
                    data["mnemonic"] = json!(mnemonic);
                    data["private_key"] = json!(keypair.private_key_hex());
                }

                let mut rows = vec![
                    ("Source", "mnemonic".to_string()),
                    ("Path", account.derivation_path.clone().unwrap_or_default()),
                    ("Address index", index.to_string()),
                ];
                if minted {
                    rows.push(("Seed", format!("new, {words} words")));
                } else {
                    rows.push(("Seed", "the wallet's existing mnemonic".to_string()));
                }
                let mut human = format!(
                    "Created {} ({})\n{}",
                    account.label,
                    account.address,
                    output::table(&rows)
                );
                if show_secret {
                    human.push_str(&format!("\n\nMnemonic (write it down):\n  {mnemonic}"));
                } else if minted {
                    human.push_str(
                        "\n\nThe mnemonic is stored in the wallet; reveal it with `account export`.",
                    );
                }
                Ok(CommandOutput::new(data, human))
            }

            AccountCommand::ImportMnemonic {
                mnemonic,
                index,
                label,
                passphrase,
            } => {
                let phrase = read_secret(self.host.as_ref(), mnemonic, "mnemonic")?;
                let keypair = Keypair::from_mnemonic(&phrase, index, &passphrase)?;
                let normalized = bip39::normalize(&phrase);
                let account = self.store.create_account(
                    label.as_deref(),
                    &keypair.address().to_checksum(None),
                    Source::Mnemonic,
                    &keypair.private_key_hex(),
                    Some(&normalized),
                    Some(&crate::bip32::ethereum_path(index)),
                    Some(index),
                )?;
                self.activate_if_first(&account)?;
                self.remember_mnemonic(&normalized, &passphrase)?;
                Ok(CommandOutput::new(
                    account.public_view(),
                    format!("Imported {} ({})", account.label, account.address),
                ))
            }

            AccountCommand::ImportKey { private_key, label } => {
                let raw = read_secret(self.host.as_ref(), private_key, "private key")?;
                let keypair = Keypair::from_hex(&raw)?;
                let account = self.store.create_account(
                    label.as_deref(),
                    &keypair.address().to_checksum(None),
                    Source::PrivateKey,
                    &keypair.private_key_hex(),
                    None,
                    None,
                    None,
                )?;
                self.activate_if_first(&account)?;
                self.store.remember_secret(
                    "private_key",
                    &keypair.private_key_hex(),
                    &keypair.address().to_checksum(None),
                    None,
                )?;
                Ok(CommandOutput::new(
                    account.public_view(),
                    format!("Imported {} ({})", account.label, account.address),
                ))
            }

            AccountCommand::List {
                format,
                output,
                secret,
            } => {
                let accounts = self.store.accounts()?;
                let active = self.store.active_account().ok().map(|a| a.id);

                // `--format` turns this into an export; without it the command
                // behaves exactly as it always has.
                if let Some(name) = format {
                    let format = crate::export::Format::parse(&name)?;
                    let rendered =
                        crate::export::render(&accounts, format, active.as_deref(), secret);
                    return match output {
                        Some(path) => {
                            if secret {
                                // Born 0600: it holds private keys from its first byte.
                                paths::write_private(&path, &rendered).map_err(|e| {
                                    Error::new(
                                        Code::IoError,
                                        format!("cannot write {}: {e}", path.display()),
                                    )
                                })?;
                            } else {
                                std::fs::write(&path, &rendered).map_err(|e| {
                                    Error::new(
                                        Code::IoError,
                                        format!("cannot write {}: {e}", path.display()),
                                    )
                                })?;
                            }
                            let shown = path.display().to_string();
                            Ok(CommandOutput::new(
                                json!({
                                    "format": format.as_str(),
                                    "path": shown,
                                    "count": accounts.len(),
                                    "secret": secret,
                                }),
                                format!("Saved {} wallets to {shown}", accounts.len()),
                            ))
                        }
                        None => Ok(CommandOutput::new(
                            json!({
                                "format": format.as_str(),
                                "path": null,
                                "count": accounts.len(),
                                "secret": secret,
                                "content": rendered,
                            }),
                            // Trailing newline is already in the rendering.
                            rendered.trim_end().to_string(),
                        )),
                    };
                }
                let data: Vec<Value> = accounts
                    .iter()
                    .map(|a| {
                        let mut value = a.public_view();
                        value["active"] = json!(Some(&a.id) == active.as_ref());
                        value
                    })
                    .collect();

                let human = if accounts.is_empty() {
                    "No accounts yet. Create one with `cwbwallet account new`.".to_string()
                } else {
                    let mut lines = Vec::with_capacity(accounts.len());
                    for a in &accounts {
                        let marker = if Some(&a.id) == active.as_ref() {
                            "*"
                        } else {
                            " "
                        };
                        lines.push(format!(
                            "{marker} {:<16} {}  {}",
                            a.label,
                            a.address,
                            a.source.as_str()
                        ));
                    }
                    lines.join("\n")
                };
                Ok(CommandOutput::new(json!(data), human))
            }

            AccountCommand::Show { selector, secret } => {
                let account = self.pick_account(selector.as_deref())?;
                let keypair = Keypair::from_hex(&account.private_key)?;
                let mut data = if secret {
                    account.secret_view()
                } else {
                    account.public_view()
                };
                data["public_key"] = json!(keypair.public_key_hex());
                data["public_key_compressed"] = json!(keypair.public_key_compressed_hex());

                let mut rows = vec![
                    ("Label", account.label.clone()),
                    ("Address", account.address.clone()),
                    ("Id", account.id.clone()),
                    ("Source", account.source.as_str().to_string()),
                ];
                if let Some(path) = &account.derivation_path {
                    rows.push(("Path", path.clone()));
                }
                rows.push((
                    "Private key",
                    if secret {
                        account.private_key.clone()
                    } else {
                        output::truncate_secret(&account.private_key)
                    },
                ));
                if let Some(mnemonic) = &account.mnemonic {
                    rows.push((
                        "Mnemonic",
                        if secret {
                            mnemonic.clone()
                        } else {
                            "<hidden — use --secret>".into()
                        },
                    ));
                }
                Ok(CommandOutput::new(data, output::table(&rows)))
            }

            AccountCommand::Use { selector } => {
                let account = self.store.find_account(&selector)?;
                self.store
                    .config_set(store::KEY_ACTIVE_ACCOUNT, &account.id)?;
                Ok(CommandOutput::new(
                    account.public_view(),
                    format!(
                        "Active account is now {} ({})",
                        account.label, account.address
                    ),
                ))
            }

            AccountCommand::Derive { index, label, from } => {
                let parent = self.pick_account(from.as_deref())?;
                let mnemonic = parent.mnemonic.clone().ok_or_else(|| {
                    error::usage(format!(
                        "account '{}' was imported from a private key, so it has no mnemonic to derive from",
                        parent.label
                    ))
                })?;
                // A wallet created with a BIP-39 passphrase stores the phrase
                // but not the passphrase, so deriving with the phrase alone
                // would silently produce an unrelated address.
                if let Some(parent_index) = parent.index {
                    let plain = Keypair::from_mnemonic(&mnemonic, parent_index, "")?;
                    if plain.address().to_checksum(None) != parent.address {
                        return Err(error::usage(format!(
                            "'{}' was created with a BIP-39 passphrase, so its mnemonic \
                             alone derives a different wallet; re-import it with \
                             --passphrase and derive from that",
                            parent.label
                        )));
                    }
                }

                let keypair = Keypair::from_mnemonic(&mnemonic, index, "")?;
                let account = self.store.create_account(
                    label.as_deref(),
                    &keypair.address().to_checksum(None),
                    Source::Mnemonic,
                    &keypair.private_key_hex(),
                    Some(&mnemonic),
                    Some(&crate::bip32::ethereum_path(index)),
                    Some(index),
                )?;
                self.remember_mnemonic(&mnemonic, "")?;
                Ok(CommandOutput::new(
                    account.public_view(),
                    format!(
                        "Derived {} ({}) at index {index} from {}",
                        account.label, account.address, parent.label
                    ),
                ))
            }

            AccountCommand::Rename { selector, label } => {
                let account = self.store.find_account(&selector)?;
                self.store.rename_account(&account.id, &label)?;
                Ok(CommandOutput::new(
                    json!({"id": account.id, "label": label, "address": account.address}),
                    format!("Renamed {} to {label}", account.label),
                ))
            }

            AccountCommand::Remove { selector } => {
                let account = self.store.find_account(&selector)?;
                self.confirm(&format!(
                    "Remove account {} ({})? Its key is only in this wallet",
                    account.label, account.address
                ))?;
                self.store.delete_account(&account.id)?;
                Ok(CommandOutput::new(
                    json!({"id": account.id, "label": account.label, "removed": true}),
                    format!("Removed {}", account.label),
                ))
            }

            AccountCommand::ImportRecent {
                selector,
                index,
                label,
                passphrase,
            } => {
                let entry = match selector.as_deref() {
                    Some(s) => self.store.find_recent(s)?,
                    None => self
                        .store
                        .recent()?
                        .into_iter()
                        .next()
                        .ok_or_else(|| error::not_found("nothing has been remembered yet"))?,
                };

                let output = if entry.kind == "mnemonic" {
                    // A phrase remembered with a passphrase names a wallet the
                    // phrase alone cannot reach. Restoring without it would hand
                    // back a different, unfunded address and say nothing, so
                    // refuse — and check the result before storing it.
                    if entry.has_passphrase && passphrase.is_empty() {
                        return Err(error::usage(format!(
                            "the remembered mnemonic for {} was used with a BIP-39 \
                             passphrase; pass --passphrase to restore that wallet",
                            entry.address
                        )));
                    }
                    let derived = Keypair::from_mnemonic(&entry.secret, 0, &passphrase)?;
                    if derived.address().to_checksum(None) != entry.address {
                        return Err(error::usage(format!(
                            "that passphrase derives {}, not the remembered {} — \
                             restoring it would create a different wallet",
                            derived.address().to_checksum(None),
                            entry.address
                        )));
                    }
                    self.account(AccountCommand::ImportMnemonic {
                        mnemonic: Some(entry.secret.clone()),
                        index,
                        label,
                        passphrase,
                    })?
                } else {
                    self.account(AccountCommand::ImportKey {
                        private_key: Some(entry.secret.clone()),
                        label,
                    })?
                };
                Ok(CommandOutput::new(
                    output.data,
                    format!(
                        "{}\n(from remembered {} {})",
                        output.human, entry.kind, entry.id
                    ),
                ))
            }

            AccountCommand::Export { selector } => {
                let account = self.pick_account(selector.as_deref())?;
                let mut rows = vec![
                    ("Label", account.label.clone()),
                    ("Address", account.address.clone()),
                    ("Private key", account.private_key.clone()),
                ];
                if let Some(mnemonic) = &account.mnemonic {
                    rows.push(("Mnemonic", mnemonic.clone()));
                }
                Ok(CommandOutput::new(
                    account.secret_view(),
                    format!("{}\n\n{}", output::WARNING, output::table(&rows)),
                ))
            }
        }
    }

    /// Add a mnemonic to the recall list, keyed by the address it derives.
    fn remember_mnemonic(&self, mnemonic: &str, passphrase: &str) -> Result<()> {
        let normalized = bip39::normalize(mnemonic);
        // Keyed by the address the passphrase actually produces, so the recall
        // list identifies the wallet the user has rather than a different one
        // that happens to share the phrase.
        let root = Keypair::from_mnemonic(&normalized, 0, passphrase)?;
        self.store.remember_secret_with(
            "mnemonic",
            &normalized,
            &root.address().to_checksum(None),
            Some(normalized.split(' ').count()),
            !passphrase.is_empty(),
        )?;
        Ok(())
    }

    // ================================================================== recall

    fn recent(&self, command: RecentCommand) -> Result<CommandOutput> {
        match command {
            RecentCommand::List { kind, limit } => {
                let wanted = kind.map(|k| k.replace('-', "_"));
                let mut entries = self.store.recent()?;
                if let Some(kind) = &wanted {
                    entries.retain(|entry| &entry.kind == kind);
                }
                entries.truncate(limit);

                let data: Vec<Value> = entries
                    .iter()
                    .enumerate()
                    .map(|(position, entry)| {
                        let mut value = entry.public_view();
                        value["position"] = json!(position + 1);
                        value
                    })
                    .collect();

                let human = if entries.is_empty() {
                    "Nothing remembered yet. Create or import an account and it will appear here."
                        .to_string()
                } else {
                    entries
                        .iter()
                        .enumerate()
                        .map(|(position, entry)| {
                            format!(
                                "{:>2}. {:<12} {}  {:<28} used {}x  {}",
                                position + 1,
                                entry.kind,
                                entry.address,
                                entry.preview(),
                                entry.uses,
                                &entry.last_used_at[..entry.last_used_at.len().min(19)],
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Ok(CommandOutput::new(json!(data), human))
            }

            RecentCommand::Show { selector, secret } => {
                let entry = match selector.as_deref() {
                    Some(s) => self.store.find_recent(s)?,
                    None => self
                        .store
                        .recent()?
                        .into_iter()
                        .next()
                        .ok_or_else(|| error::not_found("nothing has been remembered yet"))?,
                };
                let mut rows = vec![
                    ("Id", entry.id.clone()),
                    ("Kind", entry.kind.clone()),
                    ("Address", entry.address.clone()),
                    ("Uses", entry.uses.to_string()),
                    ("Last used", entry.last_used_at.clone()),
                ];
                rows.push((
                    if entry.kind == "mnemonic" {
                        "Mnemonic"
                    } else {
                        "Private key"
                    },
                    if secret {
                        entry.secret.clone()
                    } else {
                        entry.preview()
                    },
                ));
                let human = if secret {
                    format!("{}\n\n{}", output::WARNING, output::table(&rows))
                } else {
                    output::table(&rows)
                };
                Ok(CommandOutput::new(
                    if secret {
                        entry.secret_view()
                    } else {
                        entry.public_view()
                    },
                    human,
                ))
            }

            RecentCommand::Forget { selector } => {
                let entry = self.store.find_recent(&selector)?;
                self.confirm(&format!(
                    "Forget the remembered {} for {}? Any account already created from it is kept",
                    entry.kind, entry.address
                ))?;
                self.store.forget_secret(&entry.id)?;
                Ok(CommandOutput::new(
                    json!({"id": entry.id, "forgotten": true}),
                    format!("Forgot {} ({})", entry.id, entry.address),
                ))
            }

            RecentCommand::Clear => {
                let count = self.store.recent()?.len();
                if count == 0 {
                    return Ok(CommandOutput::new(
                        json!({"forgotten": 0}),
                        "Nothing to forget.",
                    ));
                }
                self.confirm(&format!("Forget all {count} remembered secrets"))?;
                let forgotten = self.store.clear_recent()?;
                Ok(CommandOutput::new(
                    json!({"forgotten": forgotten}),
                    format!("Forgot {forgotten} remembered secrets"),
                ))
            }
        }
    }

    /// The very first account becomes active automatically.
    fn activate_if_first(&self, account: &Account) -> Result<()> {
        if self.store.accounts()?.len() == 1 {
            self.store
                .config_set(store::KEY_ACTIVE_ACCOUNT, &account.id)?;
        }
        Ok(())
    }

    // ================================================================= network

    fn network_cmd(&self, command: NetworkCommand) -> Result<CommandOutput> {
        match command {
            NetworkCommand::List => {
                let current = self.store.network()?;
                let data: Vec<Value> = network::ALL
                    .iter()
                    .map(|n| {
                        json!({
                            "key": n.key,
                            "name": n.name,
                            "chain_id": n.chain_id,
                            "symbol": n.symbol,
                            "rpc": n.resolve_rpc(
                                self.store.config_get(&n.rpc_config_key()).ok().flatten().as_deref()
                            ),
                            "explorer": n.explorer,
                            "testnet": n.testnet,
                            "current": n.key == current.key,
                        })
                    })
                    .collect();
                let human = network::ALL
                    .iter()
                    .map(|n| {
                        format!(
                            "{} {:<16} chain {:<4} {}",
                            if n.key == current.key { "*" } else { " " },
                            n.key,
                            n.chain_id,
                            n.name
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(CommandOutput::new(json!(data), human))
            }

            NetworkCommand::Current => {
                let n = self.network;
                let rpc = n.resolve_rpc(self.store.config_get(&n.rpc_config_key())?.as_deref());
                Ok(CommandOutput::new(
                    json!({
                        "key": n.key, "name": n.name, "chain_id": n.chain_id,
                        "symbol": n.symbol, "rpc": rpc, "explorer": n.explorer, "testnet": n.testnet,
                    }),
                    output::table(&[
                        ("Network", n.name.into()),
                        ("Key", n.key.into()),
                        ("Chain id", n.chain_id.to_string()),
                        ("Symbol", n.symbol.into()),
                        ("RPC", rpc),
                        ("Explorer", n.explorer.into()),
                    ]),
                ))
            }

            NetworkCommand::Use { network } => {
                let target = network::find(&network)?;
                self.store.config_set(store::KEY_NETWORK, target.key)?;
                Ok(CommandOutput::new(
                    json!({"key": target.key, "name": target.name, "chain_id": target.chain_id}),
                    format!("Network is now {} (chain {})", target.name, target.chain_id),
                ))
            }

            NetworkCommand::SetRpc { network, url } => {
                let target = network::find(&network)?;
                self.store
                    .config_set(&target.rpc_config_key(), url.trim())?;
                let effective = target.resolve_rpc(if url.trim().is_empty() {
                    None
                } else {
                    Some(&url)
                });
                Ok(CommandOutput::new(
                    json!({"key": target.key, "rpc": effective}),
                    format!("RPC for {} is now {effective}", target.key),
                ))
            }
        }
    }

    // ============================================================ chain reads

    pub fn balance(&self, target: TargetArgs) -> Result<CommandOutput> {
        let (address, account) = self.pick_address(&target)?;
        let wei = self.rpc()?.get_balance(address)?;
        let formatted = units::format_ether(wei);
        Ok(CommandOutput::new(
            json!({
                "address": address.to_checksum(None),
                "account": account.as_ref().map(|a| a.label.clone()),
                "balance": formatted,
                "balance_wei": wei.to_string(),
                "symbol": self.network.symbol,
                "network": self.network.key,
            }),
            format!(
                "{formatted} {} — {}",
                self.network.symbol,
                address.to_checksum(None)
            ),
        ))
    }

    fn nonce(&self, target: TargetArgs) -> Result<CommandOutput> {
        let (address, _) = self.pick_address(&target)?;
        let nonce = self.rpc()?.get_transaction_count(address)?;
        Ok(CommandOutput::new(
            json!({"address": address.to_checksum(None), "nonce": nonce, "network": self.network.key}),
            format!("nonce {nonce} — {}", address.to_checksum(None)),
        ))
    }

    fn gas_price(&self) -> Result<CommandOutput> {
        let wei = self.rpc()?.gas_price()?;
        let gwei = units::format_gwei(wei);
        Ok(CommandOutput::new(
            json!({"gas_price_wei": wei.to_string(), "gas_price_gwei": gwei, "network": self.network.key}),
            format!("{gwei} gwei"),
        ))
    }

    fn chain_info(&self) -> Result<CommandOutput> {
        let rpc = self.rpc()?;
        let reported = rpc.chain_id()?;
        let block = rpc.block_number()?;
        let gas = rpc.gas_price()?;
        let matches = reported == self.network.chain_id;
        Ok(CommandOutput::new(
            json!({
                "network": self.network.key,
                "name": self.network.name,
                "expected_chain_id": self.network.chain_id,
                "reported_chain_id": reported,
                "chain_id_matches": matches,
                "block_number": block,
                "gas_price_wei": gas.to_string(),
                "rpc": rpc.url(),
            }),
            output::table(&[
                ("Network", self.network.name.into()),
                ("RPC", rpc.url().to_string()),
                (
                    "Chain id",
                    format!("{reported}{}", if matches { "" } else { " (MISMATCH!)" }),
                ),
                ("Block", block.to_string()),
                ("Gas price", format!("{} gwei", units::format_gwei(gas))),
            ]),
        ))
    }

    // ================================================================== sends

    fn send(&self, args: SendArgs) -> Result<CommandOutput> {
        let plan = self.plan_send(&args)?;
        self.confirm(&plan.prompt)?;
        self.execute_send(plan, args.wait)
    }

    /// Resolve everything a transfer needs and check it can actually be paid for.
    pub fn plan_send(&self, args: &SendArgs) -> Result<SendPlan> {
        let account = self.pick_account(args.account.as_deref())?;
        let keypair = Keypair::from_hex(&account.private_key)?;
        let to = wallet::parse_address(&args.to)?;
        // A transfer to the account it leaves from moves nothing and costs the
        // gas anyway. It is almost always a paste into the wrong field — the
        // sender's own address is the one most likely to be on the clipboard —
        // so it is refused here, before a node is asked anything.
        if to == keypair.address() {
            return Err(error::usage(format!(
                "the recipient is the sending account ({}); a transfer to itself \
                 moves nothing and still pays the gas",
                to.to_checksum(None)
            )));
        }
        let value = units::parse_ether(&args.amount)?;
        let data = match &args.data {
            Some(hex_data) => wallet::parse_hex(hex_data)?,
            None => Vec::new(),
        };

        let rpc = self.rpc()?;
        let nonce = match args.nonce {
            Some(n) => n,
            None => rpc.get_transaction_count(keypair.address())?,
        };
        let gas_price = match &args.gas_price_gwei {
            Some(gwei) => units::parse_gwei(gwei)?,
            None => rpc.gas_price()?,
        };
        let gas_limit = match args.gas_limit {
            Some(limit) => limit,
            None if data.is_empty() => 21_000,
            None => with_headroom(rpc.estimate_gas(keypair.address(), to, value, &data)?),
        };

        // Fail before signing when the balance obviously cannot cover the transfer.
        let balance = rpc.get_balance(keypair.address())?;
        let max_cost = value + gas_price * U256::from(gas_limit);
        if balance < max_cost {
            return Err(error::insufficient_funds(format!(
                "balance {} {} cannot cover {} {} plus up to {} {} of gas",
                units::format_ether(balance),
                self.network.symbol,
                units::format_ether(value),
                self.network.symbol,
                units::format_ether(gas_price * U256::from(gas_limit)),
                self.network.symbol,
            )));
        }

        Ok(SendPlan {
            keypair,
            to,
            value,
            nonce,
            gas_price,
            gas_limit,
            data,
            prompt: format!(
                "Send {} {} from {} to {} on {}",
                args.amount,
                self.network.symbol,
                account.label,
                to.to_checksum(None),
                self.network.name
            ),
        })
    }

    /// Sign and broadcast a plan the caller has already had confirmed.
    pub fn execute_send(&self, plan: SendPlan, wait: bool) -> Result<CommandOutput> {
        let SendPlan {
            keypair,
            to,
            value,
            nonce,
            gas_price,
            gas_limit,
            data,
            ..
        } = plan;
        let rpc = self.rpc()?;

        let transaction = LegacyTransaction {
            nonce,
            gas_price,
            gas_limit,
            to: Some(to),
            value,
            data,
            chain_id: self.network.chain_id,
        };
        let signed = transaction.sign(&keypair)?;

        // Recorded before broadcasting, using the hash computed locally: if the
        // node accepts the transaction and the response is then lost to a
        // timeout, the history still names it. Losing that record is what leads
        // to a user re-sending a transfer that already went through.
        let hash = signed.hash_hex();
        let mut record = TxRecord {
            hash: hash.clone(),
            from: keypair.address().to_checksum(None),
            to: to.to_checksum(None),
            value: units::format_ether(value),
            value_wei: value.to_string(),
            network: self.network.key.to_string(),
            chain_id: self.network.chain_id,
            nonce,
            gas_limit,
            gas_price_wei: gas_price.to_string(),
            status: "submitting".into(),
            token: None,
            block_number: None,
            gas_used: None,
            created_at: store::now_rfc3339(),
        };
        self.store.record_tx(&record)?;

        match rpc.send_raw_transaction(&signed.raw) {
            Ok(_returned) => {
                // A node echoes the hash back, but the locally computed one is
                // authoritative: it is what was signed, and it is already in the
                // log. A disagreement is the node's problem, not a crash.
                record.status = "submitted".into();
                self.store.update_tx(&hash, "submitted", None, None)?;
            }
            Err(e) => {
                // The node may still have accepted it, so the record stays and
                // is marked for what it is rather than deleted.
                self.store.update_tx(&hash, "unconfirmed", None, None)?;
                return Err(crate::error::Error::new(
                    e.code,
                    format!(
                        "{} — recorded locally as {hash}; check it with `tx {hash}` \
                         before sending again",
                        e.message
                    ),
                ));
            }
        }

        self.finish_send(&rpc, record, wait)
    }

    /// Optionally wait for the receipt, then render the result.
    fn finish_send(
        &self,
        rpc: &RpcClient,
        mut record: TxRecord,
        wait: bool,
    ) -> Result<CommandOutput> {
        if wait {
            if let Some(receipt) = rpc.wait_for_receipt(&record.hash, RECEIPT_TIMEOUT)? {
                let ok = crate::rpc::field_u64(&receipt, "status").unwrap_or(1) == 1;
                let block = crate::rpc::field_u64(&receipt, "blockNumber");
                let gas_used = crate::rpc::field_u64(&receipt, "gasUsed");
                record.status = if ok {
                    "confirmed".into()
                } else {
                    "failed".into()
                };
                record.block_number = block;
                record.gas_used = gas_used;
                self.store
                    .update_tx(&record.hash, &record.status, block, gas_used)?;
            }
        }

        let explorer = self.network.tx_url(&record.hash);
        let mut rows = vec![
            ("Hash", record.hash.clone()),
            ("From", record.from.clone()),
            ("To", record.to.clone()),
            (
                "Amount",
                format!("{} {}", record.value, self.network.symbol),
            ),
            ("Nonce", record.nonce.to_string()),
            ("Status", record.status.clone()),
        ];
        if let Some(block) = record.block_number {
            rows.push(("Block", block.to_string()));
        }
        rows.push(("Explorer", explorer.clone()));

        let mut data = serde_json::to_value(&record)
            .map_err(|e| error::internal(format!("cannot serialise transaction: {e}")))?;
        data["explorer"] = json!(explorer);
        data["symbol"] = json!(self.network.symbol);
        Ok(CommandOutput::new(data, output::table(&rows)))
    }

    fn tx(&self, hash: &str) -> Result<CommandOutput> {
        let rpc = self.rpc()?;
        let transaction = rpc.get_transaction_by_hash(hash)?;
        let receipt = rpc.get_transaction_receipt(hash)?;
        if transaction.is_none() && receipt.is_none() {
            return Err(error::not_found(format!(
                "no transaction {hash} on {}",
                self.network.name
            )));
        }
        let status = match &receipt {
            Some(r) => match crate::rpc::field_u64(r, "status") {
                Some(1) => "confirmed",
                Some(_) => "failed",
                None => "pending",
            },
            None => "pending",
        };
        // Keep the local log in step with what the chain says.
        if receipt.is_some() {
            let block = receipt
                .as_ref()
                .and_then(|r| crate::rpc::field_u64(r, "blockNumber"));
            let gas_used = receipt
                .as_ref()
                .and_then(|r| crate::rpc::field_u64(r, "gasUsed"));
            let _ = self.store.update_tx(hash, status, block, gas_used);
        }

        let value = transaction
            .as_ref()
            .and_then(|t| t.get("value"))
            .and_then(|v| crate::rpc::parse_quantity_u256(v, "value").ok())
            .map(units::format_ether);
        Ok(CommandOutput::new(
            json!({
                "hash": hash,
                "status": status,
                "network": self.network.key,
                "explorer": self.network.tx_url(hash),
                "value": value,
                "transaction": transaction,
                "receipt": receipt,
            }),
            output::table(&[
                ("Hash", hash.to_string()),
                ("Status", status.to_string()),
                (
                    "Value",
                    value
                        .map(|v| format!("{v} {}", self.network.symbol))
                        .unwrap_or_else(|| "-".into()),
                ),
                ("Explorer", self.network.tx_url(hash)),
            ]),
        ))
    }

    fn history(&self, limit: usize, network_filter: Option<&str>) -> Result<CommandOutput> {
        let filter = network_filter.map(network::find).transpose()?;
        let mut entries = self.store.history()?;
        if let Some(network) = filter {
            entries.retain(|tx| tx.network == network.key);
        }
        entries.reverse(); // newest first
        entries.truncate(limit);

        let human = if entries.is_empty() {
            "No transactions recorded yet.".to_string()
        } else {
            entries
                .iter()
                .map(|tx| {
                    format!(
                        "{}  {:<10} {:>12} -> {}  {}",
                        &tx.created_at[..tx.created_at.len().min(19)],
                        tx.status,
                        tx.value,
                        tx.to,
                        tx.hash
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(CommandOutput::new(json!(entries), human))
    }

    // ============================================================== signatures

    fn sign(&self, message: &str, account: Option<&str>) -> Result<CommandOutput> {
        let account = self.pick_account(account)?;
        let keypair = Keypair::from_hex(&account.private_key)?;
        let text = read_message(self.host.as_ref(), message)?;
        let signature = keypair.sign_message(text.as_bytes())?;
        let encoded = format!("0x{}", hex::encode(signature));
        Ok(CommandOutput::new(
            json!({
                "address": account.address,
                "account": account.label,
                "message": text,
                "signature": encoded,
            }),
            output::table(&[("Signer", account.address.clone()), ("Signature", encoded)]),
        ))
    }

    fn verify(
        &self,
        message: &str,
        signature: &str,
        address: Option<&str>,
    ) -> Result<CommandOutput> {
        let text = read_message(self.host.as_ref(), message)?;
        let bytes = wallet::parse_hex(signature)?;
        let recovered = wallet::recover_message(text.as_bytes(), &bytes)?;
        let expected = address.map(wallet::parse_address).transpose()?;
        let valid = expected.map(|e| e == recovered).unwrap_or(true);

        let human = match expected {
            Some(e) if valid => format!("Valid — signed by {}", e.to_checksum(None)),
            Some(e) => format!(
                "INVALID — signature recovers to {} but {} was expected",
                recovered.to_checksum(None),
                e.to_checksum(None)
            ),
            None => format!("Signed by {}", recovered.to_checksum(None)),
        };
        Ok(CommandOutput::new(
            json!({
                "valid": valid,
                "recovered": recovered.to_checksum(None),
                "expected": expected.map(|e| e.to_checksum(None)),
                "message": text,
            }),
            human,
        ))
    }

    // =================================================================== erc20

    fn erc20(&self, command: Erc20Command) -> Result<CommandOutput> {
        match command {
            Erc20Command::Info { token } => {
                let token = wallet::parse_address(&token)?;
                let rpc = self.rpc()?;
                let name = erc20::decode_string(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_NAME))?,
                )?;
                let symbol = erc20::decode_string(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_SYMBOL))?,
                )?;
                let decimals = erc20::decode_u8(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_DECIMALS))?,
                )?;
                let supply = erc20::decode_uint(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_TOTAL_SUPPLY))?,
                )?;
                Ok(CommandOutput::new(
                    json!({
                        "token": token.to_checksum(None),
                        "name": name, "symbol": symbol, "decimals": decimals,
                        "total_supply": units::format_units(supply, decimals),
                        "total_supply_raw": supply.to_string(),
                        "network": self.network.key,
                    }),
                    output::table(&[
                        ("Token", token.to_checksum(None)),
                        ("Name", name),
                        ("Symbol", symbol),
                        ("Decimals", decimals.to_string()),
                        ("Total supply", units::format_units(supply, decimals)),
                    ]),
                ))
            }

            Erc20Command::Balance { token, address } => {
                let token = wallet::parse_address(&token)?;
                let owner = match address {
                    Some(raw) => wallet::parse_address(&raw)?,
                    None => wallet::parse_address(&self.store.active_account()?.address)?,
                };
                let rpc = self.rpc()?;
                let decimals = erc20::decode_u8(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_DECIMALS))?,
                )?;
                let symbol = erc20::decode_string(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_SYMBOL))?,
                )
                .unwrap_or_default();
                let raw =
                    erc20::decode_uint(&rpc.eth_call(token, &erc20::encode_balance_of(owner))?)?;
                let formatted = units::format_units(raw, decimals);
                Ok(CommandOutput::new(
                    json!({
                        "token": token.to_checksum(None),
                        "address": owner.to_checksum(None),
                        "balance": formatted,
                        "balance_raw": raw.to_string(),
                        "decimals": decimals,
                        "symbol": symbol,
                        "network": self.network.key,
                    }),
                    format!("{formatted} {symbol} — {}", owner.to_checksum(None)),
                ))
            }

            Erc20Command::Send {
                token,
                to,
                amount,
                wait,
                account,
            } => {
                let account = self.pick_account(account.as_deref())?;
                let keypair = Keypair::from_hex(&account.private_key)?;
                let token = wallet::parse_address(&token)?;
                let recipient = wallet::parse_address(&to)?;
                // Same rule as a native transfer: sending a token to the account
                // holding it changes no balance and still burns the gas.
                if recipient == keypair.address() {
                    return Err(error::usage(format!(
                        "the recipient is the sending account ({}); a transfer to \
                         itself moves nothing and still pays the gas",
                        recipient.to_checksum(None)
                    )));
                }

                let rpc = self.rpc()?;
                let decimals = erc20::decode_u8(
                    &rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_DECIMALS))?,
                )?;
                let raw_amount = units::parse_units(&amount, decimals)?;
                let held = erc20::decode_uint(
                    &rpc.eth_call(token, &erc20::encode_balance_of(keypair.address()))?,
                )?;
                if held < raw_amount {
                    return Err(error::insufficient_funds(format!(
                        "token balance {} is less than {amount}",
                        units::format_units(held, decimals)
                    )));
                }

                let data = erc20::encode_transfer(recipient, raw_amount);
                let gas_limit = rpc
                    .estimate_gas(keypair.address(), token, U256::ZERO, &data)
                    .map(with_headroom)
                    .unwrap_or(100_000);
                let gas_price = rpc.gas_price()?;
                let nonce = rpc.get_transaction_count(keypair.address())?;

                self.confirm(&format!(
                    "Transfer {amount} of token {} from {} to {} on {}",
                    token.to_checksum(None),
                    account.label,
                    recipient.to_checksum(None),
                    self.network.name
                ))?;

                let transaction = LegacyTransaction {
                    nonce,
                    gas_price,
                    gas_limit,
                    to: Some(token),
                    value: U256::ZERO,
                    data,
                    chain_id: self.network.chain_id,
                };
                let signed = transaction.sign(&keypair)?;
                let hash = rpc.send_raw_transaction(&signed.raw)?;

                let record = TxRecord {
                    hash,
                    from: keypair.address().to_checksum(None),
                    to: recipient.to_checksum(None),
                    value: amount.clone(),
                    value_wei: raw_amount.to_string(),
                    network: self.network.key.to_string(),
                    chain_id: self.network.chain_id,
                    nonce,
                    gas_limit,
                    gas_price_wei: gas_price.to_string(),
                    status: "submitted".into(),
                    token: Some(token.to_checksum(None)),
                    block_number: None,
                    gas_used: None,
                    created_at: store::now_rfc3339(),
                };
                self.store.record_tx(&record)?;
                self.finish_send(&rpc, record, wait)
            }
        }
    }

    // =================================================================== utils

    fn utils(&self, command: UtilsCommand) -> Result<CommandOutput> {
        match command {
            UtilsCommand::Keccak { input, hex: as_hex } => {
                let bytes = if as_hex {
                    wallet::parse_hex(&input)?
                } else {
                    input.as_bytes().to_vec()
                };
                let digest = format!("0x{}", hex::encode(keccak256(&bytes)));
                Ok(CommandOutput::new(
                    json!({"input": input, "keccak256": digest}),
                    digest,
                ))
            }
            UtilsCommand::Checksum { address } => {
                let parsed = wallet::parse_address(&address)?.to_checksum(None);
                Ok(CommandOutput::new(json!({"address": parsed}), parsed))
            }
            UtilsCommand::ToWei { amount, decimals } => {
                let value = units::parse_units(&amount, decimals)?;
                Ok(CommandOutput::new(
                    json!({"amount": amount, "decimals": decimals, "value": value.to_string()}),
                    value.to_string(),
                ))
            }
            UtilsCommand::FromWei { value, decimals } => {
                let raw = U256::from_str_radix(value.trim(), 10)
                    .map_err(|_| error::invalid_amount(format!("not an integer: {value}")))?;
                let amount = units::format_units(raw, decimals);
                Ok(CommandOutput::new(
                    json!({"value": raw.to_string(), "decimals": decimals, "amount": amount}),
                    amount,
                ))
            }
            UtilsCommand::NewMnemonic { words } => {
                let phrase = bip39::generate(words)?;
                Ok(CommandOutput::new(
                    json!({"mnemonic": phrase, "words": words}),
                    phrase,
                ))
            }
            UtilsCommand::Derive {
                mnemonic,
                private_key,
                index,
                passphrase,
            } => self.utils_derive(mnemonic, private_key, index, &passphrase),
            UtilsCommand::Sign {
                private_key,
                message,
            } => self.utils_sign(private_key, &message),
            UtilsCommand::ValidateMnemonic { mnemonic } => self.utils_validate_mnemonic(&mnemonic),
        }
    }

    /// Derive key material and show it, storing nothing.
    ///
    /// The same derivation `account import-mnemonic` does, with the wallet
    /// left out of it — for a caller that wants an address from a phrase
    /// without acquiring an account and a recall entry as side effects.
    fn utils_derive(
        &self,
        mnemonic: Option<String>,
        private_key: Option<String>,
        index: u32,
        passphrase: &str,
    ) -> Result<CommandOutput> {
        // clap's ArgGroup guarantees exactly one of the two is present.
        let (keypair, data) = match (mnemonic, private_key) {
            (Some(phrase), _) => {
                let phrase = read_secret(self.host.as_ref(), Some(phrase), "mnemonic")?;
                let keypair = Keypair::from_mnemonic(&phrase, index, passphrase)?;
                let path = crate::bip32::ethereum_path(index);
                (
                    keypair,
                    json!({
                        "source": "mnemonic",
                        "derivation_path": path,
                        "index": index,
                    }),
                )
            }
            (None, Some(key)) => {
                let key = read_secret(self.host.as_ref(), Some(key), "private key")?;
                (Keypair::from_hex(&key)?, json!({"source": "private_key"}))
            }
            (None, None) => return Err(error::usage("pass either --mnemonic or --private-key")),
        };

        let address = keypair.address().to_checksum(None);
        let mut payload = json!({
            "address": address,
            "private_key": keypair.private_key_hex(),
            "public_key": keypair.public_key_hex(),
            "public_key_compressed": keypair.public_key_compressed_hex(),
        });
        // Merge the source-specific half in, so both shapes share one schema.
        if let (Some(target), Some(extra)) = (payload.as_object_mut(), data.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }

        let mut rows = vec![("Address", address), ("Source", source_of(&payload))];
        if let Some(path) = payload["derivation_path"].as_str() {
            rows.push(("Path", path.to_string()));
            rows.push(("Address index", index.to_string()));
        }
        rows.push(("Public key", keypair.public_key_compressed_hex()));
        rows.push(("Private key", keypair.private_key_hex()));
        Ok(CommandOutput::new(payload.clone(), output::table(&rows)))
    }

    /// Sign a message with a key the wallet does not hold.
    fn utils_sign(&self, private_key: String, message: &str) -> Result<CommandOutput> {
        let key = read_secret(self.host.as_ref(), Some(private_key), "private key")?;
        let keypair = Keypair::from_hex(&key)?;
        let text = read_message(self.host.as_ref(), message)?;
        let signature = format!("0x{}", hex::encode(keypair.sign_message(text.as_bytes())?));
        let address = keypair.address().to_checksum(None);
        Ok(CommandOutput::new(
            json!({"address": address, "message": text, "signature": signature}),
            signature,
        ))
    }

    /// Report whether a phrase is a valid mnemonic, and why not when it is not.
    fn utils_validate_mnemonic(&self, mnemonic: &str) -> Result<CommandOutput> {
        let phrase = read_message(self.host.as_ref(), mnemonic)?;
        let words = phrase.split_whitespace().count();

        // An invalid phrase is the answer here, not a failure — which is the
        // whole difference from `account import-mnemonic`.
        let reason = match bip39::mnemonic_to_entropy(&phrase) {
            Ok(_) => None,
            Err(e) => Some(e.message),
        };
        let valid = reason.is_none();
        let human = match &reason {
            None => format!("Valid — {words} words"),
            Some(why) => format!("Not a valid mnemonic: {why}"),
        };
        Ok(CommandOutput::new(
            json!({"valid": valid, "words": words, "reason": reason}),
            human,
        ))
    }

    fn info(&self) -> Result<CommandOutput> {
        let accounts = self.store.accounts()?;
        let active = self.store.active_account().ok();
        let rpc = self.network.resolve_rpc(
            self.store
                .config_get(&self.network.rpc_config_key())?
                .as_deref(),
        );
        Ok(CommandOutput::new(
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "home": self.store.home().display().to_string(),
                "files": {
                    "accounts": self.store.accounts_path().display().to_string(),
                    "config": self.store.config_path().display().to_string(),
                    "history": self.store.history_path().display().to_string(),
                    "recent": self.store.recent_path().display().to_string(),
                },
                "accounts": accounts.len(),
                "remembered": self.store.recent()?.len(),
                "active_account": active.as_ref().map(|a| a.label.clone()),
                "active_address": active.as_ref().map(|a| a.address.clone()),
                "network": self.network.key,
                "chain_id": self.network.chain_id,
                "rpc": rpc,
            }),
            output::table(&[
                ("Version", env!("CARGO_PKG_VERSION").into()),
                ("Home", self.store.home().display().to_string()),
                ("Accounts", accounts.len().to_string()),
                ("Remembered", self.store.recent()?.len().to_string()),
                (
                    "Active",
                    active
                        .map(|a| format!("{} ({})", a.label, a.address))
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Network",
                    format!("{} (chain {})", self.network.name, self.network.chain_id),
                ),
                ("RPC", rpc),
            ]),
        ))
    }
}

/// The `source` field of a derive payload, for its human rendering.
fn source_of(payload: &Value) -> String {
    payload["source"].as_str().unwrap_or("unknown").to_string()
}

/// Add 25 % headroom to an estimate so a slightly heavier execution still fits.
fn with_headroom(estimate: u64) -> u64 {
    estimate.saturating_mul(125) / 100
}

/// Read a secret from the flag, or from the host when it is `-` or absent.
fn read_secret(host: &dyn Host, value: Option<String>, what: &str) -> Result<String> {
    match value.as_deref().map(str::trim) {
        Some("-") | None => {
            // Surrounding whitespace is never part of a mnemonic or a key, and
            // a trailing newline is what a shell pipe always adds.
            let text = host.read_input(what)?.trim().to_string();
            if text.is_empty() {
                Err(error::usage(format!("no {what} supplied")))
            } else {
                Ok(text)
            }
        }
        Some("") => Err(error::usage(format!("the {what} is empty"))),
        Some(text) => Ok(text.to_string()),
    }
}

/// `-` means "the message comes from the host"; anything else is the message.
fn read_message(host: &dyn Host, message: &str) -> Result<String> {
    if message == "-" {
        let text = host.read_input("message")?;
        // Strip only the trailing newline a shell adds, not meaningful whitespace.
        Ok(text.strip_suffix('\n').unwrap_or(&text).to_string())
    } else {
        Ok(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gas_headroom_is_twenty_five_percent() {
        assert_eq!(with_headroom(100_000), 125_000);
        assert_eq!(with_headroom(21_000), 26_250);
        assert_eq!(with_headroom(0), 0);
        // Must not overflow on an absurd estimate.
        assert!(with_headroom(u64::MAX) > 0);
    }

    #[test]
    fn secrets_come_from_the_flag_when_given() {
        let host = Headless::new();
        assert_eq!(
            read_secret(&host, Some("  abc  ".into()), "key").unwrap(),
            "abc"
        );
        assert!(read_secret(&host, Some("".into()), "key").is_err());
    }

    #[test]
    fn a_literal_message_is_used_as_is() {
        let host = Headless::new();
        assert_eq!(read_message(&host, "hello world").unwrap(), "hello world");
        assert_eq!(read_message(&host, "  spaced  ").unwrap(), "  spaced  ");
    }
}
