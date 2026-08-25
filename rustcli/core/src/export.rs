//! Rendering the account list as JSONL, CSV, plain text or Markdown.
//!
//! One renderer feeds both front ends: `account list --format` on the CLI and
//! the "Save wallet list" entries in the TUI menu produce identical bytes.

use serde_json::{json, Value};

use crate::chain::ChainId;
use crate::error::{self, Result};
use crate::network::Network;
use crate::store::Account;

/// The formats an account list can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON object per line, matching the on-disk store format.
    Jsonl,
    /// Comma-separated, with RFC 4180 quoting.
    Csv,
    /// Aligned columns for reading in a terminal.
    Txt,
    /// A Markdown table.
    Markdown,
}

impl Format {
    pub fn parse(input: &str) -> Result<Self> {
        match input.trim().to_lowercase().as_str() {
            "jsonl" | "ndjson" => Ok(Format::Jsonl),
            "csv" => Ok(Format::Csv),
            "txt" | "text" | "plain" => Ok(Format::Txt),
            "md" | "markdown" => Ok(Format::Markdown),
            other => Err(error::usage(format!(
                "unknown format '{other}'; use jsonl, csv, txt or md"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Jsonl => "jsonl",
            Format::Csv => "csv",
            Format::Txt => "txt",
            Format::Markdown => "md",
        }
    }

    /// The file extension to suggest when saving.
    pub fn extension(self) -> &'static str {
        self.as_str()
    }

    pub fn all() -> [Format; 4] {
        [Format::Jsonl, Format::Csv, Format::Txt, Format::Markdown]
    }
}

/// The columns every format shares, in order.
///
/// A row is one account **on one network**, because that is the question an
/// export answers: "where do I send funds to this wallet?" On Cardano and
/// Midnight the answer differs per network — the network is inside the address
/// — so one row per account would have to leave two thirds of it out.
///
/// `address_index` is the BIP-44 index *within one mnemonic*, not a position in
/// this list — every freshly generated wallet has its own seed and so starts
/// again at 0. `position` is the row number, and `seed` groups the rows that
/// share a mnemonic, so the repetition is self-explanatory.
const COLUMNS: [&str; 14] = [
    "position",
    // `account0-cronos-testnet`: which wallet, and where.
    "name",
    "address",
    "network",
    "chain",
    // The name the store knows the account by — what `account use` takes.
    "label",
    "source",
    "address_index",
    "seed",
    "derivation_path",
    "created_at",
    "active",
    // Long, so they sit at the end where they do not push the readable columns
    // off the side of a table.
    "public_key_compressed",
    "public_key",
];

/// The extra columns appended when secrets are included.
const SECRET_COLUMNS: [&str; 2] = ["private_key", "mnemonic"];

/// The two public key encodings, from the account's private key.
///
/// Returned as `(compressed, uncompressed)`: 33 bytes with a parity prefix, and
/// 64 bytes of raw X‖Y with the SEC1 `0x04` tag dropped — the form the address
/// is hashed from.
fn public_keys(account: &Account) -> (String, String) {
    match crate::wallet::Keypair::from_hex(&account.private_key) {
        Ok(keypair) => (
            keypair.public_key_compressed_hex(),
            keypair.public_key_hex(),
        ),
        // A record whose key will not parse still gets a row, just a blank one.
        Err(_) => (String::new(), String::new()),
    }
}

/// One account flattened to strings, in `COLUMNS` order.
/// Which mnemonic an account came from, as the same id the recall list uses.
///
/// Two wallets showing the same seed were derived from one phrase; a blank means
/// the key was imported on its own.
pub fn seed_id(account: &Account) -> String {
    account
        .mnemonic
        .as_deref()
        .map(|phrase| crate::store::secret_id("mnemonic", phrase))
        .unwrap_or_default()
}

/// The networks a wallet's addresses are written out for.
///
/// An export is a record of where funds are, so every network whose address
/// differs gets a row. One key serves both Cronos networks and both are in
/// use, so both are written; Cardano, Midnight and eCash render a different
/// address per network, so a file that held only the default network's —
/// a test network on all three — would omit the very address a mainnet
/// balance sits on. Solana renders one address everywhere, and one row
/// says it all. `rows` drops the duplicates the per-network chains still
/// produce (Cardano's two test networks share one address).
fn exported_networks(chain: ChainId) -> Vec<Network> {
    match chain {
        ChainId::Solana => vec![crate::network::default_for(chain)],
        _ => crate::network::for_chain(chain),
    }
}

/// What one row calls itself: `account0-cronos-testnet`.
///
/// The chain suffix the account's own name carries is replaced by the network,
/// because the network is the more precise answer and repeating both would
/// read as `account0-evm-cronos-testnet`. A chain with a single exported
/// network keeps the plain name — `account0-solana` — since there is nothing
/// to tell apart until its mainnet arrives.
fn row_name(account: &Account, network: &Network, several: bool) -> String {
    let label = crate::store::display_label(account);
    if !several {
        return label;
    }
    let base = label
        .strip_suffix(&format!("-{}", account.chain))
        .unwrap_or(&label);
    format!("{base}-{}", network.key)
}

/// One line of an export: one account, on one network.
struct Row<'a> {
    position: usize,
    name: String,
    address: String,
    network: &'static str,
    account: &'a Account,
    active: bool,
    public_key_compressed: String,
    public_key: String,
}

impl Row<'_> {
    /// The row in `COLUMNS` order, as strings.
    fn cells(&self, secrets: bool) -> Vec<String> {
        let account = self.account;
        let mut values = vec![
            self.position.to_string(),
            self.name.clone(),
            self.address.clone(),
            self.network.to_string(),
            account.chain.as_str().to_string(),
            account.label.clone(),
            account.source.as_str().to_string(),
            account.index.map(|i| i.to_string()).unwrap_or_default(),
            seed_id(account),
            account.derivation_path.clone().unwrap_or_default(),
            account.created_at.clone(),
            if self.active { "yes" } else { "no" }.to_string(),
            self.public_key_compressed.clone(),
            self.public_key.clone(),
        ];
        if secrets {
            values.push(account.private_key.clone());
            values.push(account.mnemonic.clone().unwrap_or_default());
        }
        values
    }

