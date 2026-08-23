/*
 * json.h — just enough JSON for one side of a conversation.
 *
 * The C front end has exactly two JSON jobs: build a request object, and read
 * four fields back out of a reply. That is far less than a JSON library, and
 * pulling one in for it would mean a vendored dependency in a repository whose
 * whole point is that each front end stands on its own.
 *
 * So: an escaping writer, and a scanner that can find a key at the top level of
 * an object. The scanner is a real one — it skips nested objects, arrays and
 * escaped strings correctly, because a `data` payload containing the string
 * "}" is an ordinary Tuesday and a brace-counting shortcut would break on it.
 *
 * What it deliberately does not do: build a tree, convert numbers, or validate.
 * The input always comes from serde_json a microsecond earlier.
 */

#ifndef CWB_JSON_H
#define CWB_JSON_H

#include <stddef.h>

/* ------------------------------------------------------------------ writing */

/*
 * A growable text buffer. Zero-initialise it, append to it, free it once.
 *
 * Every append checks for failure and latches it in `failed`, so a caller can
 * build a whole request and test once at the end rather than after each piece.
 */
typedef struct {
    char *data;
    size_t len;
    size_t cap;
    int failed; /* nonzero once any append could not allocate */
} JsonBuf;

/* Append raw text, exactly as given. */
void json_raw(JsonBuf *buf, const char *text);

/*
 * Append `text` as a quoted JSON string, escaping what has to be escaped:
 * quotes, backslashes, and every control character below 0x20. Bytes at or
 * above 0x80 pass through untouched, which is correct for UTF-8 input and is
 * what argv gives us.
 */
void json_string(JsonBuf *buf, const char *text);

/* Release the buffer and zero it, so a double free is a no-op. */
void json_buf_free(JsonBuf *buf);

/* ------------------------------------------------------------------ reading */

/*
 * A slice of a JSON document: `start` points into the original text and is not
 * NUL-terminated. `len` of 0 with a NULL `start` means "not found".
 */
typedef struct {
    const char *start;
    size_t len;
} JsonSlice;

/* True when a lookup found nothing. */
int json_missing(JsonSlice slice);

/*
 * Find `key` at the top level of the JSON object `object` points at, and
 * return its value verbatim — a string value keeps its surrounding quotes and
 * its escapes, so it can be re-emitted byte for byte.
 *
 * Only the top level: `json_get(reply, "code")` will not reach inside
 * `reply.error`. Look up `error` first, then `code` in what comes back.
 */
JsonSlice json_get(const char *object, const char *key);

/* As `json_get`, but starting from a slice rather than a NUL-terminated string. */
JsonSlice json_get_slice(JsonSlice object, const char *key);

/*
 * Decode a JSON string value into a freshly allocated C string, undoing the
 * escapes. `slice` must be a string value, quotes included, as `json_get`
 * returns it. Returns NULL when the slice is not a string, or on failure.
 *
 * The caller owns the result and frees it.
 */
char *json_unescape(JsonSlice slice);

/* True when the slice is exactly the literal `true`. */
int json_is_true(JsonSlice slice);

#endif /* CWB_JSON_H */
