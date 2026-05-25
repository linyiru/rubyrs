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

/* CRuby's ruby.h transitively pulls in the C stdlib basics that most
 * extensions assume are available (NULL, free, size_t, memcpy, etc).
 * Mirror that so unmodified CRuby C extensions compile against us
 * without needing to add their own #include lines.
 * `<stdarg.h>` is for the `rb_funcall` static-inline wrapper. */
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

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

/* Test whether a VALUE is the nil singleton. Mirrors CRuby. */
#define NIL_P(v) ((v) == Qnil)

/* Pin a VALUE through the end of the enclosing scope so it can't be
 * GC'd while a C ext is using a borrowed pointer into its storage.
 * CRuby implements this via volatile + side-effect-laden trickery on
 * the C stack; rubyrs doesn't do conservative stack scanning, so
 * this is a no-op. The macro is still provided so unmodified CRuby
 * C extensions compile against our header. */
#define RB_GC_GUARD(v) ((void)(v))

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

/* CRuby exposes both rb_str_new_cstr and the shorter rb_str_new2
 * alias. Many older extensions (bcrypt-ruby included) write the
 * shorter form. */
#define rb_str_new2 rb_str_new_cstr

/* Allocate a new Ruby String from arbitrary bytes (not necessarily
 * NUL-terminated). Length is in bytes. Bytes are stored verbatim on
 * the C-ABI side; the host translates the resulting CValue back to
 * a rubyrs `Value::Str` lossily via UTF-8 conversion when the value
 * crosses the FFI boundary back into Ruby. Binary-safe at the C
 * layer; lossy at the Ruby layer. A binary-safe Ruby String variant
 * is future work. */
VALUE rb_str_new(const char *ptr, long len);

/* Return a pointer to the underlying byte buffer of a Ruby String.
 *
 * The pointer is borrowed from the per-call cext STATE and is valid
 * only for the duration of the current C function.
 *
 * Buffer guarantee: `CValue::Str` storage always ends with a
 * sentinel `'\0'` past the `RSTRING_LEN`-reported length, matching
 * CRuby's "one byte past the end is `\0`" invariant. Callers that
 * pass this pointer to `strlen` / `strcmp` / `crypt_ra` etc. get a
 * NUL-terminated view — but `RSTRING_LEN` does NOT count that
 * sentinel, so prefer the explicit length form when the string may
 * contain interior NULs. */
const char *RSTRING_PTR(VALUE v);

/* Length of a Ruby String in bytes (not characters). */
long RSTRING_LEN(VALUE v);

/* Return a frozen copy of `v` (or `v` itself if already frozen).
 * rubyrs spike: no-op — frozenness isn't tracked yet. */
VALUE rb_str_new_frozen(VALUE v);

/* Underlying functions for the StringValueCStr / StringValuePtr
 * macros below. CRuby's macros pass `&v` so the function can
 * coerce `*v` to a String in place; spike just borrows the
 * pointer. */
const char *rb_string_value_cstr(VALUE *v);
const char *rb_string_value_ptr(VALUE *v);

/* `StringValueCStr(v)` — coerce `v` to a String and return a
 * NUL-terminated `char *`. The macro's lvalue requirement on `v`
 * matches CRuby; extensions write `StringValueCStr(arg)` and
 * `arg` may be modified to the coerced String. */
#define StringValueCStr(v) rb_string_value_cstr(&(v))
#define StringValuePtr(v)  rb_string_value_ptr(&(v))

/* Integer ↔ VALUE conversions. CRuby exposes these as a mix of
 * macros and functions; we expose functions and add macros that
 * forward, matching the names extensions actually write. */
VALUE rb_int2num(int n);
VALUE rb_long2num(long n);
int rb_num2int(VALUE v);
long rb_num2long(VALUE v);
unsigned long rb_num2ulong(VALUE v);

#define INT2NUM(n)   rb_int2num((int)(n))
#define LONG2NUM(n)  rb_long2num((long)(n))
#define NUM2INT(v)   rb_num2int(v)
#define NUM2LONG(v)  rb_num2long(v)
#define NUM2ULONG(v) rb_num2ulong(v)
/* Convenience: bcrypt-ruby's `int size` field assignment. */
#define FIX2INT(v)   rb_num2int(v)
#define FIX2LONG(v)  rb_num2long(v)

