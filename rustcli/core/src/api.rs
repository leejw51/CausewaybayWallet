//! JSON in, JSON out — the wallet reduced to two strings.
//!
//! This is the whole contract a foreign caller has to learn. Hand it a
//! [`Request`] as JSON, get back the same envelope `cwbwallet --json` prints:
//!
//! ```text
//! {"argv":["account","list"],"home":"/tmp/w"}
//!   → {"ok":true,"data":[…],"human":"…"}
//!
//! {"argv":["account","show","nobody"]}
//!   → {"ok":false,"error":{"code":"account_not_found","message":"…"}}
//! ```
//!
//! `human` is the text the CLI would have printed. A GUI ignores it and reads
//! `data`; a CLI written in another language prints it and stays in step with
//! the Rust one for free.
//!
//! Nothing in here can fail in a way the caller cannot see: a malformed
//! request, an unknown command and a panic in the wallet all come back as an
//! envelope with `ok: false`, never as a torn-down process. That matters most
//! for the FFI, where a panic unwinding into Lua would take the host with it.

use serde_json::{json, Value};

use crate::app::App;
use crate::error::Error;
use crate::output::CommandOutput;
use crate::request::Request;

/// Run one request. The `Err` half is an ordinary wallet error, not a crash.
pub fn execute(request: &Request) -> Result<CommandOutput, Error> {
    App::execute(request)
}

/// [`execute`], with the request and the envelope as JSON text.
///
/// Never returns an `Err`: every failure, including a panic inside the wallet,
/// is rendered as the error envelope. A caller that cannot unwind — anything
/// on the far side of the C ABI — needs exactly that guarantee.
pub fn execute_json(request_json: &str) -> String {
    let result = std::panic::catch_unwind(|| {
        let request: Request = match serde_json::from_str(request_json) {
            Ok(request) => request,
            Err(e) => {
                return error_envelope(&crate::error::usage(format!(
                    "request is not a valid JSON object: {e}"
                )))
            }
        };
        match execute(&request) {
            Ok(output) => success_envelope(&output),
            Err(e) => error_envelope(&e),
        }
    });
    result.unwrap_or_else(|_| {
        error_envelope(&crate::error::internal(
            "the wallet panicked; this is a bug, and no state was written",
        ))
    })
}

/// `{"ok":true,"data":…,"human":"…"}` on one line.
pub fn success_envelope(output: &CommandOutput) -> String {
    json!({"ok": true, "data": output.data, "human": output.human}).to_string()
}

/// `{"ok":false,"error":{"code":…,"message":…}}` on one line.
///
/// The same bytes the CLI prints: an error envelope has no `human` field to
/// add, and one definition of its shape is one fewer thing to keep in step.
pub fn error_envelope(error: &Error) -> String {
    crate::output::error_envelope(error)
}

/// What the library is and what contract it speaks.
///
/// A host that loaded the shared library at runtime checks `abi` against the
/// number it was written for before trusting anything else.
pub fn describe() -> String {
    json!({
        "ok": true,
        "data": {
            "name": "causewaybay-wallet",
            "version": crate::VERSION,
            "abi": crate::ABI_VERSION,
            // Shipped rather than restated: a front end that prints its own
            // wording eventually prints a weaker one.
            "warning": crate::output::WARNING,
            // So a host can branch on codes without keeping its own copy of
            // the list, which is a copy that goes stale silently.
            "codes": crate::error::Code::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            "networks": crate::network::ALL.iter().map(|n| json!({
                "key": n.key,
                "name": n.name,
                "chain_id": n.chain_id,
                "symbol": n.symbol,
            })).collect::<Vec<_>>(),
        },
    })
    .to_string()
}

/// Every command the wallet accepts, as a flat list of leaves.
///
/// Read out of the clap tree rather than written down, so it cannot fall
/// behind the commands that actually exist. Two callers want it:
///
/// * a GUI, which can build its panels from `args` instead of hardcoding a
///   form per command;
/// * the Lua binding's test suite, which asserts it has a method for every
///   entry — that is what makes "the whole surface is exposed" a fact rather
///   than a claim.
///
/// ```json
/// {"path":["account","new"], "name":"account new", "about":"…",
///  "args":[{"name":"label","long":"label","short":"l","positional":false,
///           "takes_value":true,"required":false,"default":null,"about":"…"}]}
/// ```
pub fn commands() -> String {
    use clap::CommandFactory;
    let root = crate::command::Cli::command();
    let mut leaves = Vec::new();
    walk(&root, &mut Vec::new(), &mut leaves);
    json!({"ok": true, "data": leaves}).to_string()
}

