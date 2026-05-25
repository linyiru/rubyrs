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

/* Allocate a new Ruby String from a NUL-terminated C string.
 * The bytes are copied; the caller retains ownership of `s`. */
VALUE rb_str_new_cstr(const char *s);

/* Register `func` as a top-level Ruby function callable as `name(args)`.
 *
 * `arity` follows CRuby conventions, but Level 0 only honours arity == 0.
 * Calls with other arities are accepted at register time but will fail
 * with NoMethodError-style traps when invoked from Ruby — Level 1 widens
 * this. */
void rb_define_global_function(const char *name,
                               VALUE (*func)(VALUE),
                               int arity);

#ifdef __cplusplus
}
#endif

#endif /* RUBYRS_H */