    /// The same row as JSON. Built explicitly rather than from `public_view`,
    /// so all four formats carry the same columns under the same names.
    fn value(&self, secrets: bool) -> Value {
        let account = self.account;
        let mut value = json!({
            "position": self.position,
            "name": self.name,
            "address": self.address,
            "network": self.network,
            "chain": account.chain.as_str(),
            "label": account.label,
            "source": account.source.as_str(),
            "address_index": account.index,
            "seed": seed_id(account),
            "derivation_path": account.derivation_path,
            "created_at": account.created_at,
            "active": self.active,
            "public_key_compressed": self.public_key_compressed,
            "public_key": self.public_key,
        });
        if secrets {
            let map = value.as_object_mut().expect("json! builds an object");
            map.insert("private_key".into(), json!(account.private_key));
            map.insert("mnemonic".into(), json!(account.mnemonic));
        }
        value
    }
}

/// Every row of an export, flattened and in order.
///
/// Wallet by wallet, chain by chain, network by network — `account0-cronos-
/// testnet`, `account0-cronos-mainnet`, `account0-solana`, … then index 1's —
/// so the file reads as a list of wallets rather than as whatever order the
/// accounts happened to be created in.
fn rows<'a>(accounts: &'a [Account], active_id: Option<&str>) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    for account in ordered(accounts) {
        let chain = crate::chain::chain(account.chain);
        let (public_key_compressed, public_key) = public_keys(account);
        let mut pairs: Vec<(Network, String)> = Vec::new();
        for network in exported_networks(account.chain) {
            // Only the chains whose addresses carry the network re-render one;
            // everywhere else — and for a record whose key will not parse —
            // the address the store already holds is the answer.
            let address = chain
                .address_on(&network, &account.private_key)
                .ok()
                .flatten()
                .unwrap_or_else(|| account.address.clone());
            // Two networks can render one string — Cardano's preprod and
            // preview share the testnet form — and one row records it. The
            // Cronos pair is the exception: one address, two networks in use,
            // and the file has always carried both rows.
            if account.chain != ChainId::Evm && pairs.iter().any(|(_, held)| *held == address) {
                continue;
            }
            pairs.push((network, address));
        }
        // Named after what survived: a chain that came down to one row keeps
        // the plain name, however many networks were looked at.
        let several = pairs.len() > 1;
        for (network, address) in pairs {
            out.push(Row {
                position: out.len() + 1,
                name: row_name(account, &network, several),
                address,
                network: network.key,
                account,
                active: Some(account.id.as_str()) == active_id,
                public_key_compressed: public_key_compressed.clone(),
                public_key: public_key.clone(),
            });
        }
    }
    out
}

