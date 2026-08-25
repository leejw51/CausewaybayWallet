//! The networks every chain ships, and how an endpoint is resolved for one.
//!
//! One flat table across all four chains, because that is what the front ends
//! want: the TUI builds a menu entry per row, `network list` prints the rows,
//! and picking a row settles both the chain and the endpoint at once. A
//! network knows which chain it belongs to, so selecting `solana-devnet` is
//! also how you select Solana — there is no second, separate act of choosing.
//!
//! Rows are grouped by chain and each chain's first row is its default.

use serde::Serialize;

use crate::chain::ChainId;
use crate::error::{self, Result};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Network {
    /// The stable key: `<chain>-<network>`, except the two Cronos rows, whose
    /// keys predate the other chains and are kept as they were.
    pub key: &'static str,
    pub chain: ChainId,
    pub name: &'static str,
    /// The ticker of the native token.
    pub symbol: &'static str,
    /// Decimal places the native token is quoted with.
    pub decimals: u8,
    /// The EIP-155 chain id. Only EVM networks have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    /// The node, indexer or API this network is read and written through.
    pub default_endpoint: &'static str,
    /// A second endpoint, where reading and writing are different services.
    /// Midnight reads from an indexer and submits to a node RPC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_submit_endpoint: Option<&'static str>,
    pub explorer: &'static str,
    pub testnet: bool,
    /// The most this wallet will sign for in fees on this network, in the base
    /// units of whatever token pays the fee.
    ///
    /// A fee is one of the few numbers a wallet takes on an endpoint's word —
    /// `eth_gasPrice` on EVM, `min_fee_a`/`min_fee_b` from Koios on Cardano —
    /// and nothing downstream questions it: the transaction balances, the
    /// signature is valid, and the user is asked a question that used to name
    /// only the amount. So the ceiling lives here, in the wallet's own table,
    /// where no endpoint can reach it.
    ///
    /// Each is set far above what a real transfer on that network costs and
    /// far below anything worth losing — two orders of magnitude of headroom,
    /// which is room for a fee market to move and no room for a hostile
    /// endpoint to drain an account. `--max-fee` overrides it per send.
    pub max_fee: u128,
}

// ------------------------------------------------------------------ the table

pub const CRONOS_TESTNET: Network = Network {
    key: "cronos-testnet",
    chain: ChainId::Evm,
    name: "Cronos EVM Testnet",
    symbol: "TCRO",
    decimals: 18,
    chain_id: Some(338),
    default_endpoint: "https://evm-t3.cronos.org",
    default_submit_endpoint: None,
    explorer: "https://explorer.cronos.org/testnet",
    testnet: true,
    // A plain transfer at Cronos's 5000 gwei costs about 0.105 CRO; this is
    // room for a 5,000,000-gas contract call at that price.
    max_fee: 25_000_000_000_000_000_000,
};

pub const CRONOS_MAINNET: Network = Network {
    key: "cronos-mainnet",
    chain: ChainId::Evm,
    name: "Cronos EVM Mainnet",
    symbol: "CRO",
    decimals: 18,
    chain_id: Some(25),
    default_endpoint: "https://evm.cronos.org",
    default_submit_endpoint: None,
    explorer: "https://explorer.cronos.org",
    testnet: false,
    max_fee: 25_000_000_000_000_000_000,
};

pub const SOLANA_DEVNET: Network = Network {
    key: "solana-devnet",
    chain: ChainId::Solana,
    name: "Solana Devnet",
    symbol: "SOL",
    decimals: 9,
    chain_id: None,
    default_endpoint: "https://api.devnet.solana.com",
    default_submit_endpoint: None,
    explorer: "https://explorer.solana.com/?cluster=devnet",
    testnet: true,
    // Solana charges 5000 lamports per signature, and this wallet signs once.
    max_fee: 10_000_000,
};

pub const SOLANA_TESTNET: Network = Network {
    key: "solana-testnet",
    chain: ChainId::Solana,
    name: "Solana Testnet",
    symbol: "SOL",
    decimals: 9,
    chain_id: None,
    default_endpoint: "https://api.testnet.solana.com",
    default_submit_endpoint: None,
    explorer: "https://explorer.solana.com/?cluster=testnet",
    testnet: true,
    max_fee: 10_000_000,
};

pub const SOLANA_MAINNET: Network = Network {
    key: "solana-mainnet",
    chain: ChainId::Solana,
    name: "Solana Mainnet Beta",
    symbol: "SOL",
    decimals: 9,
    chain_id: None,
    default_endpoint: "https://api.mainnet-beta.solana.com",
    default_submit_endpoint: None,
    explorer: "https://explorer.solana.com",
    testnet: false,
    max_fee: 10_000_000,
};

