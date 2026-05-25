/* ruby/encoding.h — Spike L3-D shim. CRuby's M17N encoding API
 * is huge; rubyrs is UTF-8-everywhere internally, so most of the
 * surface collapses to no-ops or "the UTF-8 encoding". This is
 * enough to compile flori/json (which uses encoding APIs but
 * mostly to assert / tag strings as UTF-8). A real encoding-
 * aware impl is far beyond this wedge.
 *
 * What real CRuby provides here:
 *   - rb_encoding struct + per-encoding singletons
 *   - rb_enc_str_new / rb_enc_associate / rb_enc_get / rb_enc_check
 *   - coderange machinery (UNKNOWN/7BIT/VALID/BROKEN)
 *   - rb_str_encode, rb_enc_codepoint_len, etc.
 *
 * What this header provides:
 *   - rb_encoding as an opaque struct (alias for void).
 *   - rb_utf8_encoding() / rb_ascii8bit_encoding() return non-null
 *     sentinels — flori/json only compares for equality.
 *   - rb_enc_associate_index / rb_enc_get_index — no-ops; we
 *     always treat strings as UTF-8.
 *   - rb_enc_str_coderange returns ENC_CODERANGE_VALID.
 *   - rb_enc_interned_str delegates to rb_str_new + (we hope)
 *     the rubyrs interner is already UTF-8.
 *   - rb_usascii_encindex / rb_utf8_encindex return 0 / 1.
 *
 * If a cext actually relies on coderange-conditional behavior,
 * this stub will be wrong — but flori/json's parse / generate
 * paths take "is the encoding ASCII-7bit?" as a fast-path hint,
 * not a correctness requirement. Returning VALID is conservative
 * (assumes input is well-formed UTF-8).
 */

#ifndef RUBY_ENCODING_H
#define RUBY_ENCODING_H

#include "../rubyrs.h"  /* VALUE, RSTRING_LEN, etc. */

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque encoding. rubyrs doesn't track per-string encoding, so
 * this is just a tag the cext can compare-by-pointer against the
 * singletons returned by `rb_utf8_encoding()` etc. */
typedef struct rb_encoding_struct rb_encoding;

/* Coderange constants. rubyrs treats all strings as VALID UTF-8
 * for cext purposes (fast path), since we decode lossily into
 * `Rc<str>` on the host side. */
#define ENC_CODERANGE_UNKNOWN  0
#define ENC_CODERANGE_7BIT     1
#define ENC_CODERANGE_VALID    2
#define ENC_CODERANGE_BROKEN   4

/* Singletons. rubyrs returns distinct non-null pointers so a
 * cext doing `enc == rb_utf8_encoding()` works for the common
 * "tag this as UTF-8" pattern. */
rb_encoding *rb_utf8_encoding(void);
rb_encoding *rb_ascii8bit_encoding(void);
rb_encoding *rb_usascii_encoding(void);

/* Index API. rubyrs hardcodes USASCII=0, UTF8=1 — flori/json
 * only uses these for equality checks against handle-returned
 * indices, so the values are arbitrary as long as they're stable. */
int rb_usascii_encindex(void);
int rb_utf8_encindex(void);

/* Tagging: no-op in rubyrs (we don't track per-string encoding;
 * all strings act as UTF-8). The cext sets these "for safety"; we
 * ignore them. */
VALUE rb_enc_associate_index(VALUE str, int encindex);
int rb_enc_get_index(VALUE str);

/* Encoding-aware string constructors. Both delegate to
 * `rb_str_new(buf, len)` since rubyrs has no separate encoding
 * dimension; `enc` is recorded only to satisfy the cext's
 * type-check. */
VALUE rb_enc_str_new(const char *buf, long len, rb_encoding *enc);
VALUE rb_enc_interned_str(const char *buf, long len, rb_encoding *enc);
#define RB_ENC_INTERNED_STR(s, n, enc) rb_enc_interned_str((s), (n), (enc))

/* Common conveniences: UTF-8 / USASCII-tagged String constructors.
 * Both delegate to rb_str_new since rubyrs doesn't track per-string
 * encoding. */
VALUE rb_utf8_str_new(const char *buf, long len);
VALUE rb_utf8_str_new_cstr(const char *cstr);
VALUE rb_usascii_str_new(const char *buf, long len);

/* Coderange. Always VALID (we assume well-formed UTF-8 across
 * the cext boundary — see header doc). */
int rb_enc_str_coderange(VALUE str);

/* Raise-with-encoding. Same as rb_raise; the encoding tag is
 * discarded (we have one error path, not per-encoding). */
__attribute__((noreturn))
void rb_enc_raise(rb_encoding *enc, VALUE exc, const char *fmt, ...);

/* RB_ENCODING_GET on a String VALUE — we don't track per-string
 * encoding, so all strings report as UTF-8 (index 1). */
#define RB_ENCODING_GET(v) (rb_utf8_encindex())

#ifdef __cplusplus
}
#endif

#endif /* RUBY_ENCODING_H */