fn headers(secrets: bool) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = COLUMNS.to_vec();
    if secrets {
        names.extend_from_slice(&SECRET_COLUMNS);
    }
    names
}

/// The order every export is written in: wallet by wallet, chain by chain.
///
/// `account0-evm`, `account0-solana`, `account0-cardano`, `account0-midnight`,
/// `account0-ecash`, then index 1's — so the file reads as a list of wallets
/// rather than as
/// whatever order the accounts happened to be created in. An imported private
/// key has no wallet index, so those sit at the end rather than being folded
/// into index 0. `created_at` breaks the remaining ties, which is what keeps
/// two mnemonics at the same index in a stable order.
pub fn ordered(accounts: &[Account]) -> Vec<&Account> {
    let mut sorted: Vec<&Account> = accounts.iter().collect();
    sorted.sort_by(|a, b| {
        let key = |account: &Account| {
            (
                account.index.unwrap_or(u32::MAX),
                crate::chain::ChainId::ALL
                    .iter()
                    .position(|id| *id == account.chain)
                    .unwrap_or(usize::MAX),
                account.created_at.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    sorted
}

/// Render the account list. `active_id` marks which account is the default.
pub fn render(
    accounts: &[Account],
    format: Format,
    active_id: Option<&str>,
    secrets: bool,
) -> String {
    // Flattened and ordered once, for every format and both front ends: a
    // saved file looks the same whichever way it was asked for.
    let rows = rows(accounts, active_id);
    match format {
        Format::Jsonl => render_jsonl(&rows, secrets),
        Format::Csv => render_csv(&rows, secrets),
        Format::Txt => render_txt(&rows, secrets),
        Format::Markdown => render_markdown(&rows, secrets),
    }
}

fn render_jsonl(rows: &[Row], secrets: bool) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&serde_json::to_string(&row.value(secrets)).unwrap_or_default());
        out.push('\n');
    }
    out
}

/// Quote a CSV field per RFC 4180 when it needs it.
fn csv_escape(value: &str) -> String {
    let needs_quoting =
        value.contains([',', '"', '\n', '\r']) || value.starts_with(' ') || value.ends_with(' ');
    if needs_quoting {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn render_csv(rows: &[Row], secrets: bool) -> String {
    let mut out = String::new();
    out.push_str(&headers(secrets).join(","));
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = row
            .cells(secrets)
            .iter()
            .map(|value| csv_escape(value))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

fn render_txt(rows: &[Row], secrets: bool) -> String {
    let names = headers(secrets);
    let rows: Vec<Vec<String>> = rows.iter().map(|row| row.cells(secrets)).collect();

    // Width of the widest cell in each column, header included.
    let widths: Vec<usize> = names
        .iter()
        .enumerate()
        .map(|(column, name)| {
            rows.iter()
                .map(|values| values[column].chars().count())
                .chain(std::iter::once(name.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = String::new();
    let line = |cells: &[String], widths: &[usize]| -> String {
        cells
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };

    let header_cells: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    out.push_str(&line(&header_cells, &widths));
    out.push('\n');
    let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    out.push_str(&line(&rule, &widths));
    out.push('\n');
    for values in &rows {
        out.push_str(&line(values, &widths));
        out.push('\n');
    }
    out
}

/// A pipe or a newline inside a cell would break the table.
fn markdown_escape(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn render_markdown(rows: &[Row], secrets: bool) -> String {
    let names = headers(secrets);
    let mut out = String::new();
    out.push_str(&format!("| {} |\n", names.join(" | ")));
    out.push_str(&format!(
        "| {} |\n",
        names.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
    ));
    for row in rows {
        let cells: Vec<String> = row
            .cells(secrets)
            .iter()
            .map(|value| markdown_escape(value))
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Source;

    fn account(label: &str, address: &str) -> Account {
        Account {
            chain: crate::chain::ChainId::Evm,
            id: format!("acc_{label}"),
            label: label.to_string(),
            address: address.to_string(),
            source: Source::Mnemonic,
            // A usable key, so the derived public keys are real values.
            private_key: "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
                .into(),
            mnemonic: Some("abandon about".into()),
            derivation_path: Some("m/44'/60'/0'/0/0".into()),
            index: Some(0),
            created_at: "2026-08-22T05:00:00.000Z".into(),
        }
    }

    fn sample() -> Vec<Account> {
        vec![account("main", "0xaaa"), account("second", "0xbbb")]
    }

    /// The rows of a JSONL export, parsed.
    fn parsed(rendered: &str) -> Vec<serde_json::Value> {
        rendered
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// One row per account: an EVM account is written out on both Cronos
    /// networks, and the two rows differ only in the network and the name.
    fn per_account(rendered: &str) -> Vec<serde_json::Value> {
        parsed(rendered)
            .into_iter()
            .filter(|row| row["network"] == "cronos-testnet")
            .collect()
    }

    #[test]
    fn parses_format_names_and_aliases() {
        assert_eq!(Format::parse("jsonl").unwrap(), Format::Jsonl);
        assert_eq!(Format::parse("ndjson").unwrap(), Format::Jsonl);
        assert_eq!(Format::parse("CSV").unwrap(), Format::Csv);
        assert_eq!(Format::parse(" txt ").unwrap(), Format::Txt);
        assert_eq!(Format::parse("text").unwrap(), Format::Txt);
        assert_eq!(Format::parse("md").unwrap(), Format::Markdown);
        assert_eq!(Format::parse("markdown").unwrap(), Format::Markdown);
    }

    #[test]
    fn rejects_an_unknown_format_with_a_helpful_message() {
        let err = Format::parse("yaml").unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("jsonl"));
    }

    #[test]
    fn every_format_round_trips_through_its_name() {
        for format in Format::all() {
            assert_eq!(Format::parse(format.as_str()).unwrap(), format);
        }
    }

    /// Two EVM accounts, and one row each per Cronos network.
    #[test]
    fn jsonl_writes_one_object_per_account_and_network() {
        let rendered = render(&sample(), Format::Jsonl, Some("acc_main"), false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 4);
        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(value.get("address").is_some());
            assert!(
                value.get("private_key").is_none(),
                "secrets stay out by default"
            );
        }
        let names: Vec<String> = lines
            .iter()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["name"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            [
                "main-cronos-testnet",
                "main-cronos-mainnet",
                "second-cronos-testnet",
                "second-cronos-mainnet",
            ]
        );
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["active"], true);
        assert_eq!(first["network"], "cronos-testnet");
        assert_eq!(first["chain"], "evm");
        let third: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(third["active"], false);
    }

    #[test]
    fn jsonl_includes_secrets_only_when_asked() {
        let rendered = render(&sample(), Format::Jsonl, None, true);
        let first: serde_json::Value =
            serde_json::from_str(rendered.lines().next().unwrap()).unwrap();
        assert_eq!(
            first["private_key"],
            "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
        );
        assert_eq!(first["mnemonic"], "abandon about");
    }

    #[test]
    fn csv_has_a_header_and_one_row_per_account_and_network() {
        let rendered = render(&sample(), Format::Csv, Some("acc_main"), false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 5, "header plus two accounts on two networks");
        assert_eq!(
            lines[0],
            "position,name,address,network,chain,label,source,address_index,seed,\
             derivation_path,created_at,active,public_key_compressed,public_key"
                .replace(char::is_whitespace, "")
        );
        assert!(
            lines[1].starts_with("1,main-cronos-testnet,0xaaa,cronos-testnet,evm,main,mnemonic,0,")
        );
        assert!(lines[2].starts_with("2,main-cronos-mainnet,0xaaa,cronos-mainnet,"));
        // `active` sits before the two public key columns.
        assert!(
            lines[1].contains(",yes,0x"),
            "row 1 is the active one: {}",
            lines[1]
        );
        assert!(lines[3].contains(",no,0x"), "row 3 is not: {}", lines[3]);
    }

    /// The shape the export exists to produce: wallet by wallet, chain by
    /// chain, network by network, with a name that says which is which.
    #[test]
    fn a_multichain_wallet_is_written_out_wallet_by_wallet() {
        let chains = ChainId::ALL;
        let mut accounts = Vec::new();
        for index in [0u32, 1] {
            for chain in chains {
                // Deliberately out of order on the second wallet, to prove the
                // export sorts rather than trusting the store's order.
                let mut one = account(&crate::store::default_label(index, chain), "0xaaa");
                one.chain = chain;
                one.index = Some(index);
                if index == 1 {
                    accounts.insert(0, one);
                } else {
                    accounts.push(one);
                }
            }
        }

        let names: Vec<String> = parsed(&render(&accounts, Format::Jsonl, None, false))
            .into_iter()
            .map(|row| row["name"].as_str().unwrap().to_string())
            .collect();
        // Midnight and eCash re-render the fixture's secp256k1 key per
        // network, so each writes every distinct address. The fixture key is
        // not a Cardano key, so Cardano keeps its single stored-address row.
        assert_eq!(
            names,
            [
                "account0-cronos-testnet",
                "account0-cronos-mainnet",
                "account0-solana",
                "account0-cardano",
                "account0-midnight-preview",
                "account0-midnight-devnet",
                "account0-ecash-testnet",
                "account0-ecash-mainnet",
                "account1-cronos-testnet",
                "account1-cronos-mainnet",
                "account1-solana",
                "account1-cardano",
                "account1-midnight-preview",
                "account1-midnight-devnet",
                "account1-ecash-testnet",
                "account1-ecash-mainnet",
            ]
        );
    }

    #[test]
    fn csv_quotes_fields_that_need_it() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("has,comma"), "\"has,comma\"");
        assert_eq!(csv_escape("has\"quote"), "\"has\"\"quote\"");
        assert_eq!(csv_escape("has\nnewline"), "\"has\nnewline\"");
        assert_eq!(csv_escape(" padded "), "\" padded \"");
        assert_eq!(csv_escape(""), "");
    }

    #[test]
    fn csv_escapes_a_hostile_label() {
        let mut accounts = sample();
        accounts[0].label = "a,b\"c".into();
        let rendered = render(&accounts, Format::Csv, None, false);
        let data_line = rendered.lines().nth(1).unwrap();
        assert!(
            data_line.contains(",\"a,b\"\"c\","),
            "the label is quoted: {data_line}"
        );
        // The header column count and the row column count still agree.
        assert_eq!(
            rendered.lines().next().unwrap().split(',').count(),
            COLUMNS.len()
        );
    }

    #[test]
    fn txt_aligns_its_columns() {
        let rendered = render(&sample(), Format::Txt, None, false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 6, "header, rule, two accounts on two networks");
        assert!(lines[0].starts_with("position"));
        assert!(lines[1].starts_with("--------"));
        // "second" is the longer name, so the address column starts at the same
        // offset on both rows.
        let offset = |line: &str| line.find("0x").unwrap();
        assert_eq!(offset(lines[2]), offset(lines[3]));
    }

    #[test]
    fn markdown_is_a_valid_table() {
        let rendered = render(&sample(), Format::Markdown, Some("acc_main"), false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 6, "header, separator, four rows");
        assert!(lines[0].starts_with("| position | name | address | network |"));
        assert!(lines[1].starts_with("| --- |"));
        assert!(lines[2].contains("| main |"));
        // Every row has the same number of cells.
        let cells = |line: &str| line.matches('|').count();
        assert!(lines.iter().all(|line| cells(line) == COLUMNS.len() + 1));
    }

    #[test]
    fn markdown_escapes_pipes_so_the_table_survives() {
        let mut accounts = sample();
        accounts[0].label = "a|b".into();
        let rendered = render(&accounts, Format::Markdown, None, false);
        assert!(rendered.contains("a\\|b"));
        let row = rendered.lines().nth(2).unwrap();
        assert_eq!(row.matches("| ").count(), COLUMNS.len());
    }

    #[test]
    fn secret_columns_are_appended_not_interleaved() {
        for format in [Format::Csv, Format::Txt, Format::Markdown] {
            let rendered = render(&sample(), format, None, true);
            let header = rendered.lines().next().unwrap();
            assert!(header.contains("private_key"), "{format:?} header");
            assert!(header.contains("mnemonic"), "{format:?} header");
            assert!(
                header.find("private_key") > header.find("public_key"),
                "{format:?}: secrets belong after everything else"
            );
        }
    }

    /// The export the wallet owner actually wants when moving keys elsewhere:
    /// address, private key, and both public key encodings.
    #[test]
    fn a_full_export_carries_the_address_and_all_three_keys() {
        // A real keypair, so the derived public keys are checkable.
        let mut accounts = sample();
        accounts[0].address = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94".into();
        accounts.truncate(1);

        let rendered = render(&accounts, Format::Jsonl, None, true);
        let row: serde_json::Value =
            serde_json::from_str(rendered.lines().next().unwrap()).unwrap();

        assert_eq!(row["address"], "0x9858EfFD232B4033E47d90003D41EC34EcaEda94");
        assert_eq!(
            row["private_key"],
            "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
        );

        // 33 bytes: a 0x02/0x03 parity prefix and the X coordinate.
        let compressed = row["public_key_compressed"].as_str().unwrap();
        assert_eq!(compressed.len(), 2 + 66, "33 bytes as hex");
        assert!(compressed.starts_with("0x02") || compressed.starts_with("0x03"));

        // 64 bytes: X‖Y, with the SEC1 0x04 tag dropped.
        let uncompressed = row["public_key"].as_str().unwrap();
        assert_eq!(uncompressed.len(), 2 + 128, "64 bytes as hex");
        assert!(
            !uncompressed.starts_with("0x04"),
            "the SEC1 tag is not part of it"
        );

        // The compressed form carries the same X coordinate as the long one.
        assert_eq!(&compressed[4..], &uncompressed[2..66]);
    }

    #[test]
    fn the_public_keys_hash_to_the_address() {
        use alloy_primitives::keccak256;

        let accounts = sample();
        let (_, uncompressed) = public_keys(&accounts[0]);

        // This is the definition of an Ethereum address, so it ties the exported
        // public key to the exported address rather than trusting both.
        let bytes = hex::decode(uncompressed.trim_start_matches("0x")).unwrap();
        let digest = keccak256(&bytes);
        assert_eq!(
            format!("0x{}", hex::encode(&digest[12..])),
            "0x9858effd232b4033e47d90003d41ec34ecaeda94"
        );
    }

    /// Public keys are not secrets, so they are in every export; the private
    /// key and the mnemonic are the two that have to be asked for.
    #[test]
    fn public_keys_are_always_present_and_secrets_are_not() {
        for format in Format::all() {
            let rendered = render(&sample(), format, None, false);
            assert!(
                rendered.contains("public_key_compressed"),
                "{format:?} should carry the public keys"
            );
            assert!(
                !rendered.contains("private_key"),
                "{format:?} leaks the private key"
            );
            assert!(
                !rendered.contains("\"mnemonic\":") && !rendered.contains("abandon about"),
                "{format:?} leaks the mnemonic"
            );
        }

        let row: serde_json::Value = serde_json::from_str(
            render(&sample(), Format::Jsonl, None, false)
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert!(row["public_key"].as_str().unwrap().starts_with("0x"));
        assert!(row.get("private_key").is_none());
    }

    #[test]
    fn the_tables_carry_the_public_keys_too() {
        // csv, txt and md all get the same columns as jsonl.
        for format in [Format::Csv, Format::Txt, Format::Markdown] {
            let rendered = render(&sample(), format, None, false);
            let header = rendered.lines().next().unwrap();
            assert!(header.contains("public_key_compressed"), "{format:?}");
            assert!(header.contains("public_key"), "{format:?}");
            // And a row actually carries a key, not just the heading.
            let body = rendered
                .lines()
                .nth(if format == Format::Csv { 1 } else { 2 })
                .unwrap();
            assert!(
                body.contains("0x02") || body.contains("0x03"),
                "{format:?}: {body}"
            );
        }
    }

    #[test]
    fn an_unparsable_private_key_still_renders_a_row() {
        // The store is append-only and hand-editable, so a bad line must not
        // take the whole export down.
        let mut accounts = sample();
        accounts[0].private_key = "not-a-key".into();
        let rendered = render(&accounts, Format::Jsonl, None, true);
        let row: serde_json::Value =
            serde_json::from_str(rendered.lines().next().unwrap()).unwrap();
        assert_eq!(row["public_key"], "");
        assert_eq!(row["address"], "0xaaa");
    }

    #[test]
    fn an_empty_wallet_still_produces_headers() {
        assert_eq!(render(&[], Format::Jsonl, None, false), "");
        assert_eq!(
            render(&[], Format::Csv, None, false).lines().count(),
            1,
            "just the header"
        );
        assert_eq!(
            render(&[], Format::Markdown, None, false).lines().count(),
            2
        );
        assert_eq!(render(&[], Format::Txt, None, false).lines().count(), 2);
    }

    #[test]
    fn a_private_key_account_leaves_derivation_columns_blank() {
        let mut accounts = sample();
        accounts[0].source = Source::PrivateKey;
        accounts[0].derivation_path = None;
        accounts[0].index = None;
        accounts[0].mnemonic = None;

        let csv = render(&accounts, Format::Csv, None, false);
        // An account with no index is not part of any wallet, so it sits after
        // the ones that are.
        let data_line = csv
            .lines()
            .find(|line| line.contains(",private_key,"))
            .unwrap();
        // No mnemonic, so address_index, seed and derivation_path are all blank.
        assert!(
            data_line
                .contains(",main-cronos-testnet,0xaaa,cronos-testnet,evm,main,private_key,,,,"),
            "{data_line}"
        );
        // Still the right number of columns.
        assert_eq!(data_line.split(',').count(), COLUMNS.len());
    }

    #[test]
    fn positions_number_the_rows_from_one() {
        let rendered = render(&sample(), Format::Csv, None, false);
        let positions: Vec<&str> = rendered
            .lines()
            .skip(1)
            .map(|line| line.split(',').next().unwrap())
            .collect();
        assert_eq!(
            positions,
            ["1", "2", "3", "4"],
            "the list position is 1-based and counts rows, not accounts"
        );

        let jsonl = render(&sample(), Format::Jsonl, None, false);
        let first: serde_json::Value = serde_json::from_str(jsonl.lines().next().unwrap()).unwrap();
        assert_eq!(first["position"], 1);
    }

    /// The column that caused the confusion: two wallets generated separately
    /// both sit at address index 0, because each has its own mnemonic.
    #[test]
    fn the_address_index_is_per_mnemonic_not_per_list() {
        let mut accounts = sample();
        // Same phrase, two addresses from it.
        accounts[1].index = Some(1);
        accounts[1].derivation_path = Some("m/44'/60'/0'/0/1".into());
        // A third wallet from a different phrase starts again at 0.
        let mut other = account("other", "0xccc");
        other.mnemonic = Some("legal winner".into());
        accounts.push(other);

        let jsonl = render(&accounts, Format::Jsonl, None, false);
        let rows = per_account(&jsonl);

        // Wallet by wallet: both index 0 wallets, then index 1.
        assert_eq!(rows[0]["label"], "main");
        assert_eq!(rows[0]["address_index"], 0);
        assert_eq!(rows[1]["label"], "other");
        assert_eq!(
            rows[1]["address_index"], 0,
            "a new mnemonic starts again at 0"
        );
        assert_eq!(rows[2]["label"], "second");
        assert_eq!(rows[2]["address_index"], 1);
        assert_eq!(rows[0]["position"], 1);
        assert_eq!(
            parsed(&jsonl).last().unwrap()["position"],
            6,
            "the list position keeps counting across networks"
        );
    }

    #[test]
    fn the_seed_column_groups_wallets_that_share_a_mnemonic() {
        let mut accounts = sample();
        let mut other = account("other", "0xccc");
        other.mnemonic = Some("legal winner".into());
        accounts.push(other);

        let jsonl = render(&accounts, Format::Jsonl, None, false);
        let rows = per_account(&jsonl);

        assert_eq!(
            rows[0]["seed"], rows[1]["seed"],
            "same phrase, same seed id"
        );
        assert_ne!(
            rows[0]["seed"], rows[2]["seed"],
            "different phrase, different id"
        );
        assert!(rows[0]["seed"].as_str().unwrap().starts_with("sec_"));
    }

    #[test]
    fn a_private_key_wallet_has_no_seed() {
        let mut accounts = sample();
        // What `account import-key` actually stores: no mnemonic, so no
        // derivation path and no address index either.
        accounts[0].source = Source::PrivateKey;
        accounts[0].mnemonic = None;
        accounts[0].index = None;
        accounts[0].derivation_path = None;
        assert_eq!(seed_id(&accounts[0]), "");

        let jsonl = render(&accounts, Format::Jsonl, None, false);
        let row = parsed(&jsonl)
            .into_iter()
            .find(|row| row["label"] == "main")
            .unwrap();
        assert_eq!(row["seed"], "");
        assert!(row["address_index"].is_null());
    }

    /// The seed id is the one the recall list already shows, so the two views
    /// can be lined up against each other.
    #[test]
    fn the_seed_id_matches_the_recall_list_id() {
        let accounts = sample();
        assert_eq!(
            seed_id(&accounts[0]),
            crate::store::secret_id("mnemonic", "abandon about")
        );
    }

    #[test]
    fn every_format_ends_with_a_newline() {
        for format in Format::all() {
            let rendered = render(&sample(), format, None, false);
            assert!(rendered.ends_with('\n'), "{format:?}");
        }
    }
}
