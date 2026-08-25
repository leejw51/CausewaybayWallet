//! One way to narrow a long list, shared by the network table and the token
//! registry.
//!
//! Both tables outgrew the eye. Ten networks was a list you could read; five
//! chains' worth of networks and every stablecoin on each is a list you scan.
//! So both are searched the same way, by the same code, and a front end that
//! learns the rule for one has learned it for the other.
//!
//! The rule is deliberately small, because a search box that needs explaining
//! is a search box nobody uses:
//!
//! * An **empty query matches everything**. Filtering is something you opt
//!   into; a wallet that hides networks until you type is a wallet that has
//!   lost them.
//! * A query is split on whitespace and commas into **terms**, and every term
//!   must match — `usdc cronos` is USDC *and* Cronos, not either. Narrowing by
//!   adding words is the one habit every search box has taught.
//! * A term matches if it appears anywhere in any of the row's **haystacks**:
//!   its key, its name, its symbol and its tags. Substring, not prefix, so
//!   `net` finds `cronos-testnet` and `main` finds every mainnet.
//! * Case and the `-`/`_`/space difference are ignored, so `Cronos Mainnet`,
//!   `cronos-mainnet` and `CRONOS_MAINNET` are one query.
//!
//! What is *not* here is ranking. These are tables of a few dozen fixed rows
//! where the user is looking for one they can already name; sorting the
//! survivors by a relevance score would only move a row someone had learned
//! the position of.

/// Split a query into the terms every row must match.
///
/// Returned lowercased and hyphen-folded, the same shape [`haystack_matches`]
/// folds each candidate into, so the comparison is a plain substring test.
pub fn terms(query: &str) -> Vec<String> {
    query
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(fold)
        .collect()
}

/// Does this row survive the query?
///
/// `haystacks` is everything the row can be found by — key, name, symbol,
/// tags. Every term has to appear in at least one of them, though not
/// necessarily the same one: that is what lets `usdc cronos` find a token
/// whose symbol carries one word and whose network carries the other.
pub fn haystack_matches(haystacks: &[&str], terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let folded: Vec<String> = haystacks.iter().map(|h| fold(h)).collect();
    terms
        .iter()
        .all(|term| folded.iter().any(|h| h.contains(term.as_str())))
}

/// Search a query directly, for the callers with one row in hand.
pub fn matches(haystacks: &[&str], query: &str) -> bool {
    haystack_matches(haystacks, &terms(query))
}

/// Lowercase, and settle `-`, `_` and spaces on one character.
///
/// The registry writes keys with hyphens and names with spaces, and a user
/// types whichever they last saw. Folding both to a hyphen means one query
/// finds the row under either spelling — and it is why `cronos mainnet`
/// (two terms) and `cronos-mainnet` (one term, hyphen and all) both land.
fn fold(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .replace([' ', '_'], "-")
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRONOS: [&str; 4] = ["cronos-mainnet", "Cronos EVM Mainnet", "CRO", "evm"];

    #[test]
    fn an_empty_query_keeps_every_row() {
        assert!(matches(&CRONOS, ""));
        assert!(matches(&CRONOS, "   "));
        assert!(matches(&CRONOS, " , "));
    }

    #[test]
    fn a_tag_that_appears_in_no_name_still_finds_the_row() {
        // The whole point of tags: `evm` is nowhere in `cronos-mainnet`.
        assert!(matches(&CRONOS, "evm"));
    }

    #[test]
    fn every_term_has_to_match_so_adding_words_narrows() {
        assert!(matches(&CRONOS, "cronos mainnet"));
        assert!(matches(&CRONOS, "evm cro"));
        // `testnet` is not this row, so the pair cannot be either.
        assert!(!matches(&CRONOS, "cronos testnet"));
    }

    #[test]
    fn terms_may_land_in_different_haystacks() {
        // `usdc` from the symbol, `cronos` from the network — the flat name
        // "USDC Cronos Mainnet" is exactly this query written down.
        let usdc = [
            "usdc-cronos-mainnet",
            "USDC Cronos Mainnet",
            "USDC",
            "stablecoin",
        ];
        assert!(matches(&usdc, "usdc cronos"));
        assert!(matches(&usdc, "stablecoin mainnet"));
    }

    #[test]
    fn case_and_separators_do_not_matter() {
        assert!(matches(&CRONOS, "CRONOS-MAINNET"));
        assert!(matches(&CRONOS, "cronos_mainnet"));
        assert!(matches(&CRONOS, "Cronos Mainnet"));
        assert!(matches(&CRONOS, "  cronos   MAINNET  "));
    }

    #[test]
    fn a_substring_matches_rather_than_only_a_whole_word() {
        assert!(matches(&CRONOS, "main"));
        assert!(matches(&CRONOS, "net"));
    }

    #[test]
    fn commas_separate_terms_too() {
        assert!(matches(&CRONOS, "cronos,evm"));
        assert!(!matches(&CRONOS, "cronos,solana"));
    }

    #[test]
    fn a_term_matching_nothing_drops_the_row() {
        assert!(!matches(&CRONOS, "midnight"));
        assert!(!matches(&CRONOS, "cronos midnight"));
    }
}
