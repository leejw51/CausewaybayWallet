//! The Midnight client: indexer for reads, node RPC for submission.

use std::time::Duration;

use async_trait::async_trait;
use base_crypto::hash::HashOutput;
use base_crypto::signatures::SigningKey as LedgerSigningKey;
use coin_structure::coin::UserAddress;
use midnight_ledger::dust::DustSecretKey;
use serde_json::{json, Value};
use serialize::tagged_serialize;

use crate::chain::http;
use crate::chain::{
    self, Balance, ChainClient, ClientConfig, PreparedTransfer, TransactionStatus, TransferReceipt,
    TransferRequest,
};
use crate::error::{self, Result};
use crate::runtime;

use super::address::{MidnightAddress, NetworkId, TYPE_UNSHIELDED};
use super::dust::{self, DUST};
use super::indexer::{Indexer, UtxoInfo, NIGHT_TOKEN_TYPE};
use super::keys::MidnightAccount;
use super::send;

pub struct MidnightClient {
    config: ClientConfig,
    indexer: Indexer,
    network_id: NetworkId,
}

impl MidnightClient {
    pub fn new(config: &ClientConfig) -> Result<Self> {
        Ok(MidnightClient {
            indexer: Indexer::new(&config.endpoint),
            network_id: NetworkId::of(&config.network)?,
            config: config.clone(),
        })
    }

    fn node(&self) -> &str {
        &self.config.submit_endpoint
    }

