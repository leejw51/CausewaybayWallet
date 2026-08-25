//! A C ABI over the Causewaybay wallet: JSON in, JSON out.
//!
//! Seven functions, one data type (`char *`), and one rule: every string this
//! library returns is owned by the caller and must be handed back to
//! [`cwb_string_free`]. That is the entire surface, and it is deliberate —
//! anything richer would have to describe accounts, networks and transactions
//! in C structs, and every change to the wallet would then break every host.
//! A JSON envelope changes shape without changing the ABI.
//!
//! ```c
//! char *reply = cwb_execute("{\"argv\":[\"account\",\"list\"]}");
//! /* {"ok":true,"data":[…],"human":"…"} */
//! cwb_string_free(reply);
//! ```
//!
//! # Rules the implementation holds to
//!
//! * **Nothing unwinds.** Every entry point catches panics and reports them as
//!   `{"ok":false,"error":{"code":"internal",…}}`. A Rust panic crossing into
//!   LuaJIT is undefined behaviour, so it never gets the chance.
//! * **Nothing blocks on a terminal.** There is no stdin here; an argument
//!   written as `-` reads from the request's `stdin` field or fails.
//! * **A null pointer is an error, not a crash.** Hosts get pointers wrong.
//! * **Non-UTF-8 input is an error, not a panic.**
//!
//! See `include/causewaybay.h` for the header, and `../../luacli/` for a host
//! that uses it.

use std::ffi::{c_char, c_int, CStr, CString};

use causewaybay_core::api;

/// Version of the C ABI itself, for a host that loaded the library at runtime.
///
/// Bumped when the request or envelope shape changes incompatibly. A host that
/// gets an unexpected number should refuse to go on rather than guess.
#[no_mangle]
pub extern "C" fn cwb_abi_version() -> c_int {
    causewaybay_core::ABI_VERSION as c_int
}

/// The wallet version, as a newly allocated C string. Free with [`cwb_string_free`].
#[no_mangle]
pub extern "C" fn cwb_version() -> *mut c_char {
    into_c_string(causewaybay_core::VERSION.to_string())
}

/// A JSON envelope describing this library: name, version, ABI, networks.
///
/// The first call a host should make. Free with [`cwb_string_free`].
#[no_mangle]
pub extern "C" fn cwb_describe() -> *mut c_char {
    guarded(api::describe)
}

/// Every command the wallet accepts, as a JSON list of leaves.
///
/// What a host builds a menu from, and what a host's test suite checks its own
/// coverage against. Free with [`cwb_string_free`].
#[no_mangle]
pub extern "C" fn cwb_commands() -> *mut c_char {
    guarded(api::commands)
}

/// The chains this build supports, as a JSON list.
///
/// A host that offers a chain picker reads it from here rather than keeping
/// its own list — a list that goes stale the moment a chain is added, and
/// whose staleness shows up as a chain the user can select and nothing else.
/// The same data is inside [`cwb_describe`] under `chains`; this is the
/// direct call for a host that wants only that.
///
/// Added in ABI 2. A host written against ABI 1 will not find this symbol,
/// which is the other reason to check [`cwb_abi_version`] before use.
/// Free with [`cwb_string_free`].
#[no_mangle]
pub extern "C" fn cwb_chains() -> *mut c_char {
    guarded(|| {
        let chains: Vec<serde_json::Value> = causewaybay_core::chain::registry()
            .iter()
            .map(|c| {
                serde_json::json!({
                    "chain": c.id().as_str(),
                    "name": c.name(),
                    "derivation_path": c.derivation_path(0),
                    "networks": c.networks().iter().map(|n| serde_json::json!({
                        "key": n.key,
                        "name": n.name,
                        "symbol": n.symbol,
                        "decimals": n.decimals,
                        "testnet": n.testnet,
                    })).collect::<Vec<_>>(),
                    "capabilities": c.capabilities(),
                })
            })
            .collect();
        serde_json::json!({"ok": true, "data": chains}).to_string()
    })
}

