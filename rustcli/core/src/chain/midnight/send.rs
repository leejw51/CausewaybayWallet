//! Building a Night transfer natively.
//!
//! Midnight fees are paid in DUST, and spending DUST normally requires a
//! zero-knowledge proof. There is one escape hatch, and this module uses it
//! where it can: NIGHT UTxOs that were never *registered* for DUST generation
//! accrue an implicit fee allowance over time (the ledger calls it
//! "generationless" dust), and an intent that both spends such UTxOs and
//! carries a signed `DustRegistration` may pay its fee from that allowance —
//! signatures only, no proofs, no ZK parameters.
//!
//! The trade-off is permanent for the address. That registration puts it under
//! DUST generation, and from then on every UTxO it receives — including the
//! change from this very transfer — registers on arrival, so none of its funds
//! qualify for the cheap path again. The second send from an address therefore
//! takes [`build_via_dust`]: replay the dust ledger, prove a real DUST spend,
//! and pay from it. That is minutes of work rather than milliseconds, which is
//! why the caller is told what is happening while it runs.
//!
//! Submission is a bare (unsigned) Substrate extrinsic — the ledger-level
//! signatures live inside the transaction bytes — sent to the node RPC as
//! `author_submitExtrinsic`, wrapping pallet 5 (`Midnight`), call 0
//! (`send_mn_transaction`).

use base_crypto::data_provider::{FetchMode, MidnightDataProvider, OutputMode};
use base_crypto::hash::HashOutput;
use base_crypto::signatures::{Signature, SigningKey as LedgerSigningKey};
use base_crypto::time::Timestamp;
use coin_structure::coin::{UserAddress, NIGHT};
use midnight_ledger::dust::{
    DustActions, DustLocalState, DustPublicKey, DustRegistration, DustResolver, DustSecretKey,
    DUST_EXPECTED_FILES, INITIAL_DUST_PARAMETERS,
};
use midnight_ledger::prove::Resolver;
use midnight_ledger::structure::{
    Intent, IntentHash, ProofMarker, ProofPreimageMarker, Transaction, UnshieldedOffer, UtxoOutput,
    UtxoSpend, INITIAL_PARAMETERS,
};
use onchain_runtime::cost_model::INITIAL_COST_MODEL;
use rand::rngs::OsRng;
use storage::arena::Sp;
use storage::db::InMemoryDB;
use transient_crypto::commitment::{PedersenRandomness, PureGeneratorPedersen};
use transient_crypto::curve::Fr;
use transient_crypto::proofs::{KeyLocation, Proof, ProofPreimage, ProvingProvider};
use zkir::LocalProvingProvider;
use zswap::{prove::ZswapResolver, ZSWAP_EXPECTED_FILES};

use crate::error::{self, Result};

use super::indexer::UtxoInfo;

/// A sealed transaction, ready to serialize.
pub type Sealed = Transaction<Signature, ProofMarker, PureGeneratorPedersen, InMemoryDB>;

/// How long a transfer stays valid once built.
const TTL_SECONDS: u64 = 1800;

/// The ledger generation this wallet builds for. A node running another
/// generation would reject the bytes with nothing useful to say, so the
/// mismatch is caught before anything is built.
pub const LEDGER_GENERATION: &str = "8.";

/// A prover that refuses to prove.
///
/// The proofless path never needs one: with no contract calls, no shielded
/// coins and no DUST spends, `Transaction::prove` has nothing to prove and
/// this provider is never invoked. It exists so that assumption is enforced
/// rather than assumed — if the transaction ever did carry a proof obligation,
/// this fails loudly instead of silently producing an unprovable transaction.
struct NoProofs;

impl ProvingProvider for NoProofs {
    async fn check(&self, _: &ProofPreimage) -> anyhow::Result<Vec<Option<usize>>> {
        anyhow::bail!("this path builds proofless transactions only")
    }
    async fn prove(self, _: &ProofPreimage, _: Option<Fr>) -> anyhow::Result<Proof> {
        anyhow::bail!("this path builds proofless transactions only")
    }
    fn split(&mut self) -> Self {
        NoProofs
    }
}

