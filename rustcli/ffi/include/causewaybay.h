/*
 * causewaybay.h — the C ABI of the Causewaybay wallet.
 *
 * ⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk.
 *
 * One call does everything: hand cwb_execute a JSON request, get a JSON
 * envelope back. The wallet's vocabulary lives in the request's `argv`, so
 * this header never changes when a command is added.
 *
 *   Request  {"argv": ["account","list"],   // the command, without argv[0]
 *             "home": "/path/to/wallet",    // optional, else $CAUSEWAYBAY_HOME
 *             "network": "cronos-testnet",  // optional, this call only
 *             "yes": false,                 // answer confirmations with yes
 *             "stdin": null}                // what an argument of "-" means
 *
 *   Reply    {"ok":true,"data":{...},"human":"..."}
 *            {"ok":false,"error":{"code":"...","message":"..."}}
 *
 * `code` is one of the stable strings in SPEC.md — usage, not_found,
 * account_not_found, duplicate_label, invalid_mnemonic, invalid_private_key,
 * invalid_address, invalid_amount, no_active_account, unknown_network,
 * rpc_error, insufficient_funds, confirmation_required, io_error, internal.
 *
 * Ownership: every char* returned belongs to the caller and must be released
 * with cwb_string_free, and with nothing else. The library never keeps a
 * pointer you passed in.
 *
 * Threads: calls are independent and may run concurrently. The store is
 * append-only on disk, so two writers interleave lines rather than corrupt
 * each other — but a wallet is not a database, and concurrent writes to one
 * home are not something to design around.
 *
 * Failure: there isn't a path that returns null on error, and nothing unwinds
 * out of these functions. A panic inside the wallet arrives as an envelope
 * with code "internal".
 */

#ifndef CAUSEWAYBAY_H
#define CAUSEWAYBAY_H

#ifdef __cplusplus
extern "C" {
#endif

/*
 * The ABI this build speaks. Compare against CWB_ABI_VERSION before trusting
 * a library you loaded at runtime; a mismatch means the envelope shape moved.
 *
 * Within one ABI version the functions below never change meaning — but the
 * list may grow, and a host that declares a function an older library does not
 * export will fail when it first calls it, not when it loads. Checking this
 * number is what turns that into a clear message.
 */
#define CWB_ABI_VERSION 1

int cwb_abi_version(void);

/* The wallet version, e.g. "1.0.0". Free with cwb_string_free. */
char *cwb_version(void);

/*
 * A JSON envelope describing the library: name, version, abi, and the
 * networks it knows. The first call a host should make.
 * Free with cwb_string_free.
 */
char *cwb_describe(void);

/*
 * Every command the wallet accepts, as a JSON envelope whose `data` is a list
 * of {path, name, about, args}. Build a menu from it, or check a binding's
 * coverage against it. Free with cwb_string_free.
 */
char *cwb_commands(void);

/*
 * Run one request. `request_json` is a NUL-terminated JSON object; the result
 * is a NUL-terminated JSON envelope. A null or non-UTF-8 request comes back as
 * an envelope with code "usage".
 * Free with cwb_string_free.
 */
char *cwb_execute(const char *request_json);

/* Release a string this library returned. Passing NULL is a no-op. */
void cwb_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* CAUSEWAYBAY_H */