/// Run one request.
///
/// `request_json` is a NUL-terminated JSON object — see `causewaybay_core::request::Request`:
///
/// ```json
/// {"argv": ["account","list"], "home": null, "network": null,
///  "chain": null, "yes": false, "stdin": null}
/// ```
///
/// `chain` selects between `evm`, `solana`, `cardano` and `midnight`. Like
/// `network`, it is a default: a `--chain` inside `argv` wins. Naming a
/// network settles the chain too, so a host that already picks networks never
/// has to set it.
///
/// Returns a NUL-terminated JSON envelope, always: `{"ok":true,"data":…,"human":…}`
/// or `{"ok":false,"error":{"code":…,"message":…}}`. Never null unless the
/// process is out of memory. Free with [`cwb_string_free`].
///
/// # Safety
///
/// `request_json` must be null or a valid NUL-terminated string that stays
/// alive for the duration of the call. The returned pointer must be freed with
/// [`cwb_string_free`] and with nothing else.
#[no_mangle]
pub unsafe extern "C" fn cwb_execute(request_json: *const c_char) -> *mut c_char {
    // Copied here, on the caller's thread, before `guarded` moves the work to
    // one of ours. The pointer belongs to the host and is only promised to be
    // alive for this call; an owned `String` is the only thing that may
    // legitimately cross a thread boundary, and the borrow checker says so.
    let request = match borrow_str(request_json) {
        Ok(text) => text.to_owned(),
        Err(envelope) => return into_c_string(envelope),
    };
    guarded(move || api::execute_json(&request))
}

/// Release a string this library returned.
///
/// Passing null is fine and does nothing, which is what every C caller's error
/// path wants.
///
/// # Safety
///
/// `s` must be null, or a pointer this library returned and that has not been
/// freed already. Freeing it twice, or freeing a pointer from anywhere else,
/// corrupts the allocator.
#[no_mangle]
pub unsafe extern "C" fn cwb_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

// ------------------------------------------------------------------ internals

/// Run `f`, turning a panic into an error envelope rather than an unwind.
/// The stack every command is given, whatever the host was standing on.
///
/// A library loaded into someone else's process does not get to choose which
/// thread calls it, and threads differ enormously: a main thread has eight
/// megabytes on every platform this ships to, while a worker made by SDL —
/// which is what a LÖVE `love.thread` is — has 512 KB, and a host embedding
/// Python or LuaJIT may have less again.
///
/// Argument parsing alone does not fit in 512 KB. Clap builds its whole
/// command tree on the stack through generated `augment_subcommands` calls,
/// one frame per subcommand, and this wallet's tree is deep enough that adding
/// the `token` commands took it past that guard page — turning *every* call
/// from the GUI's worker into a SIGBUS with no Rust panic to catch, because a
/// stack overflow is not an unwind.
///
/// Eight megabytes, to match the main thread rather than to guess at a
/// sufficient number. Sizing it to what today's tree happens to need would put
/// the next subcommand one merge away from the same crash, in a place nobody
/// would think to look.
const STACK: usize = 8 * 1024 * 1024;

/// Run one command with a stack of our own, catching panics.
///
/// The thread is per call and joined before returning, so nothing outlives the
/// FFI boundary and the caller still sees an ordinary synchronous function.
/// Spawning costs tens of microseconds against commands that reach a node over
/// the network; for the offline ones it is still far below anything a person
/// or a frame can notice.
///
/// If the thread cannot be spawned or dies, that is reported as an envelope
/// like any other failure — a null pointer back into C is not an option.
fn guarded(f: impl FnOnce() -> String + std::panic::UnwindSafe + Send + 'static) -> *mut c_char {
    let envelope = std::thread::Builder::new()
        .name("cwb-command".into())
        .stack_size(STACK)
        .spawn(move || {
            std::panic::catch_unwind(f).unwrap_or_else(|_| {
                error_envelope(
                    "internal",
                    "the wallet panicked; this is a bug, and no state was written",
                )
            })
        })
        .map_err(|e| {
            error_envelope(
                "internal",
                &format!("could not start a thread to run the command: {e}"),
            )
        })
        .and_then(|handle| {
            handle.join().map_err(|_| {
                error_envelope(
                    "internal",
                    "the wallet died while running the command; no state was written",
                )
            })
        });
    into_c_string(match envelope {
        Ok(envelope) => envelope,
        Err(envelope) => envelope,
    })
}

/// Borrow a C string as UTF-8, or produce the envelope explaining why not.
///
/// # Safety
///
/// `p` must be null or a valid NUL-terminated string.
unsafe fn borrow_str<'a>(p: *const c_char) -> Result<&'a str, String> {
    if p.is_null() {
        return Err(error_envelope("usage", "request pointer is null"));
    }
    CStr::from_ptr(p)
        .to_str()
        .map_err(|_| error_envelope("usage", "request is not valid UTF-8"))
}

/// Hand ownership of a Rust string to the caller as `char *`.
///
/// An interior NUL cannot survive the trip; it can only come from a caller
/// that ignored the contract, so it is reported rather than silently truncated.
fn into_c_string(text: String) -> *mut c_char {
    match CString::new(text) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new(error_envelope(
            "internal",
            "the reply contained a NUL byte and could not be returned",
        ))
        .expect("the fallback envelope has no NUL")
        .into_raw(),
    }
}