/// A signed, sealed transfer plus the numbers deciding whether it can pay for
/// itself.
#[derive(Debug)]
pub struct BuiltTransfer {
    pub sealed: Sealed,
    /// The fee estimate under genesis parameters, in specks.
    pub fee: u128,
    /// The implicit DUST allowance the selected inputs carry, in specks. Also
    /// the registration's `allow_fee_payment` — the node rejects any more.
    pub allowance: u128,
    /// The combined accrual rate of the inputs, in specks per second.
    pub accrual_rate: u128,
    /// Whether this went the expensive, proving route.
    pub proved: bool,
}

/// Turn indexer UTxOs into ledger spends owned by `key`.
fn spends(selected: &[UtxoInfo], key: &LedgerSigningKey) -> Result<Vec<UtxoSpend>> {
    selected
        .iter()
        .map(|utxo| {
            let bytes: [u8; 32] = hex::decode(&utxo.intent_hash)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| {
                    error::rpc_error(format!(
                        "the indexer gave a malformed intent hash: {}",
                        utxo.intent_hash
                    ))
                })?;
            Ok(UtxoSpend {
                intent_hash: IntentHash(HashOutput(bytes)),
                output_no: utxo.output_index,
                owner: key.verifying_key(),
                type_: NIGHT,
                value: utxo.value,
            })
        })
        .collect()
}

/// The outputs a transfer creates: the payment, and change where there is any.
fn outputs(
    dest: UserAddress,
    amount: u128,
    change_to: UserAddress,
    change: u128,
) -> Vec<UtxoOutput> {
    let mut outputs = vec![UtxoOutput {
        owner: dest,
        type_: NIGHT,
        value: amount,
    }];
    if change > 0 {
        outputs.push(UtxoOutput {
            owner: change_to,
            type_: NIGHT,
            value: change,
        });
    }
    outputs
}

/// Build, sign and seal a proofless transfer.
///
/// Every UTxO in `selected` must be owned by `signing_key` and must never have
/// been registered for DUST generation. Pure: no network access.
pub fn build_proofless(
    signing_key: &LedgerSigningKey,
    dust_key: DustSecretKey,
    network_id: &str,
    selected: &[UtxoInfo],
    dest: UserAddress,
    amount: u128,
    unix_now: u64,
) -> Result<BuiltTransfer> {
    let night_vk = signing_key.verifying_key();
    let our_address = UserAddress::from(night_vk.clone());
    let selected_value: u128 = selected.iter().map(|u| u.value).sum();
    let change = selected_value
        .checked_sub(amount)
        .ok_or_else(|| error::internal("the selected inputs do not cover the amount"))?;

    // Declared slightly in the past so the node never sees a ctime from its
    // own future; it costs only a minute of accrual.
    let ctime = Timestamp::from_secs(unix_now.saturating_sub(60));
    let ttl = Timestamp::from_secs(unix_now + TTL_SECONDS);

    // Mirror `generationless_fee_availability` exactly: the node recomputes
    // this and rejects a registration allowing more than it finds.
    let params = &INITIAL_DUST_PARAMETERS;
    let mut allowance = 0u128;
    let mut accrual_rate = 0u128;
    for utxo in selected {
        let Some(utxo_ctime) = utxo.ctime else {
            continue;
        };
        let elapsed = (ctime.to_secs() as i128 - utxo_ctime as i128).max(0) as u128;
        let cap = utxo.value.saturating_mul(params.night_dust_ratio as u128);
        let accrued = elapsed.saturating_mul(
            utxo.value
                .saturating_mul(params.generation_decay_rate as u128),
        );
        allowance = allowance.saturating_add(accrued.min(cap));
        // Saturating like everything around it: `utxo.value` is the indexer's
        // number, and the plain `*` here was the one that could panic.
        accrual_rate = accrual_rate.saturating_add(
            utxo.value
                .saturating_mul(params.generation_decay_rate as u128),
        );
    }

    let mut rng = OsRng;
    let mut intent: Intent<Signature, ProofPreimageMarker, PedersenRandomness, InMemoryDB> =
        Intent::empty(&mut rng, ttl);

    // The ledger rejects offers whose inputs or outputs are not sorted.
    let mut inputs = spends(selected, signing_key)?;
    let mut created = outputs(dest, amount, our_address, change);
    inputs.sort();
    created.sort();
    let signature_count = inputs.len();

    intent.guaranteed_unshielded_offer = Some(Sp::new(UnshieldedOffer {
        inputs: inputs.into(),
        outputs: created.into(),
        signatures: vec![].into(),
    }));
    intent.dust_actions = Some(Sp::new(DustActions {
        spends: vec![].into(),
        registrations: vec![DustRegistration {
            allow_fee_payment: allowance,
            dust_address: Some(Sp::new(DustPublicKey::from(dust_key))),
            night_key: night_vk.clone(),
            signature: None,
        }]
        .into(),
        ctime,
    }));

    let intent = intent
        .sign(
            &mut rng,
            1,
            &vec![signing_key.clone(); signature_count],
            &[],
            std::slice::from_ref(signing_key),
        )
        .map_err(|e| error::internal(format!("signing failed: {e:?}")))?;

    let transaction = Transaction::from_intents(
        network_id,
        storage::storage::HashMap::new().insert(1u16, intent),
    );

    // `false`: skip the time-to-dismiss DoS bound here. The proof- and
    // signature-erased form this estimate runs on overstates processing time
    // (the ledger's own tests note the same), and the node enforces the real
    // bound on the real bytes anyway.
    let fee = transaction
        .erase_proofs()
        .erase_signatures()
        .fees(&INITIAL_PARAMETERS, false)
        .map_err(|e| error::internal(format!("fee estimation failed: {e:?}")))?;

    // "Prove" — a no-op, since nothing here carries a proof — then bind the
    // Pedersen commitments into their sealed form.
    let proven = futures_executor::block_on(transaction.prove(NoProofs, &INITIAL_COST_MODEL))
        .map_err(|e| error::internal(format!("transaction binding failed: {e:?}")))?;

    Ok(BuiltTransfer {
        sealed: proven.seal(OsRng),
        fee,
        allowance,
        accrual_rate,
        proved: false,
    })
}

