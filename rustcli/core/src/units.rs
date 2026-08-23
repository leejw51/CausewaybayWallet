//! Decimal-string <-> integer conversions that never touch floating point.

use alloy_primitives::U256;

use crate::error::{self, Result};

/// Parse a human decimal amount ("1.25") into its smallest unit given `decimals`.
pub fn parse_units(amount: &str, decimals: u8) -> Result<U256> {
    let s = amount.trim();
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
    let decimals = decimals as usize;
    if frac_part.len() > decimals {
        return Err(error::invalid_amount(format!(
            "amount {s} has more than {decimals} decimal places"
        )));
    }

    let mut digits = String::with_capacity(int_part.len() + decimals);
    digits.push_str(if int_part.is_empty() { "0" } else { int_part });
    digits.push_str(frac_part);
    for _ in 0..(decimals - frac_part.len()) {
        digits.push('0');
    }
    // Strip leading zeros so U256 parsing sees a canonical decimal string.
    let trimmed = digits.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };

    U256::from_str_radix(trimmed, 10)
        .map_err(|_| error::invalid_amount(format!("amount {s} does not fit in 256 bits")))
}

/// Render a smallest-unit integer as a human decimal string, without trailing zeros.
pub fn format_units(value: U256, decimals: u8) -> String {
    let digits = value.to_string();
    let decimals = decimals as usize;
    if decimals == 0 {
        return digits;
    }
    let padded = if digits.len() <= decimals {
        format!("{}{}", "0".repeat(decimals + 1 - digits.len()), digits)
    } else {
        digits
    };
    let split = padded.len() - decimals;
    let (int_part, frac_part) = padded.split_at(split);
    let frac_trimmed = frac_part.trim_end_matches('0');
    if frac_trimmed.is_empty() {
        int_part.to_string()
    } else {
        format!("{int_part}.{frac_trimmed}")
    }
}

/// Convenience wrappers for the 18-decimal native token.
pub fn parse_ether(amount: &str) -> Result<U256> {
    parse_units(amount, 18)
}

pub fn format_ether(value: U256) -> String {
    format_units(value, 18)
}

/// Gwei is the conventional unit for gas prices.
pub fn parse_gwei(amount: &str) -> Result<U256> {
    parse_units(amount, 9)
}

pub fn format_gwei(value: U256) -> String {
    format_units(value, 9)
}
