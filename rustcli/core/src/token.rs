//! The tokens this wallet knows by name, one flat row per token per network.
//!
//! # Why it is flat
//!
//! A token is not a thing you hold; a token *on a network* is. USDC on Cronos
//! and USDC on Solana share a name, a peg and nothing else — different issuer,
//! different decimals, different bytes on the wire, and an address that means
//! nothing on the other chain. Nesting them under one "USDC" would invite the
//! one mistake this table exists to prevent: paying a Cronos address with the
//! Solana row selected.
//!
//! So the row is the pair, and it is named as the pair: **`USDC Cronos
//! Mainnet`**, key `usdc-cronos-mainnet`. That is the same shape
//! [`crate::network`] settled on, for the same reason — picking a row settles
//! everything, and there is never a second act of choosing. It is also why the
//! table is searched rather than navigated: `usdc cronos` is not a path
//! through two menus, it is the row's own name typed from memory.
//!
//! # What a row promises
//!
//! Every address below was read off the chain it names, not off a list: the
//! EVM contracts answered `symbol()` and `decimals()` on Cronos mainnet, the
//! Solana mints answered `getAccountInfo` as SPL mints, and the Cardano assets
//! answered Koios with their registered metadata. Nothing here is transcribed
//! from a block explorer's search box, because a wallet that ships a wrong
//! contract address ships a way to lose money silently.
//!
//! # What a row does not promise
//!
//! That the wallet can *move* it. [`Standard`] says how the token is held, and
//! the chains are not equal here: an ERC-20 transfer is a signed call this
//! wallet has always made, an SPL transfer is one it now makes, and a Cardano
//! native asset is one it can only read — moving one means building a
//! multi-asset output and preserving every other token riding on the same
//! UTxO, which this wallet does not model. [`Token::transferable`] answers it
//! honestly per row rather than letting a send fail late.

use serde::Serialize;

use crate::chain::ChainId;
use crate::error::{self, Result};
use crate::network::{self, Network};

/// How a token is held, which is also how it is moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standard {
    /// An EVM contract at `id`, moved with `transfer(address,uint256)`.
    Erc20,
    /// An SPL mint at `id`, moved between associated token accounts.
    SplToken,
    /// A Cardano native asset, `id` being `<policy_id><asset_name_hex>`.
    /// Readable; not movable by this wallet.
    CardanoAsset,
}

impl Standard {
    pub fn as_str(self) -> &'static str {
        match self {
            Standard::Erc20 => "erc20",
            Standard::SplToken => "spl-token",
            Standard::CardanoAsset => "cardano-asset",
        }
    }

    /// What the `id` field of a row of this standard is called on its chain.
    ///
    /// Worth naming, because "address" is three different things here and a
    /// user pasting one into the wrong field gets a confusing refusal.
    pub fn id_label(self) -> &'static str {
        match self {
            Standard::Erc20 => "Contract",
            Standard::SplToken => "Mint",
            Standard::CardanoAsset => "Asset",
        }
    }

    /// Can this wallet sign a transfer of it?
    pub fn transferable(self) -> bool {
        match self {
            Standard::Erc20 | Standard::SplToken => true,
            // A Cardano UTxO carrying assets has to be rebuilt asset by asset,
            // with its own minimum-ADA and every other token on it preserved.
            // The wallet skips those UTxOs rather than risk dropping one.
            Standard::CardanoAsset => false,
        }
    }
}

/// One token, on one network.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Token {
    /// The stable key: `<symbol>-<network key>`, lowercased.
    pub key: &'static str,
    /// The flat name, which is the key with the hyphens spent: the symbol,
    /// the chain and the network, in the order someone says them out loud.
    pub name: &'static str,
    /// The network key this row belongs to. Never `None`: a token with no
    /// network is the ambiguity this table exists to remove.
    pub network: &'static str,
    pub chain: ChainId,
    pub standard: Standard,
    pub symbol: &'static str,
    pub decimals: u8,
    /// The contract address, mint or asset unit, in that chain's own encoding.
    pub id: &'static str,
    /// What this token is, beyond what its name says — the same rule the
    /// network tags follow. `stablecoin` and `usd` are here because that is
    /// what someone looking for USDC without remembering "USDC" types.
    pub tags: &'static [&'static str],
}

