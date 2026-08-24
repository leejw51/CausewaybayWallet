//! Base units and how to render them.
//!
//! Every chain here counts in integers and displays in decimals, but they
//! disagree on the scale (18 for wei, 9 for lamports, 6 for lovelace and
//! stars) and on the ticker. [`Amount`] is the pair, so a command can format a
//! balance without knowing which chain produced it.
//!
//! Everything is `u128`. `u64` is too small for wei, and the existing EVM path
//! keeps using `U256` internally where it must — the conversion happens at the
//! chain boundary, and overflows there are reported rather than wrapped.

use crate::error::{self, Result};

/// The unit a chain quotes: how many decimal places, and what to call it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount {
    pub decimals: u8,
    pub symbol: &'static str,
}

impl Amount {
    pub const fn new(decimals: u8, symbol: &'static str) -> Self {
        Amount { decimals, symbol }
    }

    /// Parse a human decimal ("1.25") into base units.
    pub fn parse(&self, text: &str) -> Result<u128> {
        parse_units(text, self.decimals)
    }

    /// Render base units as a decimal string, without trailing zeros.
    pub fn format(&self, value: u128) -> String {
        format_units(value, self.decimals)
    }

    /// Render base units with the ticker: `1.25 TCRO`.
    pub fn format_with_symbol(&self, value: u128) -> String {
        format!("{} {}", self.format(value), self.symbol)
    }
}

/// Parse a decimal string into base units at `decimals` places.
///
/// Deliberately strict: no exponents, no thousands separators, no negatives,
/// and no silent truncation of a fraction too fine for the chain. Somebody who
/// types one decimal place too many meant something, and it was not this.
pub fn parse_units(text: &str, decimals: u8) -> Result<u128> {
    let s = text.trim();
    if s.is_empty() {
        return Err(error::invalid_amount("amount is empty"));
    }
    let s = s.strip_prefix('+').unwrap_or(s);
    if s.starts_with('-') {
        return Err(error::invalid_amount("amount must not be negative"));
    }
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(error::invalid_amount(format!("not a decimal number: {s}")));
    }
    for part in [int_part, frac_part] {
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(error::invalid_amount(format!("not a decimal number: {s}")));
        }
    }
    let places = decimals as usize;
    if frac_part.len() > places {
        return Err(error::invalid_amount(format!(
            "amount {s} has more than {places} decimal places"
        )));
    }

    let mut digits = String::with_capacity(int_part.len() + places);
    digits.push_str(if int_part.is_empty() { "0" } else { int_part });
    digits.push_str(frac_part);
    for _ in 0..(places - frac_part.len()) {
        digits.push('0');
    }
    let trimmed = digits.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    trimmed
        .parse::<u128>()
        .map_err(|_| error::invalid_amount(format!("amount {s} does not fit in 128 bits")))
}

/// Render base units as a decimal string, trailing zeros trimmed.
pub fn format_units(value: u128, decimals: u8) -> String {
    let digits = value.to_string();
    let places = decimals as usize;
    if places == 0 {
        return digits;
    }
    let padded = if digits.len() <= places {
        format!("{}{}", "0".repeat(places + 1 - digits.len()), digits)
    } else {
        digits
    };
    let split = padded.len() - places;
    let (int_part, frac_part) = padded.split_at(split);
    let frac_trimmed = frac_part.trim_end_matches('0');
    if frac_trimmed.is_empty() {
        int_part.to_string()
    } else {
        format!("{int_part}.{frac_trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: Amount = Amount::new(9, "SOL");
    const ADA: Amount = Amount::new(6, "ADA");
    const CRO: Amount = Amount::new(18, "CRO");

    #[test]
    fn each_chains_scale_round_trips() {
        assert_eq!(SOL.parse("1.5").unwrap(), 1_500_000_000);
        assert_eq!(ADA.parse("1.5").unwrap(), 1_500_000);
        assert_eq!(CRO.parse("1.5").unwrap(), 1_500_000_000_000_000_000);
        for (unit, text) in [(SOL, "1.5"), (ADA, "0.000001"), (CRO, "12.345")] {
            assert_eq!(unit.format(unit.parse(text).unwrap()), text);
        }
    }

    #[test]
    fn whole_numbers_render_without_a_point() {
        assert_eq!(SOL.format(1_000_000_000), "1");
        assert_eq!(ADA.format(0), "0");
        assert_eq!(CRO.format_with_symbol(0), "0 CRO");
    }

    #[test]
    fn sub_unit_values_keep_their_leading_zero() {
        assert_eq!(SOL.format(1), "0.000000001");
        assert_eq!(ADA.format(1), "0.000001");
    }

    #[test]
    fn a_fraction_finer_than_the_chain_is_refused_not_truncated() {
        // The bug this prevents: "0.0000001 ADA" quietly becoming zero.
        let err = ADA.parse("0.0000001").unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAmount);
        assert!(err.message.contains("6 decimal places"), "{}", err.message);
        assert!(SOL.parse("0.0000000001").is_err());
    }

    #[test]
    fn junk_is_refused() {
        for bad in ["", "   ", "abc", "1.2.3", "1,000", "1e9", "-1", "0x10", "."] {
            assert!(SOL.parse(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn a_bare_fraction_and_a_bare_integer_both_work() {
        assert_eq!(SOL.parse(".5").unwrap(), 500_000_000);
        assert_eq!(SOL.parse("5.").unwrap(), 5_000_000_000);
        assert_eq!(SOL.parse("0005").unwrap(), 5_000_000_000);
    }

    #[test]
    fn the_top_of_the_range_is_an_error_rather_than_a_wrap() {
        assert_eq!(format_units(u128::MAX, 0), u128::MAX.to_string());
        assert!(CRO.parse(&"9".repeat(40)).is_err());
    }

    #[test]
    fn zero_decimals_is_the_identity() {
        let raw = Amount::new(0, "unit");
        assert_eq!(raw.parse("42").unwrap(), 42);
        assert_eq!(raw.format(42), "42");
        assert!(raw.parse("4.2").is_err());
    }
}
