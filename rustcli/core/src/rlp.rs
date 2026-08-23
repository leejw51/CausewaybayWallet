//! Minimal RLP encoder — enough for legacy Ethereum transactions.
//!
//! Only encoding is needed: transactions are built here and decoded by nodes.

use alloy_primitives::U256;

/// Encode a byte string according to RLP rules.
pub fn encode_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        out.push(bytes[0]);
    } else if bytes.len() <= 55 {
        out.push(0x80 + bytes.len() as u8);
        out.extend_from_slice(bytes);
    } else {
        let len_bytes = bytes.len().to_be_bytes();
        let len_be = trim_leading_zeros(&len_bytes);
        out.push(0xb7 + len_be.len() as u8);
        out.extend_from_slice(len_be);
        out.extend_from_slice(bytes);
    }
}

/// Wrap an already-encoded payload in a list header.
pub fn encode_list(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 9);
    if payload.len() <= 55 {
        out.push(0xc0 + payload.len() as u8);
    } else {
        let len_bytes = payload.len().to_be_bytes();
        let len_be = trim_leading_zeros(&len_bytes);
        out.push(0xf7 + len_be.len() as u8);
        out.extend_from_slice(len_be);
    }
    out.extend_from_slice(payload);
    out
}

/// Encode an unsigned integer: big-endian, minimal length, zero as the empty string.
pub fn encode_uint(out: &mut Vec<u8>, value: U256) {
    let be: [u8; 32] = value.to_be_bytes();
    encode_bytes(out, trim_leading_zeros(&be));
}

pub fn encode_u64(out: &mut Vec<u8>, value: u64) {
    encode_uint(out, U256::from(value));
}

fn trim_leading_zeros(bytes: &[u8]) -> &[u8] {
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    &bytes[first..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_bytes(&mut out, bytes);
        out
    }

    fn uint(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        encode_u64(&mut out, value);
        out
    }

    // Vectors from the Ethereum yellow paper appendix / the official RLP tests.
    #[test]
    fn encodes_single_bytes() {
        assert_eq!(enc(&[0x00]), vec![0x00]);
        assert_eq!(enc(&[0x0f]), vec![0x0f]);
        assert_eq!(enc(&[0x7f]), vec![0x7f]);
        assert_eq!(enc(&[0x80]), vec![0x81, 0x80]);
    }

    #[test]
    fn encodes_strings() {
        assert_eq!(enc(b""), vec![0x80]);
        assert_eq!(enc(b"dog"), vec![0x83, b'd', b'o', b'g']);
        let long = b"Lorem ipsum dolor sit amet, consectetur adipisicing elit";
        let out = enc(long);
        assert_eq!(&out[..2], &[0xb8, 0x38]);
        assert_eq!(&out[2..], long);
    }

    #[test]
    fn encodes_very_long_strings() {
        let long = vec![0x61u8; 1024];
        let out = enc(&long);
        assert_eq!(&out[..3], &[0xb9, 0x04, 0x00]);
        assert_eq!(out.len(), 1027);
    }

    #[test]
    fn encodes_uints_minimally() {
        assert_eq!(uint(0), vec![0x80]);
        assert_eq!(uint(1), vec![0x01]);
        assert_eq!(uint(127), vec![0x7f]);
        assert_eq!(uint(128), vec![0x81, 0x80]);
        assert_eq!(uint(1024), vec![0x82, 0x04, 0x00]);
        assert_eq!(
            uint(0xffffffffffffffff),
            vec![0x88, 255, 255, 255, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn encodes_lists() {
        // ["cat", "dog"] -> 0xc8 0x83 c a t 0x83 d o g
        let mut payload = Vec::new();
        encode_bytes(&mut payload, b"cat");
        encode_bytes(&mut payload, b"dog");
        assert_eq!(
            encode_list(&payload),
            vec![0xc8, 0x83, b'c', b'a', b't', 0x83, b'd', b'o', b'g']
        );
        assert_eq!(encode_list(&[]), vec![0xc0]);
    }

    #[test]
    fn encodes_long_lists() {
        let mut payload = Vec::new();
        for _ in 0..20 {
            encode_bytes(&mut payload, b"abcd");
        }
        assert_eq!(payload.len(), 100);
        let out = encode_list(&payload);
        assert_eq!(&out[..2], &[0xf8, 0x64]);
        assert_eq!(out.len(), 102);
    }
}
