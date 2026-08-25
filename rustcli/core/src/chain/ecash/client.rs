//! Chain access through Chronik, Bitcoin ABC's own indexer.
//!
//! Chosen because it is the only key-less eCash service that serves both the
//! mainnet and the testnet, and because it indexes by *script* rather than by
//! address — so the wallet asks about the twenty bytes it derived rather than
//! about a string it hopes the indexer spells the same way it does.
//!
//! The wire format is protobuf and lives in [`super::chronik`].

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::chain::http;
use crate::chain::{
    Balance, ChainClient, ClientConfig, PreparedTransfer, TransactionStatus, TransferReceipt,
    TransferRequest,
};
use crate::error::{self, Result};
use crate::network::Network;

use super::address::{Address, AddressKind, EcashNetwork};
use super::chronik::{self, ScriptUtxos};
use super::keys::EcashAccount;
use super::tx::{self, Builder, Utxo};

/// What Chronik calls the script kinds it indexes by.
fn script_type(kind: AddressKind) -> &'static str {
    match kind {
        AddressKind::P2pkh => "p2pkh",
        AddressKind::P2sh => "p2sh",
    }
}

/// Why the outputs this wallet will not spend are being left alone.
///
/// A predicate rather than a sentence, so the same words serve the note on a
/// transfer that went through and the error on one that could not — and it
/// agrees with its subject, because "1 unspent output ... carry an eToken" is
/// the sort of thing that makes a user distrust the number beside it.
fn held_back_reason(unspent: &ScriptUtxos, tip: u64, plural: bool) -> &'static str {
    let spendable = unspent.spendable(tip);
    let held: Vec<&chronik::Utxo> = unspent
        .utxos
        .iter()
        .filter(|utxo| !spendable.contains(utxo))
        .collect();
    let any_token = held.iter().any(|utxo| utxo.has_token);
    // Being held back for any other reason means being an immature coinbase;
    // those are the only two filters.
    let any_immature = held.iter().any(|utxo| !utxo.has_token);
    match (any_token, any_immature, plural) {
        (true, false, false) => "carries an eToken, which spending it as XEC would destroy",
        (true, false, true) => "carry an eToken, which spending them as XEC would destroy",
        (false, true, false) => "is newly mined and not yet spendable",
        (false, true, true) => "are newly mined and not yet spendable",
        (_, _, false) => "either carries an eToken or is not yet mature",
        (_, _, true) => "either carry an eToken or is not yet mature",
    }
}

pub struct EcashClient {
    network: Network,
    endpoint: String,
}

impl EcashClient {
    pub fn new(config: &ClientConfig) -> Self {
        EcashClient {
            network: config.network,
            endpoint: config.endpoint.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.endpoint)
    }

    /// GET one protobuf document, turning Chronik's own error into ours.
    async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let url = self.url(path);
        let reply = http::get_binary(&url).await?;
        if reply.is_success() {
            return Ok(Some(reply.body));
        }
        // 404 is an answer, not a failure: an address the chain has never
        // seen and a transaction it has never heard of both come back this
        // way, and both mean "nothing", not "something went wrong".
        if reply.status == 404 {
            return Ok(None);
        }
        Err(self.reply_error(&url, &reply))
    }

    fn reply_error(&self, url: &str, reply: &http::BinaryReply) -> crate::error::Error {
        match chronik::error_message(&reply.body) {
            Some(message) => {
                if message.to_lowercase().contains("insufficient") {
                    return error::insufficient_funds(message);
                }
                error::rpc_error(format!(
                    "{} refused the request: {message}",
                    self.network.name
                ))
            }
            None => error::rpc_error(format!("{url} returned HTTP {}", reply.status)),
        }
    }

    /// Everything unspent at one address's script.
    ///
    /// Both address kinds are read, though only the first can be spent from:
    /// `balance --address` is a question about somewhere else, and refusing to
    /// answer it for a script-hash address would be refusing to look up an
    /// address the wallet is perfectly capable of *paying*.
    async fn utxos(&self, address: &Address) -> Result<ScriptUtxos> {
        let path = format!(
            "script/{}/{}/utxos",
            script_type(address.kind),
            hex::encode(address.hash)
        );
        match self.get(&path).await? {
            Some(body) => ScriptUtxos::decode(&body),
            // An address with nothing at it: Chronik answers 404 rather than
            // an empty list.
            None => Ok(ScriptUtxos::default()),
        }
    }

    async fn tip_height(&self) -> Result<u64> {
        let body = self.get("blockchain-info").await?.ok_or_else(|| {
            error::rpc_error("the indexer does not know where the chain's tip is")
        })?;
        Ok(chronik::BlockchainInfo::decode(&body)?.tip_height)
    }
}