/// Depth-first over the subcommand tree, collecting the leaves.
fn walk(command: &clap::Command, path: &mut Vec<String>, out: &mut Vec<Value>) {
    // `help` is clap's own, not the wallet's; a front end has its own idea of
    // what help means and should not be asked to implement this one.
    let children: Vec<_> = command
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .collect();

    if children.is_empty() {
        if !path.is_empty() {
            out.push(describe_leaf(command, path));
        }
        return;
    }
    for child in children {
        path.push(child.get_name().to_string());
        walk(child, path, out);
        path.pop();
    }
}

fn describe_leaf(command: &clap::Command, path: &[String]) -> Value {
    let args: Vec<Value> = command
        .get_arguments()
        // Globals belong to the request, not to the command, and `help` is
        // clap's. Neither is something a caller passes in `argv`.
        .filter(|arg| !arg.is_global_set())
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
        .map(|arg| {
            json!({
                "name": arg.get_id().as_str(),
                "long": arg.get_long(),
                "short": arg.get_short().map(|c| c.to_string()),
                "positional": arg.is_positional(),
                // A flag is passed on its own; everything else needs a value.
                "takes_value": !matches!(
                    arg.get_action(),
                    clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
                ),
                "required": arg.is_required_set(),
                "default": arg
                    .get_default_values()
                    .first()
                    .map(|v| v.to_string_lossy().into_owned()),
                "about": arg.get_help().map(|h| h.to_string()),
            })
        })
        .collect();

    json!({
        "path": path,
        "name": path.join(" "),
        "about": command.get_about().map(|a| a.to_string()),
        "args": args,
    })
}