// ------------------------------------------------------------------ the table
//
// Grouped by network, in the network table's order, so that `token list` and
// `network list` scan the same way.

// --- Cronos EVM Mainnet ---------------------------------------------------
//
// Read off chain 25 on 2026-08-25: each answered `symbol()`, `name()` and
// `decimals()` at the address below.

pub const USDC_CRONOS_MAINNET: Token = Token {
    key: "usdc-cronos-mainnet",
    name: "USDC Cronos Mainnet",
    network: "cronos-mainnet",
    chain: ChainId::Evm,
    standard: Standard::Erc20,
    symbol: "USDC",
    decimals: 6,
    id: "0xc21223249CA28397B4B6541dfFaEcC539BfF0c59",
    tags: &["stablecoin", "usd", "circle"],
};

pub const USDT_CRONOS_MAINNET: Token = Token {
    key: "usdt-cronos-mainnet",
    name: "USDT Cronos Mainnet",
    network: "cronos-mainnet",
    chain: ChainId::Evm,
    standard: Standard::Erc20,
    symbol: "USDT",
    decimals: 6,
    id: "0x66e428c3f67a68878562e79A0234c1F83c208770",
    tags: &["stablecoin", "usd", "tether"],
};

pub const DAI_CRONOS_MAINNET: Token = Token {
    key: "dai-cronos-mainnet",
    name: "DAI Cronos Mainnet",
    network: "cronos-mainnet",
    chain: ChainId::Evm,
    standard: Standard::Erc20,
    symbol: "DAI",
    decimals: 18,
    id: "0xF2001B145b43032AAF5Ee2884e456CCd805F677D",
    tags: &["stablecoin", "usd", "maker"],
};

// --- Solana ---------------------------------------------------------------
//
// Each mint answered `getAccountInfo` on the cluster it is listed under, as an
// SPL mint owned by the token program, with the decimals below. The devnet
// USDC is Circle's faucet mint and is a *different* mint from the mainnet one
// — which is exactly why the two are separate rows rather than one "USDC".

pub const USDC_SOLANA_MAINNET: Token = Token {
    key: "usdc-solana-mainnet",
    name: "USDC Solana Mainnet",
    network: "solana-mainnet",
    chain: ChainId::Solana,
    standard: Standard::SplToken,
    symbol: "USDC",
    decimals: 6,
    id: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    tags: &["stablecoin", "usd", "circle"],
};

pub const USDT_SOLANA_MAINNET: Token = Token {
    key: "usdt-solana-mainnet",
    name: "USDT Solana Mainnet",
    network: "solana-mainnet",
    chain: ChainId::Solana,
    standard: Standard::SplToken,
    symbol: "USDT",
    decimals: 6,
    id: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
    tags: &["stablecoin", "usd", "tether"],
};

pub const USDC_SOLANA_DEVNET: Token = Token {
    key: "usdc-solana-devnet",
    name: "USDC Solana Devnet",
    network: "solana-devnet",
    chain: ChainId::Solana,
    standard: Standard::SplToken,
    symbol: "USDC",
    decimals: 6,
    id: "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU",
    tags: &["stablecoin", "usd", "circle", "faucet"],
};

// --- Cardano Mainnet ------------------------------------------------------
//
// `id` is the asset unit: the 28-byte policy id followed by the asset name in
// hex, which is how Koios and every Cardano tool key an asset. Each was read
// back from Koios `asset_info`, including the decimals — and the decimals are
// the reason to read rather than assume: Cardano's USDC is a bridged asset
// with **8** places, not the 6 it has everywhere else. Assuming 6 here would
// mis-state every balance by a factor of a hundred.
//
// These are read-only rows; see [`Standard::CardanoAsset`].

pub const USDC_CARDANO_MAINNET: Token = Token {
    key: "usdc-cardano-mainnet",
    name: "USDC Cardano Mainnet",
    network: "cardano-mainnet",
    chain: ChainId::Cardano,
    standard: Standard::CardanoAsset,
    symbol: "USDC",
    decimals: 8,
    id: "25c5de5f5b286073c593edfd77b48abc7a48e5a4f3d4cd9d428ff93555534443",
    tags: &["stablecoin", "usd", "bridged", "wanchain", "read-only"],
};

