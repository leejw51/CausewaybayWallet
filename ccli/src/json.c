/* json.c — see json.h for what this is and is not. */

#include "json.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ writing */

/* Grow to hold `extra` more bytes plus a NUL. Latches `failed` and gives up. */
static int reserve(JsonBuf *buf, size_t extra)
{
    if (buf->failed) {
        return 0;
    }
    size_t needed = buf->len + extra + 1;
    if (needed <= buf->cap) {
        return 1;
    }
    size_t cap = buf->cap ? buf->cap : 256;
    while (cap < needed) {
        /* Overflow would wrap to a small cap and then to a heap overflow. */
        if (cap > (size_t)-1 / 2) {
            buf->failed = 1;
            return 0;
        }
        cap *= 2;
    }
    char *grown = realloc(buf->data, cap);
    if (!grown) {
        buf->failed = 1;
        return 0;
    }
    buf->data = grown;
    buf->cap = cap;
    return 1;
}

static void append(JsonBuf *buf, const char *text, size_t len)
{
    if (!reserve(buf, len)) {
        return;
    }
    memcpy(buf->data + buf->len, text, len);
    buf->len += len;
    buf->data[buf->len] = '\0';
}

void json_raw(JsonBuf *buf, const char *text)
{
    append(buf, text, strlen(text));
}

void json_string(JsonBuf *buf, const char *text)
{
    append(buf, "\"", 1);
    for (const unsigned char *p = (const unsigned char *)text; *p; p++) {
        switch (*p) {
        case '"':
            append(buf, "\\\"", 2);
            break;
        case '\\':
            append(buf, "\\\\", 2);
            break;
        case '\n':
            append(buf, "\\n", 2);
            break;
        case '\r':
            append(buf, "\\r", 2);
            break;
        case '\t':
            append(buf, "\\t", 2);
            break;
        case '\b':
            append(buf, "\\b", 2);
            break;
        case '\f':
            append(buf, "\\f", 2);
            break;
        default:
            if (*p < 0x20) {
                /* Any other control character has to go out as \uXXXX. */
                char escape[7];
                snprintf(escape, sizeof escape, "\\u%04x", *p);
                append(buf, escape, 6);
            } else {
                /* Including every byte >= 0x80: UTF-8 passes through as-is. */
                append(buf, (const char *)p, 1);
            }
        }
    }
    append(buf, "\"", 1);
}

void json_buf_free(JsonBuf *buf)
{
    free(buf->data);
    buf->data = NULL;
    buf->len = 0;
    buf->cap = 0;
    buf->failed = 0;
}

/* ------------------------------------------------------------------ reading */

int json_missing(JsonSlice slice)
{
    return slice.start == NULL;
}

static JsonSlice nothing(void)
{
    JsonSlice slice = { NULL, 0 };
    return slice;
}

static const char *skip_space(const char *p, const char *end)
{
    while (p < end && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r')) {
        p++;
    }
    return p;
}

/*
 * Walk past a JSON string, starting at its opening quote.
 *
 * The escape handling is the point: a `\"` inside a string is not the end of
 * it, and a message like "he said \"no\"" is a thing the wallet really emits.
 */
static const char *skip_string(const char *p, const char *end)
{
    if (p >= end || *p != '"') {
        return NULL;
    }
    p++;
    while (p < end) {
        if (*p == '\\') {
            /* Skip the backslash and whatever it escapes, together. */
            p += 2;
            continue;
        }
        if (*p == '"') {
            return p + 1;
        }
        p++;
    }
    return NULL;
}

/* Walk past any JSON value: object, array, string, number or literal. */
static const char *skip_value(const char *p, const char *end)
{
    p = skip_space(p, end);
    if (p >= end) {
        return NULL;
    }

    if (*p == '"') {
        return skip_string(p, end);
    }

    if (*p == '{' || *p == '[') {
        /*
         * Nesting is tracked with a depth counter rather than recursion, so a
         * deeply nested payload cannot overflow the stack — and strings are
         * skipped whole, so a brace inside one never counts.
         */
        int depth = 0;
        while (p < end) {
            if (*p == '"') {
                p = skip_string(p, end);
                if (!p) {
                    return NULL;
                }
                continue;
            }
            if (*p == '{' || *p == '[') {
                depth++;
            } else if (*p == '}' || *p == ']') {
                depth--;
                if (depth == 0) {
                    return p + 1;
                }
            }
            p++;
        }
        return NULL;
    }

    /* A number or a literal runs until the next structural character. */
    while (p < end && *p != ',' && *p != '}' && *p != ']' && *p != ' ' && *p != '\n'
           && *p != '\r' && *p != '\t') {
        p++;
    }
    return p;
}

/* True when the string at `p` (opening quote) is exactly `key`. */
static int key_is(const char *p, const char *end, const char *key)
{
    size_t len = strlen(key);
    if ((size_t)(end - p) < len + 2) {
        return 0;
    }
    /*
     * A key with an escape in it would not compare equal here. Every key the
     * wallet emits is plain ASCII, so that case cannot arise; if it ever did,
     * the lookup would report "not found" rather than match the wrong value.
     */
    return p[0] == '"' && memcmp(p + 1, key, len) == 0 && p[len + 1] == '"';
}

