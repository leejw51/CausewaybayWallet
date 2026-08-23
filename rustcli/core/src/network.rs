//! The supported Cronos EVM networks and RPC endpoint resolution.

use serde::Serialize;

use crate::error::{self, Result};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Network {
    pub key: &'static str,
    pub name: &'static str,
    pub chain_id: u64,
    pub symbol: &'static str,
    pub default_rpc: &'static str,
    pub explorer: &'static str,
    pub testnet: bool,
}

pub const CRONOS_TESTNET: Network = Network {
    key: "cronos-testnet",
    name: "Cronos EVM Testnet",
    chain_id: 338,
    symbol: "TCRO",
    default_rpc: "https://evm-t3.cronos.org",
    explorer: "https://explorer.cronos.org/testnet",
    testnet: true,
};

pub const CRONOS_MAINNET: Network = Network {
    key: "cronos-mainnet",
    name: "Cronos EVM Mainnet",
    chain_id: 25,
    symbol: "CRO",
    default_rpc: "https://evm.cronos.org",
    explorer: "https://explorer.cronos.org",
    testnet: false,
};

pub const ALL: [Network; 2] = [CRONOS_TESTNET, CRONOS_MAINNET];

pub const DEFAULT_NETWORK: &str = CRONOS_TESTNET.key;

/// Look a network up by key, accepting a few friendly aliases.
pub fn find(key: &str) -> Result<Network> {
    let normalized = key.trim().to_lowercase().replace('_', "-");
    let canonical = match normalized.as_str() {
        "testnet" | "cronos-testnet" | "cronostestnet" | "t3" => "cronos-testnet",
        "mainnet" | "cronos-mainnet" | "cronosmainnet" | "cronos" => "cronos-mainnet",
        other => other,
    };
    ALL.iter()
        .find(|n| n.key == canonical)
        .copied()
        .ok_or_else(|| {
            let known = ALL.iter().map(|n| n.key).collect::<Vec<_>>().join(", ");
            error::unknown_network(format!("unknown network '{key}'; known networks: {known}"))
        })
}

impl Network {
    /// The environment variable that overrides this network's RPC URL.
    pub fn rpc_env_var(&self) -> String {
        format!(
            "CAUSEWAYBAY_RPC_{}",
            self.key.to_uppercase().replace('-', "_")
        )
    }

    /// The `config.jsonl` key that overrides this network's RPC URL.
    pub fn rpc_config_key(&self) -> String {
        format!("rpc.{}", self.key)
    }

    /// Environment variable beats stored config, which beats the built-in default.
    pub fn resolve_rpc(&self, configured: Option<&str>) -> String {
        if let Ok(url) = std::env::var(self.rpc_env_var()) {
            if !url.trim().is_empty() {
                return url.trim().to_string();
            }
        }
        match configured {
            Some(url) if !url.trim().is_empty() => url.trim().to_string(),
            _ => self.default_rpc.to_string(),
        }
    }

    /// Explorer link for a transaction hash.
    pub fn tx_url(&self, hash: &str) -> String {
        format!("{}/tx/{}", self.explorer, hash)
    }

    /// Explorer link for an address.
    pub fn address_url(&self, address: &str) -> String {
        format!("{}/address/{}", self.explorer, address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_both_networks_by_canonical_key() {
        assert_eq!(find("cronos-testnet").unwrap().chain_id, 338);
        assert_eq!(find("cronos-mainnet").unwrap().chain_id, 25);
    }

    #[test]
    fn accepts_aliases_and_odd_casing() {
        for alias in ["testnet", "TESTNET", " cronos_testnet ", "t3"] {
            assert_eq!(find(alias).unwrap().key, "cronos-testnet", "alias {alias}");
        }
        for alias in ["mainnet", "Cronos", "cronos_mainnet"] {
            assert_eq!(find(alias).unwrap().key, "cronos-mainnet", "alias {alias}");
        }
    }

    #[test]
    fn rejects_unknown_networks_with_a_helpful_message() {
        let err = find("ethereum").unwrap_err();
        assert_eq!(err.code, error::Code::UnknownNetwork);
        assert!(err.message.contains("cronos-testnet"));
    }

    #[test]
    fn only_testnet_is_flagged_as_testnet() {
        // Read through `find` so this exercises the lookup as well as the table.
        assert!(find("cronos-testnet").unwrap().testnet);
        assert!(!find("cronos-mainnet").unwrap().testnet);
        assert_eq!(
            ALL.iter().filter(|n| n.testnet).count(),
            1,
            "exactly one testnet in the table"
        );
    }

    #[test]
    fn rpc_override_names_are_stable() {
        assert_eq!(
            CRONOS_TESTNET.rpc_env_var(),
            "CAUSEWAYBAY_RPC_CRONOS_TESTNET"
        );
        assert_eq!(
            CRONOS_MAINNET.rpc_env_var(),
            "CAUSEWAYBAY_RPC_CRONOS_MAINNET"
        );
        assert_eq!(CRONOS_TESTNET.rpc_config_key(), "rpc.cronos-testnet");
    }

    #[test]
    fn rpc_resolution_prefers_configured_over_default() {
        assert_eq!(CRONOS_TESTNET.resolve_rpc(None), CRONOS_TESTNET.default_rpc);
        assert_eq!(
            CRONOS_TESTNET.resolve_rpc(Some("   ")),
            CRONOS_TESTNET.default_rpc
        );
        assert_eq!(
            CRONOS_TESTNET.resolve_rpc(Some("http://localhost:8545")),
            "http://localhost:8545"
        );
    }

    #[test]
    fn explorer_links_are_well_formed() {
        assert_eq!(
            CRONOS_MAINNET.tx_url("0xabc"),
            "https://explorer.cronos.org/tx/0xabc"
        );
        assert_eq!(
            CRONOS_TESTNET.address_url("0xdef"),
            "https://explorer.cronos.org/testnet/address/0xdef"
        );
    }
}
