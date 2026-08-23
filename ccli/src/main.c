/*
 * cwbwallet-c — the wallet as a C program, statically linked.
 *
 * ⚠️ EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk.
 *
 * The smallest honest front end in the repository, and the point of it is what
 * is *not* here. There is no argument parser: `argv` goes into the request and
 * clap parses it inside the library, so this cannot drift from `cwbwallet` the
 * way a second hand-written parser would. There is no cryptography, no store,
 * no RPC. What is left is the part that is genuinely C's: a request to build,
 * a reply to render, streams to choose between, and an exit status.
 *
 * It links `libcausewaybay_ffi.a`, so the result is one file with the whole
 * wallet inside it — no shared library to find at run time, nothing to install
 * beside it. (The Lua front end takes the other route and loads the cdylib at
 * run time, which is what LÖVE needs.)
 *
 *     cwbwallet-c account list
 *     cwbwallet-c --json balance
 *     echo "$MNEMONIC" | cwbwallet-c account import-mnemonic -m - -l main
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "causewaybay.h"
#include "json.h"

/* Matching the other front ends, and SPEC.md: 0 fine, 1 failed, 2 misused. */
#define EXIT_FAILED 1
#define EXIT_USAGE 2

/* Every command needs at most one of these; freeing is the caller's job. */
static void release(char *owned)
{
    if (owned) {
        cwb_string_free(owned);
    }
}

/*
 * Read all of standard input.
 *
 * Only called when an argument is the lone `-`, because there is no other
 * reason to block on a pipe — and blocking on one nobody wrote to is the most
 * annoying bug a CLI can have.
 */
static char *read_stdin(void)
{
    size_t cap = 4096, len = 0;
    char *text = malloc(cap);
    if (!text) {
        return NULL;
    }
    for (;;) {
        if (len + 1024 > cap) {
            size_t bigger = cap * 2;
            char *grown = realloc(text, bigger);
            if (!grown) {
                free(text);
                return NULL;
            }
            text = grown;
            cap = bigger;
        }
        size_t got = fread(text + len, 1, cap - len - 1, stdin);
        len += got;
        if (got == 0) {
            break;
        }
    }
    text[len] = '\0';
    return text;
}

static int argv_has(int argc, char **argv, const char *word)
{
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], word) == 0) {
            return 1;
        }
    }
    return 0;
}

/*
 * The first word that is not a flag: the subcommand, when there is one.
 *
 * Only the globals that take a value need listing — every other flag comes
 * after the subcommand, by which point the answer is already found. Without
 * this, `--home /tmp/w tui` would read "/tmp/w" as the command.
 */
static const char *first_command(int argc, char **argv)
{
    int skip = 0;
    for (int i = 1; i < argc; i++) {
        const char *word = argv[i];
        if (skip) {
            skip = 0;
        } else if (strcmp(word, "--home") == 0 || strcmp(word, "--network") == 0
                   || strcmp(word, "-n") == 0) {
            skip = 1;
        } else if (word[0] != '-') {
            return word;
        }
    }
    return NULL;
}

/* Build the request object the ABI takes. See SPEC.md §8.1. */
static char *build_request(int argc, char **argv, const char *stdin_text)
{
    JsonBuf buf = { 0 };
    json_raw(&buf, "{\"argv\":[");
    for (int i = 1; i < argc; i++) {
        if (i > 1) {
            json_raw(&buf, ",");
        }
        json_string(&buf, argv[i]);
    }
    json_raw(&buf, "]");
    if (stdin_text) {
        json_raw(&buf, ",\"stdin\":");
        json_string(&buf, stdin_text);
    }
    json_raw(&buf, "}");

    if (buf.failed) {
        json_buf_free(&buf);
        return NULL;
    }
    /* `home`, `network` and `yes` are not set here: they are flags in argv,
     * and the library's own parser is the one that should read them. */
    return buf.data;
}

/*
 * Print the machine envelope from SPEC.md §4.
 *
 * The reply carries a `human` field the two reference implementations do not
 * print, so it is dropped rather than passed through — `--json` output is
 * compared between front ends byte for byte. Rebuilding the envelope from the
 * verbatim `data` and `error` slices keeps it identical: serde_json sorts its
 * keys, so `data` before `ok` is the order to emit.
 */
