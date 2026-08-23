//! BIP-39 mnemonic generation, validation and seed derivation (English wordlist).

use hmac::Hmac;
use sha2::{Digest, Sha256, Sha512};
use unicode_normalization::UnicodeNormalization;

use crate::error::{self, Result};

/// The official BIP-39 English wordlist, 2048 entries.
pub fn wordlist() -> &'static [&'static str] {
    use std::sync::OnceLock;
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| include_str!("wordlist_en.txt").lines().collect())
}

/// Word counts we accept, paired with their entropy size in bits.
pub const WORD_COUNTS: [(usize, usize); 5] =
    [(12, 128), (15, 160), (18, 192), (21, 224), (24, 256)];

pub fn entropy_bits_for_words(words: usize) -> Result<usize> {
    WORD_COUNTS
        .iter()
        .find(|(w, _)| *w == words)
        .map(|(_, bits)| *bits)
        .ok_or_else(|| {
            error::invalid_mnemonic(format!(
                "unsupported word count {words}; use 12, 15, 18, 21 or 24"
            ))
        })
}

/// Generate a fresh mnemonic with the given number of words, using the OS RNG.
pub fn generate(words: usize) -> Result<String> {
    let bits = entropy_bits_for_words(words)?;
    let mut entropy = vec![0u8; bits / 8];
    getrandom(&mut entropy)?;
    entropy_to_mnemonic(&entropy)
}

fn getrandom(buf: &mut [u8]) -> Result<()> {
    use rand::RngCore;
    rand::rngs::OsRng
        .try_fill_bytes(buf)
        .map_err(|e| error::internal(format!("system RNG unavailable: {e}")))
}

