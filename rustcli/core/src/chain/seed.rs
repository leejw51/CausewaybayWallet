//! What a mnemonic turns into, for chains that disagree about the question.
//!
//! Three of the four chains here hash the BIP-39 *seed* — the 64 bytes
//! PBKDF2 produces from the phrase and its passphrase. Cardano hashes the
//! *entropy* the phrase encodes instead, and passes the passphrase as the
//! PBKDF2 password with that entropy as the salt, which reads backwards and is
//! nonetheless what every Cardano wallet does.
//!
//! Rather than pick a side in [`Chain::derive`], a [`Seed`] carries the phrase
//! and offers both derivations. Each chain asks for the one it needs.
//!
//! [`Chain::derive`]: super::Chain::derive

use crate::bip39;
use crate::error::Result;

/// A mnemonic and its passphrase, ready to be turned into key material.
#[derive(Clone)]
pub struct Seed {
    phrase: String,
    passphrase: String,
}

/// Redacted on purpose: a `{:?}` of a seed must never leak the phrase.
impl std::fmt::Debug for Seed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seed")
            .field("phrase", &"<redacted>")
            .field(
                "passphrase",
                &if self.passphrase.is_empty() {
                    "<none>"
                } else {
                    "<redacted>"
                },
            )
            .finish()
    }
}

impl Seed {
    /// Validate a phrase and hold on to it.
    ///
    /// The validation is eager so that an unusable mnemonic is rejected once,
    /// here, rather than four times over with four different messages.
    pub fn new(phrase: &str, passphrase: &str) -> Result<Self> {
        // Surfaces the specific reason: a bad word, a bad checksum, a bad length.
        bip39::mnemonic_to_entropy(phrase)?;
        Ok(Seed {
            phrase: bip39::normalize(phrase),
            passphrase: passphrase.to_string(),
        })
    }

    /// The normalised phrase, as it should be written to the store.
    pub fn phrase(&self) -> &str {
        &self.phrase
    }

    pub fn passphrase(&self) -> &str {
        &self.passphrase
    }

    /// The 64-byte BIP-39 seed: what EVM, Solana and Midnight derive from.
    pub fn bip39_seed(&self) -> [u8; 64] {
        bip39::to_seed(&self.phrase, &self.passphrase)
    }

    /// The raw entropy the phrase encodes: what Cardano's Icarus scheme salts with.
    pub fn entropy(&self) -> Result<Vec<u8>> {
        bip39::mnemonic_to_entropy(&self.phrase)
    }

    pub fn word_count(&self) -> usize {
        self.phrase.split(' ').count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn the_two_derivations_are_genuinely_different_material() {
        // The whole reason this type exists: a chain that reaches for the
        // wrong one produces a plausible, wrong, unfunded address.
        let seed = Seed::new(PHRASE, "").unwrap();
        assert_eq!(seed.bip39_seed().len(), 64);
        assert_eq!(seed.entropy().unwrap().len(), 16);
        assert_ne!(&seed.bip39_seed()[..16], &seed.entropy().unwrap()[..]);
    }

    #[test]
    fn the_canonical_test_vector_seed_is_stable() {
        // BIP-39's own vector for this phrase with an empty passphrase.
        let seed = Seed::new(PHRASE, "").unwrap();
        assert_eq!(
            hex::encode(seed.bip39_seed()),
            "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1\
             9a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4"
        );
        assert_eq!(
            hex::encode(seed.entropy().unwrap()),
            "00000000000000000000000000000000"
        );
    }

    #[test]
    fn a_passphrase_changes_the_seed_but_not_the_entropy() {
        // Which is exactly why Cardano's passphrase handling has to be its own
        // step rather than falling out of the seed.
        let plain = Seed::new(PHRASE, "").unwrap();
        let salted = Seed::new(PHRASE, "hunter2").unwrap();
        assert_ne!(plain.bip39_seed(), salted.bip39_seed());
        assert_eq!(plain.entropy().unwrap(), salted.entropy().unwrap());
    }

    #[test]
    fn phrases_are_normalised_once_on_the_way_in() {
        let seed = Seed::new(&format!("  {}  ", PHRASE.to_uppercase()), "").unwrap();
        assert_eq!(seed.phrase(), PHRASE);
        assert_eq!(seed.word_count(), 12);
    }

    #[test]
    fn an_invalid_mnemonic_is_refused_once_here() {
        let err = Seed::new("not a real mnemonic phrase at all here ok", "").unwrap_err();
        assert_eq!(err.code, crate::error::Code::InvalidMnemonic);
    }

    #[test]
    fn a_seeds_debug_rendering_leaks_nothing() {
        let rendered = format!("{:?}", Seed::new(PHRASE, "hunter2").unwrap());
        assert!(!rendered.contains("abandon"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }
}