static void print_envelope(const char *reply, int ok)
{
    JsonSlice body = json_get(reply, ok ? "data" : "error");
    if (json_missing(body)) {
        /* Cannot happen against this library; if it ever does, say so rather
         * than print half an envelope. */
        fprintf(stderr, "error [internal]: the reply had no %s\n", ok ? "data" : "error");
        return;
    }
    printf(ok ? "{\"data\":" : "{\"error\":");
    fwrite(body.start, 1, body.len, stdout);
    printf(ok ? ",\"ok\":true}\n" : ",\"ok\":false}\n");
}

/* The banner, taken from the library rather than restated here. */
static void print_warning(void)
{
    char *described = cwb_describe();
    if (!described) {
        return;
    }
    char *warning = json_unescape(json_get_slice(json_get(described, "data"), "warning"));
    if (warning) {
        fprintf(stderr, "%s\n", warning);
        free(warning);
    }
    release(described);
}

/* Report a failure the way the other front ends do, and pick the status. */
static int print_error(const char *reply)
{
    JsonSlice error = json_get(reply, "error");
    char *code = json_unescape(json_get_slice(error, "code"));
    char *message = json_unescape(json_get_slice(error, "message"));

    fprintf(stderr, "error [%s]: %s\n", code ? code : "internal", message ? message : "");
    int status = (code && strcmp(code, "usage") == 0) ? EXIT_USAGE : EXIT_FAILED;

    free(code);
    free(message);
    return status;
}

int main(int argc, char **argv)
{
    /* A library compiled against a different contract would answer a shape
     * this program does not know how to read. Checking costs nothing. */
    if (cwb_abi_version() != CWB_ABI_VERSION) {
        fprintf(stderr, "error [internal]: this program speaks ABI %d, the library speaks %d\n",
                CWB_ABI_VERSION, cwb_abi_version());
        return EXIT_FAILED;
    }

    const char *command = first_command(argc, argv);
    if (command && strcmp(command, "tui") == 0) {
        /* The library refuses it too, but it can only say "not here". This
         * can say where to go instead. */
        fprintf(stderr, "error [usage]: the terminal UI is only in the Rust CLI — "
                        "run `cwbwallet tui`\n");
        return EXIT_USAGE;
    }

    char *stdin_text = NULL;
    if (argv_has(argc, argv, "-")) {
        stdin_text = read_stdin();
        if (!stdin_text) {
            fprintf(stderr, "error [io_error]: could not read standard input\n");
            return EXIT_FAILED;
        }
    }

    char *request = build_request(argc, argv, stdin_text);
    free(stdin_text);
    if (!request) {
        fprintf(stderr, "error [internal]: out of memory building the request\n");
        return EXIT_FAILED;
    }

    char *reply = cwb_execute(request);
    free(request);
    if (!reply) {
        fprintf(stderr, "error [internal]: the wallet returned nothing\n");
        return EXIT_FAILED;
    }

    int as_json = argv_has(argc, argv, "--json");
    int ok = json_is_true(json_get(reply, "ok"));
    int status = 0;

    if (as_json) {
        /* One envelope on stdout, which stays the single machine channel. */
        print_envelope(reply, ok);
        if (!ok) {
            JsonSlice error = json_get(reply, "error");
            char *code = json_unescape(json_get_slice(error, "code"));
            status = (code && strcmp(code, "usage") == 0) ? EXIT_USAGE : EXIT_FAILED;
            free(code);
        }
    } else if (!ok) {
        status = print_error(reply);
    } else {
        /* Not for --help or --version, which the wallet answers before there
         * is anything to warn about. */
        if (!argv_has(argc, argv, "--help") && !argv_has(argc, argv, "-h")
            && !argv_has(argc, argv, "--version") && !argv_has(argc, argv, "-V")) {
            print_warning();
        }
        char *human = json_unescape(json_get(reply, "human"));
        printf("%s\n", human ? human : "");
        free(human);
    }

    release(reply);
    return status;
}