#[async_trait]
impl ChainClient for EcashClient {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// What the address holds, tokens included.
    ///
    /// Deliberately the whole total rather than only the part this wallet
    /// would spend: a balance is what is *there*, and one that quietly
    /// omitted the satoshis riding under an eToken would not add up against
    /// any explorer. What the wallet will spend is a separate question, and
    /// `send` is where it is asked.
    async fn balance(&self, address: &str) -> Result<Balance> {
        let parsed = Address::parse(address)?;
        Ok(Balance::native(self.utxos(&parsed).await?.total()))
    }

    async fn prepare_transfer(
        &self,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer> {
        let account = EcashAccount::from_secret(signer_secret)?;
        let ecash_network = EcashNetwork::of(&self.network);
        let source = account.address(ecash_network);
        let destination = Address::parse(&request.to)?;

        if destination.network != ecash_network {
            return Err(error::invalid_address(format!(
                "that address is an {} address but this wallet is on {}; the funds \
                 would go to a chain nobody is watching",
                destination.network.prefix(),
                self.network.name
            )));
        }
        if destination == source {
            return Err(error::usage(format!(
                "the recipient is the sending account ({source}); a transfer to \
                 itself moves nothing and still pays the fee"
            )));
        }
        let sats = u64::try_from(request.amount)
            .map_err(|_| error::invalid_amount("that amount does not fit in a u64 of satoshis"))?;

        // Neither read depends on the other, so they go out together.
        let (unspent, tip) = tokio::try_join!(self.utxos(&source), self.tip_height())?;
        let spendable: Vec<Utxo> = unspent
            .spendable(tip)
            .into_iter()
            .map(|u| Utxo {
                txid: u.txid,
                index: u.out_idx,
                sats: u.sats,
            })
            .collect();

        // The case that reads as a contradiction otherwise: `balance` says
        // 5.46 XEC and `send` says the address holds nothing to spend. Both
        // are true — the satoshis are riding under an eToken — and saying only
        // the second is how a user concludes the wallet has lost their money.
        if spendable.is_empty() && !unspent.utxos.is_empty() {
            let held = unspent.utxos.len();
            let plural = held != 1;
            let subject = if plural {
                format!("all {held} of its unspent outputs")
            } else {
                "its one unspent output".to_string()
            };
            return Err(error::insufficient_funds(format!(
                "this address holds {}, but none of it can be spent: {subject} {}",
                self.network.units().format_with_symbol(unspent.total()),
                held_back_reason(&unspent, tip, plural),
            )));
        }

        // Said plainly rather than left to show up as "insufficient funds":
        // the balance the user was just shown includes these, and a wallet
        // that refuses to spend part of a balance owes an explanation.
        let held_back = unspent.utxos.len() - spendable.len();
        let note = (held_back > 0).then(|| {
            let plural = held_back != 1;
            format!(
                ", leaving {held_back} unspent output{} untouched because {} {}",
                if plural { "s" } else { "" },
                if plural { "they" } else { "it" },
                held_back_reason(&unspent, tip, plural),
            )
        });

        let signed = Builder::new()
            .limit_fee(self.network, request.fee_ceiling(&self.network))
            .build_transfer(&spendable, &destination, sats, &source, &account)?;

        let fee = u128::try_from(signed.fee())
            .map_err(|_| error::internal("a transaction was built that pays no fee"))?;
        let raw = signed.serialize();
        Ok(PreparedTransfer {
            id: signed.txid(),
            from: source.to_cashaddr(),
            to: request.to.clone(),
            amount: request.amount,
            fee,
            // Deliberately not the satoshi-per-byte rate, though eCash has
            // one. The history has a single column for this and for a whole
            // fee, so a chain that reports a rate reports it *instead of* what
            // was paid — and "1" is not a useful record of a transfer. The
            // rate is in `detail` below and in `fee_quote`, where it costs
            // nothing.
            fee_rate: None,
            fee_unit: None,
            amount_unit: None,
            token: None,
            nonce: None,
            gas_limit: None,
            network: self.network,
            note,
            detail: json!({
                "fee_sats": fee,
                "fee_per_byte": tx::FEE_PER_BYTE,
                "size_bytes": raw.len(),
                "inputs": signed.inputs.len(),
                "outputs": signed.outputs.len(),
                "change_sats": signed.outputs.get(1).map(|o| o.sats),
                "unspent_outputs_held_back": held_back,
            }),
            signed: raw,
        })
    }

    async fn submit(&self, prepared: &PreparedTransfer) -> Result<TransferReceipt> {
        let url = self.url("broadcast-tx");
        let reply = http::post_binary(
            &url,
            "application/x-protobuf",
            chronik::broadcast_request(&prepared.signed),
        )
        .await?;
        if !reply.is_success() {
            return Err(self.reply_error(&url, &reply));
        }
        // The locally computed id is authoritative, as it is on every other
        // chain here: it is the hash of the bytes that were signed, and no
        // endpoint can change what those hash to. Chronik's answer is kept
        // beside it only when the two disagree, which is worth seeing.
        let returned = chronik::broadcast_response(&reply.body)?;
        Ok(TransferReceipt {
            secondary_id: returned.filter(|id| *id != prepared.id),
            id: prepared.id.clone(),
        })
    }

    async fn transaction(&self, id: &str) -> Result<Option<TransactionStatus>> {
        // Refused here rather than sent on, because a path segment that is not
        // a hash is a request for some other Chronik route entirely.
        if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(error::usage(format!(
                "'{id}' is not an eCash transaction id; one is 64 hex characters"
            )));
        }
        // Chronik indexes ids in lower case, and an explorer will happily hand
        // someone the other one.
        let Some(body) = self.get(&format!("tx/{}", id.to_lowercase())).await? else {
            return Ok(None);
        };
        let decoded = chronik::Tx::decode(&body)?;
        Ok(Some(TransactionStatus {
            id: decoded.txid.clone(),
            // Chronik answers about mempool transactions too, so being known
            // is not the same as having confirmed.
            status: if decoded.block_height.is_some() {
                "confirmed"
            } else {
                "pending"
            }
            .into(),
            block: decoded.block_height,
            fee: decoded.fee,
            gas_used: None,
            raw: json!({
                "txid": decoded.txid,
                "block_height": decoded.block_height,
                "size": decoded.size,
                "is_final": decoded.is_final,
                "fee_sats": decoded.fee,
            }),
        }))
    }