pub const CARDANO_PREPROD: Network = Network {
    key: "cardano-preprod",
    chain: ChainId::Cardano,
    name: "Cardano Preprod",
    symbol: "tADA",
    decimals: 6,
    chain_id: None,
    default_endpoint: "https://preprod.koios.rest/api/v1",
    default_submit_endpoint: None,
    explorer: "https://preprod.cardanoscan.io",
    testnet: true,
    // The protocol's own worst case is min_fee_a x 16384 + min_fee_b, about
    // 0.88 ADA at today's parameters; a real transfer costs about 0.17.
    max_fee: 5_000_000,
};

pub const CARDANO_PREVIEW: Network = Network {
    key: "cardano-preview",
    chain: ChainId::Cardano,
    name: "Cardano Preview",
    symbol: "tADA",
    decimals: 6,
    chain_id: None,
    default_endpoint: "https://preview.koios.rest/api/v1",
    default_submit_endpoint: None,
    explorer: "https://preview.cardanoscan.io",
    testnet: true,
    max_fee: 5_000_000,
};

pub const CARDANO_MAINNET: Network = Network {
    key: "cardano-mainnet",
    chain: ChainId::Cardano,
    name: "Cardano Mainnet",
    symbol: "ADA",
    decimals: 6,
    chain_id: None,
    default_endpoint: "https://api.koios.rest/api/v1",
    default_submit_endpoint: None,
    explorer: "https://cardanoscan.io",
    testnet: false,
    max_fee: 5_000_000,
};

pub const MIDNIGHT_PREVIEW: Network = Network {
    key: "midnight-preview",
    chain: ChainId::Midnight,
    name: "Midnight Preview",
    symbol: "NIGHT",
    decimals: 6,
    chain_id: None,
    default_endpoint: "https://indexer.preview.midnight.network/api/v4/graphql",
    default_submit_endpoint: Some("https://rpc.preview.midnight.network"),
    explorer: "https://preview.midnightexplorer.com",
    testnet: true,
    // Counted in DUST, not NIGHT: 100 DUST against a transfer's usual 0.82.
    max_fee: 100_000_000_000_000_000,
};

pub const MIDNIGHT_DEVNET: Network = Network {
    key: "midnight-devnet",
    chain: ChainId::Midnight,
    name: "Midnight Devnet",
    symbol: "NIGHT",
    decimals: 6,
    chain_id: None,
    default_endpoint: "https://indexer.devnet.midnight.network/api/v4/graphql",
    default_submit_endpoint: Some("https://rpc.devnet.midnight.network"),
    explorer: "https://devnet.midnightexplorer.com",
    testnet: true,
    max_fee: 100_000_000_000_000_000,
};

/// Every network, grouped by chain, each chain's default first.
pub const ALL: [Network; 10] = [
    CRONOS_TESTNET,
    CRONOS_MAINNET,
    SOLANA_DEVNET,
    SOLANA_TESTNET,
    SOLANA_MAINNET,
    CARDANO_PREPROD,
    CARDANO_PREVIEW,
    CARDANO_MAINNET,
    MIDNIGHT_PREVIEW,
    MIDNIGHT_DEVNET,
];

/// The network a wallet with no stored preference uses.
pub const DEFAULT_NETWORK: &str = CRONOS_TESTNET.key;

// ------------------------------------------------------------------- lookup

/// The networks belonging to one chain, in menu order.
pub fn for_chain(chain: ChainId) -> Vec<Network> {
    ALL.iter().filter(|n| n.chain == chain).copied().collect()
}

/// A chain's default network: the first row it lists.
pub fn default_for(chain: ChainId) -> Network {
    ALL.iter()
        .find(|n| n.chain == chain)
        .copied()
        .expect("every chain ships at least one network")
}

