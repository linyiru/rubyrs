/* mini_json.c — Spike L3-C wedge: minimal JSON parser/generator
 * as a C extension. Same API shape as flori/json without the
 * full Ragel parser:
 *
 *   MiniJson.parse(str)     -> Ruby Object  (Hash | Array | Int | Str | Bool | Nil)
 *   MiniJson.generate(obj)  -> String
 *
 * Hand-rolled recursive descent + recursive-print covers the
 * load-bearing cext API surface:
 *
 *   - rb_ary_new + rb_ary_push           (parser builds Arrays)
 *   - rb_hash_new + rb_hash_aset         (parser builds Hashes)
 *   - rb_str_new + rb_str_new_cstr       (string values + result)
 *   - rb_long2num / NUM2LONG             (integers)
 *   - rb_raise(rb_eArgumentError, ...)   (parse errors, with fmt)
 *   - RARRAY_LEN + rb_ary_entry          (generator iterates)
 *   - rb_funcall(obj, "to_s", 0)         (fallback for unknown types)
 *
 * Scope deliberately small: no floats, no escape sequences in
 * strings, no unicode, no streaming. Wedge proves the FFI
 * pattern, NOT JSON conformance. Real flori/json vendoring is
 * L3-D.
 *
 * Demonstrates the assertion-of-progress claim made in PR #19's
 * summary: "L3-A + L3-B combined unlock real wrapper gems." This
 * is the smallest possible production-shaped wedge that uses
 * both together (raise on bad input, recursive build of nested
 * structures via the L2-3 Array/Hash builders).
 */

#include <ctype.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "rubyrs.h"

/* ===== Parser ===== */

typedef struct {
    const char *src;     /* original string, for error msgs */
    const char *p;       /* current cursor */
    const char *end;     /* one past last */
} Parser;

static void skip_ws(Parser *ps) {
    while (ps->p < ps->end && isspace((unsigned char)*ps->p)) ps->p++;
}

static VALUE parse_value(Parser *ps);  /* forward */

static VALUE parse_string(Parser *ps) {
    /* Caller positioned us at the opening `"`. Read until the
     * closing `"`. No escapes in the wedge — `\"` would fool us
     * into ending early. The acceptance test only uses
     * non-special characters. */
    if (ps->p >= ps->end || *ps->p != '"') {
        rb_raise(rb_eArgumentError, "expected '\"' at offset %ld",
                 (long)(ps->p - ps->src));
    }
    ps->p++;
    const char *start = ps->p;
    while (ps->p < ps->end && *ps->p != '"') ps->p++;
    if (ps->p >= ps->end) {
        rb_raise(rb_eArgumentError, "unterminated string starting at offset %ld",
                 (long)(start - 1 - ps->src));
    }
    VALUE s = rb_str_new(start, ps->p - start);
    ps->p++;  /* skip closing " */
    return s;
}

static VALUE parse_number(Parser *ps) {
    const char *start = ps->p;
    if (ps->p < ps->end && (*ps->p == '-' || *ps->p == '+')) ps->p++;
    /* Track digits-only start AFTER consuming optional sign so a
     * bare "+" or "-" isn't accepted as a number (review #9 on PR
     * #27 — without this guard the `ps->p == start` check below
     * only catches the empty-input case, and strtol("+")/strtol("-")
     * silently return 0). */
    const char *digits_start = ps->p;
    while (ps->p < ps->end && isdigit((unsigned char)*ps->p)) ps->p++;
    if (ps->p == digits_start) {
        rb_raise(rb_eArgumentError, "expected digit at offset %ld",
                 (long)(start - ps->src));
    }
    char buf[64];
    long n = ps->p - start;
    if (n >= (long)sizeof(buf)) {
        rb_raise(rb_eArgumentError, "number too long at offset %ld",
                 (long)(start - ps->src));
    }
    memcpy(buf, start, n);
    buf[n] = 0;
    return rb_long2num(strtol(buf, NULL, 10));
}

static VALUE parse_array(Parser *ps) {
    /* Caller positioned at `[`. */
    ps->p++;
    VALUE arr = rb_ary_new();
    skip_ws(ps);
    if (ps->p < ps->end && *ps->p == ']') { ps->p++; return arr; }
    for (;;) {
        rb_ary_push(arr, parse_value(ps));
        skip_ws(ps);
        if (ps->p >= ps->end) {
            rb_raise(rb_eArgumentError, "unterminated array");
        }
        if (*ps->p == ',') { ps->p++; skip_ws(ps); continue; }
        if (*ps->p == ']') { ps->p++; return arr; }
        rb_raise(rb_eArgumentError, "expected ',' or ']' at offset %ld",
                 (long)(ps->p - ps->src));
    }
}

