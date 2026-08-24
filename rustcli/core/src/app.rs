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

use alloy_primitives::{keccak256, U256};
use serde_json::{json, Value};

use crate::bip39;
use crate::chain::{
    self, Chain, ChainClient, ChainId, ClientConfig, PreparedTransfer, Seed, TransferRequest,
};
use crate::command::*;
use crate::erc20;
use crate::error::{self, Code, Error, Result};
use crate::host::{Headless, Host};
use crate::network::{self, Network};
use crate::output::{self, CommandOutput};
use crate::paths;
use crate::request::{Parsed, Request};
use crate::runtime;
use crate::store::{self, Account, Source, Store, TxRecord};
use crate::tx::LegacyTransaction;
use crate::units;
use crate::wallet::{self, Keypair};

/// How long `--wait` polls for a receipt before giving up.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(180);

/// Where a chain may cache state under the wallet home.
const CACHE_DIR: &str = "cache";

/// How often `--wait` asks whether a transfer has landed.
const CONFIRM_POLL: Duration = Duration::from_millis(1500);

/// A send that has passed every check and is waiting only on a yes.
///
/// Splitting the plan from the execution is what lets the TUI run its own
/// confirmation prompt while still going through the same validation, fee
/// resolution and funding check the CLI uses. By the time one of these exists
/// the transfer is already signed — and still entirely local, so dropping it
/// costs nothing.
pub struct SendPlan {
    pub prepared: PreparedTransfer,
    pub account: Account,
    pub network: Network,
    client: Arc<dyn ChainClient>,
}

impl SendPlan {
    /// The question a caller should put to the user.
    pub fn prompt(&self) -> &str {
        &self.prepared.prompt
    }

    /// How to render this transfer's fee, which is not always the unit the
    /// transfer itself is counted in.
    pub fn fee_units(&self) -> chain::Amount {
        self.prepared
            .fee_unit
            .unwrap_or_else(|| self.network.units())
    }
}

pub struct App {
    pub store: Store,
    /// The network of [`Self::chain`].
    pub network: Network,
    /// The chain every command acts on unless it says otherwise.
    pub chain: ChainId,
    /// Where a `-` argument and a confirmation prompt are answered.
    pub host: Arc<dyn Host>,
}

impl App {
    pub fn new(
        home: Option<PathBuf>,
        network_override: Option<&str>,
        chain_override: Option<&str>,
        host: Arc<dyn Host>,
    ) -> Result<Self> {
        let home = paths::resolve_home(home.as_deref())?;
        let store = Store::open(home)?;
        let (chain, network) = resolve_chain_and_network(&store, network_override, chain_override)?;
        Ok(App {
            store,
            network,
            chain,
            host,
        })
    }

    /// Open the wallet a [`Request`] describes, with a host built from it.
    ///
    /// The one call a foreign front end needs: it turns `home`, `network`,
    /// `chain`, `yes` and `stdin` into an `App` without the caller having to
    /// know that hosts exist.
    pub fn open(request: &Request) -> Result<App> {
        let host = Headless::new()
            .assume_yes(request.yes)
            .with_input(request.stdin.clone());
        App::new(
            request.home.clone(),
            request.network.as_deref(),
            request.chain.as_deref(),
            Arc::new(host),
        )
    }

