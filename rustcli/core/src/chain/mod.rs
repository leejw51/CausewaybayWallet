//! The multi-chain layer: what every chain must be able to do, and how the
//! wallet reaches it.
//!
//! A chain is described by two traits, split along the line that matters most
//! in a wallet — whether the operation touches a network:
//!
//! * [`Chain`] is the **offline** half. Deriving a key, rendering an address,
//!   parsing an amount, signing a message: all pure, all synchronous, all
//!   testable without a node. It is what makes `utils derive --chain solana`
//!   work on a train.
//! * [`ChainClient`] is the **online** half, and it is `async`. Reading a
//!   balance and moving funds are the only things a wallet genuinely has to
//!   wait on, so they are the only things coloured by it. One
//!   [`Chain::client`] call per network yields the client for it.
//!
//! The split is also what keeps `send` honest across four very different
//! ledgers. A transfer is always [`ChainClient::prepare_transfer`] followed by
//! [`ChainClient::submit`], never one call: everything that can be refused —
//! a malformed recipient, a fee that outruns the balance, a Cardano output
//! below the minimum, a Midnight UTxO that cannot pay its own dust — is
//! refused by `prepare`, *before* the user is asked to confirm and before a
//! key signs anything. `submit` then only broadcasts what was already agreed
//! to. `--dry-run` is simply a `prepare` with no `submit` after it.
//!
//! # Adding a chain
//!
//! Implement both traits, add a variant to [`ChainId`], and list the chain in
//! [`registry`]. Nothing else in the wallet needs to learn its name: the
//! command surface, the store, the C ABI and the TUI all read the registry.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{self, Result};
use crate::network::Network;

pub mod amount;
pub mod cardano;
pub mod evm;
pub mod http;
pub mod midnight;
pub mod seed;
pub mod solana;

pub use amount::Amount;
pub use seed::Seed;

// ============================================================== chain identity

/// Every chain the wallet knows about.
///
/// The string form is the stable one: it is what `--chain` accepts, what the
/// store writes into an account record, and what a foreign caller sends over
/// the C ABI. Adding a variant is additive; renaming one is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainId {
    /// Cronos and any other EVM chain: secp256k1, EIP-55 addresses.
    Evm,
    /// Solana: ed25519 over SLIP-0010, base58 addresses.
    Solana,
    /// Cardano: extended ed25519 over BIP32-Ed25519, bech32 addresses.
    Cardano,
    /// Midnight: secp256k1 → BIP-340 Schnorr, bech32m addresses.
    Midnight,
}