/* Register `func` as a top-level Ruby function callable as `name(args)`.
 *
 * `arity` follows CRuby conventions. Spike dispatches 0–5; other
 * arities register but trap with ArgumentError when invoked. */
void rb_define_global_function(const char *name,
                               VALUE (*func)(ANYARGS),
                               int arity);

/* Sentinel handle for the implicit Object class. Provided so
 * `rb_define_class_under(parent, name, rb_cObject)` accepts its
 * third arg unchanged. Superclass is ignored at spike scope. */
extern VALUE rb_cObject;

/* Declare a top-level module. Returns a class/module handle that
 * can be passed to `rb_define_class_under` or
 * `rb_define_singleton_method`. */
VALUE rb_define_module(const char *name);

/* Declare a class nested under `parent` inheriting from `super`.
 * Spike scope: nesting becomes a `"parent::name"` joined string
 * used flat for top-level lookup; `super` is ignored. */
VALUE rb_define_class_under(VALUE parent, const char *name, VALUE super);

/* Attach a singleton method to a class/module. From Ruby this
 * dispatches via `Class.method_name(args)`. */
void rb_define_singleton_method(VALUE klass,
                                const char *name,
                                VALUE (*func)(ANYARGS),
                                int arity);

/* CRuby's `ID` — opaque identifier for an interned name (method,
 * symbol, class name). Stable across the process. C extensions
 * cache `rb_intern` results in static globals at `Init_` time and
 * pass them to `rb_funcall*`. */
typedef uint64_t ID;

/* Intern `name` to an [`ID`]. Idempotent: repeated calls with the
 * same name return the same `ID`. Stable across the per-call cext
 * state being reset. Backed by a thread-local table inside rubyrs
 * — effectively process-wide given rubyrs's current single-thread
 * cext model. See `pub type ID` in rubyrs-cext for the threading
 * scope rationale. */
ID rb_intern(const char *name);

/* Dispatch a Ruby method from C. Calls `recv.<id>(argv[..argc])`
 * on the host VM, returns the result as a fresh VALUE handle.
 *
 * Spike scope: returning value types are limited to what the host
 * cext FFI currently models (Nil / Bool / Str / Int / Class).
 * Exceptions raised from the Ruby side currently collapse to Nil
 * — proper `rb_raise`-style propagation lands when the C ABI gets
 * an exception machinery (Level 3+).
 *
 * Re-entrancy: the C extension is free to call `rb_funcallv`
 * arbitrarily many times nested; the host pushes a fresh cext
 * state on each dispatch and pops on return. */
VALUE rb_funcallv(VALUE recv, ID mid, int argc, const VALUE *argv);

/* Convenience wrapper for the CRuby-style `rb_funcall(recv, id, n,
 * arg1, arg2, ...)`. Implemented as a `static inline` C function
 * (not a macro) because CRuby C extensions universally write
 * `rb_funcall(obj, id, 0)` for no-arg dispatch — a macro built on
 * `(VALUE[]){ __VA_ARGS__ }` produces an empty compound-literal
 * initializer for that case, which ISO C rejects and modern
 * gcc/clang warn on. A real variadic function handles n == 0
 * cleanly.
 *
 * Variable-length stack allocation via `__builtin_alloca`
 * (available on gcc + clang, no header needed). The buffer lives
 * until the enclosing C-ext function returns, freed automatically
 * — `rb_funcallv` reads the bytes synchronously so the lifetime
 * is plenty. No fixed cap, no silent truncation. */
static inline VALUE rb_funcall(VALUE recv, ID mid, int n, ...) {
    VALUE *argv = NULL;
    if (n > 0) argv = (VALUE *)__builtin_alloca((size_t)n * sizeof(VALUE));
    va_list ap;
    va_start(ap, n);
    for (int i = 0; i < n; i++) argv[i] = va_arg(ap, VALUE);
    va_end(ap);
    return rb_funcallv(recv, mid, n, argv);
}

#ifdef __cplusplus
}
#endif

#endif /* RUBYRS_H */