/// Look a network up by key, accepting a few friendly aliases.
///
/// Unqualified names are ambiguous now that four chains ship networks —
/// `devnet` is a Solana cluster *and* a Midnight network — so a bare name only
/// resolves when exactly one chain uses it. Otherwise the error names the
/// candidates rather than guessing, because guessing here sends funds to the
/// wrong chain.
pub fn find(key: &str) -> Result<Network> {
    let wanted = canonicalize(key);
    if let Some(network) = ALL.iter().find(|n| n.key == wanted) {
        return Ok(*network);
    }
    let matches: Vec<&Network> = ALL.iter().filter(|n| short_name(n.key) == wanted).collect();
    match matches.as_slice() {
        [only] => Ok(**only),
        [] => Err(error::unknown_network(format!(
            "unknown network '{key}'; known networks: {}",
            ALL.iter().map(|n| n.key).collect::<Vec<_>>().join(", ")
        ))),
        several => Err(error::unknown_network(format!(
            "'{key}' is ambiguous across chains: {}. Name the network in full, \
             or pass --chain to say which one you mean",
            several.iter().map(|n| n.key).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// Look a network up within one chain, where a short name is unambiguous.
///
/// This is what `--chain solana -n devnet` goes through: the chain has already
/// been settled, so `devnet` can only mean one thing.
pub fn find_for(chain: ChainId, key: &str) -> Result<Network> {
    // Deliberately *not* `canonicalize`: those aliases exist to keep bare
    // `testnet`/`mainnet` meaning Cronos for the chain-less lookup, and
    // applying them here would stop `--chain solana -n testnet` resolving.
    // Inside a chain a short name is already unambiguous.
    let wanted = normalize(key);
    let candidates = for_chain(chain);
    candidates
        .iter()
        .find(|n| {
            n.key == wanted
                || short_name(n.key) == wanted
                // The one spelling Solana's own tooling uses.
                || (short_name(n.key) == "mainnet" && wanted == "mainnet-beta")
        })
        .copied()
        .ok_or_else(|| {
            error::unknown_network(format!(
                "unknown {chain} network '{key}'; known {chain} networks: {}",
                candidates
                    .iter()
                    .map(|n| n.key)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// Lowercase and settle on hyphens, without changing what was meant.
fn normalize(key: &str) -> String {
    key.trim().to_lowercase().replace('_', "-")
}

/// Fold the spellings people actually type into the canonical key.
fn canonicalize(key: &str) -> String {
    let normalized = normalize(key);
    match normalized.as_str() {
        // `testnet` and `mainnet` meant Cronos before this wallet had other
        // chains, and stores and scripts in the wild are full of them. They
        // keep meaning Cronos rather than becoming ambiguous — a rename that
        // breaks every existing config is not a nicety worth having.
        "cronos" | "cronosmainnet" | "mainnet" => "cronos-mainnet".into(),
        "cronostestnet" | "t3" | "testnet" => "cronos-testnet".into(),
        "mainnet-beta" | "solana-mainnet-beta" => "solana-mainnet".into(),
        other => other.into(),
    }
}

/// The part of a key after the chain prefix: `solana-devnet` → `devnet`.
fn short_name(key: &str) -> &str {
    key.split_once('-').map(|(_, rest)| rest).unwrap_or(key)
}

impl Network {
    /// The environment variable that overrides this network's endpoint.
    pub fn endpoint_env_var(&self) -> String {
        format!(
            "CAUSEWAYBAY_RPC_{}",
            self.key.to_uppercase().replace('-', "_")
        )
    }

    /// The `config.jsonl` key that overrides this network's endpoint.
    pub fn endpoint_config_key(&self) -> String {
        format!("rpc.{}", self.key)
    }

    /// Environment beats stored config, which beats the built-in default.
    pub fn resolve_endpoint(&self, configured: Option<&str>) -> String {
        if let Ok(url) = std::env::var(self.endpoint_env_var()) {
            if !url.trim().is_empty() {
                return url.trim().to_string();
            }
        }
        match configured {
            Some(url) if !url.trim().is_empty() => url.trim().to_string(),
            _ => self.default_endpoint.to_string(),
        }
    }

    /// The environment variable overriding where transactions are submitted.
    pub fn submit_env_var(&self) -> String {
        format!(
            "CAUSEWAYBAY_SUBMIT_{}",
            self.key.to_uppercase().replace('-', "_")
        )
    }

    /// Where a signed transaction goes, when that is not where reads come from.
    ///
    /// Same precedence as reads — environment, then config, then the built-in
    /// default — so the two halves of a Midnight endpoint are overridden the
    /// same way rather than one of them being a special case.
    pub fn resolve_submit_endpoint(&self, configured: Option<&str>) -> String {
        let Some(default) = self.default_submit_endpoint else {
            // A chain that reads and writes in one place follows the read
            // endpoint, override and all.
            return self.resolve_endpoint(configured);
        };
        if let Ok(url) = std::env::var(self.submit_env_var()) {
            if !url.trim().is_empty() {
                return url.trim().to_string();
            }
        }
        match configured {
            Some(url) if !url.trim().is_empty() => url.trim().to_string(),
            _ => default.to_string(),
        }
    }

    /// The `config.jsonl` key overriding where transactions are submitted.
    pub fn submit_config_key(&self) -> String {
        format!("submit.{}", self.key)
    }

    /// How this network's native token is counted and named.
    pub fn units(&self) -> crate::chain::Amount {
        crate::chain::Amount::new(self.decimals, self.symbol)
    }

    /// Explorer link for a transaction.
    ///
    /// Solana's explorer carries its cluster in a query string, so the link is
    /// built by inserting the path before it rather than appending.
    pub fn tx_url(&self, id: &str) -> String {
        self.explorer_url("tx", id)
    }

    /// Explorer link for an address.
    pub fn address_url(&self, address: &str) -> String {
        let kind = match self.chain {
            ChainId::Solana => "address",
            _ => "address",
        };
        self.explorer_url(kind, address)
    }

    fn explorer_url(&self, kind: &str, value: &str) -> String {
        match self.explorer.split_once('?') {
            Some((base, query)) => {
                format!("{}/{kind}/{value}?{query}", base.trim_end_matches('/'))
            }
            None => format!("{}/{kind}/{value}", self.explorer.trim_end_matches('/')),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chain_has_at_least_one_network_and_a_default() {
        for chain in ChainId::ALL {
            let rows = for_chain(chain);
            assert!(!rows.is_empty(), "{chain} ships no networks");
            assert_eq!(default_for(chain).key, rows[0].key);
        }
        assert_eq!(
            ALL.len(),
            ChainId::ALL
                .iter()
                .map(|c| for_chain(*c).len())
                .sum::<usize>()
        );
    }

    #[test]
    fn network_keys_are_unique() {
        let mut keys: Vec<&str> = ALL.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate network key in the table");
    }

    #[test]
    fn only_evm_networks_carry_an_eip155_chain_id() {
        for n in ALL {
            assert_eq!(
                n.chain_id.is_some(),
                n.chain == ChainId::Evm,
                "{} has the wrong chain_id shape",
                n.key
            );
        }
    }

    #[test]
    fn the_cronos_keys_and_aliases_survived_the_move_to_many_chains() {
        // Every spelling that reached a Cronos network before must still do so:
        // stores and scripts in the wild are full of them.
        for alias in ["cronos-testnet", "TESTNET", " cronos_testnet ", "t3"] {
            assert_eq!(find(alias).unwrap().key, "cronos-testnet", "alias {alias}");
        }
        for alias in ["cronos-mainnet", "Cronos", "cronos_mainnet"] {
            assert_eq!(find(alias).unwrap().key, "cronos-mainnet", "alias {alias}");
        }
        assert_eq!(find("cronos-testnet").unwrap().chain_id, Some(338));
        assert_eq!(find("cronos-mainnet").unwrap().chain_id, Some(25));
    }

    #[test]
    fn a_short_name_used_by_one_chain_resolves() {
        assert_eq!(find("preprod").unwrap().key, "cardano-preprod");
        assert_eq!(find("mainnet-beta").unwrap().key, "solana-mainnet");
    }

    #[test]
    fn a_short_name_used_by_two_chains_is_refused_rather_than_guessed() {
        // `devnet` is Solana's and Midnight's, and neither predates the other
        // here. Picking one would send funds to whichever sorted first.
        let err = find("devnet").unwrap_err();
        assert_eq!(err.code, error::Code::UnknownNetwork);
        assert!(err.message.contains("solana-devnet"), "{}", err.message);
        assert!(err.message.contains("midnight-devnet"), "{}", err.message);
        assert!(err.message.contains("--chain"), "{}", err.message);
    }

    /// `testnet` and `mainnet` are shared by three chains apiece, but they
    /// meant Cronos before this wallet had any others — so they still do.
    /// Making them ambiguous would break every existing config and script.
    #[test]
    fn the_bare_legacy_names_still_mean_cronos() {
        assert_eq!(find("testnet").unwrap().key, "cronos-testnet");
        assert_eq!(find("mainnet").unwrap().key, "cronos-mainnet");
        // Naming the chain still reaches the others' networks of that name.
        assert_eq!(
            find_for(ChainId::Solana, "testnet").unwrap().key,
            "solana-testnet"
        );
        assert_eq!(
            find_for(ChainId::Cardano, "mainnet").unwrap().key,
            "cardano-mainnet"
        );
    }

    #[test]
    fn naming_the_chain_disambiguates_a_short_name() {
        assert_eq!(
            find_for(ChainId::Solana, "devnet").unwrap().key,
            "solana-devnet"
        );
        assert_eq!(
            find_for(ChainId::Midnight, "devnet").unwrap().key,
            "midnight-devnet"
        );
        assert_eq!(
            find_for(ChainId::Evm, "mainnet").unwrap().key,
            "cronos-mainnet"
        );
    }

    #[test]
    fn a_network_from_the_wrong_chain_is_refused() {
        let err = find_for(ChainId::Solana, "cardano-preprod").unwrap_err();
        assert_eq!(err.code, error::Code::UnknownNetwork);
        assert!(err.message.contains("solana-devnet"), "{}", err.message);
    }

    #[test]
    fn unknown_networks_list_what_is_available() {
        let err = find("ethereum-sepolia").unwrap_err();
        assert!(err.message.contains("cronos-testnet"));
        assert!(err.message.contains("midnight-preview"));
    }

    #[test]
    fn endpoint_override_names_are_stable_and_per_network() {
        assert_eq!(
            CRONOS_TESTNET.endpoint_env_var(),
            "CAUSEWAYBAY_RPC_CRONOS_TESTNET"
        );
        assert_eq!(
            SOLANA_DEVNET.endpoint_env_var(),
            "CAUSEWAYBAY_RPC_SOLANA_DEVNET"
        );
        assert_eq!(CRONOS_TESTNET.endpoint_config_key(), "rpc.cronos-testnet");
        assert_eq!(
            MIDNIGHT_PREVIEW.submit_config_key(),
            "submit.midnight-preview"
        );
    }

    #[test]
    fn endpoint_resolution_prefers_configured_over_default() {
        assert_eq!(
            CRONOS_TESTNET.resolve_endpoint(None),
            CRONOS_TESTNET.default_endpoint
        );
        assert_eq!(
            CRONOS_TESTNET.resolve_endpoint(Some("   ")),
            CRONOS_TESTNET.default_endpoint
        );
        assert_eq!(
            CRONOS_TESTNET.resolve_endpoint(Some("http://localhost:8545")),
            "http://localhost:8545"
        );
    }

    #[test]
    fn a_chain_that_submits_elsewhere_keeps_the_two_endpoints_apart() {
        // Midnight reads from an indexer and submits to a node; conflating them
        // sends a signed transaction to something that cannot accept it.
        assert_ne!(
            MIDNIGHT_PREVIEW.resolve_endpoint(None),
            MIDNIGHT_PREVIEW.resolve_submit_endpoint(None)
        );
        assert!(MIDNIGHT_PREVIEW
            .resolve_submit_endpoint(None)
            .contains("rpc."));
        // A chain with one endpoint answers the same for both.
        assert_eq!(
            CRONOS_TESTNET.resolve_endpoint(None),
            CRONOS_TESTNET.resolve_submit_endpoint(None)
        );
    }

    #[test]
    fn explorer_links_are_well_formed_including_the_query_string_kind() {
        assert_eq!(
            CRONOS_MAINNET.tx_url("0xabc"),
            "https://explorer.cronos.org/tx/0xabc"
        );
        assert_eq!(
            CRONOS_TESTNET.address_url("0xdef"),
            "https://explorer.cronos.org/testnet/address/0xdef"
        );
        // Solana's cluster lives in a query string, so the path goes before it.
        assert_eq!(
            SOLANA_DEVNET.tx_url("5xy"),
            "https://explorer.solana.com/tx/5xy?cluster=devnet"
        );
        assert_eq!(
            SOLANA_MAINNET.tx_url("5xy"),
            "https://explorer.solana.com/tx/5xy"
        );
    }

    #[test]
    fn units_come_from_the_network_row() {
        assert_eq!(CRONOS_TESTNET.units().symbol, "TCRO");
        assert_eq!(CRONOS_TESTNET.units().decimals, 18);
        assert_eq!(SOLANA_DEVNET.units().decimals, 9);
        assert_eq!(CARDANO_PREPROD.units().decimals, 6);
        assert_eq!(MIDNIGHT_PREVIEW.units().symbol, "NIGHT");
    }

    #[test]
    fn testnet_flags_match_the_names() {
        assert!(find("cronos-testnet").unwrap().testnet);
        assert!(!find("cronos-mainnet").unwrap().testnet);
        assert!(!find("solana-mainnet").unwrap().testnet);
        assert!(!find("cardano-mainnet").unwrap().testnet);
        assert!(find("midnight-preview").unwrap().testnet);
    }
}