impl ChainId {
    pub const ALL: [ChainId; 4] = [
        ChainId::Evm,
        ChainId::Solana,
        ChainId::Cardano,
        ChainId::Midnight,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ChainId::Evm => "evm",
            ChainId::Solana => "solana",
            ChainId::Cardano => "cardano",
            ChainId::Midnight => "midnight",
        }
    }

    /// Parse a chain name, accepting the aliases people actually type.
    ///
    /// `cronos` resolves to [`ChainId::Evm`] because that is the EVM chain
    /// this wallet ships networks for — but the chain itself is not
    /// Cronos-specific, which is why the canonical name is not either.
    pub fn parse(name: &str) -> Result<ChainId> {
        let normalized = name.trim().to_lowercase().replace(['_', ' '], "-");
        match normalized.as_str() {
            "evm" | "cronos" | "ethereum" | "eth" => Ok(ChainId::Evm),
            "solana" | "sol" => Ok(ChainId::Solana),
            "cardano" | "ada" => Ok(ChainId::Cardano),
            "midnight" | "night" | "mn" => Ok(ChainId::Midnight),
            _ => Err(error::usage(format!(
                "unknown chain '{name}'; known chains: {}",
                ChainId::ALL
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Default for ChainId {
    /// An account record written before this wallet knew about other chains
    /// has no `chain` field, and it is an EVM account.
    fn default() -> Self {
        ChainId::Evm
    }
}

// ================================================================== key material

/// A freshly derived (or imported) account, before the store gets hold of it.
///
/// `secret` is whatever that chain calls a private key, in whatever encoding
/// that chain's tooling uses — `0x…` hex for EVM and Midnight, base58 for a
/// Solana keypair, hex of the 96-byte extended key for Cardano. It is opaque
/// to everything except the [`Chain`] that produced it, which is the whole
/// reason the store can hold four kinds of key in one column.
pub struct DerivedAccount {
    pub address: String,
    pub secret: String,
    pub public_key: String,
    pub derivation_path: Option<String>,
    /// Extra chain-specific facts worth showing: a Cardano reward address, a
    /// Midnight dust key, a Solana keypair file body.
    pub extra: Value,
}

/// A key that can sign. Produced by [`Chain::signer`] from a stored secret.
pub trait Signer: Send + Sync {
    /// The address this key controls, in the chain's own rendering.
    fn address(&self) -> String;
    /// Sign an opaque message, in whatever scheme the chain considers
    /// canonical for personal messages.
    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>>;
}

// ================================================================== transfers

/// What the user asked for, before any of it has been checked.
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub to: String,
    /// The amount in the chain's base units — lamports, lovelace, wei, stars.
    pub amount: u128,
    /// Fee override, in the chain's own terms (gas price in wei for EVM).
    pub fee_override: Option<u128>,
    /// Nonce/sequence override, where the chain has one.
    pub nonce_override: Option<u64>,
    /// Gas limit, where the chain has one.
    pub gas_limit: Option<u64>,
    /// Extra call data, where the chain has any.
    pub data: Vec<u8>,
    /// The most the user is willing to pay in fees, in the fee token's base
    /// units. `None` means the network's own [`Network::max_fee`].
    pub max_fee: Option<u128>,
}

impl TransferRequest {
    pub fn new(to: impl Into<String>, amount: u128) -> Self {
        TransferRequest {
            to: to.into(),
            amount,
            fee_override: None,
            nonce_override: None,
            gas_limit: None,
            data: Vec::new(),
            max_fee: None,
        }
    }

    /// The ceiling this request should be held to on `network`.
    pub fn fee_ceiling(&self, network: &Network) -> u128 {
        self.max_fee.unwrap_or(network.max_fee)
    }
}

/// Refuse a fee no honest transfer on this network would charge.
///
/// Every chain calls this from `prepare_transfer` *before* it signs, because
/// the fee is the one number in a transfer that the wallet takes on the
/// endpoint's word. A Koios instance answering `min_fee_a = 10^9` builds a
/// transaction that is valid, balanced and signed, and hands almost the whole
/// balance to a stake pool; an EVM node quoting an absurd `eth_gasPrice` does
/// the same through the gas cost. Neither is caught by the balance check —
/// that only asks whether the account *can* pay.
///
/// So the wallet keeps its own idea of what a fee may be, and a signature is
/// never produced for one above it. `--max-fee` moves the line for a caller
/// who means it.
pub fn check_fee(network: &Network, ceiling: u128, fee: u128, fee_units: Amount) -> Result<()> {
    if fee <= ceiling {
        return Ok(());
    }
    Err(error::invalid_amount(format!(
        "{} quoted a fee of {}, above the {} this wallet will sign for. \
         That is the endpoint's number, not yours — check where the wallet is \
         pointed before overriding the ceiling with --max-fee",
        network.name,
        fee_units.format_with_symbol(fee),
        fee_units.format_with_symbol(ceiling),
    )))
}

/// A transfer that has passed every check the chain can make offline *and*
/// against the node, signed and ready to broadcast — but not broadcast.
///
/// Holding this is the point at which the user is asked to confirm. Dropping
/// it costs nothing: nothing has left the machine.
///
/// The `{:?}` is safe to log: a prepared transfer holds signed bytes and
/// public facts, never the key that signed them.
#[derive(Debug)]
pub struct PreparedTransfer {
    /// The signed bytes, in the form [`ChainClient::submit`] expects them.
    pub signed: Vec<u8>,
    /// The transaction's identity, computed locally so it is known even if
    /// the broadcast's reply is lost.
    pub id: String,
    pub from: String,
    pub to: String,
    pub amount: u128,
    /// The fee this will cost, in the chain's base units, as well as it can
    /// be known before the fact.
    pub fee: u128,
    /// The price of one unit of work, where the chain quotes fees that way.
    ///
    /// Kept apart from `fee` because they are different numbers: on EVM this
    /// is the gas price and `fee` is `gas_price × gas_limit`. Recording the
    /// total under the price's name is a silent factor-of-21000 error.
    pub fee_rate: Option<u128>,
    /// The unit `fee` is counted in, when it is not the token being sent.
    ///
    /// Midnight is why this exists: a transfer moves NIGHT (6 decimals) and
    /// pays its fee in DUST (15). Formatting the fee with the transfer's unit
    /// turns 0.82 DUST into "821830000 NIGHT" — a number alarming enough to
    /// stop a send that was perfectly fine.
    pub fee_unit: Option<Amount>,
    /// The sequence number consumed, where the chain has one.
    pub nonce: Option<u64>,
    /// The gas limit signed into the transaction, where the chain has one.
    pub gas_limit: Option<u64>,
    /// The network this is a transfer on, which the question names.
    pub network: Network,
    /// A clause only this chain has to add to the question.
    pub note: Option<String>,
    /// Chain-specific detail worth showing in a dry run.
    pub detail: Value,
}

impl PreparedTransfer {
    /// How this transfer's fee is counted, which is not always how the
    /// transfer itself is.
    pub fn fee_units(&self) -> Amount {
        self.fee_unit.unwrap_or_else(|| self.network.units())
    }

    /// The question to put to the user before [`ChainClient::submit`].
    ///
    /// Built here rather than by each chain, because a chain that writes its
    /// own sentence is a chain that can leave the fee out of it — and all four
    /// did. The fee is the number that decides whether a send is reasonable,
    /// so it belongs in the sentence the user says yes to, on every front end,
    /// including the ones that never draw a screen.
    pub fn prompt(&self) -> String {
        format!(
            "Send {} from {} to {} on {}, paying a fee of {}{}",
            self.network.units().format_with_symbol(self.amount),
            self.from,
            self.to,
            self.network.name,
            self.fee_units().format_with_symbol(self.fee),
            self.note.as_deref().unwrap_or(""),
        )
    }
}

/// What a chain reports back once a transfer is on its way.
#[derive(Debug, Clone)]
pub struct TransferReceipt {
    pub id: String,
    /// A second identifier where the chain has one — Midnight's extrinsic
    /// hash is not its transaction hash.
    pub secondary_id: Option<String>,
}

/// Where a transaction has got to, as the chain sees it.
#[derive(Debug, Clone)]
pub struct TransactionStatus {
    pub id: String,
    /// One of `pending`, `confirmed`, `failed`.
    pub status: String,
    pub block: Option<u64>,
    pub fee: Option<u128>,
    /// Units of work the transaction actually consumed, where the chain
    /// reports one — EVM's `gasUsed`.
    pub gas_used: Option<u64>,
    pub raw: Value,
}

/// A balance, in base units, with the per-token breakdown a chain may have.
#[derive(Debug, Clone, Default)]
pub struct Balance {
    /// The native token, in base units.
    pub native: u128,
    /// Everything else the address holds, keyed by the chain's token id.
    pub tokens: std::collections::BTreeMap<String, u128>,
}

impl Balance {
    pub fn native(amount: u128) -> Self {
        Balance {
            native: amount,
            tokens: Default::default(),
        }
    }
}

/// Everything a [`ChainClient`] needs from the wallet around it.
///
/// One struct rather than a widening argument list, because the things a chain
/// needs from its host are not the same from chain to chain and the set is
/// still growing. Midnight is the reason all four fields exist: it reads from
/// an indexer and submits to a different node, it caches a replayed dust state
/// that costs minutes to rebuild, and it does work slow enough that saying
/// nothing would look like a hang.
#[derive(Clone)]
pub struct ClientConfig {
    pub network: Network,
    /// Where balances and history are read from.
    pub endpoint: String,
    /// Where signed transactions go. The same as `endpoint` for most chains.
    pub submit_endpoint: String,
    /// A directory a chain may cache expensive-to-rebuild state in.
    ///
    /// It lives under the wallet home, so it travels with `--home` and a
    /// throwaway home in a test stays throwaway.
    pub cache_dir: PathBuf,
    /// Where to report slow work. Wired to [`Host::progress`].
    ///
    /// [`Host::progress`]: crate::host::Host::progress
    pub progress: Arc<dyn Fn(&str) + Send + Sync>,
}

impl ClientConfig {
    /// A config that caches nowhere and reports progress to no one.
    ///
    /// For tests and for read-only calls that cannot reach the slow paths.
    pub fn bare(network: Network) -> Self {
        ClientConfig {
            endpoint: network.default_endpoint.to_string(),
            submit_endpoint: network
                .default_submit_endpoint
                .unwrap_or(network.default_endpoint)
                .to_string(),
            cache_dir: std::env::temp_dir().join("causewaybay-cache"),
            progress: Arc::new(|_| {}),
            network,
        }
    }

    /// Report that something slow is still going.
    pub fn report(&self, message: impl AsRef<str>) {
        (self.progress)(message.as_ref());
    }
}

// ==================================================================== the traits

/// The offline half of a chain: keys, addresses, amounts, signatures.
pub trait Chain: Send + Sync + 'static {
    fn id(&self) -> ChainId;

    /// Human name, for a heading.
    fn name(&self) -> &'static str;

    /// The networks this chain ships. The first is its default.
    fn networks(&self) -> &'static [Network];

    /// Base units per whole token, as a decimal exponent, and the ticker.
    fn units(&self, network: &Network) -> Amount;

    /// The unit this chain's fees are counted in.
    ///
    /// The same as [`Self::units`] for three chains out of four. Midnight
    /// moves NIGHT and pays in DUST, and `--max-fee` has to be read in the
    /// unit the ceiling is actually applied in.
    fn fee_units(&self, network: &Network) -> Amount {
        self.units(network)
    }

    /// The derivation path this chain uses at `index`, for display.
    fn derivation_path(&self, index: u32) -> String;

    /// Derive the account at `index` from a mnemonic.
    ///
    /// Chains disagree about what a mnemonic *is*: three of the four hash the
    /// BIP-39 seed, Cardano hashes the entropy. [`Seed`] carries both, so this
    /// signature does not have to pick a side.
    fn derive(&self, seed: &Seed, index: u32) -> Result<DerivedAccount>;

    /// Rebuild an account from a stored secret, in this chain's encoding.
    fn account_from_secret(&self, secret: &str) -> Result<DerivedAccount>;

    /// A signer over a stored secret.
    fn signer(&self, secret: &str) -> Result<Box<dyn Signer>>;

    /// The address a stored secret controls on `network`, when the chain
    /// renders a different one per network.
    ///
    /// `None` — the default — means "the same address everywhere", and the
    /// caller should keep the address it already has rather than re-deriving
    /// one. Cardano and Midnight put the network *inside* the address, a
    /// header nibble and a bech32m prefix, so a wallet's mainnet address is
    /// not its testnet address and they answer with `Some`.
    fn address_on(&self, network: &Network, secret: &str) -> Result<Option<String>> {
        let _ = (network, secret);
        Ok(None)
    }

    /// Check an address is well formed *and* belongs to `network`.
    ///
    /// The second half is not pedantry: a Cardano mainnet address and a
    /// testnet one differ by one nibble, and a Midnight address carries its
    /// network in the bech32m prefix. Sending across that line burns the funds.
    fn check_address(&self, network: &Network, address: &str) -> Result<()>;

    /// Verify a message signature.
    ///
    /// `identity` says whose signature this should be, and the chains need
    /// different things from it — which is why it is one loosely-typed
    /// parameter rather than four incompatible methods:
    ///
    /// * EVM recovers the signer from the signature itself, so an identity is
    ///   optional and, when given, is an address to compare against.
    /// * Solana can check against a bare address, because the address *is* the
    ///   public key.
    /// * Cardano and Midnight hash the public key into the address, so no
    ///   address can verify anything; they need key material.
    ///
    /// Every chain therefore accepts the account's stored secret, and the two
    /// that can also accept an address say so. A chain that is handed
    /// something it cannot use must say what it needed rather than reporting
    /// the signature invalid — a wrong answer is worse than a refusal here.
    fn recover_message(
        &self,
        message: &[u8],
        signature: &[u8],
        identity: Option<&str>,
    ) -> Result<Recovered>;

    /// A client for one network.
    fn client(&self, config: &ClientConfig) -> Result<Arc<dyn ChainClient>>;

    /// What this chain can do beyond the baseline.
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}

/// The result of checking a signature.
#[derive(Debug)]
pub struct Recovered {
    /// The address the signature belongs to, when the scheme can recover it.
    pub address: Option<String>,
    /// Whether it matched the address the caller expected, if one was given.
    pub valid: bool,
}

/// The online half of a chain, for one network.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// The endpoint this client talks to, for `info` and error messages.
    fn endpoint(&self) -> &str;

    /// The address's balance.
    async fn balance(&self, address: &str) -> Result<Balance>;

    /// Resolve, check and sign a transfer without broadcasting it.
    async fn prepare_transfer(
        &self,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer>;

    /// Broadcast a transfer that [`Self::prepare_transfer`] produced.
    async fn submit(&self, prepared: &PreparedTransfer) -> Result<TransferReceipt>;

    /// Look a transaction up. `Ok(None)` means the chain has never heard of it.
    async fn transaction(&self, id: &str) -> Result<Option<TransactionStatus>>;

    /// The next sequence number for an address, where the chain has one.
    async fn nonce(&self, _address: &str) -> Result<Option<u64>> {
        Ok(None)
    }

    /// What one unit of fee costs right now, where the chain quotes one.
    async fn fee_quote(&self) -> Result<Option<u128>> {
        Ok(None)
    }

    /// A short description of the chain's head: height, id, whatever it has.
    async fn chain_info(&self) -> Result<Value>;

    /// Ask a test network for funds.
    async fn faucet(&self, _address: &str, _amount: u128) -> Result<String> {
        Err(error::usage("this chain has no faucet the wallet can call"))
    }
}

/// Optional behaviour, so a front end can grey out what a chain cannot do
/// rather than offering it and failing.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Capabilities {
    /// `faucet` works on at least one of this chain's networks.
    pub faucet: bool,
    /// The chain has fungible tokens this wallet can read and move.
    pub tokens: bool,
    /// Transfers carry an explicit gas limit the user may override.
    pub gas_limit: bool,
    /// A message signature recovers the signer's address without being told it.
    pub recoverable_signatures: bool,
}

// ==================================================================== registry

/// Every chain, in the order a front end should present them.
pub fn registry() -> &'static [&'static dyn Chain] {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Vec<&'static dyn Chain>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            &evm::EvmChain as &'static dyn Chain,
            &solana::SolanaChain,
            &cardano::CardanoChain,
            &midnight::MidnightChain,
        ]
    })
}