fn error_envelope(code: &str, message: &str) -> String {
    // Built by hand so it cannot itself fail while reporting a failure.
    format!(
        r#"{{"ok":false,"error":{{"code":"{}","message":"{}"}}}}"#,
        code, message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Call `cwb_execute` the way a C host would, and parse the reply.
    fn execute(request: &str) -> Value {
        let input = CString::new(request).unwrap();
        unsafe {
            let reply = cwb_execute(input.as_ptr());
            assert!(!reply.is_null());
            let text = CStr::from_ptr(reply).to_str().unwrap().to_owned();
            cwb_string_free(reply);
            serde_json::from_str(&text).expect("the reply is JSON")
        }
    }

    fn owned(p: *mut c_char) -> String {
        unsafe {
            let text = CStr::from_ptr(p).to_str().unwrap().to_owned();
            cwb_string_free(p);
            text
        }
    }

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn request(dir: &tempfile::TempDir, argv: &[&str]) -> String {
        serde_json::json!({
            "argv": argv,
            "home": dir.path().to_string_lossy(),
            "yes": true,
        })
        .to_string()
    }

    /// The regression this library's own stack exists for.
    ///
    /// A host does not get to choose which thread calls a shared library, and
    /// they are not alike: SDL gives a worker 512 KB, which is where the LÖVE
    /// GUI runs every command from. Clap builds its command tree on the stack,
    /// one frame per subcommand, and adding the `token` commands took the tree
    /// past that guard page — every call from the GUI became a SIGBUS, with no
    /// panic to catch, because a stack overflow does not unwind.
    ///
    /// So this runs a command from a thread with a stack far smaller than any
    /// real host's. If the entry point stopped moving the work onto a stack of
    /// its own, this would not fail — it would kill the test process outright,
    /// which is exactly how the bug announced itself the first time.
    #[test]
    fn a_command_survives_a_host_thread_with_almost_no_stack() {
        let dir = home();
        let payload = request(&dir, &["network", "list"]);
        // 128 KB: a quarter of what SDL hands a worker, and nowhere near
        // enough to build the command tree in.
        let handle = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || execute(&payload))
            .expect("the test thread should start");
        let reply = handle.join().expect("the wallet took the host thread down");
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(reply["data"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()));
    }

    /// And the deepest command tree in particular, since it is the one that
    /// blew the budget: `token send` sits three levels down with six flags.
    #[test]
    fn the_deepest_subcommand_parses_on_a_small_stack_too() {
        let dir = home();
        let payload = request(&dir, &["token", "info", "usdc-cronos-mainnet"]);
        let handle = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || execute(&payload))
            .expect("the test thread should start");
        let reply = handle.join().expect("the wallet took the host thread down");
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["data"]["symbol"], "USDC");
    }

    #[test]
    fn the_abi_version_matches_the_library() {
        assert_eq!(cwb_abi_version(), causewaybay_core::ABI_VERSION as c_int);
    }

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(owned(cwb_version()), causewaybay_core::VERSION);
    }

    #[test]
    fn describe_is_the_handshake_a_host_reads_first() {
        let described: Value = serde_json::from_str(&owned(cwb_describe())).unwrap();
        assert_eq!(described["ok"], true);
        assert_eq!(described["data"]["abi"], causewaybay_core::ABI_VERSION);
        assert_eq!(described["data"]["name"], "causewaybay-wallet");
        assert!(described["data"]["warning"]
            .as_str()
            .unwrap()
            .contains("unencrypted"));
    }

    #[test]
    fn the_command_tree_comes_across_whole() {
        let listed: Value = serde_json::from_str(&owned(cwb_commands())).unwrap();
        let leaves = listed["data"].as_array().unwrap();
        assert!(leaves.len() > 30, "got {} commands", leaves.len());

        let send = leaves
            .iter()
            .find(|c| c["name"] == "send")
            .expect("send is a command");
        let to = send["args"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "to")
            .expect("send takes --to");
        assert_eq!(to["required"], true);
        assert_eq!(to["takes_value"], true);
        assert_eq!(to["long"], "to");
    }

    #[test]
    fn the_chain_list_is_readable_without_a_wallet() {
        let listed: Value = serde_json::from_str(&owned(cwb_chains())).unwrap();
        assert_eq!(listed["ok"], true);
        let chains = listed["data"].as_array().unwrap();
        assert_eq!(chains.len(), 4);

        let names: Vec<&str> = chains
            .iter()
            .map(|c| c["chain"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["evm", "solana", "cardano", "midnight"]);

        for chain in chains {
            assert!(!chain["networks"].as_array().unwrap().is_empty());
            assert!(!chain["derivation_path"].as_str().unwrap().is_empty());
            assert!(chain["capabilities"]["faucet"].is_boolean());
        }
    }

    #[test]
    fn describe_carries_the_same_chains_as_the_direct_call() {
        let described: Value = serde_json::from_str(&owned(cwb_describe())).unwrap();
        let listed: Value = serde_json::from_str(&owned(cwb_chains())).unwrap();
        let from_describe: Vec<&str> = described["data"]["chains"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["chain"].as_str().unwrap())
            .collect();
        let direct: Vec<&str> = listed["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["chain"].as_str().unwrap())
            .collect();
        assert_eq!(from_describe, direct);
    }

    /// A host on the far side of the ABI must be able to reach every chain,
    /// which is the whole point of the `chain` request field.
    #[test]
    fn a_request_can_select_any_chain() {
        let dir = home();
        for chain in ["evm", "solana", "cardano", "midnight"] {
            let request = serde_json::json!({
                "argv": ["account", "import-mnemonic",
                         "-m", "abandon abandon abandon abandon abandon abandon \
                                abandon abandon abandon abandon abandon about",
                         "--label", chain],
                "home": dir.path().to_string_lossy(),
                "chain": chain,
                "yes": true,
            })
            .to_string();
            let reply = execute(&request);
            assert_eq!(reply["ok"], true, "{chain}: {reply}");
            assert_eq!(reply["data"]["chain"], chain);
        }

        // All four are in the one store, each on its own chain.
        let listed = execute(&request(&dir, &["account", "list"]));
        let rows = listed["data"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn an_unknown_chain_is_an_envelope_not_a_crash() {
        let dir = home();
        let request = serde_json::json!({
            "argv": ["info"],
            "home": dir.path().to_string_lossy(),
            "chain": "dogecoin",
        })
        .to_string();
        let reply = execute(&request);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "usage");
    }

    /// The field was added in ABI 2, so the number has to have moved with it.
    #[test]
    fn the_abi_version_moved_when_the_request_shape_changed() {
        assert!(
            cwb_abi_version() >= 2,
            "the chain field is an ABI 2 addition"
        );
    }

    #[test]
    fn an_offline_command_round_trips() {
        let dir = home();
        let reply = execute(&request(&dir, &["utils", "to-wei", "1.5"]));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["data"]["value"], "1500000000000000000");
    }

    #[test]
    fn a_wallet_survives_between_calls() {
        let dir = home();
        let made = execute(&request(&dir, &["account", "new", "--label", "alpha"]));
        assert_eq!(made["ok"], true, "{made}");
        let listed = execute(&request(&dir, &["account", "list"]));
        assert_eq!(listed["data"][0]["label"], "alpha");
    }

    #[test]
    fn a_null_request_is_an_error_not_a_segfault() {
        let reply = unsafe {
            let p = cwb_execute(std::ptr::null());
            let text = CStr::from_ptr(p).to_str().unwrap().to_owned();
            cwb_string_free(p);
            text
        };
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "usage");
    }

    #[test]
    fn freeing_null_is_allowed() {
        unsafe { cwb_string_free(std::ptr::null_mut()) };
    }

    #[test]
    fn malformed_json_comes_back_as_an_envelope() {
        let reply = execute("{ not json");
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "usage");
    }

    #[test]
    fn a_failing_command_keeps_its_code() {
        let dir = home();
        let reply = execute(&request(&dir, &["account", "show", "ghost"]));
        assert_eq!(reply["error"]["code"], "account_not_found");
    }

    #[test]
    fn non_utf8_input_is_rejected_cleanly() {
        // A C host can hand over any bytes at all; invalid UTF-8 is a message,
        // not a panic.
        let bytes = [0xffu8, 0xfe, 0x00];
        let reply = unsafe {
            let p = cwb_execute(bytes.as_ptr() as *const c_char);
            let text = CStr::from_ptr(p).to_str().unwrap().to_owned();
            cwb_string_free(p);
            text
        };
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "usage");
    }

    #[test]
    fn the_handbuilt_error_envelope_is_valid_json() {
        // It is the one envelope built without serde_json, so it gets checked.
        let parsed: Value = serde_json::from_str(&error_envelope("usage", "nope")).unwrap();
        assert_eq!(parsed["error"]["code"], "usage");
        assert_eq!(parsed["error"]["message"], "nope");
    }
}
