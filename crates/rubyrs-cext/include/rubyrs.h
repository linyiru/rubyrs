/* rubyrs.h — minimal CRuby-shape C API surface.
 *
 * Level 0 spike. Implements only what's needed for a hello-world C ext:
 *   - VALUE as an opaque handle (Option A in the spike plan)
 *   - Qnil / Qtrue / Qfalse singletons
 *   - rb_str_new_cstr to allocate a String
 *   - rb_define_global_function to register a callback
 *
 * Existing CRuby-flavoured hello-world C extensions should compile
 * against this header unchanged.
 */

#ifndef RUBYRS_H
#define RUBYRS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque token. The real value lives in a per-Vm handle table on
 * the Rust side. C ext code MUST treat this as opaque — no bit
 * tricks at Level 0. (CRuby's FIXNUM_P / NIL_P style macros are
 * deferred to a later level when we decide whether to adopt
 * CRuby's tagged representation.) */
typedef uint64_t VALUE;

/* Singleton handles. Their numeric values are part of the ABI and
 * must match the constants defined in the Rust-side implementation. */
extern VALUE Qnil;
extern VALUE Qtrue;
extern VALUE Qfalse;

/* Mirrors CRuby's `ANYARGS` (empty in modern C). Callback type below
 * therefore has zero declared args; real callbacks have 1..N args
 * matching the registered arity, and the conversion happens at the
 * call site via `RUBY_METHOD_FUNC`. */
#define ANYARGS

/* Cast a typed callback function pointer to the opaque type accepted
 * by `rb_define_global_function` / future `rb_define_method`. Mirrors
 * CRuby's macro of the same name — extensions written for CRuby can
 * use this verbatim. */
#define RUBY_METHOD_FUNC(func) ((VALUE (*)(ANYARGS))(func))

/* Allocate a new Ruby String from a NUL-terminated C string.
 * The bytes are copied; the caller retains ownership of `s`. */
VALUE rb_str_new_cstr(const char *s);

/* Allocate a new Ruby String from arbitrary bytes (not necessarily
 * NUL-terminated). Length is in bytes. Spike scope: the bytes must
 * still be valid UTF-8 — we don't yet have a binary-string variant.
 * Real-bcrypt salt and hash output happen to be ASCII so this is
 * fine for Level 1. */
VALUE rb_str_new(const char *ptr, long len);

/* Return a pointer to the underlying byte buffer of a Ruby String.
 *
 * The pointer is borrowed from the per-call cext STATE and is valid
 * only for the duration of the current C function. Spike scope: the
 * buffer is NOT NUL-terminated (unlike CRuby). Use `RSTRING_LEN` and
 * pass an explicit length to any consumer. */
const char *RSTRING_PTR(VALUE v);

/* Length of a Ruby String in bytes (not characters). */
long RSTRING_LEN(VALUE v);

/* Register `func` as a top-level Ruby function callable as `name(args)`.
 *
 * `arity` follows CRuby conventions. Level 1 dispatches 0, 1, and 2;
 * other arities register but trap with ArgumentError when invoked. */
void rb_define_global_function(const char *name,
                               VALUE (*func)(ANYARGS),
                               int arity);

#ifdef __cplusplus
}
#endif

#endif /* RUBYRS_H */