    /// Satoshis per byte.
    ///
    /// A constant rather than an estimate: eCash's `minRelayTxFee` is 1000
    /// satoshis per kilobyte, blocks are nowhere near full, and there is no
    /// fee market to read. Quoting a number an endpoint made up would be
    /// worse than quoting the one the protocol states.
    async fn fee_quote(&self) -> Result<Option<u128>> {
        Ok(Some(u128::from(tx::FEE_PER_BYTE)))
    }

    async fn chain_info(&self) -> Result<Value> {
        let body = self.get("blockchain-info").await?.ok_or_else(|| {
            error::rpc_error("the indexer does not know where the chain's tip is")
        })?;
        let info = chronik::BlockchainInfo::decode(&body)?;
        Ok(json!({
            "network": self.network.key,
            "block_height": info.tip_height,
            "tip_hash": info.tip_hash,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Seed;
    use crate::network::{ECASH_MAINNET, ECASH_TESTNET};

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn account() -> EcashAccount {
        EcashAccount::from_seed(&Seed::new(PHRASE, "").unwrap(), 0).unwrap()
    }

    fn prepare(network: Network, to: &str) -> crate::error::Error {
        let client = EcashClient::new(&ClientConfig::bare(network));
        let request = TransferRequest::new(to, 10_000);
        crate::runtime::block_on(client.prepare_transfer(&account().secret_hex(), &request))
            .unwrap()
            .unwrap_err()
    }

    #[test]
    fn the_endpoint_is_kept_without_a_trailing_slash() {
        let mut config = ClientConfig::bare(ECASH_MAINNET);
        config.endpoint = "https://chronik.e.cash/".into();
        let client = EcashClient::new(&config);
        assert_eq!(client.endpoint(), "https://chronik.e.cash");
        assert_eq!(
            client.url("blockchain-info"),
            "https://chronik.e.cash/blockchain-info"
        );
    }

    /// The costliest mistake this wallet can make on eCash, refused offline.
    #[test]
    fn a_mainnet_recipient_on_a_testnet_wallet_is_refused_before_any_node_is_asked() {
        let mainnet = account().address(EcashNetwork::Mainnet).to_cashaddr();
        let err = prepare(ECASH_TESTNET, &mainnet);
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(
            err.message.contains("nobody is watching"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_testnet_recipient_on_a_mainnet_wallet_is_refused_too() {
        let testnet = account().address(EcashNetwork::Testnet).to_cashaddr();
        let err = prepare(ECASH_MAINNET, &testnet);
        assert_eq!(err.code, error::Code::InvalidAddress);
    }

    #[test]
    fn sending_to_yourself_is_refused_before_any_node_is_asked() {
        let own = account().address(EcashNetwork::Testnet).to_cashaddr();
        let err = prepare(ECASH_TESTNET, &own);
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("itself"), "{}", err.message);
    }

    #[test]
    fn a_transaction_id_that_is_not_one_never_reaches_the_indexer() {
        let client = EcashClient::new(&ClientConfig::bare(ECASH_MAINNET));
        for bad in ["", "0x1234", "../blockchain-info", &"z".repeat(64)] {
            let err = crate::runtime::block_on(client.transaction(bad))
                .unwrap()
                .unwrap_err();
            assert_eq!(err.code, error::Code::Usage, "{bad}");
        }
    }

    /// Chronik puts the reason in the body of a 400, and dropping it would
    /// turn "dust" into "HTTP 400".
    #[test]
    fn a_refusal_keeps_the_reason_the_indexer_gave() {
        let client = EcashClient::new(&ClientConfig::bare(ECASH_MAINNET));
        let mut body = Vec::new();
        chronik::write_bytes_field(
            &mut body,
            2,
            b"400: Broadcast failed: Transaction rejected by mempool: dust",
        );
        let err = client.reply_error(
            "https://chronik.e.cash/broadcast-tx",
            &http::BinaryReply { status: 400, body },
        );
        assert_eq!(err.code, error::Code::RpcError);
        assert!(err.message.contains("dust"), "{}", err.message);

        // A body with nothing readable in it still names the status.
        let bare = client.reply_error(
            "https://chronik.e.cash/broadcast-tx",
            &http::BinaryReply {
                status: 502,
                body: Vec::new(),
            },
        );
        assert!(bare.message.contains("502"), "{}", bare.message);
    }
}