    /// Parse a request's arguments and run them, in one step.
    pub fn execute(request: &Request) -> Result<CommandOutput> {
        match request.parse()? {
            Parsed::Message(text) => Ok(CommandOutput::message(text)),
            Parsed::Command(cli) => {
                let host = Headless::new()
                    .assume_yes(cli.yes)
                    .with_input(request.stdin.clone());
                let app = App::new(
                    cli.home.clone(),
                    cli.network.as_deref(),
                    cli.chain.as_deref(),
                    Arc::new(host),
                )?;
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

    /// A second `App` over the same home, on another chain.
    ///
    /// The TUI derives an account on a chain it is not currently viewing, and
    /// a command follows `self.chain` — so rather than threading a chain
    /// through every command, the caller opens the wallet on the chain it
    /// means and runs there.
    pub fn reopen_on(&self, chain: ChainId) -> Result<App> {
        App::new(
            Some(self.store.home().to_path_buf()),
            None,
            Some(chain.as_str()),
            Arc::clone(&self.host),
        )
    }

    /// A second `App` over the same home, for when the network changes.
    pub fn reopen(&self) -> Result<App> {
        App::new(
            Some(self.store.home().to_path_buf()),
            Some(self.network.key),
            None,
            Arc::clone(&self.host),
        )
    }

    /// The chain this `App` acts on.
    pub fn chain(&self) -> &'static dyn Chain {
        chain::chain(self.chain)
    }

    /// How the active chain's native token is counted and named.
    pub fn units(&self) -> chain::Amount {
        self.network.units()
    }

    /// Everything a chain client needs from the wallet around it.
    fn client_config(&self, network: Network) -> Result<ClientConfig> {
        let endpoint = network.resolve_endpoint(
            self.store
                .config_get(&network.endpoint_config_key())?
                .as_deref(),
        );
        let submit_endpoint = network.resolve_submit_endpoint(
            self.store
                .config_get(&network.submit_config_key())?
                .as_deref(),
        );
        // Progress goes to whatever the host does with it: stderr for the CLI,
        // a status line in the TUI, nothing at all over the C ABI.
        let host = Arc::clone(&self.host);
        Ok(ClientConfig {
            network,
            endpoint,
            submit_endpoint,
            cache_dir: self.store.home().join(CACHE_DIR),
            progress: Arc::new(move |message: &str| host.progress(message)),
        })
    }

    /// A client for the active chain on the active network.
    pub fn client(&self) -> Result<Arc<dyn ChainClient>> {
        self.client_for(self.chain, self.network)
    }

    /// A client for one chain on one of its networks.
    fn client_for(&self, id: ChainId, network: Network) -> Result<Arc<dyn ChainClient>> {
        if network.chain != id {
            return Err(error::internal(format!(
                "{} is not a {id} network",
                network.key
            )));
        }
        chain::chain(id).client(&self.client_config(network)?)
    }

    /// The account a command should act on: `--account`, else the active one
    /// for this chain.
    fn pick_account(&self, selector: Option<&str>) -> Result<Account> {
        match selector {
            Some(s) => {
                let account = self.store.find_account(s)?;
                // Naming an account of another chain is almost always a
                // mistake, and acting on it would use the wrong network and
                // the wrong address format. Say so rather than guessing.
                if account.chain != self.chain {
                    return Err(error::usage(format!(
                        "'{}' is on {}, but this command is running on {}; pass \
                         --chain {} to act on it",
                        account.label, account.chain, self.chain, account.chain
                    )));
                }
                Ok(account)
            }
            None => self.store.active_account_on(self.chain),
        }
    }

    /// The address a read-only query should use, in the chain's own rendering.
    fn pick_address(&self, target: &TargetArgs) -> Result<(String, Option<Account>)> {
        if let Some(raw) = &target.address {
            self.chain().check_address(&self.network, raw)?;
            return Ok((raw.trim().to_string(), None));
        }
        let account = self.pick_account(target.account.as_deref())?;
        Ok((account.address.clone(), Some(account)))
    }

    /// Ask before doing something irreversible.
    ///
    /// Whether that means a terminal prompt, a GUI dialog, or a flat refusal
    /// is the host's business; a command only needs the yes or the error.
    fn confirm(&self, prompt: &str) -> Result<()> {
        self.host.confirm(prompt)
    }

    /// The EVM client behind the chain trait, for the commands that are EVM
    /// only and need its JSON-RPC directly.
    ///
    /// ERC-20 is a contract standard rather than a chain capability, so it
    /// reaches past the trait rather than widening it for one chain's sake.
    fn evm_client(&self) -> Result<Arc<chain::evm::client::EvmClient>> {
        self.require_chain(ChainId::Evm, "erc20")?;
        Ok(Arc::new(chain::evm::client::EvmClient::new(
            &self.client_config(self.network)?,
        )?))
    }

    /// Refuse a command that only one chain has.
    fn require_chain(&self, wanted: ChainId, what: &str) -> Result<()> {
        if self.chain != wanted {
            return Err(error::usage(format!(
                "{what} is specific to {wanted}, and this command is running on \
                 {}; pass --chain {wanted}",
                self.chain
            )));
        }
        Ok(())
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
            Command::Airdrop { amount, address } => self.airdrop(&amount, address.as_deref()),
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
            Command::Chains => self.chains(),
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
                every_chain,
            } => {
                // A wallet holds one mnemonic and many indices derived from
                // it, so a new one continues that sequence. Only an empty
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
                let seed = Seed::new(&mnemonic, "")?;
                let chains = self.chains_for(every_chain);
                let created = self.create_wallet(&seed, &chains, index, label.as_deref())?;

                let heading = format!(
                    "Created {} account{}",
                    created.len(),
                    if created.len() == 1 { "" } else { "s" }
                );
                let mut output =
                    self.render_created(&created, Some(&mnemonic), show_secret, &heading);
                if let Some(object) = output.data.as_object_mut() {
                    object.insert("new_seed".into(), json!(minted));
                }
                if show_secret {
                    output
                        .human
                        .push_str(&format!("\n\nMnemonic (write it down):\n  {mnemonic}"));
                } else if minted {
                    output.human.push_str(
                        "\n\nThe mnemonic is stored in the wallet; reveal it with `account export`.",
                    );
                }
                Ok(output)
            }

            AccountCommand::ImportMnemonic {
                mnemonic,
                index,
                label,
                passphrase,
                every_chain,
            } => {
                let phrase = read_secret(self.host.as_ref(), mnemonic, "mnemonic")?;
                let seed = Seed::new(&phrase, &passphrase)?;
                let chains = self.chains_for(every_chain);
                let created = self.create_wallet(&seed, &chains, Some(index), label.as_deref())?;
                let heading = format!(
                    "Imported {} account{} at index {index}",
                    created.len(),
                    if created.len() == 1 { "" } else { "s" }
                );
                Ok(self.render_created(&created, Some(seed.phrase()), false, &heading))
            }

            AccountCommand::ImportKey { private_key, label } => {
                let raw = read_secret(self.host.as_ref(), private_key, "private key")?;
                // Each chain parses its own key format, so this is where an
                // EVM key pasted under --chain solana is caught.
                let derived = self.chain().account_from_secret(&raw)?;
                let account = self.store.create_account(
                    label.as_deref(),
                    &derived.address,
                    self.chain,
                    Source::PrivateKey,
                    &derived.secret,
                    None,
                    None,
                    None,
                )?;
                self.activate_if_first(&account)?;
                self.store.remember_secret(
                    "private_key",
                    &derived.secret,
                    &derived.address,
                    None,
                )?;
                Ok(CommandOutput::new(
                    account.public_view(),
                    format!(
                        "Imported {} ({}) on {}",
                        account.label, account.address, account.chain
                    ),
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
                        // Wide enough for the longest name the wallet gives
                        // itself — `account10-midnight` — so the address
                        // column stays a column.
                        lines.push(format!(
                            "{marker} {:<19} {}  {}",
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
                // Each chain reads its own key format, so this must not reach
                // for an EVM keypair: three of the four would fail outright.
                let derived =
                    chain::chain(account.chain).account_from_secret(&account.private_key)?;
                let mut data = if secret {
                    account.secret_view()
                } else {
                    account.public_view()
                };
                data["public_key"] = json!(derived.public_key);
                data["extra"] = derived.extra.clone();
                if account.chain == ChainId::Evm {
                    // The name the envelope has always carried on EVM.
                    data["public_key_compressed"] = derived.extra["public_key_compressed"].clone();
                }

                let mut rows = vec![
                    ("Label", account.label.clone()),
                    ("Address", account.address.clone()),
                    ("Id", account.id.clone()),
                    ("Chain", account.chain.as_str().to_string()),
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

            AccountCommand::Derive {
                index,
                label,
                from,
                every_chain,
            } => {
                let parent = self.pick_account(from.as_deref())?;
                let mnemonic = parent.mnemonic.clone().ok_or_else(|| {
                    error::usage(format!(
                        "account '{}' was imported from a private key, so it has no \
                         mnemonic to derive from",
                        parent.label
                    ))
                })?;
                let seed = Seed::new(&mnemonic, "")?;

                // A wallet created with a BIP-39 passphrase stores the phrase
                // but not the passphrase, so deriving with the phrase alone
                // would silently produce an unrelated address.
                if let Some(parent_index) = parent.index {
                    let plain = chain::chain(parent.chain).derive(&seed, parent_index)?;
                    if plain.address != parent.address {
                        return Err(error::usage(format!(
                            "'{}' was created with a BIP-39 passphrase, so its \
                             mnemonic alone derives a different wallet; re-import \
                             it with --passphrase and derive from that",
                            parent.label
                        )));
                    }
                }

                // Deriving from a parent follows that parent's chain unless
                // asked for all of them: the account named is the context.
                let chains = if every_chain {
                    ChainId::ALL.to_vec()
                } else {
                    vec![parent.chain]
                };
                let created = self.create_wallet(&seed, &chains, Some(index), label.as_deref())?;
                let heading = format!(
                    "Derived {} account{} at index {index} from {}",
                    created.len(),
                    if created.len() == 1 { "" } else { "s" },
                    parent.label
                );
                Ok(self.render_created(&created, Some(seed.phrase()), false, &heading))
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
                every_chain,
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
                        every_chain,
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

    /// Derive one wallet index across a set of chains and store the accounts.
    ///
    /// A wallet is one mnemonic and one index; every chain derives its own
    /// account there. Every way of making one — `new`, `import-mnemonic`,
    /// `derive`, `import-recent` — goes through here, so none of them can end
    /// up producing a quarter of a wallet while the others produce a whole one.
    ///
    /// `index` of `None` means "the next free one", worked out per chain.
    fn create_wallet(
        &self,
        seed: &Seed,
        chains: &[ChainId],
        index: Option<u32>,
        label: Option<&str>,
    ) -> Result<Vec<(Account, chain::DerivedAccount, u32)>> {
        let mnemonic = seed.phrase().to_string();
        let mut created = Vec::new();
        for id in chains {
            let chain = chain::chain(*id);
            let at = match index {
                Some(chosen) => chosen,
                None => self.store.next_address_index(&mnemonic, *id)?,
            };
            let derived = chain.derive(seed, at)?;
            // With several chains at once one label cannot serve them all, so
            // each gets the chain's name appended: unique, and self-describing.
            let chain_label = match (label, chains.len()) {
                (Some(base), 1) => Some(base.to_string()),
                (Some(base), _) => Some(format!("{base}-{id}")),
                (None, _) => None,
            };
            let account = self.store.create_account(
                chain_label.as_deref(),
                &derived.address,
                *id,
                Source::Mnemonic,
                &derived.secret,
                Some(&mnemonic),
                derived.derivation_path.as_deref(),
                Some(at),
            )?;
            self.activate_if_first(&account)?;
            created.push((account, derived, at));
        }
        self.remember_mnemonic(&mnemonic, seed.passphrase())?;
        Ok(created)
    }

    /// Which chains a command should act on: all of them, or just the one in
    /// view.
    fn chains_for(&self, every_chain: bool) -> Vec<ChainId> {
        if every_chain {
            ChainId::ALL.to_vec()
        } else {
            vec![self.chain]
        }
    }

    /// Render what [`Self::create_wallet`] made.
    ///
    /// One account still reports as a single object, so a caller that only
    /// ever makes one does not have to learn a new shape.
    fn render_created(
        &self,
        created: &[(Account, chain::DerivedAccount, u32)],
        mnemonic: Option<&str>,
        show_secret: bool,
        heading: &str,
    ) -> CommandOutput {
        let mut data: Vec<Value> = Vec::new();
        let mut lines = Vec::new();
        for (account, derived, at) in created {
            let mut value = account.public_view();
            value["public_key"] = json!(derived.public_key);
            value["extra"] = derived.extra.clone();
            if show_secret {
                value["mnemonic"] = json!(mnemonic);
                value["private_key"] = json!(derived.secret);
            }
            data.push(value);
            lines.push(format!(
                "{:<9} {:<20} {}\n{:>10}{} index {at}",
                account.chain.as_str(),
                account.label,
                account.address,
                "",
                account.derivation_path.clone().unwrap_or_default(),
            ));
        }
        let human = format!("{heading}\n\n{}", lines.join("\n"));
        let payload = if created.len() == 1 {
            data.into_iter().next().expect("one account")
        } else {
            json!(data)
        };
        CommandOutput::new(payload, human)
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
                let current = self.network;
                let data: Vec<Value> = network::ALL
                    .iter()
                    .map(|n| {
                        json!({
                            "key": n.key,
                            "chain": n.chain.as_str(),
                            "name": n.name,
                            "chain_id": n.chain_id,
                            "symbol": n.symbol,
                            "decimals": n.decimals,
                            "endpoint": n.resolve_endpoint(
                                self.store.config_get(&n.endpoint_config_key()).ok().flatten().as_deref()
                            ),
                            "rpc": n.resolve_endpoint(
                                self.store.config_get(&n.endpoint_config_key()).ok().flatten().as_deref()
                            ),
                            "explorer": n.explorer,
                            "testnet": n.testnet,
                            "current": n.key == current.key,
                        })
                    })
                    .collect();

                // Grouped by chain, because that is how the table is laid out
                // and how someone scanning it thinks.
                let mut lines = Vec::new();
                for id in ChainId::ALL {
                    lines.push(format!("{}:", chain::chain(id).name()));
                    for n in network::ALL.iter().filter(|n| n.chain == id) {
                        lines.push(format!(
                            "  {} {:<18} {}",
                            if n.key == current.key { "*" } else { " " },
                            n.key,
                            n.name
                        ));
                    }
                }
                Ok(CommandOutput::new(json!(data), lines.join("\n")))
            }

            NetworkCommand::Current => {
                let n = self.network;
                let endpoint =
                    n.resolve_endpoint(self.store.config_get(&n.endpoint_config_key())?.as_deref());
                let mut rows = vec![
                    ("Chain", self.chain().name().to_string()),
                    ("Network", n.name.to_string()),
                    ("Key", n.key.to_string()),
                ];
                if let Some(chain_id) = n.chain_id {
                    rows.push(("Chain id", chain_id.to_string()));
                }
                rows.push(("Symbol", n.symbol.to_string()));
                rows.push(("Endpoint", endpoint.clone()));
                // Only the chains that submit elsewhere have a second one, and
                // conflating them is exactly the mistake worth surfacing.
                let submit = n.resolve_submit_endpoint(
                    self.store.config_get(&n.submit_config_key())?.as_deref(),
                );
                if submit != endpoint {
                    rows.push(("Submit to", submit.clone()));
                }
                rows.push(("Explorer", n.explorer.to_string()));

                Ok(CommandOutput::new(
                    json!({
                        "key": n.key, "chain": n.chain.as_str(), "name": n.name,
                        "chain_id": n.chain_id, "symbol": n.symbol, "decimals": n.decimals,
                        "endpoint": endpoint,
                        // `rpc` is the name SPEC.md and the Python CLI use, and
                        // callers branch on it; `endpoint` is the same value
                        // under the name the other three chains make sense of.
                        "rpc": endpoint,
                        "submit_endpoint": submit,
                        "explorer": n.explorer, "testnet": n.testnet,
                    }),
                    output::table(&rows),
                ))
            }

            NetworkCommand::Use { network } => {
                // A bare name is resolved within the chain in play when there
                // is one, so `--chain solana network use devnet` works even
                // though `devnet` alone is ambiguous.
                let target =
                    network::find_for(self.chain, &network).or_else(|_| network::find(&network))?;
                self.store.set_network(&target)?;
                Ok(CommandOutput::new(
                    json!({
                        "key": target.key,
                        "chain": target.chain.as_str(),
                        "name": target.name,
                        "chain_id": target.chain_id,
                    }),
                    format!(
                        "Network is now {} ({})",
                        target.name,
                        match target.chain_id {
                            Some(id) => format!("chain {id}"),
                            None => target.chain.as_str().to_string(),
                        }
                    ),
                ))
            }

            NetworkCommand::SetRpc { network, url } => {
                let target =
                    network::find_for(self.chain, &network).or_else(|_| network::find(&network))?;
                self.store
                    .config_set(&target.endpoint_config_key(), url.trim())?;
                let effective = target.resolve_endpoint(if url.trim().is_empty() {
                    None
                } else {
                    Some(&url)
                });
                Ok(CommandOutput::new(
                    json!({"key": target.key, "endpoint": effective, "rpc": effective}),
                    format!("The endpoint for {} is now {effective}", target.key),
                ))
            }
        }
    }

    /// Every chain this build supports, and what each can do.
    fn chains(&self) -> Result<CommandOutput> {
        let data: Vec<Value> = chain::registry()
            .iter()
            .map(|c| {
                let capabilities = c.capabilities();
                json!({
                    "chain": c.id().as_str(),
                    "name": c.name(),
                    "current": c.id() == self.chain,
                    "derivation_path": c.derivation_path(0),
                    "networks": c.networks().iter().map(|n| n.key).collect::<Vec<_>>(),
                    "capabilities": capabilities,
                })
            })
            .collect();

        let human = chain::registry()
            .iter()
            .map(|c| {
                let mut notes = Vec::new();
                if c.capabilities().faucet {
                    notes.push("faucet");
                }
                if c.capabilities().tokens {
                    notes.push("tokens");
                }
                if c.capabilities().recoverable_signatures {
                    notes.push("recoverable signatures");
                }
                format!(
                    "{} {:<9} {:<22} {}\n{:>12}{}",
                    if c.id() == self.chain { "*" } else { " " },
                    c.id().as_str(),
                    c.derivation_path(0),
                    c.networks()
                        .iter()
                        .map(|n| n.key)
                        .collect::<Vec<_>>()
                        .join(", "),
                    "",
                    if notes.is_empty() {
                        String::new()
                    } else {
                        notes.join(", ")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(CommandOutput::new(json!(data), human))
    }

    // ============================================================ chain reads

    pub fn balance(&self, target: TargetArgs) -> Result<CommandOutput> {
        if target.all {
            return self.balance_everywhere();
        }
        let (address, account) = self.pick_address(&target)?;
        let client = self.client()?;
        let balance = runtime::block_on(client.balance(&address))??;
        let units = self.units();
        Ok(CommandOutput::new(
            json!({
                "address": address,
                "chain": self.chain.as_str(),
                "account": account.as_ref().map(|a| a.label.clone()),
                "balance": units.format(balance.native),
                "balance_raw": balance.native.to_string(),
                // The name SPEC.md and the Python CLI use. It says "wei",
                // which only means something on EVM, so the chain-neutral
                // `balance_raw` carries the same number for the others.
                "balance_wei": balance.native.to_string(),
                "symbol": units.symbol,
                "decimals": units.decimals,
                "network": self.network.key,
                "tokens": balance.tokens,
            }),
            format!("{} — {address}", units.format_with_symbol(balance.native)),
        ))
    }

    /// Every chain's active account, read at once.
    ///
    /// The requests go out together, so this costs roughly the slowest chain
    /// rather than the sum of all four — which is the whole reason the chain
    /// layer is async.
    fn balance_everywhere(&self) -> Result<CommandOutput> {
        // Resolve each chain's account and network first: that part is local,
        // and a chain the wallet holds nothing on should be reported as such
        // rather than counted as a failure.
        let mut targets = Vec::new();
        for id in ChainId::ALL {
            let Ok(account) = self.store.active_account_on(id) else {
                continue;
            };
            let network = self.store.network_on(id)?;
            targets.push((id, account, network));
        }
        if targets.is_empty() {
            return Ok(CommandOutput::new(
                json!([]),
                "No accounts yet. Create one with `cwbwallet account new`.",
            ));
        }

        let clients: Vec<Arc<dyn ChainClient>> = targets
            .iter()
            .map(|(id, _, network)| self.client_for(*id, *network))
            .collect::<Result<_>>()?;

        let balances = runtime::block_on(async {
            let queries = targets
                .iter()
                .zip(&clients)
                .map(|((_, account, _), client)| client.balance(&account.address));
            futures_util::future::join_all(queries).await
        })?;

        let mut rows = Vec::new();
        let mut data = Vec::new();
        for ((id, account, network), balance) in targets.iter().zip(balances) {
            let units = network.units();
            // One chain being unreachable must not hide the other three, so a
            // failure is reported in its row rather than aborting the command.
            let (shown, value, error) = match balance {
                Ok(balance) => (
                    units.format_with_symbol(balance.native),
                    Some(balance.native.to_string()),
                    None,
                ),
                Err(e) => (
                    format!("unavailable ({})", e.message),
                    None,
                    Some(e.message),
                ),
            };
            data.push(json!({
                "chain": id.as_str(),
                "network": network.key,
                "account": account.label,
                "address": account.address,
                "balance": value.as_ref().map(|_| units.format(
                    value.as_ref().and_then(|v| v.parse::<u128>().ok()).unwrap_or(0)
                )),
                "balance_raw": value,
                "symbol": units.symbol,
                "error": error,
            }));
            rows.push(format!(
                "{:<9} {:<16} {}\n          {}",
                id.as_str(),
                account.label,
                shown,
                account.address
            ));
        }
        Ok(CommandOutput::new(json!(data), rows.join("\n")))
    }

    fn nonce(&self, target: TargetArgs) -> Result<CommandOutput> {
        let (address, _) = self.pick_address(&target)?;
        let client = self.client()?;
        let nonce = runtime::block_on(client.nonce(&address))??;
        let Some(nonce) = nonce else {
            return Err(error::usage(format!(
                "{} has no per-account sequence number; it is not an \
                 account-based chain",
                self.chain
            )));
        };
        Ok(CommandOutput::new(
            json!({"address": address, "nonce": nonce, "chain": self.chain.as_str(), "network": self.network.key}),
            format!("nonce {nonce} — {address}"),
        ))
    }

    fn gas_price(&self) -> Result<CommandOutput> {
        let client = self.client()?;
        let quote = runtime::block_on(client.fee_quote())??;
        let Some(quote) = quote else {
            return Err(error::usage(format!(
                "{} does not quote a standing fee price; a fee is worked out \
                 per transaction, so use `send --dry-run` to see one",
                self.chain
            )));
        };
        let units = self.units();
        // Gwei is an EVM idea; the other chains quote in their own base units.
        let human = match self.chain {
            ChainId::Evm => format!("{} gwei", units::format_gwei(U256::from(quote))),
            _ => units.format_with_symbol(quote),
        };
        let mut data = json!({
            "chain": self.chain.as_str(),
            "network": self.network.key,
            "fee_quote_raw": quote.to_string(),
            "fee_quote": human,
        });
        if self.chain == ChainId::Evm {
            // The two names the envelope has always carried. They are EVM
            // vocabulary, so only EVM emits them.
            data["gas_price_wei"] = json!(quote.to_string());
            data["gas_price_gwei"] = json!(units::format_gwei(U256::from(quote)));
        }
        Ok(CommandOutput::new(data, human))
    }

    fn chain_info(&self) -> Result<CommandOutput> {
        let client = self.client()?;
        let info = runtime::block_on(client.chain_info())??;
        let mut rows: Vec<(String, String)> = vec![
            ("Chain".into(), self.chain().name().to_string()),
            ("Network".into(), self.network.name.to_string()),
            ("Endpoint".into(), client.endpoint().to_string()),
        ];
        // The reported values differ per chain, so they are rendered from
        // whatever the chain chose to report rather than a fixed list.
        if let Some(map) = info.as_object() {
            for (key, value) in map {
                if matches!(key.as_str(), "network" | "name") || value.is_null() {
                    continue;
                }
                let shown = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                rows.push((label_for(key), shown));
            }
        }
        let rendered: Vec<(&str, String)> = rows
            .iter()
            .map(|(key, value)| (&key[..], value.clone()))
            .collect();
        Ok(CommandOutput::new(info, output::table(&rendered)))
    }

    // ================================================================== sends

    fn send(&self, args: SendArgs) -> Result<CommandOutput> {
        let plan = self.plan_send(&args)?;
        if args.dry_run {
            // Everything a real send checks has already happened, and the
            // transfer is signed — it simply never leaves the machine.
            return Ok(self.render_dry_run(&plan));
        }
        self.confirm(plan.prompt())?;
        self.execute_send(plan, args.wait)
    }

    /// Resolve everything a transfer needs, check it, and sign it.
    ///
    /// Nothing has left the machine when this returns; dropping the plan costs
    /// nothing. That is what lets a caller confirm afterwards, and what makes
    /// `--dry-run` the same code path as a real send.
    pub fn plan_send(&self, args: &SendArgs) -> Result<SendPlan> {
        let account = self.pick_account(args.account.as_deref())?;
        self.chain().check_address(&self.network, &args.to)?;

        let units = self.units();
        let mut request = TransferRequest::new(args.to.trim(), units.parse(&args.amount)?);
        request.gas_limit = args.gas_limit;
        request.nonce_override = args.nonce;
        if let Some(gwei) = &args.gas_price_gwei {
            request.fee_override = Some(
                u128::try_from(units::parse_gwei(gwei)?)
                    .map_err(|_| error::invalid_amount("that gas price is absurd"))?,
            );
        }
        if let Some(data) = &args.data {
            request.data = wallet::parse_hex(data)?;
        }
        if !request.data.is_empty() && self.chain != ChainId::Evm {
            return Err(error::usage(format!(
                "--data attaches EVM call data, and this transfer is on {}",
                self.chain
            )));
        }

        let client = self.client()?;
        let prepared = runtime::block_on(client.prepare_transfer(&account.private_key, &request))??;
        Ok(SendPlan {
            prepared,
            account,
            network: self.network,
            client,
        })
    }

    /// Show a signed transfer without broadcasting it.
    fn render_dry_run(&self, plan: &SendPlan) -> CommandOutput {
        let units = self.units();
        // A chain may pay its fee in another token entirely.
        let fee_units = plan.prepared.fee_unit.unwrap_or(units);
        let prepared = &plan.prepared;
        let mut rows: Vec<(String, String)> = vec![
            ("Chain".into(), self.chain.as_str().to_string()),
            ("Network".into(), self.network.name.to_string()),
            ("From".into(), prepared.from.clone()),
            ("To".into(), prepared.to.clone()),
            ("Amount".into(), units.format_with_symbol(prepared.amount)),
            ("Fee".into(), fee_units.format_with_symbol(prepared.fee)),
            ("Id".into(), prepared.id.clone()),
            ("Signed bytes".into(), prepared.signed.len().to_string()),
        ];
        if let Some(nonce) = prepared.nonce {
            rows.push(("Nonce".into(), nonce.to_string()));
        }
        let mut data = json!({
            "dry_run": true,
            "chain": self.chain.as_str(),
            "network": self.network.key,
            "from": prepared.from,
            "to": prepared.to,
            "amount": units.format(prepared.amount),
            "amount_raw": prepared.amount.to_string(),
            "fee": fee_units.format(prepared.fee),
            "fee_raw": prepared.fee.to_string(),
            "fee_symbol": fee_units.symbol,
            "symbol": units.symbol,
            "id": prepared.id,
            "nonce": prepared.nonce,
            "signed": format!("0x{}", hex::encode(&prepared.signed)),
            "detail": prepared.detail,
        });
        if let Some(map) = prepared.detail.as_object() {
            for (key, value) in map {
                if value.is_null() {
                    continue;
                }
                let shown = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                rows.push((label_for(key), shown));
            }
        }
        data["human_rows"] = json!(rows.len());
        let rendered: Vec<(&str, String)> = rows
            .iter()
            .map(|(key, value)| (&key[..], value.clone()))
            .collect();
        CommandOutput::new(
            data,
            format!(
                "Nothing was broadcast — this is what would have been sent.\n\n{}",
                output::table(&rendered)
            ),
        )
    }

    /// Broadcast a plan the caller has already had confirmed.
    pub fn execute_send(&self, plan: SendPlan, wait: bool) -> Result<CommandOutput> {
        let SendPlan {
            prepared,
            account,
            network,
            client,
        } = plan;
        let units = network.units();

        // Recorded before broadcasting, using the id computed locally: if the
        // node accepts the transaction and the reply is then lost to a
        // timeout, the history still names it. Losing that record is what
        // leads to someone re-sending a transfer that already went through.
        let mut record = TxRecord {
            hash: prepared.id.clone(),
            from: prepared.from.clone(),
            to: prepared.to.clone(),
            value: units.format(prepared.amount),
            value_wei: prepared.amount.to_string(),
            chain: account.chain,
            network: network.key.to_string(),
            chain_id: network.chain_id.unwrap_or(0),
            nonce: prepared.nonce.unwrap_or(0),
            gas_limit: prepared.gas_limit.unwrap_or(0),
            // The per-unit price where the chain has one, and the whole fee
            // where it does not — never the total under the price's name.
            gas_price_wei: prepared.fee_rate.unwrap_or(prepared.fee).to_string(),
            status: "submitting".into(),
            token: None,
            block_number: None,
            gas_used: None,
            created_at: store::now_rfc3339(),
        };
        self.store.record_tx(&record)?;

        let receipt = match runtime::block_on(client.submit(&prepared))? {
            Ok(receipt) => {
                record.status = "submitted".into();
                self.store
                    .update_tx(&record.hash, "submitted", None, None)?;
                receipt
            }
            Err(e) => {
                // The node may still have accepted it, so the record stays and
                // is marked for what it is rather than deleted.
                self.store
                    .update_tx(&record.hash, "unconfirmed", None, None)?;
                return Err(Error::new(
                    e.code,
                    format!(
                        "{} — recorded locally as {}; check it with `tx {}` before \
                         sending again",
                        e.message, record.hash, record.hash
                    ),
                ));
            }
        };

        self.finish_send(client.as_ref(), record, receipt.secondary_id, wait)
    }

    /// Optionally wait for confirmation, then render the result.
    fn finish_send(
        &self,
        client: &dyn ChainClient,
        mut record: TxRecord,
        secondary_id: Option<String>,
        wait: bool,
    ) -> Result<CommandOutput> {
        if wait {
            if let Some(status) = self.await_confirmation(client, &record.hash)? {
                record.status = status.status.clone();
                record.block_number = status.block;
                record.gas_used = status.gas_used;
                self.store.update_tx(
                    &record.hash,
                    &record.status,
                    status.block,
                    status.gas_used,
                )?;
            }
        }

        let network = network::find(&record.network)?;
        let units = network.units();
        let explorer = network.tx_url(&record.hash);
        let mut rows = vec![
            ("Chain", record.chain.as_str().to_string()),
            ("Id", record.hash.clone()),
            ("From", record.from.clone()),
            ("To", record.to.clone()),
            (
                "Amount",
                units.format_with_symbol(record.value_wei.parse().unwrap_or(0)),
            ),
            ("Status", record.status.clone()),
        ];
        if let Some(block) = record.block_number {
            rows.push(("Block", block.to_string()));
        }
        if let Some(secondary) = &secondary_id {
            rows.push(("Extrinsic", secondary.clone()));
        }
        rows.push(("Explorer", explorer.clone()));

        let mut data = serde_json::to_value(&record)
            .map_err(|e| error::internal(format!("cannot serialise the transaction: {e}")))?;
        data["explorer"] = json!(explorer);
        data["symbol"] = json!(units.symbol);
        data["secondary_id"] = json!(secondary_id);
        Ok(CommandOutput::new(data, output::table(&rows)))
    }

    /// Poll until a transaction is no longer pending, or the deadline passes.
    ///
    /// Transient failures are retried rather than propagated: by this point
    /// the transfer has already been broadcast, and reporting a single 502 as
    /// a failure invites the user to send it a second time.
    fn await_confirmation(
        &self,
        client: &dyn ChainClient,
        id: &str,
    ) -> Result<Option<chain::TransactionStatus>> {
        runtime::block_on(async {
            let deadline = tokio::time::Instant::now() + RECEIPT_TIMEOUT;
            loop {
                if let Ok(Some(status)) = client.transaction(id).await {
                    if status.status != "pending" {
                        return Ok(Some(status));
                    }
                }
                if tokio::time::Instant::now() + CONFIRM_POLL > deadline {
                    return Ok(None);
                }
                tokio::time::sleep(CONFIRM_POLL).await;
            }
        })?
    }

    fn airdrop(&self, amount: &str, address: Option<&str>) -> Result<CommandOutput> {
        let units = self.units();
        let requested = units.parse(amount)?;
        let target = match address {
            Some(raw) => {
                self.chain().check_address(&self.network, raw)?;
                raw.trim().to_string()
            }
            None => self.pick_account(None)?.address,
        };
        let client = self.client()?;
        let id = runtime::block_on(client.faucet(&target, requested))??;
        Ok(CommandOutput::new(
            json!({
                "chain": self.chain.as_str(),
                "network": self.network.key,
                "address": target,
                "amount": units.format(requested),
                "amount_raw": requested.to_string(),
                "id": id,
                "explorer": self.network.tx_url(&id),
            }),
            format!(
                "Asked {} for {} to {target}\n{id}",
                self.network.name,
                units.format_with_symbol(requested)
            ),
        ))
    }

    fn tx(&self, id: &str) -> Result<CommandOutput> {
        let client = self.client()?;
        let status = runtime::block_on(client.transaction(id))??;
        let Some(status) = status else {
            return Err(error::not_found(format!(
                "no transaction {id} on {}",
                self.network.name
            )));
        };
        // Keep the local log in step with what the chain says.
        let _ = self
            .store
            .update_tx(id, &status.status, status.block, status.gas_used);

        let units = self.units();
        let mut rows = vec![
            ("Id", id.to_string()),
            ("Chain", self.chain.as_str().to_string()),
            ("Status", status.status.clone()),
        ];
        if let Some(block) = status.block {
            rows.push(("Block", block.to_string()));
        }
        if let Some(fee) = status.fee {
            rows.push(("Fee", units.format_with_symbol(fee)));
        }
        rows.push(("Explorer", self.network.tx_url(id)));
        // The transferred amount, where the chain reports it back. EVM puts
        // it on the transaction; the UTxO chains do not report one number.
        let value = status
            .raw
            .pointer("/transaction/value")
            .and_then(|v| crate::rpc::parse_quantity_u256(v, "value").ok())
            .and_then(|v| u128::try_from(v).ok())
            .map(|v| units.format(v));
        if let Some(value) = &value {
            rows.insert(3, ("Value", format!("{value} {}", units.symbol)));
        }
        Ok(CommandOutput::new(
            json!({
                "id": id,
                "hash": id,
                "chain": self.chain.as_str(),
                "status": status.status,
                "block": status.block,
                "value": value,
                "fee": status.fee.map(|f| units.format(f)),
                "network": self.network.key,
                "explorer": self.network.tx_url(id),
                "transaction": status.raw.get("transaction"),
                "receipt": status.raw.get("receipt"),
                "raw": status.raw,
            }),
            output::table(&rows),
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
                        "{}  {:<9} {:<10} {:>14} -> {}\n{:>12}{}",
                        &tx.created_at[..tx.created_at.len().min(19)],
                        tx.chain.as_str(),
                        tx.status,
                        tx.value,
                        tx.to,
                        "",
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
        let signer = self.chain().signer(&account.private_key)?;
        let text = read_message(self.host.as_ref(), message)?;
        let signature = signer.sign_message(text.as_bytes())?;
        let encoded = format!("0x{}", hex::encode(&signature));
        Ok(CommandOutput::new(
            json!({
                "address": account.address,
                "account": account.label,
                "chain": account.chain.as_str(),
                "scheme": signing_scheme(account.chain),
                "message": text,
                "signature": encoded,
            }),
            output::table(&[
                ("Signer", account.address.clone()),
                ("Scheme", signing_scheme(account.chain).to_string()),
                ("Signature", encoded),
            ]),
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

        // Only secp256k1 recovers a signer from a signature. The other three
        // chains need to be told whose signature this should be — and on
        // Cardano and Midnight an address is a *hash* of the key, so what they
        // need is an account this wallet holds rather than an address.
        let recoverable = self.chain().capabilities().recoverable_signatures;
        let against = match (address, recoverable) {
            (Some(given), true) => Some(given.to_string()),
            (given, false) => {
                let selector = given.map(str::to_string);
                let account = match selector {
                    Some(s) => self.store.find_account(&s).ok(),
                    None => self.store.active_account_on(self.chain).ok(),
                };
                match account {
                    // These chains verify with key material, so the account's
                    // secret is what goes through, not its address.
                    Some(account) => Some(account.private_key),
                    None if given.is_some() => Some(given.unwrap().to_string()),
                    None => None,
                }
            }
            // Nothing to compare against would make `valid` mean only "this
            // parsed", which is true of a signature over any message at all.
            // The wallet's own account is the expectation worth defaulting to.
            (None, true) => self
                .store
                .active_account_on(self.chain)
                .ok()
                .map(|account| account.address),
        };

        let recovered =
            self.chain()
                .recover_message(text.as_bytes(), &bytes, against.as_deref())?;

        // The address is echoed back rather than the key that checked it.
        let shown = recovered.address.clone();
        let human = match (&shown, recovered.valid) {
            (Some(address), true) => format!("Valid — signed by {address}"),
            (Some(address), false) => {
                format!(
                    "INVALID — the signature recovers to {address}, which is not who was expected"
                )
            }
            (None, true) => "Valid".to_string(),
            (None, false) => "INVALID — that signature was not made by this key".to_string(),
        };
        Ok(CommandOutput::new(
            json!({
                "valid": recovered.valid,
                "recovered": shown,
                // What it was checked against, so `valid` is never a claim
                // whose basis the caller has to guess at.
                "expected": recoverable.then(|| against.clone()).flatten(),
                "chain": self.chain.as_str(),
                "scheme": signing_scheme(self.chain),
                "message": text,
            }),
            human,
        ))
    }

    // =================================================================== erc20

    fn erc20(&self, command: Erc20Command) -> Result<CommandOutput> {
        // ERC-20 is an EVM contract standard; the other chains have their own
        // token models this wallet does not move.
        self.require_chain(ChainId::Evm, "erc20")?;
        match command {
            Erc20Command::Info { token } => {
                let token = wallet::parse_address(&token)?;
                let client = self.evm_client()?;
                let rpc = client.rpc();
                let name = erc20::decode_string(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_NAME)),
                )??)?;
                let symbol = erc20::decode_string(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_SYMBOL)),
                )??)?;
                let decimals = erc20::decode_u8(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_DECIMALS)),
                )??)?;
                let supply = erc20::decode_uint(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_TOTAL_SUPPLY)),
                )??)?;
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
                let client = self.evm_client()?;
                let rpc = client.rpc();
                let decimals = erc20::decode_u8(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_DECIMALS)),
                )??)?;
                let symbol = erc20::decode_string(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_SYMBOL)),
                )??)
                .unwrap_or_default();
                let raw = erc20::decode_uint(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_balance_of(owner)),
                )??)?;
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

                let client = self.evm_client()?;
                let rpc = client.rpc();
                let decimals = erc20::decode_u8(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_getter(erc20::SELECTOR_DECIMALS)),
                )??)?;
                let raw_amount = units::parse_units(&amount, decimals)?;
                let held = erc20::decode_uint(&runtime::block_on(
                    rpc.eth_call(token, &erc20::encode_balance_of(keypair.address())),
                )??)?;
                if held < raw_amount {
                    return Err(error::insufficient_funds(format!(
                        "token balance {} is less than {amount}",
                        units::format_units(held, decimals)
                    )));
                }

                let data = erc20::encode_transfer(recipient, raw_amount);
                let gas_limit = runtime::block_on(rpc.estimate_gas(
                    keypair.address(),
                    token,
                    U256::ZERO,
                    &data,
                ))?
                .map(chain::evm::client::with_headroom)
                .unwrap_or(100_000);
                let (gas_price, nonce) = runtime::block_on(async {
                    tokio::try_join!(
                        rpc.gas_price(),
                        rpc.get_transaction_count(keypair.address())
                    )
                })??;

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
                    chain_id: self
                        .network
                        .chain_id
                        .ok_or_else(|| error::internal("an EVM network with no chain id"))?,
                };
                let signed = transaction.sign(&keypair)?;
                let hash = runtime::block_on(rpc.send_raw_transaction(&signed.raw))??;

                let record = TxRecord {
                    hash,
                    from: keypair.address().to_checksum(None),
                    to: recipient.to_checksum(None),
                    value: amount.clone(),
                    value_wei: raw_amount.to_string(),
                    chain: ChainId::Evm,
                    network: self.network.key.to_string(),
                    chain_id: self.network.chain_id.unwrap_or(0),
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
                self.finish_send(client.as_ref(), record, None, wait)
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
        let endpoint = self.network.resolve_endpoint(
            self.store
                .config_get(&self.network.endpoint_config_key())?
                .as_deref(),
        );

        // How many accounts sit on each chain, so a multi-chain wallet can be
        // taken in at a glance rather than through four `account list` calls.
        let by_chain: Vec<Value> = ChainId::ALL
            .iter()
            .map(|id| {
                let network = self
                    .store
                    .network_on(*id)
                    .unwrap_or_else(|_| network::default_for(*id));
                json!({
                    "chain": id.as_str(),
                    "accounts": accounts.iter().filter(|a| a.chain == *id).count(),
                    "network": network.key,
                })
            })
            .collect();

        let mut rows: Vec<(String, String)> = vec![
            ("Version".into(), env!("CARGO_PKG_VERSION").to_string()),
            ("Home".into(), self.store.home().display().to_string()),
            ("Accounts".into(), accounts.len().to_string()),
            ("Remembered".into(), self.store.recent()?.len().to_string()),
            (
                "Active".into(),
                active
                    .as_ref()
                    .map(|a| format!("{} ({}) on {}", a.label, a.address, a.chain))
                    .unwrap_or_else(|| "-".into()),
            ),
            ("Chain".into(), self.chain().name().to_string()),
            (
                "Network".into(),
                match self.network.chain_id {
                    Some(id) => format!("{} (chain {id})", self.network.name),
                    None => self.network.name.to_string(),
                },
            ),
            ("Endpoint".into(), endpoint.clone()),
        ];
        for entry in &by_chain {
            let count = entry["accounts"].as_u64().unwrap_or(0);
            if count > 0 {
                rows.push((
                    label_for(entry["chain"].as_str().unwrap_or("")),
                    format!(
                        "{count} account(s) on {}",
                        entry["network"].as_str().unwrap_or("")
                    ),
                ));
            }
        }
        let rendered: Vec<(&str, String)> = rows
            .iter()
            .map(|(key, value)| (&key[..], value.clone()))
            .collect();

        Ok(CommandOutput::new(
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "home": self.store.home().display().to_string(),
                "files": {
                    "accounts": self.store.accounts_path().display().to_string(),
                    "config": self.store.config_path().display().to_string(),
                    "history": self.store.history_path().display().to_string(),
                    "recent": self.store.recent_path().display().to_string(),
                    "cache": self.store.home().join(CACHE_DIR).display().to_string(),
                },
                "accounts": accounts.len(),
                "remembered": self.store.recent()?.len(),
                "active_account": active.as_ref().map(|a| a.label.clone()),
                "active_address": active.as_ref().map(|a| a.address.clone()),
                "chain": self.chain.as_str(),
                "chains": by_chain,
                "network": self.network.key,
                "chain_id": self.network.chain_id,
                "rpc": endpoint.clone(),
                "endpoint": endpoint,
            }),
            output::table(&rendered),
        ))
    }
}

