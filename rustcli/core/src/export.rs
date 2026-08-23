//! Rendering the account list as JSONL, CSV, plain text or Markdown.
//!
//! One renderer feeds both front ends: `account list --format` on the CLI and
//! the "Save wallet list" entries in the TUI menu produce identical bytes.

use serde_json::json;

use crate::error::{self, Result};
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
/// `address_index` is the BIP-44 index *within one mnemonic*, not a position in
/// this list — every freshly generated wallet has its own seed and so starts
/// again at 0. `position` is the row number, and `seed` groups the rows that
/// share a mnemonic, so the repetition is self-explanatory.
const COLUMNS: [&str; 11] = [
    "position",
    "label",
    "address",
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

/// One account flattened to strings, in `COLUMNS` order.
fn row(account: &Account, position: usize, active_id: Option<&str>, secrets: bool) -> Vec<String> {
    let active = if Some(account.id.as_str()) == active_id {
        "yes"
    } else {
        "no"
    };
    let (compressed, uncompressed) = public_keys(account);
    let mut values = vec![
        position.to_string(),
        account.label.clone(),
        account.address.clone(),
        account.source.as_str().to_string(),
        account.index.map(|i| i.to_string()).unwrap_or_default(),
        seed_id(account),
        account.derivation_path.clone().unwrap_or_default(),
        account.created_at.clone(),
        active.to_string(),
        compressed,
        uncompressed,
    ];
    if secrets {
        values.push(account.private_key.clone());
        values.push(account.mnemonic.clone().unwrap_or_default());
    }
    values
}

fn headers(secrets: bool) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = COLUMNS.to_vec();
    if secrets {
        names.extend_from_slice(&SECRET_COLUMNS);
    }
    names
}

/// Render the account list. `active_id` marks which account is the default.
pub fn render(
    accounts: &[Account],
    format: Format,
    active_id: Option<&str>,
    secrets: bool,
) -> String {
    match format {
        Format::Jsonl => render_jsonl(accounts, active_id, secrets),
        Format::Csv => render_csv(accounts, active_id, secrets),
        Format::Txt => render_txt(accounts, active_id, secrets),
        Format::Markdown => render_markdown(accounts, active_id, secrets),
    }
}

fn render_jsonl(accounts: &[Account], active_id: Option<&str>, secrets: bool) -> String {
    let mut out = String::new();
    for (offset, account) in accounts.iter().enumerate() {
        let (compressed, uncompressed) = public_keys(account);
        // Built explicitly rather than from `public_view`, so all four formats
        // carry the same columns under the same names. The on-disk record in
        // accounts.jsonl keeps its own `index` field; this is the export view.
        let mut value = json!({
            "position": offset + 1,
            "label": account.label,
            "address": account.address,
            "source": account.source.as_str(),
            "address_index": account.index,
            "seed": seed_id(account),
            "derivation_path": account.derivation_path,
            "created_at": account.created_at,
            "active": Some(account.id.as_str()) == active_id,
            "public_key_compressed": compressed,
            "public_key": uncompressed,
        });
        if secrets {
            let map = value.as_object_mut().expect("json! builds an object");
            map.insert("private_key".into(), json!(account.private_key));
            map.insert("mnemonic".into(), json!(account.mnemonic));
        }
        out.push_str(&serde_json::to_string(&value).unwrap_or_default());
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

fn render_csv(accounts: &[Account], active_id: Option<&str>, secrets: bool) -> String {
    let mut out = String::new();
    out.push_str(&headers(secrets).join(","));
    out.push('\n');
    for (offset, account) in accounts.iter().enumerate() {
        let cells: Vec<String> = row(account, offset + 1, active_id, secrets)
            .iter()
            .map(|value| csv_escape(value))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

fn render_txt(accounts: &[Account], active_id: Option<&str>, secrets: bool) -> String {
    let names = headers(secrets);
    let rows: Vec<Vec<String>> = accounts
        .iter()
        .enumerate()
        .map(|(offset, account)| row(account, offset + 1, active_id, secrets))
        .collect();

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

fn render_markdown(accounts: &[Account], active_id: Option<&str>, secrets: bool) -> String {
    let names = headers(secrets);
    let mut out = String::new();
    out.push_str(&format!("| {} |\n", names.join(" | ")));
    out.push_str(&format!(
        "| {} |\n",
        names.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
    ));
    for (offset, account) in accounts.iter().enumerate() {
        let cells: Vec<String> = row(account, offset + 1, active_id, secrets)
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

    #[test]
    fn jsonl_writes_one_object_per_account() {
        let rendered = render(&sample(), Format::Jsonl, Some("acc_main"), false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(value.get("address").is_some());
            assert!(
                value.get("private_key").is_none(),
                "secrets stay out by default"
            );
        }
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["active"], true);
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["active"], false);
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
    fn csv_has_a_header_and_one_row_per_account() {
        let rendered = render(&sample(), Format::Csv, Some("acc_main"), false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3, "header plus two accounts");
        assert_eq!(
            lines[0],
            "position,label,address,source,address_index,seed,derivation_path,\
             created_at,active,public_key_compressed,public_key"
                .replace(char::is_whitespace, "")
        );
        assert!(lines[1].starts_with("1,main,0xaaa,mnemonic,0,"));
        // `active` sits before the two public key columns now.
        assert!(
            lines[1].contains(",yes,0x"),
            "row 1 is the active one: {}",
            lines[1]
        );
        assert!(lines[2].contains(",no,0x"), "row 2 is not: {}", lines[2]);
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
        assert!(data_line.starts_with("1,\"a,b\"\"c\","));
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
        assert_eq!(lines.len(), 4, "header, rule, two accounts");
        assert!(lines[0].starts_with("position"));
        assert!(lines[1].starts_with("--------"));
        // "second" is the longer label, so the address column starts at the same
        // offset on both rows.
        let offset = |line: &str| line.find("0x").unwrap();
        assert_eq!(offset(lines[2]), offset(lines[3]));
    }

    #[test]
    fn markdown_is_a_valid_table() {
        let rendered = render(&sample(), Format::Markdown, Some("acc_main"), false);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 4, "header, separator, two accounts");
        assert!(lines[0].starts_with("| position | label | address |"));
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
        let data_line = csv.lines().nth(1).unwrap();
        // No mnemonic, so address_index, seed and derivation_path are all blank.
        assert!(data_line.starts_with("1,main,0xaaa,private_key,,,,"));
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
        assert_eq!(positions, ["1", "2"], "the list position is 1-based");

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
        let rows: Vec<serde_json::Value> = jsonl
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(rows[0]["address_index"], 0);
        assert_eq!(rows[1]["address_index"], 1);
        assert_eq!(
            rows[2]["address_index"], 0,
            "a new mnemonic starts again at 0"
        );
        assert_eq!(rows[0]["position"], 1);
        assert_eq!(rows[2]["position"], 3, "the list position keeps counting");
    }

    #[test]
    fn the_seed_column_groups_wallets_that_share_a_mnemonic() {
        let mut accounts = sample();
        let mut other = account("other", "0xccc");
        other.mnemonic = Some("legal winner".into());
        accounts.push(other);

        let jsonl = render(&accounts, Format::Jsonl, None, false);
        let rows: Vec<serde_json::Value> = jsonl
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

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
        let first: serde_json::Value = serde_json::from_str(jsonl.lines().next().unwrap()).unwrap();
        assert_eq!(first["seed"], "");
        assert!(first["address_index"].is_null());
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