/// Encode entropy (16..=32 bytes, multiple of 4) as a mnemonic phrase.
pub fn entropy_to_mnemonic(entropy: &[u8]) -> Result<String> {
    if entropy.len() < 16 || entropy.len() > 32 || entropy.len() % 4 != 0 {
        return Err(error::invalid_mnemonic(format!(
            "entropy must be 16..32 bytes and a multiple of 4, got {}",
            entropy.len()
        )));
    }
    let checksum_bits = entropy.len() * 8 / 32;
    let checksum = Sha256::digest(entropy);

    // Concatenate entropy||checksum as a bit string, then slice it into 11-bit indices.
    let mut bits: Vec<bool> = Vec::with_capacity(entropy.len() * 8 + checksum_bits);
    for byte in entropy {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1 == 1);
        }
    }
    for i in 0..checksum_bits {
        bits.push((checksum[i / 8] >> (7 - (i % 8))) & 1 == 1);
    }

    let words = wordlist();
    let phrase = bits
        .chunks(11)
        .map(|chunk| {
            let idx = chunk
                .iter()
                .fold(0usize, |acc, b| (acc << 1) | usize::from(*b));
            words[idx]
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(phrase)
}

/// Recover the entropy behind a mnemonic, verifying its checksum.
pub fn mnemonic_to_entropy(phrase: &str) -> Result<Vec<u8>> {
    let normalized = normalize(phrase);
    let tokens: Vec<&str> = normalized.split(' ').filter(|t| !t.is_empty()).collect();
    entropy_bits_for_words(tokens.len())?;

    let words = wordlist();
    let mut bits: Vec<bool> = Vec::with_capacity(tokens.len() * 11);
    for token in &tokens {
        let idx = words
            .binary_search(token)
            .map_err(|_| error::invalid_mnemonic(format!("'{token}' is not a BIP-39 word")))?;
        for i in (0..11).rev() {
            bits.push((idx >> i) & 1 == 1);
        }
    }

    let entropy_bits = bits.len() * 32 / 33;
    let checksum_bits = bits.len() - entropy_bits;
    let mut entropy = vec![0u8; entropy_bits / 8];
    for (i, bit) in bits[..entropy_bits].iter().enumerate() {
        if *bit {
            entropy[i / 8] |= 1 << (7 - (i % 8));
        }
    }

    let expected = Sha256::digest(&entropy);
    for i in 0..checksum_bits {
        let want = (expected[i / 8] >> (7 - (i % 8))) & 1 == 1;
        if bits[entropy_bits + i] != want {
            return Err(error::invalid_mnemonic("mnemonic checksum does not match"));
        }
    }
    Ok(entropy)
}

/// True when the phrase is a well-formed mnemonic with a valid checksum.
pub fn validate(phrase: &str) -> bool {
    mnemonic_to_entropy(phrase).is_ok()
}

/// Derive the 64-byte BIP-39 seed (PBKDF2-HMAC-SHA512, 2048 rounds).
pub fn to_seed(phrase: &str, passphrase: &str) -> [u8; 64] {
    let normalized = normalize(phrase);
    let salt = format!("mnemonic{}", passphrase.nfkd().collect::<String>());
    let mut seed = [0u8; 64];
    pbkdf2::pbkdf2::<Hmac<Sha512>>(normalized.as_bytes(), salt.as_bytes(), 2048, &mut seed)
        .expect("PBKDF2 output length is valid");
    seed
}

/// NFKD-normalise and collapse whitespace, as BIP-39 requires.
pub fn normalize(phrase: &str) -> String {
    phrase
        .nfkd()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordlist_is_sorted_and_complete() {
        let words = wordlist();
        assert_eq!(words.len(), 2048);
        assert_eq!(words[0], "abandon");
        assert_eq!(words[2047], "zoo");
        assert!(
            words.windows(2).all(|w| w[0] < w[1]),
            "wordlist must be sorted for binary search"
        );
    }

    /// Official Trezor BIP-39 vectors: (entropy hex, phrase, seed with passphrase "TREZOR").
    const VECTORS: &[(&str, &str, &str)] = &[
        (
            "00000000000000000000000000000000",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04",
        ),
        (
            "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            "legal winner thank year wave sausage worth useful legal winner thank yellow",
            "2e8905819b8723fe2c1d161860e5ee1830318dbf49a83bd451cfb8440c28bd6fa457fe1296106559a3c80937a1c1069be3a3a5bd381ee6260e8d9739fce1f607",
        ),
        (
            "80808080808080808080808080808080",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
            "d71de856f81a8acc65e6fc851a38d4d7ec216fd0796d0a6827a3ad6ed5511a30fa280f12eb2e47ed2ac03b5c462a0358d18d69fe4f985ec81778c1b370b652a8",
        ),
        (
            "ffffffffffffffffffffffffffffffff",
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
            "ac27495480225222079d7be181583751e86f571027b0497b5b5d11218e0a8a13332572917f0f8e5a589620c6f15b11c61dee327651a14c34e18231052e48c069",
        ),
        (
            "9e885d952ad362caeb4efe34a8e91bd2",
            "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic",
            "274ddc525802f7c828d8ef7ddbcdc5304e87ac3535913611fbbfa986d0c9e5476c91689f9c8a54fd55bd38606aa6a8595ad213d4c9c9f9aca3fb217069a41028",
        ),
        (
            "77c2b00716cec7213839159e404db50d",
            "jelly better achieve collect unaware mountain thought cargo oxygen act hood bridge",
            "b5b6d0127db1a9d2226af0c3346031d77af31e918dba64287a1b44b8ebf63cdd52676f672a290aae502472cf2d602c051f3e6f18055e84e4c43897fc4e51a6ff",
        ),
        (
            "0000000000000000000000000000000000000000000000000000000000000000",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
            "bda85446c68413707090a52022edd26a1c9462295029f2e60cd7c4f2bbd3097170af7a4d73245cafa9c3cca8d561a7c3de6f5d4a10be8ed2a5e608d68f92fcc8",
        ),
        (
            "8080808080808080808080808080808080808080808080808080808080808080",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
            "c0c519bd0e91a2ed54357d9d1ebef6f5af218a153624cf4f2da911a0ed8f7a09e2ef61af0aca007096df430022f7a2b6fb91661a9589097069720d015e4e982f",
        ),
        (
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
            "dd48c104698c30cfe2b6142103248622fb7bb0ff692eebb00089b32d22484e1613912f0a5b694407be899ffd31ed3992c456cdf60f5d4564b8ba3f05a69890ad",
        ),
    ];

    #[test]
    fn matches_official_vectors() {
        for (hex_entropy, phrase, seed) in VECTORS {
            let entropy = hex::decode(hex_entropy).unwrap();
            assert_eq!(
                &entropy_to_mnemonic(&entropy).unwrap(),
                phrase,
                "entropy {hex_entropy}"
            );
            assert_eq!(
                &mnemonic_to_entropy(phrase).unwrap(),
                &entropy,
                "phrase {phrase}"
            );
            assert_eq!(
                &hex::encode(to_seed(phrase, "TREZOR")),
                seed,
                "seed for {hex_entropy}"
            );
            assert!(validate(phrase));
        }
    }

    #[test]
    fn known_vector_all_zero_entropy() {
        let entropy = [0u8; 16];
        let phrase = entropy_to_mnemonic(&entropy).unwrap();
        assert_eq!(
            phrase,
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        );
        assert_eq!(mnemonic_to_entropy(&phrase).unwrap(), entropy.to_vec());
        assert_eq!(
            hex::encode(to_seed(&phrase, "TREZOR")),
            "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04"
        );
    }

    #[test]
    fn known_vector_24_words() {
        let entropy = [0x80u8; 32];
        let phrase = entropy_to_mnemonic(&entropy).unwrap();
        assert_eq!(phrase.split(' ').count(), 24);
        assert_eq!(mnemonic_to_entropy(&phrase).unwrap(), entropy.to_vec());
    }

    #[test]
    fn empty_passphrase_seed_matches_reference() {
        // Reference seed for the canonical "abandon ... about" phrase with no passphrase.
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert_eq!(
            hex::encode(to_seed(phrase, "")),
            "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4"
        );
    }

    #[test]
    fn rejects_bad_checksum() {
        let bad = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        assert!(!validate(bad));
        assert_eq!(
            mnemonic_to_entropy(bad).unwrap_err().code,
            crate::error::Code::InvalidMnemonic
        );
    }

    #[test]
    fn rejects_unknown_word() {
        let bad = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon notaword";
        assert!(!validate(bad));
    }

    #[test]
    fn rejects_bad_word_count() {
        assert!(!validate("abandon abandon about"));
        assert!(entropy_bits_for_words(13).is_err());
    }

    #[test]
    fn normalises_whitespace_and_case() {
        let messy = "  ABANDON   abandon\tabandon abandon abandon abandon abandon abandon abandon abandon abandon\nabout ";
        assert!(validate(messy));
        assert_eq!(
            mnemonic_to_entropy(messy).unwrap(),
            mnemonic_to_entropy("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap()
        );
    }

    #[test]
    fn generates_every_supported_length() {
        for (words, bits) in WORD_COUNTS {
            let phrase = generate(words).unwrap();
            assert_eq!(phrase.split(' ').count(), words);
            assert!(validate(&phrase));
            assert_eq!(mnemonic_to_entropy(&phrase).unwrap().len(), bits / 8);
        }
    }

    #[test]
    fn generated_phrases_differ() {
        let a = generate(12).unwrap();
        let b = generate(12).unwrap();
        assert_ne!(a, b, "two generated mnemonics must not collide");
    }

    #[test]
    fn passphrase_changes_the_seed() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert_ne!(to_seed(phrase, ""), to_seed(phrase, "TREZOR"));
    }

    #[test]
    fn rejects_malformed_entropy_lengths() {
        assert!(entropy_to_mnemonic(&[0u8; 15]).is_err());
        assert!(entropy_to_mnemonic(&[0u8; 33]).is_err());
        assert!(entropy_to_mnemonic(&[0u8; 18]).is_err());
    }
}
