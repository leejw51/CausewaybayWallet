//! The append-only JSONL store described in `SPEC.md`.
//!
//! Nothing is ever rewritten: every mutation appends one line, and state is the
//! fold of every line in order. That makes the files trivially inspectable, safe
//! against partial writes, and shareable between the Rust and Python front ends.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use alloy_primitives::keccak256;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::chain::ChainId;
use crate::error::{self, Result};
use crate::network;
use crate::paths;

pub const SCHEMA: u32 = 1;

pub const ACCOUNTS_FILE: &str = "accounts.jsonl";
pub const CONFIG_FILE: &str = "config.jsonl";
pub const HISTORY_FILE: &str = "history.jsonl";
pub const RECENT_FILE: &str = "recent.jsonl";

pub const KEY_NETWORK: &str = "network";
pub const KEY_ACTIVE_ACCOUNT: &str = "active_account";

/// The config key naming a chain's selected network: `network.solana`.
pub fn network_key_for(chain: ChainId) -> String {
    format!("{KEY_NETWORK}.{chain}")
}

/// The config key naming a chain's active account: `active_account.cardano`.
///
/// The unqualified `active_account` remains the wallet's overall current
/// account. This is the fallback for "which account does `--chain solana`
/// mean when the current one is an EVM account", so switching chains does not
/// lose the account you were last using on each.
pub fn active_account_key_for(chain: ChainId) -> String {
    format!("{KEY_ACTIVE_ACCOUNT}.{chain}")
}

/// How an account's key material was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Mnemonic,
    PrivateKey,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Mnemonic => "mnemonic",
            Source::PrivateKey => "private_key",
        }
    }
}

/// An account as reconstructed by replaying the account log.
#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub address: String,
    /// Which chain this account's key and address belong to.
    ///
    /// Absent from every record written before the wallet was multi-chain, and
    /// those are all EVM accounts — so the default is what makes an existing
    /// store replay unchanged rather than needing a migration.
    #[serde(default)]
    pub chain: ChainId,
    pub source: Source,
    pub private_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mnemonic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    pub created_at: String,
}

/// Redacted on purpose: a `{:?}` of an account must never leak its secrets.
/// `Keypair` has the same policy; `secret_view()` is the deliberate way out.
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("address", &self.address)
            .field("chain", &self.chain)
            .field("source", &self.source)
            .field("index", &self.index)
            .field("private_key", &"<redacted>")
            .field("mnemonic", &self.mnemonic.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Account {
    /// The account without any secret fields — what commands print by default.
    pub fn public_view(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "address": self.address,
            "chain": self.chain.as_str(),
            "source": self.source.as_str(),
            "derivation_path": self.derivation_path,
            "index": self.index,
            "created_at": self.created_at,
        })
    }

    /// The account including secrets — only for `account export` / `--secret`.
    pub fn secret_view(&self) -> Value {
        let mut value = self.public_view();
        let map = value.as_object_mut().expect("public_view builds an object");
        map.insert("private_key".into(), json!(self.private_key));
        map.insert("mnemonic".into(), json!(self.mnemonic));
        value
    }
}

/// A recorded transaction, as reconstructed by replaying the history log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxRecord {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub value: String,
    pub value_wei: String,
    /// Which chain this transfer went out on.
    ///
    /// Absent from records written before the wallet was multi-chain, and
    /// those were all EVM — same reasoning as [`Account::chain`].
    #[serde(default)]
    pub chain: ChainId,
    pub network: String,
    pub chain_id: u64,
    pub nonce: u64,
    pub gas_limit: u64,
    pub gas_price_wei: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<u64>,
    pub created_at: String,
}

/// Key material the wallet has seen before, offered back so a returning user
/// can pick a mnemonic or private key instead of retyping it.
#[derive(Clone, Serialize, Deserialize)]
pub struct RecentSecret {
    pub id: String,
    pub kind: String,
    pub secret: String,
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
    /// True when the address above was derived with a BIP-39 passphrase.
    ///
    /// The passphrase itself is deliberately not stored — keeping it apart from
    /// the phrase is the whole point of it. What is stored is the fact that one
    /// exists, so a restore can demand it back instead of silently producing a
    /// different, unfunded wallet.
    #[serde(default)]
    pub has_passphrase: bool,
    pub first_seen_at: String,
    pub last_used_at: String,
    pub uses: u64,
}

/// Redacted on purpose, like `Account`: the whole point of this type is to
/// hold a secret, so its `{:?}` shows the preview instead.
impl std::fmt::Debug for RecentSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecentSecret")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("address", &self.address)
            .field("secret", &self.preview())
            .finish()
    }
}

impl RecentSecret {
    /// Everything except the secret itself, plus a preview that identifies it.
    pub fn public_view(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "address": self.address,
            "word_count": self.word_count,
            "has_passphrase": self.has_passphrase,
            "preview": self.preview(),
            "first_seen_at": self.first_seen_at,
            "last_used_at": self.last_used_at,
            "uses": self.uses,
        })
    }

    pub fn secret_view(&self) -> Value {
        let mut value = self.public_view();
        let map = value.as_object_mut().expect("public_view builds an object");
        map.insert("secret".into(), json!(self.secret));
        value
    }

    /// Enough of the secret to recognise it, never enough to use it.
    pub fn preview(&self) -> String {
        if self.kind == "mnemonic" {
            let words: Vec<&str> = self.secret.split(' ').collect();
            match words.len() {
                0 => String::new(),
                1 => "…".to_string(),
                _ => format!("{} … {}", words[0], words[words.len() - 1]),
            }
        } else {
            crate::output::truncate_secret(&self.secret)
        }
    }
}

pub struct Store {
    home: PathBuf,
}

