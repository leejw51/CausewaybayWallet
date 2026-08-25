//! DUST wallet state: replaying the chain's dust ledger events.
//!
//! Spending DUST needs merkle paths into two on-chain trees (generation and
//! commitment). The ledger crate ships the exact replay routine its own tests
//! use — `DustLocalState::replay_events` — and the indexer's global
//! `dustLedgerEvents` stream serves precisely the events it consumes, as
//! hex-encoded tagged-serialized `Event`s. So syncing is: stream, decode,
//! replay. Foreign leaves are collapsed automatically; only this wallet's own
//! outputs are kept in full.
//!
//! The stream is chain-global — on preview that is well over a hundred
//! thousand events — so a cold sync takes minutes and a warm one seconds. The
//! replayed state is cached under the wallet home, keyed by network and
//! address, which is the difference between the two.

use std::path::{Path, PathBuf};
use std::time::Duration;

use midnight_ledger::dust::{DustLocalState, DustSecretKey, INITIAL_DUST_PARAMETERS};
use midnight_ledger::events::Event;
use serde_json::json;
use serialize::{tagged_deserialize, tagged_serialize};
use storage::db::InMemoryDB;

use crate::error::{self, Result};

use super::indexer::Indexer;

/// How long a full dust replay may take. A cold sync on preview is minutes,
/// not seconds, so this is generous on purpose.
const SYNC_TIMEOUT: Duration = Duration::from_secs(600);

/// How many events to buffer before folding them into the state.
const BATCH: usize = 2048;

pub struct DustWallet {
    pub state: DustLocalState<InMemoryDB>,
    /// The id of the last ledger event replayed into `state`.
    pub last_event_id: u64,
    /// How many events this sync replayed.
    pub events_replayed: u64,
}

/// Where a wallet's dust state is cached: keyed by network and address, so
/// distinct wallets and indices never collide.
pub fn cache_path(cache_dir: &Path, network: &str, address: &str) -> PathBuf {
    // The address is bech32m — lowercase alphanumerics and one separator — so
    // it is already safe as a file name, but the hash keeps the name short and
    // survives any future address form.
    let digest = sha2::Sha256::digest_hex(address);
    cache_dir
        .join("midnight")
        .join(format!("{network}-{digest}.dust"))
}

/// A tiny helper so the hashing above reads as one thing.
trait DigestHex {
    fn digest_hex(input: &str) -> String;
}

impl DigestHex for sha2::Sha256 {
    fn digest_hex(input: &str) -> String {
        use sha2::Digest;
        hex::encode(&<sha2::Sha256 as Digest>::digest(input.as_bytes())[..8])
    }
}

/// Replay the dust event stream into a local state for `secret_key`, resuming
/// from the cache where possible and updating it after.
pub async fn sync(
    indexer_url: &str,
    secret_key: &DustSecretKey,
    cache_file: Option<&Path>,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<DustWallet> {
    const QUERY: &str = r#"
subscription($id: Int) {
  dustLedgerEvents(id: $id) {
    id
    raw
    maxId
  }
}"#;

    let (mut state, start_id) = load_cache(cache_file, progress);
    let mut last_id = start_id;
    let mut batch: Vec<Event<InMemoryDB>> = Vec::with_capacity(BATCH);
    let mut total = 0u64;
    let mut last_reported = 0u64;

    // Subscribing at the last known id re-delivers that event immediately, so
    // an already-caught-up wallet completes at once instead of hanging until
    // the chain's next event. The replay below skips the duplicate.
    let from = if start_id > 0 {
        json!({"id": start_id})
    } else {
        json!({})
    };

    let indexer = Indexer::new(indexer_url);
    let caught_up = indexer
        .subscribe(QUERY, from, "dustLedgerEvents", SYNC_TIMEOUT, |item| {
            let id = item
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| error::rpc_error(format!("a dust event has no id: {item}")))?;
            let max_id = item
                .get("maxId")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if id <= start_id {
                return Ok(id >= max_id);
            }
            let raw = item
                .get("raw")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let bytes = hex::decode(raw)
                .map_err(|e| error::rpc_error(format!("dust event {id} is not hex: {e}")))?;
            let event: Event<InMemoryDB> = tagged_deserialize(&mut &bytes[..])
                .map_err(|e| error::rpc_error(format!("dust event {id} is undecodable: {e}")))?;
            batch.push(event);
            last_id = id;
            total += 1;

            if batch.len() >= BATCH {
                state = replay(&state, secret_key, &mut batch)?;
                // Roughly every batch, so a long sync visibly moves rather
                // than sitting silent for minutes.
                if total - last_reported >= BATCH as u64 {
                    last_reported = total;
                    progress(&format!("dust sync: event {id} of {max_id}"));
                }
            }
            Ok(id >= max_id)
        })
        .await?;

    if !batch.is_empty() {
        state = replay(&state, secret_key, &mut batch)?;
    }
    if !caught_up {
        return Err(error::rpc_error(format!(
            "the dust event replay did not reach the chain tip within {SYNC_TIMEOUT:?}; \
             a transaction built on a partial dust state would be rejected"
        )));
    }
    progress(&format!("dust sync: {total} new events replayed"));

    if let Some(path) = cache_file {
        save_cache(path, last_id, &state, progress);
    }

    Ok(DustWallet {
        state,
        last_event_id: last_id,
        events_replayed: total,
    })
}

