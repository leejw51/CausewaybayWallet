//! Minimal ABI encoding and decoding for the ERC-20 functions the wallet uses.

use alloy_primitives::{Address, U256};

use crate::error::{self, Result};

pub const SELECTOR_TRANSFER: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
pub const SELECTOR_BALANCE_OF: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
pub const SELECTOR_DECIMALS: [u8; 4] = [0x31, 0x3c, 0xe5, 0x67];
pub const SELECTOR_SYMBOL: [u8; 4] = [0x95, 0xd8, 0x9b, 0x41];
pub const SELECTOR_NAME: [u8; 4] = [0x06, 0xfd, 0xde, 0x03];
pub const SELECTOR_TOTAL_SUPPLY: [u8; 4] = [0x18, 0x16, 0x0d, 0xdd];
pub const SELECTOR_ALLOWANCE: [u8; 4] = [0xdd, 0x62, 0xed, 0x3e];
pub const SELECTOR_APPROVE: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// `transfer(address,uint256)`
pub fn encode_transfer(to: Address, amount: U256) -> Vec<u8> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&SELECTOR_TRANSFER);
    data.extend_from_slice(&pad_address(to));
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    data
}

/// `approve(address,uint256)`
pub fn encode_approve(spender: Address, amount: U256) -> Vec<u8> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&SELECTOR_APPROVE);
    data.extend_from_slice(&pad_address(spender));
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    data
}

/// `balanceOf(address)`
pub fn encode_balance_of(owner: Address) -> Vec<u8> {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&SELECTOR_BALANCE_OF);
    data.extend_from_slice(&pad_address(owner));
    data
}

/// `allowance(address,address)`
pub fn encode_allowance(owner: Address, spender: Address) -> Vec<u8> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&SELECTOR_ALLOWANCE);
    data.extend_from_slice(&pad_address(owner));
    data.extend_from_slice(&pad_address(spender));
    data
}

/// A zero-argument getter such as `decimals()` or `symbol()`.
pub fn encode_getter(selector: [u8; 4]) -> Vec<u8> {
    selector.to_vec()
}

fn pad_address(address: Address) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address.as_slice());
    word
}

/// Decode a single `uint256` return value.
pub fn decode_uint(data: &[u8]) -> Result<U256> {
    if data.len() < 32 {
        return Err(error::rpc_error(format!(
            "expected a 32-byte uint return value, got {} bytes",
            data.len()
        )));
    }
    Ok(U256::from_be_slice(&data[..32]))
}

/// Decode a `uint8` (such as `decimals()`), which is right-aligned in one word.
pub fn decode_u8(data: &[u8]) -> Result<u8> {
    let value = decode_uint(data)?;
    u8::try_from(value).map_err(|_| error::rpc_error("decimals() returned a value larger than 255"))
}