/// Work out which chain and which network a command is running on.
///
/// The two settle each other, and the order matters:
///
/// * A network names its chain, so `-n solana-devnet` is enough on its own.
/// * A chain narrows a bare network name, so `-c solana -n devnet` works where
///   `-n devnet` alone is ambiguous between Solana and Midnight.
/// * With neither, the wallet follows its active account's chain, which is
///   what keeps an EVM-only wallet behaving exactly as it always did.
///
/// Naming both and having them disagree is refused rather than resolved in
/// either direction: `-c solana -n cronos-mainnet` cannot mean anything
/// sensible, and guessing would either use the wrong keys or the wrong node.
fn resolve_chain_and_network(
    store: &Store,
    network_override: Option<&str>,
    chain_override: Option<&str>,
) -> Result<(ChainId, Network)> {
    let requested_chain = chain_override.map(ChainId::parse).transpose()?;

    let network = match (network_override, requested_chain) {
        (Some(key), Some(id)) => {
            let found = network::find_for(id, key)?;
            if found.chain != id {
                return Err(error::unknown_network(format!(
                    "{} is a {} network, but --chain says {id}",
                    found.key, found.chain
                )));
            }
            found
        }
        (Some(key), None) => network::find(key)?,
        (None, Some(id)) => store.network_on(id)?,
        (None, None) => {
            // No hint at all: follow the account the wallet is on, and fall
            // back to the stored overall network for a wallet with none.
            match store.active_account().ok() {
                Some(account) => store.network_on(account.chain)?,
                None => store.network()?,
            }
        }
    };
    Ok((network.chain, network))
}

/// Turn a JSON key into a table label: `gas_price_gwei` → `Gas price gwei`.
///
/// Chains report whatever detail they have rather than a fixed set of fields,
/// so the human rendering is derived from the keys instead of a match arm per
/// chain that would go stale the moment one of them reported something new.
fn label_for(key: &str) -> String {
    let mut label = key.replace('_', " ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

/// What `sign` actually does on a chain, for the output to say so.
fn signing_scheme(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Evm => "EIP-191 personal message (secp256k1)",
        ChainId::Solana => "ed25519",
        ChainId::Cardano => "ed25519 (BIP32-Ed25519 payment key)",
        ChainId::Midnight => "BIP-340 Schnorr (secp256k1)",
    }
}

/// The `source` field of a derive payload, for its human rendering.
fn source_of(payload: &Value) -> String {
    payload["source"].as_str().unwrap_or("unknown").to_string()
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