/// The chain behind an id. Total: every [`ChainId`] is in the registry, and
/// the test below is what keeps that true.
pub fn chain(id: ChainId) -> &'static dyn Chain {
    registry()
        .iter()
        .copied()
        .find(|c| c.id() == id)
        .expect("every ChainId is registered; the registry test enforces it")
}

/// The chain a name refers to.
pub fn find(name: &str) -> Result<&'static dyn Chain> {
    Ok(chain(ChainId::parse(name)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chain_id_is_registered_exactly_once() {
        for id in ChainId::ALL {
            let found: Vec<_> = registry().iter().filter(|c| c.id() == id).collect();
            assert_eq!(found.len(), 1, "{id} should appear once in the registry");
        }
        assert_eq!(registry().len(), ChainId::ALL.len());
    }

    #[test]
    fn chain_names_round_trip_through_their_string_form() {
        for id in ChainId::ALL {
            assert_eq!(ChainId::parse(id.as_str()).unwrap(), id);
        }
    }

    #[test]
    fn chain_aliases_resolve_the_way_people_type_them() {
        for alias in ["cronos", "EVM", " ethereum ", "eth"] {
            assert_eq!(ChainId::parse(alias).unwrap(), ChainId::Evm, "{alias}");
        }
        assert_eq!(ChainId::parse("SOL").unwrap(), ChainId::Solana);
        assert_eq!(ChainId::parse("ada").unwrap(), ChainId::Cardano);
        assert_eq!(ChainId::parse("night").unwrap(), ChainId::Midnight);
    }

    #[test]
    fn an_unknown_chain_lists_the_known_ones() {
        let err = ChainId::parse("dogecoin").unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("solana"), "{}", err.message);
        assert!(err.message.contains("midnight"), "{}", err.message);
    }

    #[test]
    fn an_absent_chain_field_means_evm() {
        // Every account written before this wallet was multi-chain.
        assert_eq!(ChainId::default(), ChainId::Evm);
        assert_eq!(
            serde_json::from_str::<ChainId>(r#""solana""#).unwrap(),
            ChainId::Solana
        );
    }

    /// A fee is not always counted in the token being sent, and the two
    /// chains that differ must say so — formatting a DUST fee with NIGHT's
    /// six decimals renders 0.82 DUST as "821830000 NIGHT".
    #[test]
    fn a_chain_whose_fee_token_differs_declares_its_fee_unit() {
        use crate::network;

        // Midnight moves NIGHT and pays in DUST, so its fee carries a unit.
        let midnight = network::MIDNIGHT_PREVIEW.units();
        assert_eq!(midnight.symbol, "NIGHT");
        assert_eq!(midnight.decimals, 6);

        let dust = Amount::new(15, "DUST");
        assert_ne!(dust.decimals, midnight.decimals);
        // The number that made the bug obvious, rendered both ways.
        let fee_specks = 821_830_000_000_001u128;
        assert_eq!(dust.format(fee_specks), "0.821830000000001");
        assert_eq!(midnight.format(fee_specks), "821830000.000001");
    }

    #[test]
    fn every_chain_ships_at_least_one_network_and_names_itself() {
        for c in registry() {
            assert!(!c.networks().is_empty(), "{} has no networks", c.id());
            assert!(!c.name().is_empty());
            // Every network a chain lists must claim that chain.
            for network in c.networks() {
                assert_eq!(network.chain, c.id(), "{} lists {}", c.id(), network.key);
            }
        }
    }
}
