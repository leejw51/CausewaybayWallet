//! The two rendering modes: human text, and the machine envelope from `SPEC.md`.

use serde_json::{json, Value};

use crate::error::Error;

/// What a command produces: structured data plus its human rendering.
pub struct CommandOutput {
    pub data: Value,
    pub human: String,
}

impl CommandOutput {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        CommandOutput {
            data,
            human: human.into(),
        }
    }

    /// For commands whose result is only worth stating in one line.
    pub fn message(human: impl Into<String>) -> Self {
        let human = human.into();
        CommandOutput {
            data: json!({"message": human}),
            human,
        }
    }
}

/// `{"ok":true,"data":…}` on one line.
pub fn success_envelope(data: &Value) -> String {
    json!({"ok": true, "data": data}).to_string()
}

/// `{"ok":false,"error":{…}}` on one line.
pub fn error_envelope(error: &Error) -> String {
    json!({
        "ok": false,
        "error": {"code": error.code.as_str(), "message": error.message},
    })
    .to_string()
}

/// Render a list of key/value pairs as an aligned block.
pub fn table(rows: &[(&str, String)]) -> String {
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(key, value)| format!("{key:<width$}  {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Show only the ends of a secret, for at-a-glance identification.
pub fn truncate_secret(secret: &str) -> String {
    let body = secret.strip_prefix("0x").unwrap_or(secret);
    if body.len() <= 12 {
        return "*".repeat(body.len());
    }
    format!("0x{}…{}", &body[..6], &body[body.len() - 4..])
}

/// The banner every human-facing entry point prints.
pub const WARNING: &str =
    "⚠️  Educational wallet. Keys are stored unencrypted. Do not use with funds you cannot lose.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{self, Code};

    #[test]
    fn success_envelope_is_one_line_and_well_shaped() {
        let rendered = success_envelope(&json!({"address": "0xabc"}));
        assert!(!rendered.contains('\n'));
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["data"]["address"], "0xabc");
    }

    #[test]
    fn error_envelope_carries_the_stable_code() {
        let rendered = error_envelope(&error::account_not_found("no account matching 'bob'"));
        assert!(!rendered.contains('\n'));
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "account_not_found");
        assert_eq!(parsed["error"]["message"], "no account matching 'bob'");
    }

    #[test]
    fn every_error_code_has_a_snake_case_name() {
        let codes = [
            Code::Usage,
            Code::NotFound,
            Code::AccountNotFound,
            Code::DuplicateLabel,
            Code::InvalidMnemonic,
            Code::InvalidPrivateKey,
            Code::InvalidAddress,
            Code::InvalidAmount,
            Code::NoActiveAccount,
            Code::UnknownNetwork,
            Code::RpcError,
            Code::InsufficientFunds,
            Code::ConfirmationRequired,
            Code::IoError,
            Code::Internal,
        ];
        for code in codes {
            let name = code.as_str();
            assert!(!name.is_empty());
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{name} is not snake_case"
            );
        }
    }

    #[test]
    fn tables_align_on_the_longest_key() {
        let rendered = table(&[("Address", "0x1".into()), ("Network", "Cronos".into())]);
        let lines: Vec<_> = rendered.lines().collect();
        assert_eq!(lines[0], "Address  0x1");
        assert_eq!(lines[1], "Network  Cronos");
    }

    #[test]
    fn empty_tables_do_not_panic() {
        assert_eq!(table(&[]), "");
    }

    #[test]
    fn truncation_hides_the_middle_of_a_secret() {
        let secret = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727";
        let short = truncate_secret(secret);
        assert!(short.starts_with("0x1ab42c"));
        assert!(short.ends_with("b727"));
        assert!(!short.contains("b618bdea"), "the middle must not leak");
        assert_eq!(truncate_secret("0xabc"), "***");
    }

    #[test]
    fn message_output_mirrors_data_and_text() {
        let output = CommandOutput::message("done");
        assert_eq!(output.human, "done");
        assert_eq!(output.data["message"], "done");
    }
}