pub const USDM_CARDANO_MAINNET: Token = Token {
    key: "usdm-cardano-mainnet",
    name: "USDM Cardano Mainnet",
    network: "cardano-mainnet",
    chain: ChainId::Cardano,
    standard: Standard::CardanoAsset,
    symbol: "USDM",
    decimals: 6,
    id: "c48cbb3d5e57ed56e276bc45f99ab39abe94e6cd7ac39fb402da47ad0014df105553444d",
    tags: &["stablecoin", "usd", "moneta", "read-only"],
};

pub const DJED_CARDANO_MAINNET: Token = Token {
    key: "djed-cardano-mainnet",
    name: "DJED Cardano Mainnet",
    network: "cardano-mainnet",
    chain: ChainId::Cardano,
    standard: Standard::CardanoAsset,
    symbol: "DJED",
    decimals: 6,
    id: "8db269c3ec630e06ae29f74bc39edd1f87c819f1056206e879a1cd61446a65644d6963726f555344",
    tags: &["stablecoin", "usd", "overcollateralised", "read-only"],
};

pub const IUSD_CARDANO_MAINNET: Token = Token {
    key: "iusd-cardano-mainnet",
    name: "iUSD Cardano Mainnet",
    network: "cardano-mainnet",
    chain: ChainId::Cardano,
    standard: Standard::CardanoAsset,
    symbol: "iUSD",
    decimals: 6,
    id: "f66d78b4a3cb3d37afa0ec36461e51ecbde00f26c8f0a68f94b6988069555344",
    tags: &["stablecoin", "usd", "indigo", "read-only"],
};

/// Every token, grouped by network in the network table's order.
pub const ALL: [Token; 10] = [
    USDC_CRONOS_MAINNET,
    USDT_CRONOS_MAINNET,
    DAI_CRONOS_MAINNET,
    USDC_SOLANA_MAINNET,
    USDT_SOLANA_MAINNET,
    USDC_SOLANA_DEVNET,
    USDC_CARDANO_MAINNET,
    USDM_CARDANO_MAINNET,
    DJED_CARDANO_MAINNET,
    IUSD_CARDANO_MAINNET,
];

// -------------------------------------------------------------------- lookup

/// The tokens a query keeps, in table order. An empty query keeps them all.
pub fn search(query: &str) -> Vec<Token> {
    let terms = crate::search::terms(query);
    ALL.iter().filter(|t| t.matches(&terms)).copied().collect()
}

/// The tokens registered on one network.
pub fn for_network(network: &str) -> Vec<Token> {
    ALL.iter()
        .filter(|t| t.network == network)
        .copied()
        .collect()
}

/// Every tag any token carries, sorted and deduplicated.
pub fn tags() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = ALL.iter().flat_map(|t| t.tags.iter().copied()).collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// Look a token up by key.
///
/// A bare symbol resolves only when one row carries it, which is the whole
/// point of the flat table: `dai` is unambiguous and `usdc` is not, and the
/// error for `usdc` names the four rows rather than picking whichever came
/// first. Guessing here would move real money on the wrong chain.
pub fn find(key: &str) -> Result<Token> {
    let wanted = normalize(key);
    if let Some(token) = ALL.iter().find(|t| t.key == wanted) {
        return Ok(*token);
    }
    // The on-chain id, for someone who has the address and not the name.
    if let Some(token) = ALL
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(key.trim()) || t.id.eq_ignore_ascii_case(&wanted))
    {
        return Ok(*token);
    }
    let by_symbol: Vec<&Token> = ALL
        .iter()
        .filter(|t| t.symbol.to_lowercase() == wanted)
        .collect();
    match by_symbol.as_slice() {
        [only] => Ok(**only),
        [] => Err(error::not_found(format!(
            "unknown token '{key}'; try `token list` to see the {} this wallet knows",
            ALL.len()
        ))),
        several => Err(error::not_found(format!(
            "'{key}' is on {} networks: {}. Name the network too — the row is \
             called `{}`, not `{}`",
            several.len(),
            // Capped, because this list grows with the table and an error that
            // fills the terminal buries the sentence that says what to do.
            // `token list {key}` is the uncapped answer.
            naming(several, 6),
            several[0].key,
            wanted
        ))),
    }
}

