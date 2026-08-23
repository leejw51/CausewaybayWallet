//! A C ABI over the Causewaybay wallet: JSON in, JSON out.
//!
//! Six functions, one data type (`char *`), and one rule: every string this
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

/// Run one request.
///
/// `request_json` is a NUL-terminated JSON object — see `causewaybay_core::request::Request`:
///
/// ```json
/// {"argv": ["account","list"], "home": null, "network": null,
///  "yes": false, "stdin": null}
/// ```
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
    guarded(|| match borrow_str(request_json) {
        Ok(text) => api::execute_json(text),
        Err(envelope) => envelope,
    })
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
fn guarded(f: impl FnOnce() -> String + std::panic::UnwindSafe) -> *mut c_char {
    let envelope = std::panic::catch_unwind(f).unwrap_or_else(|_| {
        error_envelope(
            "internal",
            "the wallet panicked; this is a bug, and no state was written",
        )
    });
    into_c_string(envelope)
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