/// Decode a `string` return value.
///
/// Some older tokens (notably MKR-style ones) return a raw `bytes32` instead of
/// a proper dynamic string, so both layouts are accepted.
pub fn decode_string(data: &[u8]) -> Result<String> {
    if data.is_empty() {
        return Ok(String::new());
    }
    if data.len() >= 64 {
        let offset = U256::from_be_slice(&data[..32]);
        if let Ok(offset) = usize::try_from(offset) {
            if offset == 32 && data.len() >= 64 {
                let length = U256::from_be_slice(&data[32..64]);
                if let Ok(length) = usize::try_from(length) {
                    if 64 + length <= data.len() {
                        return Ok(String::from_utf8_lossy(&data[64..64 + length]).to_string());
                    }
                }
            }
        }
    }
    // bytes32 fallback: trim the zero padding.
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    Ok(String::from_utf8_lossy(&data[..end.min(32)])
        .trim()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(hex_str: &str) -> Address {
        hex_str.parse().unwrap()
    }

    /// Selectors are the first four bytes of the keccak hash of the signature;
    /// these values are cross-checked against `eth_utils.keccak`.
    #[test]
    fn selectors_match_the_standard() {
        assert_eq!(hex::encode(SELECTOR_TRANSFER), "a9059cbb");
        assert_eq!(hex::encode(SELECTOR_BALANCE_OF), "70a08231");
        assert_eq!(hex::encode(SELECTOR_DECIMALS), "313ce567");
        assert_eq!(hex::encode(SELECTOR_SYMBOL), "95d89b41");
        assert_eq!(hex::encode(SELECTOR_NAME), "06fdde03");
        assert_eq!(hex::encode(SELECTOR_TOTAL_SUPPLY), "18160ddd");
        assert_eq!(hex::encode(SELECTOR_ALLOWANCE), "dd62ed3e");
        assert_eq!(hex::encode(SELECTOR_APPROVE), "095ea7b3");
    }

    #[test]
    fn encodes_transfer_calldata() {
        let data = encode_transfer(
            addr("0x3535353535353535353535353535353535353535"),
            U256::from(1_000_000u64),
        );
        assert_eq!(data.len(), 68);
        assert_eq!(
            hex::encode(&data),
            "a9059cbb\
             0000000000000000000000003535353535353535353535353535353535353535\
             00000000000000000000000000000000000000000000000000000000000f4240"
        );
    }

    #[test]
    fn encodes_balance_of_calldata() {
        let data = encode_balance_of(addr("0x9858EfFD232B4033E47d90003D41EC34EcaEda94"));
        assert_eq!(data.len(), 36);
        assert_eq!(
            hex::encode(&data),
            "70a08231\
             0000000000000000000000009858effd232b4033e47d90003d41ec34ecaeda94"
        );
    }

    #[test]
    fn encodes_allowance_and_approve() {
        let owner = addr("0x1111111111111111111111111111111111111111");
        let spender = addr("0x2222222222222222222222222222222222222222");
        assert_eq!(encode_allowance(owner, spender).len(), 68);
        assert_eq!(encode_approve(spender, U256::MAX).len(), 68);
        assert!(hex::encode(encode_approve(spender, U256::MAX)).ends_with(&"f".repeat(64)));
    }

    #[test]
    fn getters_are_just_the_selector() {
        assert_eq!(
            encode_getter(SELECTOR_DECIMALS),
            vec![0x31, 0x3c, 0xe5, 0x67]
        );
    }

    #[test]
    fn decodes_uints() {
        let mut word = [0u8; 32];
        word[31] = 18;
        assert_eq!(decode_uint(&word).unwrap(), U256::from(18));
        assert_eq!(decode_u8(&word).unwrap(), 18);
        assert_eq!(decode_uint(&[0xffu8; 32]).unwrap(), U256::MAX);
        assert!(decode_u8(&[0xffu8; 32]).is_err());
        assert!(decode_uint(&[0u8; 16]).is_err());
        assert!(decode_uint(&[]).is_err());
    }

    #[test]
    fn decodes_dynamic_strings() {
        // offset = 0x20, length = 3, "VVS" padded to 32 bytes
        let encoded = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000020\
             0000000000000000000000000000000000000000000000000000000000000003\
             5656530000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        assert_eq!(decode_string(&encoded).unwrap(), "VVS");
    }

    #[test]
    fn decodes_long_dynamic_strings() {
        let text = "A Rather Long Token Name That Exceeds One Word";
        let mut encoded = vec![0u8; 64];
        encoded[31] = 0x20;
        encoded[32..64].copy_from_slice(&U256::from(text.len()).to_be_bytes::<32>());
        encoded.extend_from_slice(text.as_bytes());
        encoded.resize(64 + text.len().div_ceil(32) * 32, 0);
        assert_eq!(decode_string(&encoded).unwrap(), text);
    }

    #[test]
    fn decodes_bytes32_style_strings() {
        let mut word = [0u8; 32];
        word[..3].copy_from_slice(b"MKR");
        assert_eq!(decode_string(&word).unwrap(), "MKR");
    }

    #[test]
    fn decodes_empty_return_data() {
        assert_eq!(decode_string(&[]).unwrap(), "");
    }

    #[test]
    fn tolerates_a_truncated_dynamic_string() {
        // A length that overruns the buffer must not panic.
        let mut encoded = vec![0u8; 64];
        encoded[31] = 0x20;
        encoded[63] = 0xff;
        assert!(decode_string(&encoded).is_ok());
    }
}