/// Look a token up within one network, where a bare symbol is unambiguous.
///
/// This is what `-n cronos-mainnet token balance usdc` goes through: the
/// network is already settled, so `usdc` can only mean one row.
pub fn find_on(network: &str, key: &str) -> Result<Token> {
    let wanted = normalize(key);
    let candidates = for_network(network);
    candidates
        .iter()
        .find(|t| {
            t.key == wanted
                || t.symbol.to_lowercase() == wanted
                || t.id.eq_ignore_ascii_case(key.trim())
        })
        .copied()
        .ok_or_else(|| {
            if candidates.is_empty() {
                error::not_found(format!(
                    "this wallet knows no tokens on {network}; `token list` \
                     shows which networks have them"
                ))
            } else {
                error::not_found(format!(
                    "unknown token '{key}' on {network}; known there: {}",
                    naming(&candidates.iter().collect::<Vec<_>>(), 8)
                ))
            }
        })
}

/// Name at most `most` rows, and say how many were left out.
fn naming(tokens: &[&Token], most: usize) -> String {
    let named = tokens
        .iter()
        .take(most)
        .map(|t| t.key)
        .collect::<Vec<_>>()
        .join(", ");
    match tokens.len().saturating_sub(most) {
        0 => named,
        rest => format!("{named}, and {rest} more"),
    }
}

fn normalize(key: &str) -> String {
    key.trim().to_lowercase().replace([' ', '_'], "-")
}

impl Token {
    /// Does this row survive a search query?
    ///
    /// Searched by everything the row is: its key, its flat name, its symbol,
    /// its chain, its network key, its standard and its tags. The network key
    /// is in there because `usdc cronos` has to work, and `cronos` is a word
    /// this row only owns through the network it sits on.
    pub fn matches(&self, terms: &[String]) -> bool {
        let mut haystacks = vec![
            self.key,
            self.name,
            self.symbol,
            self.chain.as_str(),
            self.network,
            self.standard.as_str(),
        ];
        haystacks.extend_from_slice(self.tags);
        crate::search::haystack_matches(&haystacks, terms)
    }

    /// The network row this token sits on.
    pub fn network(&self) -> Network {
        network::find(self.network).expect("every token names a network in the table")
    }

    /// Can this wallet sign a transfer of it?
    pub fn transferable(&self) -> bool {
        self.standard.transferable()
    }

    /// How this token is counted and named.
    pub fn units(&self) -> crate::chain::Amount {
        crate::chain::Amount::new(self.decimals, self.symbol)
    }