/// Build a transfer paying its fee with a real, proved DUST spend.
///
/// This is the path for funds at an address already registered for DUST
/// generation. It needs a synced dust state and, on first use, downloads the
/// proving artifacts (~4 MB) from Midnight's parameter server. Proving is
/// local CPU work; no proof server is involved.
///
/// The argument list is long because a proved spend genuinely depends on all
/// of it — the two keys, the replayed state, the network, the coins, the
/// destination, the amount, the clock, and somewhere to report progress — and
/// bundling them into a struct used once would hide that rather than reduce it.
#[allow(clippy::too_many_arguments)]
pub fn build_via_dust(
    signing_key: &LedgerSigningKey,
    dust_key: &DustSecretKey,
    dust_state: &DustLocalState<InMemoryDB>,
    network_id: &str,
    selected: &[UtxoInfo],
    dest: UserAddress,
    amount: u128,
    unix_now: u64,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<BuiltTransfer> {
    let night_vk = signing_key.verifying_key();
    let our_address = UserAddress::from(night_vk.clone());
    let selected_value: u128 = selected.iter().map(|u| u.value).sum();
    let change = selected_value
        .checked_sub(amount)
        .ok_or_else(|| error::internal("the selected inputs do not cover the amount"))?;

    // The node verifies the spend proof against its dust-tree roots *as of the
    // declared ctime* (`root_history.get(ctime)`), while our proof is built
    // against the root after the last replayed dust event. Declaring ctime as
    // that event's block time makes the lookup land on exactly our root, no
    // matter what lands on chain afterwards. The ledger requires ctime within
    // the 3h dust grace period of the block time, so on a long-quiet chain
    // this falls forward to the window's edge.
    let ctime = {
        let synced = dust_state.sync_time.to_secs();
        let floor = unix_now.saturating_sub(3 * 3600 - 600);
        Timestamp::from_secs(synced.max(floor).min(unix_now.saturating_sub(1)))
    };
    let ttl = Timestamp::from_secs(unix_now + TTL_SECONDS);

    let mut inputs = spends(selected, signing_key)?;
    let mut created = outputs(dest, amount, our_address, change);
    inputs.sort();
    created.sort();
    let signature_count = inputs.len();

    // Two passes. The fee depends on the transaction's shape but not on the
    // v_fee number inside it (a fixed-width field), so build once with a
    // placeholder to measure, then rebuild the spend with the real fee plus
    // margin for the chain's drifting fee prices.
    let mut fee_estimate = 0u128;
    let mut sealed = None;

    for pass in 0..2 {
        let v_fee = if pass == 0 {
            1
        } else {
            fee_estimate.saturating_add(fee_estimate / 2)
        };

        // The first dust UTxO that can cover the fee.
        let mut spend = None;
        let mut last_error = None;
        for candidate in dust_state.utxos() {
            match dust_state.spend(dust_key, &candidate, v_fee, ctime) {
                Ok((_, s)) => {
                    spend = Some(s);
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        let Some(spend) = spend else {
            return Err(error::insufficient_funds(format!(
                "no DUST output can cover the {} DUST fee: {}",
                super::dust::format_dust(v_fee),
                last_error
                    .map(|e| format!("{e:?}"))
                    .unwrap_or_else(|| "this wallet holds no DUST".into()),
            )));
        };

        let mut rng = OsRng;
        let mut intent: Intent<Signature, ProofPreimageMarker, PedersenRandomness, InMemoryDB> =
            Intent::empty(&mut rng, ttl);
        intent.guaranteed_unshielded_offer = Some(Sp::new(UnshieldedOffer {
            inputs: inputs.clone().into(),
            outputs: created.clone().into(),
            signatures: vec![].into(),
        }));
        intent.dust_actions = Some(Sp::new(DustActions {
            spends: vec![spend].into(),
            registrations: vec![].into(),
            ctime,
        }));
        let intent = intent
            .sign(
                &mut rng,
                1,
                &vec![signing_key.clone(); signature_count],
                &[],
                &[],
            )
            .map_err(|e| error::internal(format!("signing failed: {e:?}")))?;
        let transaction = Transaction::from_intents(
            network_id,
            storage::storage::HashMap::new().insert(1u16, intent),
        );

        if pass == 0 {
            fee_estimate = transaction
                .erase_proofs()
                .erase_signatures()
                .fees(&INITIAL_PARAMETERS, false)
                .map_err(|e| error::internal(format!("fee estimation failed: {e:?}")))?;
            continue;
        }

        progress(&format!(
            "proving the DUST spend (fee {} DUST) — the first run downloads ~4 MB of parameters",
            super::dust::format_dust(v_fee)
        ));

        // Resolve the dust circuit artifacts (cached under
        // ~/.cache/midnight/zk-params, fetched from srs.midnight.network on
        // first use) and run the zkir prover in process.
        let resolver = Resolver::new(
            ZswapResolver(
                MidnightDataProvider::new(
                    FetchMode::OnDemand,
                    OutputMode::Log,
                    ZSWAP_EXPECTED_FILES.to_owned(),
                )
                .map_err(|e| error::internal(format!("the parameter provider failed: {e}")))?,
            ),
            DustResolver(
                MidnightDataProvider::new(
                    FetchMode::OnDemand,
                    OutputMode::Log,
                    DUST_EXPECTED_FILES.to_owned(),
                )
                .map_err(|e| error::internal(format!("the parameter provider failed: {e}")))?,
            ),
            Box::new(|_: KeyLocation| Box::pin(std::future::ready(Ok(None)))),
        );
        let prover = LocalProvingProvider {
            rng: OsRng,
            params: &resolver,
            resolver: &resolver,
        };
        let proven = futures_executor::block_on(transaction.prove(prover, &INITIAL_COST_MODEL))
            .map_err(|e| error::internal(format!("proof generation failed: {e:?}")))?;
        sealed = Some(proven.seal(OsRng));
    }

    Ok(BuiltTransfer {
        sealed: sealed.expect("the second pass always seals"),
        fee: fee_estimate,
        allowance: fee_estimate.saturating_add(fee_estimate / 2),
        accrual_rate: 0,
        proved: true,
    })
}

/// Wrap serialized transaction bytes as a bare extrinsic for pallet 5
/// (`Midnight`), call 0 (`send_mn_transaction`), ready for
/// `author_submitExtrinsic`:
/// `compact(len) ‖ 0x05 (bare, format v5) ‖ pallet ‖ call ‖ SCALE(Vec<u8>)`.
pub fn wrap_extrinsic(tx_bytes: &[u8]) -> Vec<u8> {
    let mut call = vec![5u8, 0u8];
    call.extend_from_slice(&scale_compact(tx_bytes.len() as u64));
    call.extend_from_slice(tx_bytes);
    let mut extrinsic = vec![5u8];
    extrinsic.extend_from_slice(&call);
    let mut wire = scale_compact(extrinsic.len() as u64);
    wire.extend_from_slice(&extrinsic);
    wire
}

/// SCALE compact encoding of a length.
fn scale_compact(n: u64) -> Vec<u8> {
    match n {
        0..=0x3f => vec![(n as u8) << 2],
        0x40..=0x3fff => (((n as u16) << 2) | 0b01).to_le_bytes().to_vec(),
        0x4000..=0x3fff_ffff => (((n as u32) << 2) | 0b10).to_le_bytes().to_vec(),
        _ => {
            let mut out = vec![0b11u8]; // 4-byte big mode
            out.extend_from_slice(&(n as u32).to_le_bytes());
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_ledger::structure::LedgerState;
    use midnight_ledger::verify::WellFormedStrictness;
    use serialize::{tagged_deserialize, tagged_serialize};

    const NOW: u64 = 1_787_500_000;

    fn utxo(hash_byte: u8, output_index: u32, value: u128, age_secs: i64) -> UtxoInfo {
        UtxoInfo {
            intent_hash: hex::encode([hash_byte; 32]),
            output_index,
            token_type: super::super::indexer::NIGHT_TOKEN_TYPE.into(),
            value,
            ctime: Some(NOW as i64 - age_secs),
            registered_for_dust: false,
        }
    }

    fn keys() -> (LedgerSigningKey, DustSecretKey) {
        (
            LedgerSigningKey::from_bytes(&[7u8; 32]).unwrap(),
            DustSecretKey::derive_secret_key(&[8u8; 32]),
        )
    }

    fn build() -> BuiltTransfer {
        let (signing_key, dust_key) = keys();
        // Deliberately unsorted (the 0xEE hash before the 0x11 one) to prove
        // the builder sorts; two inputs, an amount spanning both, change due.
        let utxos = [
            utxo(0xEE, 3, 40_000_000, 7200),
            utxo(0x11, 0, 10_000_000, 3600),
        ];
        build_proofless(
            &signing_key,
            dust_key,
            "preview",
            &utxos,
            UserAddress(HashOutput([0xAB; 32])),
            45_000_000,
            NOW,
        )
        .unwrap()
    }

    /// The strongest check available offline: the ledger's own validator, with
    /// real BIP-340 signature verification, run against what this builds.
    /// Balancing is off because the fabricated inputs exist in no ledger state.
    #[test]
    fn a_built_transfer_is_well_formed_per_the_ledger() {
        let built = build();
        let state: LedgerState<InMemoryDB> = LedgerState::new("preview");
        let mut strictness = WellFormedStrictness::default();
        strictness.enforce_balancing = false;
        built
            .sealed
            .well_formed(&state, strictness, Timestamp::from_secs(NOW))
            .expect("the ledger must accept what this wallet builds");
    }

    /// A transaction is bound to the network it was built for, so the same
    /// bytes cannot be replayed onto another Midnight chain.
    #[test]
    fn a_transfer_built_for_one_network_is_rejected_on_another() {
        let built = build();
        let mut strictness = WellFormedStrictness::default();
        strictness.enforce_balancing = false;
        let wrong: LedgerState<InMemoryDB> = LedgerState::new("devnet");
        assert!(built
            .sealed
            .well_formed(&wrong, strictness, Timestamp::from_secs(NOW))
            .is_err());
    }

    #[test]
    fn a_built_transfer_survives_a_serialization_round_trip() {
        let built = build();
        let mut bytes = Vec::new();
        tagged_serialize(&built.sealed, &mut bytes).unwrap();
        let back: Sealed = tagged_deserialize(&mut &bytes[..]).unwrap();
        assert_eq!(
            back.transaction_hash(),
            built.sealed.transaction_hash(),
            "a round-tripped transaction must hash identically"
        );
    }

    #[test]
    fn inputs_and_outputs_come_out_sorted_with_the_change_kept() {
        let built = build();
        let (_, intent) = built.sealed.intents().next().unwrap();

        let outputs = intent.guaranteed_outputs();
        assert_eq!(outputs.len(), 2, "the payment and the change");
        let mut values: Vec<u128> = outputs.iter().map(|o| o.value).collect();
        values.sort();
        assert_eq!(values, vec![5_000_000, 45_000_000]);

        let sorted = {
            let mut s = outputs.clone();
            s.sort();
            s
        };
        assert_eq!(outputs, sorted, "the ledger rejects unsorted outputs");

        let inputs = intent.guaranteed_inputs();
        let sorted_inputs = {
            let mut s = inputs.clone();
            s.sort();
            s
        };
        assert_eq!(inputs, sorted_inputs, "the ledger rejects unsorted inputs");
    }

    /// The node recomputes this exact number and rejects a registration that
    /// allows more, so the arithmetic has to match the ledger's.
    #[test]
    fn the_fee_allowance_mirrors_the_ledgers_dust_arithmetic() {
        let built = build();
        let rate = INITIAL_DUST_PARAMETERS.generation_decay_rate as u128;
        // Both inputs are below the cap; each accrues for its age less the
        // 60-second safety margin the builder declares.
        let expected =
            (7200 - 60) as u128 * 40_000_000 * rate + (3600 - 60) as u128 * 10_000_000 * rate;
        assert_eq!(built.allowance, expected);
        assert!(built.fee > 0);
        assert!(!built.proved, "the cheap path proves nothing");
    }

    #[test]
    fn a_transfer_that_consumes_its_inputs_exactly_has_no_change_output() {
        let (signing_key, dust_key) = keys();
        let built = build_proofless(
            &signing_key,
            dust_key,
            "preview",
            &[utxo(0x11, 0, 10_000_000, 3600)],
            UserAddress(HashOutput([0xAB; 32])),
            10_000_000,
            NOW,
        )
        .unwrap();
        let (_, intent) = built.sealed.intents().next().unwrap();
        assert_eq!(intent.guaranteed_outputs().len(), 1);
    }

    #[test]
    fn spending_more_than_was_selected_is_refused() {
        let (signing_key, dust_key) = keys();
        let err = build_proofless(
            &signing_key,
            dust_key,
            "preview",
            &[utxo(0x11, 0, 1_000, 3600)],
            UserAddress(HashOutput([0xAB; 32])),
            5_000,
            NOW,
        )
        .unwrap_err();
        assert!(err.message.contains("do not cover"), "{}", err.message);
    }

    #[test]
    fn a_malformed_intent_hash_from_the_indexer_is_reported() {
        let (signing_key, dust_key) = keys();
        let mut bad = utxo(0x11, 0, 10_000_000, 3600);
        bad.intent_hash = "not hex".into();
        let err = build_proofless(
            &signing_key,
            dust_key,
            "preview",
            &[bad],
            UserAddress(HashOutput([0xAB; 32])),
            1_000,
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code, error::Code::RpcError);
        assert!(err.message.contains("intent hash"), "{}", err.message);
    }

    #[test]
    fn the_extrinsic_envelope_matches_the_live_chain_format() {
        let payload = vec![0xCD; 100];
        let wire = wrap_extrinsic(&payload);
        let inner_len = 1 + 2 + 2 + 100; // version + pallet/call + compact(100) + payload
        let mut expected = scale_compact(inner_len as u64);
        expected.extend_from_slice(&[0x05, 0x05, 0x00]); // bare v5, pallet 5, call 0
        expected.extend_from_slice(&scale_compact(100));
        expected.extend_from_slice(&payload);
        assert_eq!(wire, expected);
    }

    #[test]
    fn compact_encoding_matches_scale() {
        assert_eq!(scale_compact(10), vec![40]);
        assert_eq!(scale_compact(63), vec![0xfc]);
        assert_eq!(scale_compact(64), vec![0x01, 0x01]);
        assert_eq!(scale_compact(365), vec![0xb5, 0x05]); // observed on chain
        assert_eq!(scale_compact(16384), vec![0x02, 0x00, 0x01, 0x00]);
    }
}