/// Parse an envelope back into data, for tests and for Rust hosts.
pub fn parse_envelope(envelope: &str) -> Result<Value, Error> {
    let parsed: Value = serde_json::from_str(envelope)
        .map_err(|e| crate::error::internal(format!("envelope is not JSON: {e}")))?;
    if parsed["ok"] == Value::Bool(true) {
        Ok(parsed["data"].clone())
    } else {
        let code = parsed["error"]["code"].as_str().unwrap_or("internal");
        let message = parsed["error"]["message"].as_str().unwrap_or_default();
        Err(Error::new(crate::error::Code::from_name(code), message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    /// Every test runs against a throwaway home so nothing touches ~/.
    fn home(dir: &tempfile::TempDir) -> String {
        dir.path().to_string_lossy().into_owned()
    }

    fn call(dir: &tempfile::TempDir, argv: &[&str]) -> Value {
        let request = json!({"argv": argv, "home": home(dir), "yes": true}).to_string();
        let envelope = execute_json(&request);
        assert!(!envelope.contains('\n'), "an envelope is one line");
        serde_json::from_str(&envelope).expect("envelope is JSON")
    }

    #[test]
    fn a_successful_call_carries_both_data_and_human_text() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = call(&dir, &["utils", "keccak", "hello"]);
        assert_eq!(envelope["ok"], true);
        assert_eq!(
            envelope["data"]["keccak256"],
            "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"
        );
        assert!(envelope["human"].as_str().unwrap().contains("0x1c8aff95"));
    }

    #[test]
    fn a_failure_carries_the_stable_code() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = call(&dir, &["account", "show", "ghost"]);
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "account_not_found");
        assert!(envelope.get("data").is_none());
    }

    #[test]
    fn malformed_json_is_an_envelope_not_a_crash() {
        let envelope: Value = serde_json::from_str(&execute_json("not json at all")).unwrap();
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "usage");
    }

    #[test]
    fn an_unknown_command_is_a_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = call(&dir, &["teleport"]);
        assert_eq!(envelope["error"]["code"], "usage");
        assert!(envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("teleport"));
    }

    #[test]
    fn help_is_a_success_carrying_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = call(&dir, &["--help"]);
        assert_eq!(envelope["ok"], true);
        assert!(envelope["human"].as_str().unwrap().contains("cwbwallet"));
    }

    #[test]
    fn the_tui_is_refused_rather_than_launched() {
        // A library must never seize the terminal out from under its host.
        let dir = tempfile::tempdir().unwrap();
        let envelope = call(&dir, &["tui"]);
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "usage");
    }

    #[test]
    fn state_written_by_one_call_is_visible_to_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let created = call(&dir, &["account", "new", "--label", "alpha"]);
        assert_eq!(created["data"]["label"], "alpha");

        let listed = call(&dir, &["account", "list"]);
        assert_eq!(listed["data"].as_array().unwrap().len(), 1);
        assert_eq!(listed["data"][0]["address"], created["data"]["address"]);
    }

    #[test]
    fn stdin_arrives_in_the_request_rather_than_from_a_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon about";
        let request = json!({
            "argv": ["account", "import-mnemonic", "-m", "-", "--label", "seeded"],
            "home": home(&dir),
            "stdin": phrase,
        })
        .to_string();
        let envelope: Value = serde_json::from_str(&execute_json(&request)).unwrap();
        assert_eq!(envelope["ok"], true, "{envelope}");
        assert_eq!(
            envelope["data"]["address"],
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
        );
    }

    #[test]
    fn a_dash_with_no_stdin_is_a_usage_error_not_a_hang() {
        // The failure mode this whole design exists to prevent: a library that
        // blocks forever waiting on a stdin its host never intended to give.
        let dir = tempfile::tempdir().unwrap();
        let request = json!({
            "argv": ["account", "import-mnemonic", "-m", "-"],
            "home": home(&dir),
        })
        .to_string();
        let envelope: Value = serde_json::from_str(&execute_json(&request)).unwrap();
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "usage");
    }

    #[test]
    fn a_confirmation_is_refused_unless_yes_was_given() {
        let dir = tempfile::tempdir().unwrap();
        call(&dir, &["account", "new", "--label", "doomed"]);

        let request = json!({
            "argv": ["account", "remove", "doomed"],
            "home": home(&dir),
        })
        .to_string();
        let envelope: Value = serde_json::from_str(&execute_json(&request)).unwrap();
        assert_eq!(envelope["error"]["code"], "confirmation_required");

        // The account survived the refusal.
        assert_eq!(
            call(&dir, &["account", "list"])["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_requested_home_outranks_the_ambient_environment() {
        // The bug this prevents: a GUI passes the wallet it means to open,
        // and a CAUSEWAYBAY_HOME left over in the environment wins anyway.
        let dir = tempfile::tempdir().unwrap();
        let stray = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded test; the variable is removed right after.
        unsafe { std::env::set_var("CAUSEWAYBAY_HOME", stray.path()) };

        let envelope = call(&dir, &["info"]);
        unsafe { std::env::remove_var("CAUSEWAYBAY_HOME") };

        assert_eq!(envelope["data"]["home"], home(&dir));
    }

    #[test]
    fn the_network_default_is_overridden_by_an_argv_flag() {
        let dir = tempfile::tempdir().unwrap();
        let request = json!({
            "argv": ["-n", "mainnet", "network", "current"],
            "home": home(&dir),
            "network": "testnet",
        })
        .to_string();
        let envelope: Value = serde_json::from_str(&execute_json(&request)).unwrap();
        assert_eq!(envelope["data"]["chain_id"], 25);
    }

    #[test]
    fn describe_names_the_abi_the_caller_must_match() {
        let described: Value = serde_json::from_str(&describe()).unwrap();
        assert_eq!(described["data"]["abi"], crate::ABI_VERSION);
        assert_eq!(described["data"]["version"], crate::VERSION);
        assert_eq!(described["data"]["warning"], crate::output::WARNING);
        assert_eq!(
            described["data"]["codes"].as_array().unwrap().len(),
            Code::ALL.len()
        );
        assert!(!described["data"]["networks"].as_array().unwrap().is_empty());
    }

    #[test]
    fn envelopes_parse_back_into_data_or_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let ok = execute_json(&json!({"argv": ["info"], "home": home(&dir)}).to_string());
        assert!(parse_envelope(&ok).unwrap()["home"].is_string());

        let bad = execute_json(
            &json!({"argv": ["account", "show", "ghost"], "home": home(&dir)}).to_string(),
        );
        assert_eq!(
            parse_envelope(&bad).unwrap_err().code,
            Code::AccountNotFound
        );
    }
}