impl Store {
    /// Open (and create, if needed) a store rooted at `home`.
    pub fn open(home: PathBuf) -> Result<Self> {
        paths::ensure_dir(&home)?;
        Ok(Store { home })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn accounts_path(&self) -> PathBuf {
        self.home.join(ACCOUNTS_FILE)
    }

    pub fn config_path(&self) -> PathBuf {
        self.home.join(CONFIG_FILE)
    }

    pub fn history_path(&self) -> PathBuf {
        self.home.join(HISTORY_FILE)
    }

    pub fn recent_path(&self) -> PathBuf {
        self.home.join(RECENT_FILE)
    }

    // ---------------------------------------------------------------- raw I/O

    /// Append one compact JSON line, creating the file with owner-only permissions.
    ///
    /// The mode is set at creation, not by a chmod afterwards, so there is no
    /// window in which a freshly created log sits behind the umask.
    fn append(&self, path: &Path, record: &Value) -> Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        // open(2) applies the mode only on creation; tighten a pre-existing
        // file too, in case it was made by something less careful.
        paths::set_private(path, 0o600)?;
        let mut line = serde_json::to_string(record)
            .map_err(|e| error::internal(format!("cannot serialise record: {e}")))?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        file.flush()?;
        Ok(())
    }

    /// Read every well-formed JSON object from a log, skipping junk lines.
    ///
    /// Lines from a newer schema are skipped too, so an old binary degrades
    /// gracefully instead of misreading records it does not understand.
    fn read_lines(&self, path: &Path) -> Result<Vec<Value>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(path)?;
        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if !value.is_object() {
                continue;
            }
            let schema = value.get("schema").and_then(Value::as_u64).unwrap_or(0);
            if schema > SCHEMA as u64 {
                continue;
            }
            out.push(value);
        }
        Ok(out)
    }

    // -------------------------------------------------------------- accounts

    /// Replay the account log into the live account list, in creation order.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut order: Vec<String> = Vec::new();
        let mut by_id: BTreeMap<String, Account> = BTreeMap::new();

        for record in self.read_lines(&self.accounts_path())? {
            let kind = record
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                continue;
            }
            match kind {
                "account.create" => {
                    let Ok(account) = serde_json::from_value::<Account>(record.clone()) else {
                        continue;
                    };
                    if by_id.insert(id.clone(), account).is_none() {
                        order.push(id);
                    }
                }
                "account.rename" => {
                    if let (Some(account), Some(label)) = (
                        by_id.get_mut(&id),
                        record.get("label").and_then(Value::as_str),
                    ) {
                        account.label = label.to_string();
                    }
                }
                "account.delete" => {
                    by_id.remove(&id);
                    order.retain(|existing| existing != &id);
                }
                _ => {}
            }
        }

        Ok(order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect())
    }

    /// The mnemonic new addresses should come from.
    ///
    /// A wallet normally holds one seed and many addresses derived from it, so
    /// "new address" has to know which phrase to walk. The active account's
    /// phrase wins; failing that, the first one in the wallet.
    pub fn current_seed(&self) -> Result<Option<String>> {
        if let Ok(active) = self.active_account() {
            if let Some(mnemonic) = active.mnemonic {
                return Ok(Some(mnemonic));
            }
        }
        Ok(self
            .accounts()?
            .into_iter()
            .find_map(|account| account.mnemonic))
    }

    /// Every account belonging to one chain, in creation order.
    pub fn accounts_on(&self, chain: ChainId) -> Result<Vec<Account>> {
        Ok(self
            .accounts()?
            .into_iter()
            .filter(|account| account.chain == chain)
            .collect())
    }

    /// The lowest address index not yet taken on `mnemonic`, for one chain.
    ///
    /// Indexes run 0, 1, 2, … across the accounts sharing one phrase *on one
    /// chain*. The sequences are per chain because the derivation paths are:
    /// one mnemonic's Solana index 0 and its Cardano index 0 are different
    /// keys, and neither should push the other along.
    pub fn next_address_index(&self, mnemonic: &str, chain: ChainId) -> Result<u32> {
        let taken: Vec<u32> = self
            .accounts()?
            .into_iter()
            .filter(|account| account.chain == chain)
            .filter(|account| account.mnemonic.as_deref() == Some(mnemonic))
            .filter_map(|account| account.index)
            .collect();
        Ok(match taken.iter().max() {
            Some(highest) => highest + 1,
            None => 0,
        })
    }

    /// Resolve an account by id, label, or address (case-insensitive).
    pub fn find_account(&self, selector: &str) -> Result<Account> {
        let needle = selector.trim();
        if needle.is_empty() {
            return Err(error::usage("account selector is empty"));
        }
        let lowered = needle.to_lowercase();
        let accounts = self.accounts()?;
        accounts
            .iter()
            .find(|a| a.id == needle || a.label == needle || a.address.to_lowercase() == lowered)
            .or_else(|| accounts.iter().find(|a| a.label.to_lowercase() == lowered))
            .cloned()
            .ok_or_else(|| error::account_not_found(format!("no account matching '{needle}'")))
    }

    /// The account that commands operate on when none is named.
    pub fn active_account(&self) -> Result<Account> {
        let accounts = self.accounts()?;
        if accounts.is_empty() {
            return Err(error::no_active_account(
                "no accounts yet; create one with `cwbwallet account new`",
            ));
        }
        match self.config_get(KEY_ACTIVE_ACCOUNT)? {
            Some(id) => accounts
                .iter()
                .find(|a| a.id == id)
                .cloned()
                // A stale pointer (deleted account) falls back to the first account.
                .or_else(|| accounts.first().cloned())
                .ok_or_else(|| error::no_active_account("no active account")),
            None => Ok(accounts[0].clone()),
        }
    }

    /// The account a command on `chain` should act on when none is named.
    ///
    /// The overall active account wins when it is already on that chain, so a
    /// single-chain wallet behaves exactly as it always did. Otherwise the
    /// chain's own remembered account is used, and failing that the first
    /// account on it — which is what makes `--chain solana` land somewhere
    /// sensible without the user having to `account use` first.
    pub fn active_account_on(&self, chain: ChainId) -> Result<Account> {
        if let Ok(active) = self.active_account() {
            if active.chain == chain {
                return Ok(active);
            }
        }
        let accounts = self.accounts_on(chain)?;
        if accounts.is_empty() {
            return Err(error::no_active_account(format!(
                "no {chain} accounts yet; create one with \
                 `cwbwallet account new --chain {chain}`"
            )));
        }
        if let Some(id) = self.config_get(&active_account_key_for(chain))? {
            if let Some(account) = accounts.iter().find(|a| a.id == id) {
                return Ok(account.clone());
            }
        }
        Ok(accounts[0].clone())
    }

    /// Remember an account as the current one, overall and for its chain.
    pub fn set_active_account(&self, account: &Account) -> Result<()> {
        self.config_set(KEY_ACTIVE_ACCOUNT, &account.id)?;
        self.config_set(&active_account_key_for(account.chain), &account.id)
    }

    /// Append an `account.create` record. Labels must be unique.
    #[allow(clippy::too_many_arguments)]
    pub fn create_account(
        &self,
        label: Option<&str>,
        address: &str,
        chain: ChainId,
        source: Source,
        private_key: &str,
        mnemonic: Option<&str>,
        derivation_path: Option<&str>,
        index: Option<u32>,
    ) -> Result<Account> {
        let existing = self.accounts()?;
        let label = match label {
            Some(l) => {
                validate_label(l)?;
                if existing.iter().any(|a| a.label.eq_ignore_ascii_case(l)) {
                    return Err(error::duplicate_label(format!(
                        "an account named '{l}' already exists"
                    )));
                }
                l.to_string()
            }
            None => next_free_label(&existing, index, chain),
        };

        let created_at = now_rfc3339();
        let id = account_id(address, &created_at, &label);
        let account = Account {
            id,
            label,
            address: address.to_string(),
            chain,
            source,
            private_key: private_key.to_string(),
            mnemonic: mnemonic.map(str::to_string),
            derivation_path: derivation_path.map(str::to_string),
            index,
            created_at,
        };

        let mut record = serde_json::to_value(&account)
            .map_err(|e| error::internal(format!("cannot serialise account: {e}")))?;
        let map = record
            .as_object_mut()
            .expect("Account serialises to an object");
        map.insert("schema".into(), json!(SCHEMA));
        map.insert("type".into(), json!("account.create"));
        self.append(&self.accounts_path(), &record)?;
        Ok(account)
    }

    pub fn rename_account(&self, id: &str, label: &str) -> Result<()> {
        validate_label(label)?;
        if self
            .accounts()?
            .iter()
            .any(|a| a.id != id && a.label.eq_ignore_ascii_case(label))
        {
            return Err(error::duplicate_label(format!(
                "an account named '{label}' already exists"
            )));
        }
        self.append(
            &self.accounts_path(),
            &json!({
                "schema": SCHEMA,
                "type": "account.rename",
                "id": id,
                "label": label,
                "updated_at": now_rfc3339(),
            }),
        )
    }

    pub fn delete_account(&self, id: &str) -> Result<()> {
        self.append(
            &self.accounts_path(),
            &json!({
                "schema": SCHEMA,
                "type": "account.delete",
                "id": id,
                "deleted_at": now_rfc3339(),
            }),
        )?;
        // Drop a dangling active-account pointer so later reads stay consistent.
        if self.config_get(KEY_ACTIVE_ACCOUNT)?.as_deref() == Some(id) {
            if let Some(next) = self.accounts()?.first() {
                let next_id = next.id.clone();
                self.config_set(KEY_ACTIVE_ACCOUNT, &next_id)?;
            } else {
                self.config_set(KEY_ACTIVE_ACCOUNT, "")?;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- config

    /// Replay the config log; later writes win.
    pub fn config(&self) -> Result<BTreeMap<String, String>> {
        let mut map = BTreeMap::new();
        for record in self.read_lines(&self.config_path())? {
            if record.get("type").and_then(Value::as_str) != Some("config.set") {
                continue;
            }
            let (Some(key), Some(value)) = (
                record.get("key").and_then(Value::as_str),
                record.get("value").and_then(Value::as_str),
            ) else {
                continue;
            };
            if value.is_empty() {
                map.remove(key);
            } else {
                map.insert(key.to_string(), value.to_string());
            }
        }
        Ok(map)
    }

    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.config()?.get(key).cloned())
    }

    pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
        self.append(
            &self.config_path(),
            &json!({
                "schema": SCHEMA,
                "type": "config.set",
                "key": key,
                "value": value,
                "updated_at": now_rfc3339(),
            }),
        )
    }

    /// The selected network key, defaulting to the testnet.
    /// The wallet's overall selected network.
    ///
    /// An unreadable or since-removed key falls back to the default rather
    /// than failing: a config naming a network this build no longer ships
    /// should not make the whole wallet unusable.
    pub fn network(&self) -> Result<network::Network> {
        match self.config_get(KEY_NETWORK)? {
            Some(key) => network::find(&key).or_else(|_| network::find(network::DEFAULT_NETWORK)),
            None => network::find(network::DEFAULT_NETWORK),
        }
    }

    /// The selected network for one chain.
    ///
    /// Each chain remembers its own, so moving to Solana and back does not
    /// silently reset Cronos to testnet. The overall `network` key still
    /// answers for whichever chain it names, which is what keeps a wallet
    /// written before this change on the network it was left on.
    pub fn network_on(&self, chain: ChainId) -> Result<network::Network> {
        if let Some(key) = self.config_get(&network_key_for(chain))? {
            if let Ok(found) = network::find(&key) {
                if found.chain == chain {
                    return Ok(found);
                }
            }
        }
        if let Some(key) = self.config_get(KEY_NETWORK)? {
            if let Ok(found) = network::find(&key) {
                if found.chain == chain {
                    return Ok(found);
                }
            }
        }
        Ok(network::default_for(chain))
    }

    /// Select a network, for its chain and as the wallet's overall one.
    pub fn set_network(&self, target: &network::Network) -> Result<()> {
        self.config_set(KEY_NETWORK, target.key)?;
        self.config_set(&network_key_for(target.chain), target.key)
    }

    // ---------------------------------------------------------------- recent

    /// Replay the recall log, most recently used first.
    pub fn recent(&self) -> Result<Vec<RecentSecret>> {
        let mut by_id: BTreeMap<String, RecentSecret> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();

        for record in self.read_lines(&self.recent_path())? {
            let kind = record
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                continue;
            }
            match kind {
                "secret.remember" => {
                    let Ok(entry) = serde_json::from_value::<RecentSecret>(record.clone()) else {
                        continue;
                    };
                    if by_id.insert(id.clone(), entry).is_none() {
                        order.push(id);
                    }
                }
                "secret.forget" => {
                    by_id.remove(&id);
                    order.retain(|existing| existing != &id);
                }
                _ => {}
            }
        }

        let mut entries: Vec<RecentSecret> = order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect();
        // Newest use first; the log order breaks ties deterministically.
        entries.reverse();
        entries.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
        Ok(entries)
    }

    /// Record that this key material was used, refreshing an existing entry.
    pub fn remember_secret(
        &self,
        kind: &str,
        secret: &str,
        address: &str,
        word_count: Option<usize>,
    ) -> Result<RecentSecret> {
        self.remember_secret_with(kind, secret, address, word_count, false)
    }

    /// As `remember_secret`, recording whether a passphrase produced `address`.
    pub fn remember_secret_with(
        &self,
        kind: &str,
        secret: &str,
        address: &str,
        word_count: Option<usize>,
        has_passphrase: bool,
    ) -> Result<RecentSecret> {
        let id = secret_id(kind, secret);
        let now = now_rfc3339();
        let existing = self.recent()?.into_iter().find(|entry| entry.id == id);
        let entry = RecentSecret {
            id,
            kind: kind.to_string(),
            secret: secret.to_string(),
            address: address.to_string(),
            word_count,
            has_passphrase,
            first_seen_at: existing
                .as_ref()
                .map(|e| e.first_seen_at.clone())
                .unwrap_or_else(|| now.clone()),
            last_used_at: now,
            uses: existing.map(|e| e.uses).unwrap_or(0) + 1,
        };

        let mut record = serde_json::to_value(&entry)
            .map_err(|e| error::internal(format!("cannot serialise recalled secret: {e}")))?;
        let map = record
            .as_object_mut()
            .expect("RecentSecret serialises to an object");
        map.insert("schema".into(), json!(SCHEMA));
        map.insert("type".into(), json!("secret.remember"));
        self.append(&self.recent_path(), &record)?;
        Ok(entry)
    }

    /// Resolve a recalled secret by id, 1-based position, or address.
    pub fn find_recent(&self, selector: &str) -> Result<RecentSecret> {
        let needle = selector.trim();
        if needle.is_empty() {
            return Err(error::usage("recall selector is empty"));
        }
        let entries = self.recent()?;
        if entries.is_empty() {
            return Err(error::not_found("nothing has been remembered yet"));
        }
        // Positions are 1-based and written in plain digits. Parsing loosely
        // here would let "0" resolve to the newest entry — so `recent forget 0`
        // would destroy a secret nobody selected — and would accept "+1".
        if needle.bytes().all(|b| b.is_ascii_digit()) {
            let position: usize = needle.parse().map_err(|_| {
                error::not_found(format!("no remembered entry at position {needle}"))
            })?;
            let index = position.checked_sub(1).ok_or_else(|| {
                error::not_found("remembered entries are numbered from 1, so 0 selects nothing")
            })?;
            return entries.get(index).cloned().ok_or_else(|| {
                error::not_found(format!("no remembered entry at position {position}"))
            });
        }
        let lowered = needle.to_lowercase();
        entries
            .into_iter()
            .find(|entry| entry.id == needle || entry.address.to_lowercase() == lowered)
            .ok_or_else(|| error::not_found(format!("no remembered entry matching '{needle}'")))
    }

    pub fn forget_secret(&self, id: &str) -> Result<()> {
        self.append(
            &self.recent_path(),
            &json!({
                "schema": SCHEMA,
                "type": "secret.forget",
                "id": id,
                "deleted_at": now_rfc3339(),
            }),
        )
    }

    /// Forget everything, one record per entry so the log stays append-only.
    pub fn clear_recent(&self) -> Result<usize> {
        let entries = self.recent()?;
        for entry in &entries {
            self.forget_secret(&entry.id)?;
        }
        Ok(entries.len())
    }

    // --------------------------------------------------------------- history

    /// Replay the transaction log, newest last.
    pub fn history(&self) -> Result<Vec<TxRecord>> {
        let mut order: Vec<String> = Vec::new();
        let mut by_hash: BTreeMap<String, TxRecord> = BTreeMap::new();

        for record in self.read_lines(&self.history_path())? {
            let kind = record
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let hash = record
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            if hash.is_empty() {
                continue;
            }
            match kind {
                "tx.send" => {
                    let Ok(tx) = serde_json::from_value::<TxRecord>(record.clone()) else {
                        continue;
                    };
                    if by_hash.insert(hash.clone(), tx).is_none() {
                        order.push(hash);
                    }
                }
                "tx.update" => {
                    if let Some(tx) = by_hash.get_mut(&hash) {
                        if let Some(status) = record.get("status").and_then(Value::as_str) {
                            tx.status = status.to_string();
                        }
                        if let Some(block) = record.get("block_number").and_then(Value::as_u64) {
                            tx.block_number = Some(block);
                        }
                        if let Some(gas) = record.get("gas_used").and_then(Value::as_u64) {
                            tx.gas_used = Some(gas);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(order
            .into_iter()
            .filter_map(|h| by_hash.remove(&h))
            .collect())
    }

    pub fn record_tx(&self, tx: &TxRecord) -> Result<()> {
        let mut record = serde_json::to_value(tx)
            .map_err(|e| error::internal(format!("cannot serialise transaction: {e}")))?;
        let map = record
            .as_object_mut()
            .expect("TxRecord serialises to an object");
        map.insert("schema".into(), json!(SCHEMA));
        map.insert("type".into(), json!("tx.send"));
        self.append(&self.history_path(), &record)
    }

    pub fn update_tx(
        &self,
        hash: &str,
        status: &str,
        block_number: Option<u64>,
        gas_used: Option<u64>,
    ) -> Result<()> {
        self.append(
            &self.history_path(),
            &json!({
                "schema": SCHEMA,
                "type": "tx.update",
                "hash": hash.to_lowercase(),
                "status": status,
                "block_number": block_number,
                "gas_used": gas_used,
                "updated_at": now_rfc3339(),
            }),
        )
    }
}

/// Deterministic, collision-resistant account id.
///
/// The label is part of the preimage because labels are unique: without it,
/// importing the same address twice inside one millisecond would produce two
/// records that share an id and get folded together on replay.
pub fn account_id(address: &str, created_at: &str, label: &str) -> String {
    let digest =
        keccak256(format!("{}|{}|{}", address.to_lowercase(), created_at, label).as_bytes());
    format!("acc_{}", hex::encode(&digest[..8]))
}

/// Stable id for key material, so re-using a phrase updates one entry.
pub fn secret_id(kind: &str, secret: &str) -> String {
    let digest = keccak256(format!("{kind}|{}", secret.trim()).as_bytes());
    format!("sec_{}", hex::encode(&digest[..8]))
}

/// Labels are used as command line selectors, so keep them shell-friendly.
pub fn validate_label(label: &str) -> Result<()> {
    if label.is_empty() || label.len() > 64 {
        return Err(error::usage("label must be 1..64 characters"));
    }
    if !label
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(error::usage(
            "label may only contain letters, digits, '.', '_' and '-'",
        ));
    }
    if label.starts_with("0x") {
        return Err(error::usage("label must not look like an address"));
    }
    Ok(())
}

/// The name an account gets when the caller does not supply one.
///
/// `account0-evm` says both halves of what an account is — the wallet index
/// and the chain — so the four accounts of one wallet read as one wallet
/// rather than as `account-1` through `account-4`. An imported key has no
/// index to name it by, and a name already taken falls back to `account-N`.
pub fn default_label(index: u32, chain: ChainId) -> String {
    format!("account{index}-{chain}")
}

/// What to call an account on screen and in an export.
///
/// A name the user chose is theirs and is used as it is. The names the wallet
/// gave itself before it was multi-chain — `account-1`, `account-2` — say
/// neither which wallet nor which chain, so an account that has an index is
/// re-rendered in the scheme new accounts are created with: `account0-evm`.
pub fn display_label(account: &Account) -> String {
    let auto = account
        .label
        .strip_prefix("account-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
    // Only an account that belongs to a wallet can be named after one: an
    // imported private key has no index, and claiming index 0 for it would put
    // two different keys under one name.
    match account.index {
        Some(index) if auto => default_label(index, account.chain),
        _ => account.label.clone(),
    }
}

fn next_free_label(existing: &[Account], index: Option<u32>, chain: ChainId) -> String {
    let taken = |candidate: &str| {
        existing
            .iter()
            .any(|a| a.label.eq_ignore_ascii_case(candidate))
    };
    if let Some(index) = index {
        let preferred = default_label(index, chain);
        if !taken(&preferred) {
            return preferred;
        }
    }
    for n in 1.. {
        let candidate = format!("account-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("the loop always returns")
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("home")).unwrap();
        (dir, store)
    }

    fn add(store: &Store, label: Option<&str>, address: &str) -> Account {
        store
            .create_account(
                label,
                address,
                ChainId::Evm,
                Source::PrivateKey,
                "0xkey",
                None,
                None,
                None,
            )
            .unwrap()
    }

    #[test]
    fn creates_the_home_directory() {
        let (_dir, store) = store();
        assert!(store.home().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn home_and_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        let mode = std::fs::metadata(store.home())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);

        add(&store, Some("a"), "0x1");
        let mode = std::fs::metadata(store.accounts_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "secrets must not be world readable");
    }

    #[test]
    fn accounts_start_empty() {
        let (_dir, store) = store();
        assert!(store.accounts().unwrap().is_empty());
        assert_eq!(
            store.active_account().unwrap_err().code,
            error::Code::NoActiveAccount
        );
    }

    #[test]
    fn accounts_replay_in_creation_order() {
        let (_dir, store) = store();
        add(&store, Some("first"), "0xaaa");
        add(&store, Some("second"), "0xbbb");
        add(&store, Some("third"), "0xccc");
        let labels: Vec<_> = store
            .accounts()
            .unwrap()
            .into_iter()
            .map(|a| a.label)
            .collect();
        assert_eq!(labels, ["first", "second", "third"]);
    }

    #[test]
    fn auto_labels_fill_the_lowest_gap() {
        let (_dir, store) = store();
        assert_eq!(add(&store, None, "0x1").label, "account-1");
        assert_eq!(add(&store, None, "0x2").label, "account-2");
        let target = add(&store, None, "0x3");
        assert_eq!(target.label, "account-3");
        store.delete_account(&target.id).unwrap();
        assert_eq!(add(&store, None, "0x4").label, "account-3");
    }

    #[test]
    fn a_derived_account_is_named_for_its_index_and_chain() {
        let (_dir, store) = store();
        let evm = store
            .create_account(
                None,
                "0x1",
                ChainId::Evm,
                Source::Mnemonic,
                "0xkey",
                Some("phrase"),
                None,
                Some(0),
            )
            .unwrap();
        let solana = store
            .create_account(
                None,
                "sol1",
                ChainId::Solana,
                Source::Mnemonic,
                "key",
                Some("phrase"),
                None,
                Some(0),
            )
            .unwrap();
        // One wallet, one index, two chains — and the names say so.
        assert_eq!(evm.label, "account0-evm");
        assert_eq!(solana.label, "account0-solana");
    }

    #[test]
    fn duplicate_labels_are_rejected_case_insensitively() {
        let (_dir, store) = store();
        add(&store, Some("main"), "0x1");
        let err = store
            .create_account(
                Some("MAIN"),
                "0x2",
                ChainId::Evm,
                Source::PrivateKey,
                "0xk",
                None,
                None,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::DuplicateLabel);
    }

    #[test]
    fn rejects_hostile_labels() {
        let (_dir, store) = store();
        for bad in ["", "has space", "quote'", "0xdeadbeef", &"x".repeat(65)] {
            assert!(
                store
                    .create_account(
                        Some(bad),
                        "0x1",
                        ChainId::Evm,
                        Source::PrivateKey,
                        "0xk",
                        None,
                        None,
                        None
                    )
                    .is_err(),
                "label {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn finds_accounts_by_id_label_and_address() {
        let (_dir, store) = store();
        let account = add(
            &store,
            Some("main"),
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
        );
        assert_eq!(store.find_account(&account.id).unwrap().id, account.id);
        assert_eq!(store.find_account("main").unwrap().id, account.id);
        assert_eq!(store.find_account("MAIN").unwrap().id, account.id);
        assert_eq!(
            store
                .find_account("0x9858effd232b4033e47d90003d41ec34ecaeda94")
                .unwrap()
                .id,
            account.id
        );
        assert_eq!(
            store.find_account("nope").unwrap_err().code,
            error::Code::AccountNotFound
        );
    }

    #[test]
    fn rename_updates_the_label_without_losing_history() {
        let (_dir, store) = store();
        let account = add(&store, Some("old"), "0x1");
        store.rename_account(&account.id, "new").unwrap();
        assert_eq!(store.find_account("new").unwrap().id, account.id);
        assert!(store.find_account("old").is_err());
        // Renaming appends rather than rewriting.
        let lines = std::fs::read_to_string(store.accounts_path()).unwrap();
        assert_eq!(lines.lines().count(), 2);
    }

    #[test]
    fn rename_rejects_a_taken_label() {
        let (_dir, store) = store();
        let a = add(&store, Some("a"), "0x1");
        add(&store, Some("b"), "0x2");
        assert_eq!(
            store.rename_account(&a.id, "b").unwrap_err().code,
            error::Code::DuplicateLabel
        );
        // Renaming an account to its own label is a no-op, not a conflict.
        assert!(store.rename_account(&a.id, "a").is_ok());
    }

    #[test]
    fn delete_removes_the_account_and_repoints_active() {
        let (_dir, store) = store();
        let a = add(&store, Some("a"), "0x1");
        let b = add(&store, Some("b"), "0x2");
        store.config_set(KEY_ACTIVE_ACCOUNT, &a.id).unwrap();
        store.delete_account(&a.id).unwrap();

        assert_eq!(store.accounts().unwrap().len(), 1);
        assert!(store.find_account("a").is_err());
        assert_eq!(store.active_account().unwrap().id, b.id);
    }

    #[test]
    fn deleting_the_last_account_clears_the_pointer() {
        let (_dir, store) = store();
        let a = add(&store, Some("a"), "0x1");
        store.config_set(KEY_ACTIVE_ACCOUNT, &a.id).unwrap();
        store.delete_account(&a.id).unwrap();
        assert_eq!(store.config_get(KEY_ACTIVE_ACCOUNT).unwrap(), None);
    }

    #[test]
    fn a_stale_active_pointer_falls_back_to_the_first_account() {
        let (_dir, store) = store();
        let a = add(&store, Some("a"), "0x1");
        store
            .config_set(KEY_ACTIVE_ACCOUNT, "acc_doesnotexist")
            .unwrap();
        assert_eq!(store.active_account().unwrap().id, a.id);
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(account_id("0xABC", "t", "l"), account_id("0xabc", "t", "l"));
        assert_ne!(
            account_id("0xabc", "t1", "l"),
            account_id("0xabc", "t2", "l")
        );
        // Same address and instant, different label — must not collide.
        assert_ne!(
            account_id("0xabc", "t", "one"),
            account_id("0xabc", "t", "two")
        );
        assert!(account_id("0xabc", "t", "l").starts_with("acc_"));
        assert_eq!(account_id("0xabc", "t", "l").len(), 4 + 16);
    }

    #[test]
    fn the_same_address_can_be_stored_twice_under_different_labels() {
        let (_dir, store) = store();
        let first = add(&store, Some("copy-one"), "0xabc");
        let second = add(&store, Some("copy-two"), "0xabc");
        assert_ne!(first.id, second.id);
        assert_eq!(store.accounts().unwrap().len(), 2);
    }

    #[test]
    fn config_last_write_wins_and_empty_clears() {
        let (_dir, store) = store();
        assert_eq!(store.config_get("k").unwrap(), None);
        store.config_set("k", "one").unwrap();
        store.config_set("k", "two").unwrap();
        assert_eq!(store.config_get("k").unwrap().as_deref(), Some("two"));
        store.config_set("k", "").unwrap();
        assert_eq!(store.config_get("k").unwrap(), None);
    }

    #[test]
    fn network_defaults_to_testnet_and_survives_garbage() {
        let (_dir, store) = store();
        assert_eq!(store.network().unwrap().key, "cronos-testnet");
        store.config_set(KEY_NETWORK, "cronos-mainnet").unwrap();
        assert_eq!(store.network().unwrap().key, "cronos-mainnet");
        store.config_set(KEY_NETWORK, "who-knows").unwrap();
        assert_eq!(store.network().unwrap().key, "cronos-testnet");
    }

    fn sample_tx(hash: &str) -> TxRecord {
        TxRecord {
            chain: ChainId::Evm,
            hash: hash.to_string(),
            from: "0xfrom".into(),
            to: "0xto".into(),
            value: "1".into(),
            value_wei: "1000000000000000000".into(),
            network: "cronos-testnet".into(),
            chain_id: 338,
            nonce: 0,
            gas_limit: 21000,
            gas_price_wei: "5000000000".into(),
            status: "submitted".into(),
            token: None,
            block_number: None,
            gas_used: None,
            created_at: now_rfc3339(),
        }
    }

    #[test]
    fn history_records_and_updates_transactions() {
        let (_dir, store) = store();
        assert!(store.history().unwrap().is_empty());

        store.record_tx(&sample_tx("0xAA")).unwrap();
        store.record_tx(&sample_tx("0xbb")).unwrap();
        assert_eq!(store.history().unwrap().len(), 2);

        // Updates match case-insensitively on the hash.
        store
            .update_tx("0xaa", "confirmed", Some(42), Some(21000))
            .unwrap();
        let tx = store.history().unwrap().into_iter().next().unwrap();
        assert_eq!(tx.status, "confirmed");
        assert_eq!(tx.block_number, Some(42));
        assert_eq!(tx.gas_used, Some(21000));
        // The second transaction is untouched.
        assert_eq!(store.history().unwrap()[1].status, "submitted");
    }

    #[test]
    fn an_update_for_an_unknown_hash_is_harmless() {
        let (_dir, store) = store();
        store.update_tx("0xzz", "confirmed", None, None).unwrap();
        assert!(store.history().unwrap().is_empty());
    }

    #[test]
    fn corrupt_lines_are_skipped_not_fatal() {
        let (_dir, store) = store();
        add(&store, Some("good"), "0x1");
        let mut file = OpenOptions::new()
            .append(true)
            .open(store.accounts_path())
            .unwrap();
        writeln!(file, "this is not json").unwrap();
        writeln!(file, "[1, 2, 3]").unwrap();
        writeln!(file, "{{\"type\": \"account.create\"}}").unwrap();
        writeln!(file).unwrap();
        drop(file);

        add(&store, Some("also-good"), "0x2");
        let labels: Vec<_> = store
            .accounts()
            .unwrap()
            .into_iter()
            .map(|a| a.label)
            .collect();
        assert_eq!(labels, ["good", "also-good"]);
    }

    #[test]
    fn records_from_a_newer_schema_are_ignored() {
        let (_dir, store) = store();
        add(&store, Some("known"), "0x1");
        let mut file = OpenOptions::new()
            .append(true)
            .open(store.accounts_path())
            .unwrap();
        writeln!(
            file,
            r#"{{"schema":99,"type":"account.create","id":"acc_future","label":"future","address":"0x2","source":"private_key","private_key":"0xk","created_at":"now"}}"#
        )
        .unwrap();
        drop(file);
        assert_eq!(store.accounts().unwrap().len(), 1);
    }

    #[test]
    fn files_stay_append_only() {
        let (_dir, store) = store();
        let a = add(&store, Some("a"), "0x1");
        let after_create = std::fs::read_to_string(store.accounts_path()).unwrap();
        store.rename_account(&a.id, "b").unwrap();
        store.delete_account(&a.id).unwrap();
        let after_delete = std::fs::read_to_string(store.accounts_path()).unwrap();
        assert!(
            after_delete.starts_with(&after_create),
            "earlier lines must never change"
        );
        assert_eq!(after_delete.lines().count(), 3);
    }

    #[test]
    fn every_line_is_a_single_compact_json_object() {
        let (_dir, store) = store();
        add(&store, Some("a"), "0x1");
        store.config_set("k", "v").unwrap();
        store.record_tx(&sample_tx("0x1")).unwrap();
        for path in [
            store.accounts_path(),
            store.config_path(),
            store.history_path(),
        ] {
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.ends_with('\n'), "{path:?} must end with a newline");
            for line in content.lines() {
                let value: Value = serde_json::from_str(line).unwrap();
                assert!(
                    value.get("schema").is_some(),
                    "every record carries a schema"
                );
                assert!(value.get("type").is_some(), "every record carries a type");
            }
        }
    }

    #[test]
    fn secret_view_adds_what_public_view_hides() {
        let (_dir, store) = store();
        let account = store
            .create_account(
                Some("m"),
                "0xabc",
                ChainId::Evm,
                Source::Mnemonic,
                "0xdeadbeef",
                Some("word word"),
                Some("m/44'/60'/0'/0/0"),
                Some(0),
            )
            .unwrap();
        let public = account.public_view();
        assert!(public.get("private_key").is_none());
        assert!(public.get("mnemonic").is_none());
        let secret = account.secret_view();
        assert_eq!(secret["private_key"], "0xdeadbeef");
        assert_eq!(secret["mnemonic"], "word word");
    }

    // ------------------------------------------------------------ recall log

    #[test]
    fn recall_starts_empty() {
        let (_dir, store) = store();
        assert!(store.recent().unwrap().is_empty());
        assert_eq!(
            store.find_recent("1").unwrap_err().code,
            error::Code::NotFound
        );
    }

    #[test]
    fn remembering_the_same_secret_updates_one_entry() {
        let (_dir, store) = store();
        let first = store
            .remember_secret("mnemonic", "abandon about", "0xabc", Some(12))
            .unwrap();
        assert_eq!(first.uses, 1);

        let again = store
            .remember_secret("mnemonic", "abandon about", "0xabc", Some(12))
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.uses, 2);
        assert_eq!(
            again.first_seen_at, first.first_seen_at,
            "first sighting is preserved"
        );

        let entries = store.recent().unwrap();
        assert_eq!(entries.len(), 1, "a repeat must not duplicate the entry");
        assert_eq!(entries[0].uses, 2);
        // Both writes are on disk; only the replay folds them.
        assert_eq!(
            std::fs::read_to_string(store.recent_path())
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn the_same_material_under_different_kinds_stays_distinct() {
        let (_dir, store) = store();
        store
            .remember_secret("mnemonic", "shared", "0xa", None)
            .unwrap();
        store
            .remember_secret("private_key", "shared", "0xa", None)
            .unwrap();
        assert_eq!(store.recent().unwrap().len(), 2);
    }

    #[test]
    fn recall_is_ordered_by_most_recent_use() {
        let (_dir, store) = store();
        store
            .remember_secret("mnemonic", "one", "0x1", Some(12))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .remember_secret("mnemonic", "two", "0x2", Some(12))
            .unwrap();
        assert_eq!(store.recent().unwrap()[0].secret, "two");

        // Re-using the older one moves it back to the front.
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .remember_secret("mnemonic", "one", "0x1", Some(12))
            .unwrap();
        assert_eq!(store.recent().unwrap()[0].secret, "one");
    }

    #[test]
    fn recall_resolves_by_id_position_and_address() {
        let (_dir, store) = store();
        let entry = store
            .remember_secret(
                "private_key",
                "0xdead",
                "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
                None,
            )
            .unwrap();
        assert_eq!(store.find_recent(&entry.id).unwrap().id, entry.id);
        assert_eq!(store.find_recent("1").unwrap().id, entry.id);
        assert_eq!(
            store
                .find_recent("0x9858effd232b4033e47d90003d41ec34ecaeda94")
                .unwrap()
                .id,
            entry.id
        );
        assert!(store.find_recent("9").is_err());
        assert!(store.find_recent("nope").is_err());
        assert!(store.find_recent("  ").is_err());
    }

    #[test]
    fn forgetting_removes_one_entry_and_clear_removes_all() {
        let (_dir, store) = store();
        let first = store
            .remember_secret("mnemonic", "one", "0x1", Some(12))
            .unwrap();
        store
            .remember_secret("mnemonic", "two", "0x2", Some(12))
            .unwrap();

        store.forget_secret(&first.id).unwrap();
        assert_eq!(store.recent().unwrap().len(), 1);
        assert!(store.find_recent(&first.id).is_err());

        assert_eq!(store.clear_recent().unwrap(), 1);
        assert!(store.recent().unwrap().is_empty());
    }

    #[test]
    fn forgetting_is_append_only() {
        let (_dir, store) = store();
        let entry = store
            .remember_secret("mnemonic", "one", "0x1", Some(12))
            .unwrap();
        let before = std::fs::read_to_string(store.recent_path()).unwrap();
        store.forget_secret(&entry.id).unwrap();
        let after = std::fs::read_to_string(store.recent_path()).unwrap();
        assert!(after.starts_with(&before));
    }

    #[test]
    fn secret_ids_are_stable_and_kind_scoped() {
        assert_eq!(secret_id("mnemonic", "a b"), secret_id("mnemonic", " a b "));
        assert_ne!(
            secret_id("mnemonic", "a b"),
            secret_id("private_key", "a b")
        );
        assert_ne!(secret_id("mnemonic", "a b"), secret_id("mnemonic", "a c"));
        assert!(secret_id("mnemonic", "a").starts_with("sec_"));
        assert_eq!(secret_id("mnemonic", "a").len(), 4 + 16);
    }

    #[test]
    fn recall_previews_identify_without_revealing() {
        let (_dir, store) = store();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let entry = store
            .remember_secret("mnemonic", phrase, "0xabc", Some(12))
            .unwrap();
        let preview = entry.preview();
        assert_eq!(preview, "abandon … about");
        assert!(!preview.contains(phrase));

        let key = store
            .remember_secret(
                "private_key",
                "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727",
                "0xabc",
                None,
            )
            .unwrap();
        assert!(key.preview().starts_with("0x1ab42c"));
        assert!(!key.preview().contains("b618bdea"));
    }

    #[test]
    fn recall_public_view_hides_the_secret() {
        let (_dir, store) = store();
        let entry = store
            .remember_secret("mnemonic", "one two", "0xabc", Some(2))
            .unwrap();
        assert!(entry.public_view().get("secret").is_none());
        assert_eq!(entry.secret_view()["secret"], "one two");
    }

    /// A stray `{:?}` — a dbg!, a panic message, an error context — must never
    /// print key material. `secret_view()` is the deliberate way out.
    #[test]
    fn debug_output_never_contains_secrets() {
        let (_dir, store) = store();
        let account = store
            .create_account(
                Some("m"),
                "0xabc",
                ChainId::Evm,
                Source::Mnemonic,
                "0xdeadbeefdeadbeef",
                Some("correct horse battery staple"),
                Some("m/44'/60'/0'/0/0"),
                Some(0),
            )
            .unwrap();
        let debugged = format!("{account:?}");
        assert!(!debugged.contains("deadbeef"), "{debugged}");
        assert!(!debugged.contains("correct horse"), "{debugged}");
        assert!(
            debugged.contains("0xabc"),
            "the address still identifies it"
        );

        let entry = store
            .remember_secret("mnemonic", "correct horse battery staple", "0xabc", Some(4))
            .unwrap();
        let debugged = format!("{entry:?}");
        assert!(!debugged.contains("battery staple"), "{debugged}");
        assert!(
            debugged.contains("correct … staple"),
            "the preview identifies it"
        );
    }

    /// The log file must be owner-only from the moment it exists — a chmod
    /// after the first record is written leaves that record behind the umask.
    #[cfg(unix)]
    #[test]
    fn the_store_file_is_born_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        // Force a permissive umask-like starting point: pre-create the file
        // world-readable and confirm the store tightens it on the next append.
        std::fs::write(store.accounts_path(), "").unwrap();
        std::fs::set_permissions(
            store.accounts_path(),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        add(&store, Some("a"), "0x1");
        let mode = std::fs::metadata(store.accounts_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "a pre-existing lax file gets tightened");
    }

    #[test]
    fn two_stores_over_one_home_see_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let a = Store::open(dir.path().to_path_buf()).unwrap();
        let b = Store::open(dir.path().to_path_buf()).unwrap();
        add(&a, Some("from-a"), "0x1");
        assert_eq!(b.accounts().unwrap()[0].label, "from-a");
    }
}