    /// Refuse early and clearly on a node running a different ledger
    /// generation, rather than letting it reject opaque bytes.
    async fn check_ledger_generation(&self) -> Result<()> {
        let reported = http::json_rpc(self.node(), "midnight_ledgerVersion", json!([])).await?;
        let reported = reported.as_str().unwrap_or_default();
        if !reported
            .trim_start_matches('=')
            .starts_with(send::LEDGER_GENERATION)
        {
            return Err(error::rpc_error(format!(
                "the {} node runs ledger {reported}, but this wallet builds \
                 ledger-{} transactions; switch to a network on that generation",
                self.config.network.name,
                send::LEDGER_GENERATION.trim_end_matches('.'),
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl ChainClient for MidnightClient {
    fn endpoint(&self) -> &str {
        self.indexer.url()
    }

    async fn balance(&self, address: &str) -> Result<Balance> {
        let unshielded = self.indexer.balance(address).await?;
        let mut balance = Balance::native(unshielded.night());
        for (token, value) in unshielded.by_token {
            if token != NIGHT_TOKEN_TYPE {
                balance.tokens.insert(token, value);
            }
        }
        Ok(balance)
    }

    async fn prepare_transfer(
        &self,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer> {
        let account = MidnightAccount::from_secret(signer_secret)?;
        let signing_key = LedgerSigningKey::from_bytes(&account.secret_bytes()).map_err(|e| {
            error::invalid_private_key(format!("the ledger rejected this key: {e}"))
        })?;

        // Cross-check the ledger's address derivation against this wallet's
        // own. They must agree, or we would be spending from an address the
        // wallet cannot see.
        if UserAddress::from(signing_key.verifying_key()).0 .0 != account.address_bytes() {
            return Err(error::internal(
                "the ledger and this wallet disagree on how to derive the sender \
                 address; refusing to sign",
            ));
        }
        let sender = account.address(self.network_id.clone()).to_bech32m()?;

        let destination = MidnightAddress::parse(&request.to)?;
        if destination.kind != TYPE_UNSHIELDED {
            return Err(error::invalid_address(format!(
                "`{}` is not an unshielded (mn_addr…) address; this wallet moves \
                 transparent NIGHT only",
                request.to
            )));
        }
        if destination.network != self.network_id {
            return Err(error::invalid_address(format!(
                "that address is for `{}` but this wallet is on `{}`; the funds \
                 would go to a chain nobody is watching",
                destination.network, self.network_id
            )));
        }
        if destination.payload == account.address_bytes() {
            return Err(error::usage(format!(
                "the recipient is the sending account ({sender}); a transfer to \
                 itself moves nothing and still pays the fee"
            )));
        }
        let dest_address = UserAddress(HashOutput(destination.unshielded_bytes()?));

        self.check_ledger_generation().await?;

        // --- coin selection -------------------------------------------------
        let utxos = self.indexer.utxos(&sender).await?;
        let mut night: Vec<UtxoInfo> = utxos
            .into_iter()
            .filter(|u| u.token_type == NIGHT_TOKEN_TYPE)
            .collect();
        let total: u128 = night.iter().map(|u| u.value).sum();
        let units = self.config.network.units();
        if total < request.amount {
            return Err(error::insufficient_funds(format!(
                "balance {} is less than the requested {}",
                units.format_with_symbol(total),
                units.format_with_symbol(request.amount),
            )));
        }

        // Never-registered outputs can pay their fee prooflessly; registered
        // ones need a real DUST spend, which costs a state sync and a proving
        // run. Prefer the cheap path whenever it suffices.
        let proofless_funds: u128 = night
            .iter()
            .filter(|u| !u.registered_for_dust)
            .map(|u| u.value)
            .sum();
        let via_dust = proofless_funds < request.amount;
        if !via_dust {
            night.retain(|u| !u.registered_for_dust);
        }
        // Oldest first: implicit dust accrues with age, so the oldest coins
        // carry the most fee allowance.
        night.sort_by_key(|u| u.ctime.unwrap_or(i64::MAX));

        let mut selected = Vec::new();
        let mut selected_value = 0u128;
        for utxo in night {
            selected_value += utxo.value;
            selected.push(utxo);
            if selected_value >= request.amount {
                break;
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| error::internal(format!("the system clock is before 1970: {e}")))?
            .as_secs();
        // Fees are paid in DUST, whose key lives at another role of the same
        // path. An account imported as a bare night key does not have it, and
        // `dust_seed` explains that rather than deriving a stranger's.
        let dust_key = DustSecretKey::derive_secret_key(&account.dust_seed()?);

        // --- build ----------------------------------------------------------
        let network_id = self.network_id.to_string();
        let amount = request.amount;
        let built = if via_dust {
            self.config.report(
                "this address is registered for DUST generation, so the fee needs a \
                 proved DUST spend — syncing the dust ledger first",
            );
            let cache = dust::cache_path(&self.config.cache_dir, &network_id, &sender);
            let wallet = dust::sync(
                self.indexer.url(),
                &dust_key,
                Some(&cache),
                self.config.progress.as_ref(),
            )
            .await?;

            // Proving is seconds of CPU with no await points in it; leaving it
            // on a reactor thread would stall every other request.
            let progress = self.config.progress.clone();
            let selected = selected.clone();
            runtime::blocking(move || {
                send::build_via_dust(
                    &signing_key,
                    &dust_key,
                    &wallet.state,
                    &network_id,
                    &selected,
                    dest_address,
                    amount,
                    now,
                    progress.as_ref(),
                )
            })
            .await??
        } else {
            send::build_proofless(
                &signing_key,
                dust_key,
                &network_id,
                &selected,
                dest_address,
                amount,
                now,
            )?
        };

        // Half again as margin: the chain's fee prices drift away from the
        // genesis parameters this estimate uses.
        let fee_needed = built.fee.saturating_add(built.fee / 2);
        if !built.proved && built.allowance < fee_needed {
            let wait = if built.accrual_rate > 0 {
                (fee_needed - built.allowance).div_ceil(built.accrual_rate)
            } else {
                u128::MAX
            };
            return Err(error::insufficient_funds(format!(
                "not enough implicit DUST has accrued to cover the fee: about \
                 {} needed, {} available. DUST accrues while NIGHT sits unspent \
                 — try again in roughly {} minutes",
                dust::format_dust(fee_needed),
                dust::format_dust(built.allowance),
                wait.div_ceil(60),
            )));
        }

        // The fee is the ledger's own estimate rather than an endpoint's
        // answer, but it is built from parameters the endpoint supplied, and
        // this is the last point before the transfer leaves the machine.
        chain::check_fee(
            &self.config.network,
            request.fee_ceiling(&self.config.network),
            built.fee,
            DUST,
        )?;

        let id = hex::encode(built.sealed.transaction_hash().0 .0);
        let mut bytes = Vec::new();
        tagged_serialize(&built.sealed, &mut bytes)
            .map_err(|e| error::internal(format!("serialization failed: {e}")))?;

        Ok(PreparedTransfer {
            signed: send::wrap_extrinsic(&bytes),
            id,
            from: sender.clone(),
            to: request.to.clone(),
            amount: request.amount,
            fee: built.fee,
            fee_unit: Some(DUST),
            fee_rate: None,
            nonce: None,
            gas_limit: None,
            network: self.config.network,
            note: built
                .proved
                .then(|| " (fee paid by a proved DUST spend)".to_string()),
            detail: json!({
                "fee_specks": built.fee.to_string(),
                "fee_dust": dust::format_dust(built.fee),
                "fee_path": if built.proved { "proved dust spend" } else { "implicit dust allowance" },
                "dust_allowance": dust::format_dust(built.allowance),
                "inputs": selected.len(),
            }),
        })
    }

    async fn submit(&self, prepared: &PreparedTransfer) -> Result<TransferReceipt> {
        let wire = format!("0x{}", hex::encode(&prepared.signed));
        let extrinsic =
            http::json_rpc(self.node(), "author_submitExtrinsic", json!([wire])).await?;
        Ok(TransferReceipt {
            id: prepared.id.clone(),
            // Not the same thing as the transaction hash: one is the ledger's
            // identity for the transfer, the other Substrate's for its
            // envelope, and only the first is searchable in the explorer.
            secondary_id: extrinsic.as_str().map(str::to_string),
        })
    }

    async fn transaction(&self, id: &str) -> Result<Option<TransactionStatus>> {
        const QUERY: &str = r#"
query($hash: HexEncoded!) {
  transactions(offset: { hash: $hash }) {
    hash
    block { height }
    ... on RegularTransaction { transactionResult { status } }
  }
}"#;
        let data = http::graphql(self.indexer.url(), QUERY, json!({"hash": id})).await?;
        let Some(transaction) = data
            .get("transactions")
            .and_then(Value::as_array)
            .and_then(|t| t.first())
        else {
            return Ok(None);
        };
        let reported = transaction
            .pointer("/transactionResult/status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        Ok(Some(TransactionStatus {
            id: id.to_string(),
            status: match reported {
                "SUCCESS" => "confirmed".into(),
                "FAILURE" | "PARTIAL_SUCCESS" => "failed".into(),
                _ => "pending".into(),
            },
            block: transaction.pointer("/block/height").and_then(Value::as_u64),
            fee: None,
            gas_used: None,
            raw: transaction.clone(),
        }))
    }

    async fn chain_info(&self) -> Result<Value> {
        Ok(json!({
            "network": self.config.network.key,
            "network_id": self.network_id.to_string(),
            "indexer": self.indexer.url(),
            "node": self.node(),
            "block_height": self.indexer.block_height().await?,
        }))
    }
}

/// How long to wait for the indexer to catch up with a submitted transfer.
pub const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(180);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Seed;
    use crate::network::{MIDNIGHT_DEVNET, MIDNIGHT_PREVIEW};

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn account() -> MidnightAccount {
        MidnightAccount::from_seed(&Seed::new(PHRASE, "").unwrap().bip39_seed(), 0).unwrap()
    }

    fn client(network: crate::network::Network) -> MidnightClient {
        MidnightClient::new(&ClientConfig::bare(network)).unwrap()
    }

    #[test]
    fn reads_and_submissions_go_to_different_services() {
        // Conflating them would post a signed transaction to a GraphQL
        // indexer, which cannot accept one.
        let client = client(MIDNIGHT_PREVIEW);
        assert!(client.endpoint().contains("indexer."));
        assert!(client.node().contains("rpc."));
        assert_ne!(client.endpoint(), client.node());
    }

    #[test]
    fn the_network_id_comes_from_the_wallet_row() {
        assert_eq!(
            client(MIDNIGHT_PREVIEW).network_id,
            NetworkId::Named("preview".into())
        );
        assert_eq!(
            client(MIDNIGHT_DEVNET).network_id,
            NetworkId::Named("devnet".into())
        );
    }

    #[test]
    fn an_address_for_another_midnight_network_is_refused_before_any_node_is_asked() {
        let account = account();
        let devnet_address = account
            .address(NetworkId::Named("devnet".into()))
            .to_bech32m()
            .unwrap();
        let request = TransferRequest::new(devnet_address, 1_000_000);
        let err = runtime::block_on(
            client(MIDNIGHT_PREVIEW).prepare_transfer(&account.secret_hex(), &request),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(
            err.message.contains("nobody is watching"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_shielded_address_is_refused_before_any_node_is_asked() {
        let account = account();
        let shielded = MidnightAddress {
            kind: super::super::address::TYPE_SHIELDED.into(),
            network: NetworkId::Named("preview".into()),
            payload: vec![0u8; 64],
        }
        .to_bech32m()
        .unwrap();
        let request = TransferRequest::new(shielded, 1_000_000);
        let err = runtime::block_on(
            client(MIDNIGHT_PREVIEW).prepare_transfer(&account.secret_hex(), &request),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("transparent NIGHT"), "{}", err.message);
    }

    #[test]
    fn sending_to_yourself_is_refused_before_any_node_is_asked() {
        let account = account();
        let own = account
            .address(NetworkId::Named("preview".into()))
            .to_bech32m()
            .unwrap();
        let request = TransferRequest::new(own, 1_000_000);
        let err = runtime::block_on(
            client(MIDNIGHT_PREVIEW).prepare_transfer(&account.secret_hex(), &request),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("itself"), "{}", err.message);
    }
}