JsonSlice json_get_slice(JsonSlice object, const char *key)
{
    if (!object.start) {
        return nothing();
    }
    const char *p = object.start;
    const char *end = object.start + object.len;

    p = skip_space(p, end);
    if (p >= end || *p != '{') {
        return nothing();
    }
    p++;

    while (p < end) {
        p = skip_space(p, end);
        if (p >= end || *p == '}') {
            return nothing();
        }

        int matched = key_is(p, end, key);
        const char *after_key = skip_string(p, end);
        if (!after_key) {
            return nothing();
        }

        p = skip_space(after_key, end);
        if (p >= end || *p != ':') {
            return nothing();
        }
        p = skip_space(p + 1, end);

        const char *after_value = skip_value(p, end);
        if (!after_value) {
            return nothing();
        }
        if (matched) {
            JsonSlice found = { p, (size_t)(after_value - p) };
            return found;
        }

        p = skip_space(after_value, end);
        if (p < end && *p == ',') {
            p++;
        }
    }
    return nothing();
}

JsonSlice json_get(const char *object, const char *key)
{
    JsonSlice whole = { object, object ? strlen(object) : 0 };
    return json_get_slice(whole, key);
}

/* Write one code point as UTF-8. Returns how many bytes it took. */
static size_t encode_utf8(unsigned long code, char *out)
{
    if (code < 0x80) {
        out[0] = (char)code;
        return 1;
    }
    if (code < 0x800) {
        out[0] = (char)(0xC0 | (code >> 6));
        out[1] = (char)(0x80 | (code & 0x3F));
        return 2;
    }
    if (code < 0x10000) {
        out[0] = (char)(0xE0 | (code >> 12));
        out[1] = (char)(0x80 | ((code >> 6) & 0x3F));
        out[2] = (char)(0x80 | (code & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (code >> 18));
    out[1] = (char)(0x80 | ((code >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((code >> 6) & 0x3F));
    out[3] = (char)(0x80 | (code & 0x3F));
    return 4;
}

static int hex4(const char *p, unsigned long *out)
{
    unsigned long value = 0;
    for (int i = 0; i < 4; i++) {
        char c = p[i];
        value <<= 4;
        if (c >= '0' && c <= '9') {
            value |= (unsigned long)(c - '0');
        } else if (c >= 'a' && c <= 'f') {
            value |= (unsigned long)(c - 'a' + 10);
        } else if (c >= 'A' && c <= 'F') {
            value |= (unsigned long)(c - 'A' + 10);
        } else {
            return 0;
        }
    }
    *out = value;
    return 1;
}

char *json_unescape(JsonSlice slice)
{
    if (!slice.start || slice.len < 2 || slice.start[0] != '"') {
        return NULL;
    }
    const char *p = slice.start + 1;
    const char *end = slice.start + slice.len - 1; /* the closing quote */

    /* Decoding never grows the text, so the source length is always enough. */
    char *out = malloc(slice.len);
    if (!out) {
        return NULL;
    }
    size_t written = 0;

    while (p < end) {
        if (*p != '\\') {
            out[written++] = *p++;
            continue;
        }
        p++;
        if (p >= end) {
            break;
        }
        switch (*p) {
        case 'n': out[written++] = '\n'; p++; break;
        case 't': out[written++] = '\t'; p++; break;
        case 'r': out[written++] = '\r'; p++; break;
        case 'b': out[written++] = '\b'; p++; break;
        case 'f': out[written++] = '\f'; p++; break;
        case '"': out[written++] = '"'; p++; break;
        case '\\': out[written++] = '\\'; p++; break;
        case '/': out[written++] = '/'; p++; break;
        case 'u': {
            unsigned long code = 0;
            if (p + 5 > end || !hex4(p + 1, &code)) {
                free(out);
                return NULL;
            }
            p += 5;
            /*
             * A code point above the BMP arrives as a surrogate pair. Joining
             * them is what keeps an emoji in a label intact; a lone high
             * surrogate is passed through as itself rather than rejected,
             * since this is a display path and not a validator.
             */
            if (code >= 0xD800 && code <= 0xDBFF && p + 6 <= end && p[0] == '\\'
                && p[1] == 'u') {
                unsigned long low = 0;
                if (hex4(p + 2, &low) && low >= 0xDC00 && low <= 0xDFFF) {
                    code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                    p += 6;
                }
            }
            written += encode_utf8(code, out + written);
            break;
        }
        default:
            /* An unknown escape keeps its character, which is the least
             * surprising thing to show. */
            out[written++] = *p++;
        }
    }
    out[written] = '\0';
    return out;
}

int json_is_true(JsonSlice slice)
{
    return slice.start && slice.len == 4 && memcmp(slice.start, "true", 4) == 0;
}