static VALUE parse_object(Parser *ps) {
    /* Caller positioned at `{`. */
    ps->p++;
    VALUE h = rb_hash_new();
    skip_ws(ps);
    if (ps->p < ps->end && *ps->p == '}') { ps->p++; return h; }
    for (;;) {
        skip_ws(ps);
        VALUE k = parse_string(ps);
        skip_ws(ps);
        if (ps->p >= ps->end || *ps->p != ':') {
            rb_raise(rb_eArgumentError, "expected ':' after key at offset %ld",
                     (long)(ps->p - ps->src));
        }
        ps->p++;
        skip_ws(ps);
        VALUE v = parse_value(ps);
        rb_hash_aset(h, k, v);
        skip_ws(ps);
        if (ps->p >= ps->end) {
            rb_raise(rb_eArgumentError, "unterminated object");
        }
        if (*ps->p == ',') { ps->p++; continue; }
        if (*ps->p == '}') { ps->p++; return h; }
        rb_raise(rb_eArgumentError, "expected ',' or '}' at offset %ld",
                 (long)(ps->p - ps->src));
    }
}

static int starts_with(Parser *ps, const char *lit) {
    long n = (long)strlen(lit);
    if (ps->end - ps->p < n) return 0;
    return memcmp(ps->p, lit, n) == 0;
}

static VALUE parse_value(Parser *ps) {
    skip_ws(ps);
    if (ps->p >= ps->end) {
        rb_raise(rb_eArgumentError, "unexpected end of input");
    }
    if (starts_with(ps, "true"))  { ps->p += 4; return Qtrue; }
    if (starts_with(ps, "false")) { ps->p += 5; return Qfalse; }
    if (starts_with(ps, "null"))  { ps->p += 4; return Qnil; }
    char c = *ps->p;
    if (c == '"')                 return parse_string(ps);
    if (c == '[')                 return parse_array(ps);
    if (c == '{')                 return parse_object(ps);
    if (c == '-' || c == '+' || isdigit((unsigned char)c)) return parse_number(ps);
    rb_raise(rb_eArgumentError, "unexpected character '%c' at offset %ld",
             c, (long)(ps->p - ps->src));
}

static VALUE mj_parse(VALUE self, VALUE str) {
    (void)self;
    long n = RSTRING_LEN(str);
    const char *p = RSTRING_PTR(str);
    Parser ps = { p, p, p + n };
    VALUE v = parse_value(&ps);
    skip_ws(&ps);
    if (ps.p != ps.end) {
        rb_raise(rb_eArgumentError, "trailing junk at offset %ld",
                 (long)(ps.p - ps.src));
    }
    return v;
}

/* ===== Generator ===== */

/* Cached method IDs for the generator's per-element calls into
 * Ruby — same pattern as callback-cext's rb_intern-at-Init. */
static ID id_class;
static ID id_to_s;
static ID id_name;
/* `+` is hit on EVERY string fragment append in mj_gen's Array/
 * Hash arms; caching avoids per-iteration rb_intern lookups
 * (review #3 / #7 on PR #27). */
static ID id_plus;
/* `to_a` is the Hash generator's iteration trampoline (no rb_yield
 * yet; see comment in mj_gen Hash arm). */
static ID id_to_a;

/* Append the generated form of `v` onto `out`. The C ext owns
 * `out` (Vm-managed VALUE) and mutates it via repeated rb_str_cat-
 * equivalent strategy: build small fragments via rb_str_new_cstr,
 * concatenate via the Ruby-level String#+ at the call site. For
 * the wedge this trades a little efficiency for using only the
 * existing rb_funcall surface — no new string-mutation API
 * needed. */
static VALUE mj_gen(VALUE v);

static VALUE class_name_of(VALUE v) {
    /* v.class.name as a Ruby String — used to format unknown
     * types' fallback shape. */
    VALUE cls = rb_funcallv(v, id_class, 0, NULL);
    return rb_funcallv(cls, id_name, 0, NULL);
}