/// Fold a batch of events into the state.
///
/// The secret key is what decides which outputs this wallet keeps in full and
/// which are collapsed as foreign leaves, so it must be the real one — replay
/// under any other key produces a state that cannot spend a thing.
fn replay(
    state: &DustLocalState<InMemoryDB>,
    secret_key: &DustSecretKey,
    batch: &mut Vec<Event<InMemoryDB>>,
) -> Result<DustLocalState<InMemoryDB>> {
    let next = state
        .replay_events(secret_key, batch.iter())
        .map_err(|e| error::rpc_error(format!("dust event replay failed: {e:?}")))?;
    batch.clear();
    Ok(next)
}

/// Load a cached state, falling back to an empty one.
///
/// A stale or corrupt cache costs only a full resync, so it is discarded
/// quietly rather than reported as a failure.
fn load_cache(
    cache_file: Option<&Path>,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> (DustLocalState<InMemoryDB>, u64) {
    let empty = || (DustLocalState::new(INITIAL_DUST_PARAMETERS), 0);
    let Some(path) = cache_file else {
        return empty();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return empty();
    };
    if bytes.len() <= 8 {
        return empty();
    }
    let id = u64::from_le_bytes(bytes[..8].try_into().expect("checked length"));
    match tagged_deserialize::<DustLocalState<InMemoryDB>>(&mut &bytes[8..]) {
        Ok(state) => {
            progress(&format!("dust sync: resuming from cached event {id}"));
            (state, id)
        }
        Err(_) => empty(),
    }
}

fn save_cache(
    path: &Path,
    last_id: u64,
    state: &DustLocalState<InMemoryDB>,
    progress: &(dyn Fn(&str) + Send + Sync),
) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut bytes = last_id.to_le_bytes().to_vec();
    if tagged_serialize(state, &mut bytes).is_ok() {
        // A cache that cannot be written costs a slow next sync and nothing
        // else, so it is mentioned rather than raised.
        if let Err(e) = crate::paths::write_private(path, &bytes) {
            progress(&format!(
                "could not cache the dust state at {}: {e}",
                path.display()
            ));
        }
    }
}

/// The unit fees are counted in: DUST, at 15 decimal places ("specks").
///
/// Not the unit a transfer moves — that is NIGHT, at six — which is why a
/// prepared Midnight transfer carries its fee unit explicitly.
pub const DUST: crate::chain::Amount = crate::chain::Amount::new(15, "DUST");

/// DUST is quoted with 15 decimal places ("specks").
pub fn format_dust(specks: u128) -> String {
    crate::chain::amount::format_units(specks, 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dust_is_quoted_with_fifteen_decimal_places() {
        assert_eq!(format_dust(1_000_000_000_000_000), "1");
        assert_eq!(format_dust(1), "0.000000000000001");
        assert_eq!(format_dust(0), "0");
        assert_eq!(format_dust(1_500_000_000_000_000), "1.5");
    }

    #[test]
    fn the_cache_path_separates_networks_and_addresses() {
        let dir = Path::new("/tmp/wallet");
        let preview = cache_path(dir, "preview", "mn_addr_preview1aaa");
        let devnet = cache_path(dir, "devnet", "mn_addr_preview1aaa");
        let other = cache_path(dir, "preview", "mn_addr_preview1bbb");

        // Three different wallets, three different files: a shared one would
        // replay another address's dust state into this one.
        assert_ne!(preview, devnet);
        assert_ne!(preview, other);
        assert!(preview.starts_with("/tmp/wallet/midnight"));
        assert!(preview.to_string_lossy().contains("preview-"));
        assert!(preview.extension().unwrap() == "dust");
    }

    #[test]
    fn the_cache_file_name_stays_short_whatever_the_address() {
        let long = format!("mn_addr_preview1{}", "q".repeat(400));
        let path = cache_path(Path::new("/tmp"), "preview", &long);
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.len() < 64, "{name} is too long to be a file name");
    }

    #[test]
    fn an_absent_or_corrupt_cache_falls_back_to_an_empty_state() {
        let noted = std::sync::Mutex::new(Vec::<String>::new());
        let note = |m: &str| noted.lock().unwrap().push(m.to_string());

        // No file at all.
        let (_, id) = load_cache(None, &note);
        assert_eq!(id, 0);

        // A file that is too short to hold an id.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.dust");
        std::fs::write(&path, b"1234").unwrap();
        let (_, id) = load_cache(Some(&path), &note);
        assert_eq!(id, 0);

        // A file with a plausible id and garbage after it.
        let mut bytes = 42u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"not a serialized dust state");
        std::fs::write(&path, &bytes).unwrap();
        let (_, id) = load_cache(Some(&path), &note);
        assert_eq!(
            id, 0,
            "a corrupt cache must resync, not resume from nowhere"
        );

        // None of that is worth telling the user about.
        assert!(noted.lock().unwrap().is_empty());
    }
}