    /// Why a transfer of this token is refused, when it is.
    pub fn refuse_transfer(&self) -> error::Error {
        error::usage(format!(
            "{} is a Cardano native asset, which this wallet reads but does not \
             move: spending the output holding it means rebuilding every other \
             asset riding on the same UTxO, and dropping one silently is not \
             something a wallet may do. `token balance {}` still works",
            self.name, self.key
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_key_with_the_hyphens_spent() {
        // The flat naming this table is built on: `usdc-cronos-mainnet` and
        // "USDC Cronos Mainnet" are one row said two ways, and a search box
        // has to find it from either.
        for t in ALL {
            assert_eq!(
                t.name.to_lowercase().replace(' ', "-"),
                t.key,
                "{} and {} are not the same row said twice",
                t.key,
                t.name
            );
        }
    }

    #[test]
    fn a_key_is_its_symbol_and_its_network() {
        for t in ALL {
            assert_eq!(
                t.key,
                format!("{}-{}", t.symbol.to_lowercase(), t.network),
                "{} is not named after what it is and where it is",
                t.key
            );
        }
    }

    #[test]
    fn token_keys_are_unique() {
        let mut keys: Vec<&str> = ALL.iter().map(|t| t.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate token key in the table");
    }

    #[test]
    fn every_token_sits_on_a_network_of_its_own_chain() {
        // The one inconsistency that would send funds to another chain.
        for t in ALL {
            let n = network::find(t.network).unwrap_or_else(|e| panic!("{}: {e}", t.key));
            assert_eq!(n.chain, t.chain, "{} disagrees with its network", t.key);
            assert_eq!(t.network().key, t.network);
        }
    }

    #[test]
    fn the_standard_matches_the_chain() {
        for t in ALL {
            let expected = match t.chain {
                ChainId::Evm => Standard::Erc20,
                ChainId::Solana => Standard::SplToken,
                ChainId::Cardano => Standard::CardanoAsset,
                ChainId::Midnight => unreachable!("no Midnight token rows yet"),
                ChainId::Ecash => unreachable!("no eCash token rows yet"),
            };
            assert_eq!(t.standard, expected, "{}", t.key);
        }
    }

    #[test]
    fn on_chain_ids_are_shaped_like_their_standard() {
        for t in ALL {
            match t.standard {
                // 0x + 40 hex, and checksummed: a lowercase address in the
                // table would still work but would not match what an explorer
                // shows, which is how a user checks it.
                Standard::Erc20 => {
                    assert!(t.id.starts_with("0x") && t.id.len() == 42, "{}", t.key);
                    assert!(hex::decode(&t.id[2..]).is_ok(), "{}", t.key);
                    let parsed: alloy_primitives::Address = t.id.parse().unwrap();
                    assert_eq!(parsed.to_checksum(None), t.id, "{} is not EIP-55", t.key);
                }
                // A base58 32-byte pubkey.
                Standard::SplToken => {
                    let raw = bs58::decode(t.id).into_vec().unwrap_or_default();
                    assert_eq!(raw.len(), 32, "{} is not a 32-byte mint", t.key);
                }
                // 28-byte policy id then the asset name, all hex.
                Standard::CardanoAsset => {
                    assert!(t.id.len() > 56, "{} has no asset name", t.key);
                    assert!(hex::decode(t.id).is_ok(), "{} is not hex", t.key);
                }
            }
        }
    }

    #[test]
    fn the_usdc_rows_are_four_different_tokens_not_one() {
        // The reason the table is flat. Same name, same peg, nothing else:
        // four ids, and two different decimal counts.
        let usdc: Vec<Token> = ALL.iter().filter(|t| t.symbol == "USDC").copied().collect();
        assert_eq!(usdc.len(), 4);
        let mut ids: Vec<&str> = usdc.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4, "two USDC rows share an id");
        // Cardano's is bridged and carries eight places, not six.
        assert_eq!(USDC_CARDANO_MAINNET.decimals, 8);
        assert_eq!(USDC_CRONOS_MAINNET.decimals, 6);
    }

    #[test]
    fn the_headline_row_is_usdc_on_cronos_mainnet() {
        let t = find("usdc-cronos-mainnet").unwrap();
        assert_eq!(t.id, "0xc21223249CA28397B4B6541dfFaEcC539BfF0c59");
        assert_eq!(t.decimals, 6);
        assert_eq!(t.chain, ChainId::Evm);
        assert_eq!(t.network().chain_id, Some(25));
        assert!(t.transferable());
    }

    #[test]
    fn an_unambiguous_symbol_resolves_and_an_ambiguous_one_names_the_rows() {
        assert_eq!(find("dai").unwrap().key, "dai-cronos-mainnet");
        assert_eq!(find("DJED").unwrap().key, "djed-cardano-mainnet");
        let err = find("usdc").unwrap_err();
        assert_eq!(err.code, error::Code::NotFound);
        assert!(
            err.message.contains("usdc-cronos-mainnet"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains("usdc-solana-mainnet"),
            "{}",
            err.message
        );
    }

    #[test]
    fn naming_the_network_makes_a_bare_symbol_unambiguous() {
        assert_eq!(
            find_on("cronos-mainnet", "usdc").unwrap().key,
            "usdc-cronos-mainnet"
        );
        assert_eq!(
            find_on("solana-devnet", "USDC").unwrap().key,
            "usdc-solana-devnet"
        );
        // And it refuses a token that lives somewhere else, rather than
        // quietly reaching across the chain boundary.
        let err = find_on("cronos-mainnet", "djed").unwrap_err();
        assert!(
            err.message.contains("usdc-cronos-mainnet"),
            "{}",
            err.message
        );
        let err = find_on("cronos-testnet", "usdc").unwrap_err();
        assert!(err.message.contains("knows no tokens"), "{}", err.message);
    }

    #[test]
    fn a_contract_address_resolves_as_well_as_a_name() {
        // Someone who pasted an address should not have to learn our key.
        assert_eq!(
            find("0xc21223249CA28397B4B6541dfFaEcC539BfF0c59")
                .unwrap()
                .key,
            "usdc-cronos-mainnet"
        );
        assert_eq!(
            find("0xC21223249ca28397b4b6541dffaecc539bff0c59")
                .unwrap()
                .key,
            "usdc-cronos-mainnet"
        );
        assert_eq!(
            find("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
                .unwrap()
                .key,
            "usdc-solana-mainnet"
        );
    }

    #[test]
    fn an_unknown_token_says_how_to_find_the_known_ones() {
        let err = find("shib-cronos-mainnet").unwrap_err();
        assert!(err.message.contains("token list"), "{}", err.message);
    }

    #[test]
    fn search_finds_a_row_by_the_name_it_is_said_by() {
        let hits: Vec<&str> = search("usdc cronos").iter().map(|t| t.key).collect();
        assert_eq!(hits, vec!["usdc-cronos-mainnet"]);
        // And by what it is rather than what it is called.
        let stable: Vec<&str> = search("stablecoin solana").iter().map(|t| t.key).collect();
        assert_eq!(
            stable,
            vec![
                "usdc-solana-mainnet",
                "usdt-solana-mainnet",
                "usdc-solana-devnet"
            ]
        );
        assert_eq!(search("").len(), ALL.len());
        assert!(search("usdc midnight").is_empty());
    }

    #[test]
    fn every_row_is_tagged_and_no_tag_merely_repeats_the_key() {
        for t in ALL {
            assert!(!t.tags.is_empty(), "{} carries no tags", t.key);
            for tag in t.tags {
                // A word the key already carries; `usd` survives because
                // `usdc` is a different word, and it is the tag that will
                // still mean something the day a euro stablecoin lands here.
                assert!(
                    !t.key.split('-').any(|word| word == *tag),
                    "{}: tag `{tag}` only repeats a word of the row's own key",
                    t.key
                );
            }
        }
        assert!(tags().contains(&"stablecoin"));
    }

    #[test]
    fn a_read_only_row_says_so_in_its_tags_and_in_its_refusal() {
        for t in ALL {
            assert_eq!(
                t.tags.contains(&"read-only"),
                !t.transferable(),
                "{} does not say whether it can be moved",
                t.key
            );
        }
        // And the refusal explains itself rather than reporting a failure.
        let err = USDC_CARDANO_MAINNET.refuse_transfer();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("token balance"), "{}", err.message);
    }

    #[test]
    fn every_network_carrying_tokens_is_grouped_together_in_the_table() {
        // `token list` prints in table order and groups by network; a row out
        // of place would split a network's block in the output.
        let mut seen: Vec<&str> = Vec::new();
        let mut last = "";
        for t in ALL {
            if t.network != last {
                assert!(!seen.contains(&t.network), "{} is listed twice", t.network);
                seen.push(t.network);
                last = t.network;
            }
        }
        assert_eq!(for_network("cronos-mainnet").len(), 3);
        assert_eq!(for_network("cardano-mainnet").len(), 4);
        assert!(for_network("midnight-devnet").is_empty());
    }

    #[test]
    fn units_come_from_the_token_row_not_the_network() {
        // The bug this prevents: formatting a USDC balance with the network's
        // 18 places, which reads as a trillionth of the real number.
        assert_eq!(USDC_CRONOS_MAINNET.units().symbol, "USDC");
        assert_eq!(USDC_CRONOS_MAINNET.units().decimals, 6);
        assert_ne!(
            USDC_CRONOS_MAINNET.units().decimals,
            USDC_CRONOS_MAINNET.network().decimals
        );
        assert_eq!(USDC_CRONOS_MAINNET.units().format(1_500_000), "1.5");
    }
}