static VALUE mj_gen(VALUE v) {
    if (v == Qnil)   return rb_str_new_cstr("null");
    if (v == Qtrue)  return rb_str_new_cstr("true");
    if (v == Qfalse) return rb_str_new_cstr("false");

    /* Probe class.name to dispatch on type. Avoids needing
     * TYPE() macros, which our cext header doesn't have yet. */
    VALUE cname_str = class_name_of(v);
    const char *cname = RSTRING_PTR(cname_str);

    if (strcmp(cname, "Integer") == 0) {
        long n = NUM2LONG(v);
        char buf[32];
        snprintf(buf, sizeof(buf), "%ld", n);
        return rb_str_new_cstr(buf);
    }
    if (strcmp(cname, "String") == 0) {
        /* Naive escaping: just wrap in quotes. Acceptance test
         * uses non-special chars; full escape handling is L3-D
         * vendoring of real flori/json.
         *
         * Bounds + OOM guard (review #10 on PR #27): refuse if
         * `n` is negative (RSTRING_LEN contract violation) or so
         * large that `n + 2` would overflow `size_t`. malloc
         * failure raises NoMemError-shape via rb_eRuntimeError
         * (we don't have rb_eNoMemError as a separate sentinel
         * yet — L3-A.1 follow-up). */
        long n = RSTRING_LEN(v);
        if (n < 0 || (size_t)n > SIZE_MAX - 3) {
            rb_raise(rb_eArgumentError, "string too large to JSON-encode (len=%ld)", n);
        }
        const char *p = RSTRING_PTR(v);
        char *buf = malloc((size_t)n + 3);
        if (!buf) {
            rb_raise(rb_eRuntimeError, "malloc failed for %ld-byte string", n + 3);
        }
        buf[0] = '"';
        memcpy(buf + 1, p, (size_t)n);
        buf[n + 1] = '"';
        buf[n + 2] = 0;
        VALUE out = rb_str_new(buf, n + 2);
        free(buf);
        return out;
    }
    if (strcmp(cname, "Array") == 0) {
        long n = RARRAY_LEN(v);
        /* Build "[e1,e2,e3]" by concatenating piece by piece. */
        VALUE out = rb_str_new_cstr("[");
        for (long i = 0; i < n; i++) {
            if (i > 0) {
                VALUE comma = rb_str_new_cstr(",");
                out = rb_funcall(out, id_plus, 1, comma);
            }
            VALUE elem_json = mj_gen(rb_ary_entry(v, i));
            out = rb_funcall(out, id_plus, 1, elem_json);
        }
        VALUE close = rb_str_new_cstr("]");
        out = rb_funcall(out, id_plus, 1, close);
        return out;
    }
    if (strcmp(cname, "Hash") == 0) {
        /* Hash iteration: use each_pair via rb_funcall ... but
         * we'd need block dispatch (rb_yield) which the wedge
         * doesn't have. Instead lean on Ruby-side to_a then
         * iterate the resulting Array of [k,v] pairs.
         *
         * VALUE pairs = h.to_a   →  [[k1,v1],[k2,v2],...]
         */
        VALUE pairs = rb_funcallv(v, id_to_a, 0, NULL);
        long n = RARRAY_LEN(pairs);
        VALUE out = rb_str_new_cstr("{");
        for (long i = 0; i < n; i++) {
            if (i > 0) {
                VALUE comma = rb_str_new_cstr(",");
                out = rb_funcall(out, id_plus, 1, comma);
            }
            VALUE pair = rb_ary_entry(pairs, i);
            VALUE k_json = mj_gen(rb_ary_entry(pair, 0));
            VALUE v_json = mj_gen(rb_ary_entry(pair, 1));
            VALUE colon = rb_str_new_cstr(":");
            out = rb_funcall(out, id_plus, 1, k_json);
            out = rb_funcall(out, id_plus, 1, colon);
            out = rb_funcall(out, id_plus, 1, v_json);
        }
        VALUE close = rb_str_new_cstr("}");
        out = rb_funcall(out, id_plus, 1, close);
        return out;
    }

    /* Fallback: call .to_s on the unknown type, wrap in quotes.
     * Matches CRuby JSON's "stringify if unknown" default.
     * Same OOM + overflow guards as the String arm above
     * (review #11 on PR #27). */
    VALUE s = rb_funcallv(v, id_to_s, 0, NULL);
    long n = RSTRING_LEN(s);
    if (n < 0 || (size_t)n > SIZE_MAX - 3) {
        rb_raise(rb_eArgumentError, "to_s result too large to JSON-encode (len=%ld)", n);
    }
    const char *p = RSTRING_PTR(s);
    char *buf = malloc((size_t)n + 3);
    if (!buf) {
        rb_raise(rb_eRuntimeError, "malloc failed for %ld-byte fallback string", n + 3);
    }
    buf[0] = '"';
    memcpy(buf + 1, p, (size_t)n);
    buf[n + 1] = '"';
    buf[n + 2] = 0;
    VALUE out = rb_str_new(buf, n + 2);
    free(buf);
    return out;
}

static VALUE mj_generate(VALUE self, VALUE v) {
    (void)self;
    return mj_gen(v);
}

void Init_mini_json(void) {
    id_class = rb_intern("class");
    id_to_s  = rb_intern("to_s");
    id_name  = rb_intern("name");
    id_plus  = rb_intern("+");
    id_to_a  = rb_intern("to_a");

    /* MiniJson Module with .parse / .generate as singleton
     * methods — mirrors `JSON.parse` / `JSON.generate`.
     *
     * rb_define_module + rb_define_singleton_method matches what
     * flori/json's top-level json.rb does after loading the C
     * ext (it defines JSON as a module and forwards parse/
     * generate to the ext). */
    VALUE mod = rb_define_module("MiniJson");
    rb_define_singleton_method(mod, "parse",    RUBY_METHOD_FUNC(mj_parse),    1);
    rb_define_singleton_method(mod, "generate", RUBY_METHOD_FUNC(mj_generate), 1);
}
