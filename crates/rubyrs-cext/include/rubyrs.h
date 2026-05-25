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
#include <limits.h>
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

/* Singleton handles. Numeric values ARE the ABI — kept in sync
 * with the `pub static Q*: Value = N` exports in rubyrs-cext's
 * lib.rs (handles 0/1/2). Compile-time-constant macros (mirroring
 * CRuby's `enum ruby_special_consts` shape) so cexts can use them
 * in static initializers, e.g. `VALUE cFoo = Qnil;` at file scope.
 *
 * The Rust statics still exist as dlopen-resolvable symbols for
 * cexts that reference them by extern declaration; the macros
 * here just give the literal value path. */
#define Qnil   ((VALUE)0)
#define Qtrue  ((VALUE)1)
#define Qfalse ((VALUE)2)

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

/* NORETURN(decl): wrap a declaration with the `[[noreturn]]`-like
 * attribute. CRuby's macro shape: `NORETURN(static void foo(int a))`.
 * Without this define, the preprocessor leaves NORETURN(...) as
 * what looks like a function call, and the inner declaration's
 * arg names get parsed as K&R-style parameters — yielding
 * "undeclared identifier" on every named arg. */
#define NORETURN(decl) __attribute__((noreturn)) decl

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
extern VALUE rb_cString;
extern VALUE rb_cArray;
extern VALUE rb_cHash;
extern VALUE rb_cFloat;
extern VALUE rb_cInteger;
extern VALUE rb_cNumeric;
extern VALUE rb_cSymbol;
extern VALUE rb_cTrueClass;
extern VALUE rb_cFalseClass;
extern VALUE rb_cNilClass;
extern VALUE rb_cBasicObject;
extern VALUE rb_cModule;
extern VALUE rb_cClass;

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

/* ===== Array C ABI (Level 2-3) ===== */

/* Allocate a new empty Array. */
VALUE rb_ary_new(void);

/* Allocate a new empty Array with a capacity hint. Hint is advisory;
 * spike accepts and may pre-reserve internal storage but does not
 * enforce. CRuby compat: extensions also call `rb_ary_new2`. */
VALUE rb_ary_new_capa(long capa);
#define rb_ary_new2 rb_ary_new_capa

/* Append `v` to `ary`. Returns `ary` for chaining. */
VALUE rb_ary_push(VALUE ary, VALUE v);

/* Read element at signed `idx` (negative = from end). Out-of-range
 * returns Qnil, matching CRuby's `Array#[]`. */
VALUE rb_ary_entry(VALUE ary, long idx);

/* Array length in elements. */
long RARRAY_LEN(VALUE ary);

/* ===== Hash C ABI (Level 2-3) ===== */

/* Allocate a new empty Hash. */
VALUE rb_hash_new(void);

/* Set `h[key] = value`. Existing key is overwritten; new key is
 * appended (preserves insertion order, Ruby 1.9+ semantics).
 * Returns `value`. */
VALUE rb_hash_aset(VALUE h, VALUE key, VALUE value);

/* Get `h[key]`. Returns Qnil if key is absent (spike: no support for
 * Hash default proc / value). */
VALUE rb_hash_aref(VALUE h, VALUE key);

/* Register an instance method on a class or module.
 *
 * Spike L3-C. Mirrors CRuby's rb_define_method (defined in
 * ruby/intern.h). `klass` must be a class/module handle returned
 * by `rb_define_class_under` OR `rb_define_module` (both produce
 * CValue::Class handles internally — review #8 on PR #27); `name`
 * is a NUL-terminated method name; `func` is the C entry point
 * (transmuted at dispatch time according to `arity`); `arity`
 * is 0..5 in the spike.
 *
 * At call time, the receiver is passed as the FIRST argument
 * (CRuby convention: `static VALUE my_method(VALUE self, VALUE
 * arg1, ...)`). The cext typically extracts the wrapped data
 * pointer via `TypedData_Get_Struct(self, MyStruct, &my_type,
 * my_struct_ptr)`.
 *
 * Method-lookup priority on a Value::Object receiver:
 *   1. Script-defined methods on the class (a `def foo` wins).
 *   2. cext-registered methods (this surface) as fallback.
 *   3. NoMethodError otherwise.
 * Matches CRuby's "user override wins" semantics.
 *
 * IMPORTANT: VALUE handles are per-call, NOT process-stable.
 * Unlike CRuby where a VALUE is a pointer/tag valid for the
 * object's lifetime, rubyrs-cext indexes into the topmost per-
 * call CExtState which is pushed at every cext entry and popped
 * on return. A `klass` handle stashed in a C `static` from a
 * prior Init_/call IS NOT VALID later — the index points into a
 * popped state's value table.
 *
 * Practical rule: register the class AND its methods in the same
 * Init pass. Don't cache the VALUE returned by
 * rb_define_class_under / rb_define_module across calls. */
void rb_define_method(VALUE klass, const char *name,
                      VALUE (*func)(ANYARGS), int arity);

/* ===== Exception raising (Level 3-A) ===== */

/* Well-known exception class handles. Like CRuby, these are
 * pre-defined VALUEs (sentinels in rubyrs's opaque-handle scheme)
 * that the host maps back to its internal exception classes when a
 * `rb_raise` longjmp is caught. Pass any of these as the first arg
 * to `rb_raise`. */
extern VALUE rb_eRuntimeError;
extern VALUE rb_eArgumentError;
extern VALUE rb_eTypeError;
extern VALUE rb_eRangeError;
extern VALUE rb_eStandardError;
extern VALUE rb_eNoMethodError;
extern VALUE rb_eIOError;
extern VALUE rb_eNameError;
extern VALUE rb_eZeroDivError;
extern VALUE rb_eNotImpError;

/* Raise an exception. Formats `fmt` + varargs via vsnprintf into a
 * 1024-byte buffer (matching CRuby's RUBY_FATAL cap), stashes the
 * (class, msg) pair in a thread-local, and longjmps back to the
 * nearest enclosing C-ext entry point installed by the rubyrs host.
 * The host catches the longjmp and converts the stashed exception
 * into a Ruby-level exception that propagates through the user's
 * normal `rescue` handlers.
 *
 * `__attribute__((noreturn))` lets the compiler omit any code that
 * would follow a call to rb_raise in a C extension function. */
__attribute__((noreturn))
void rb_raise(VALUE exc_class, const char *fmt, ...);

/* ===== TypedData ABI (Level 3-B) ===== */

/* Function-pointer table inside `rb_data_type_t`. Mirrors CRuby's
 * ruby/ruby.h layout (dmark / dfree / dsize / reserved[2]) so a C
 * extension can declare a static `rb_data_type_t` against the host
 * header with no per-host adjustments.
 *
 * Spike scope: `dfree` is the only field the host currently calls
 * (during GC sweep when the wrapping slot is collected). `dmark`
 * and `dsize` are parsed for ABI compatibility but unused —
 * Ruby-references-inside-TypedData (the `dmark` use case) is
 * L3-B.1 follow-up. */
struct rb_data_type_struct;

typedef struct {
    void (*dmark)(void *);
    void (*dfree)(void *);
    size_t (*dsize)(const void *);
    /* dcompact: invoked by CRuby's compaction GC to update embedded
     * VALUE references after objects move. rubyrs has no compaction
     * GC, so this slot is parsed for ABI compat but never called.
     * Cexts initialize it; we ignore it. */
    void (*dcompact)(void *);
    void *reserved[1];
} rb_data_type_function_t;

typedef struct rb_data_type_struct {
    const char *wrap_struct_name;
    rb_data_type_function_t function;
    const struct rb_data_type_struct *parent;
    const void *data;
    VALUE flags;
} rb_data_type_t;

/* Wrap a C pointer in a Ruby Object of class `klass`, using
 * `type` as the type descriptor. The host allocates a fresh
 * Object slot whose lifetime is GC-managed; when collected, the
 * descriptor's `dfree(data)` fires.
 *
 * Returns the wrapped VALUE — usable immediately for nested
 * `rb_funcall` etc. and for return from the calling C function. */
VALUE rb_data_typed_object_wrap(VALUE klass, void *data, const rb_data_type_t *type);

/* TypedData_Wrap_Struct macro — same convenience wrapper as
 * CRuby. `klass` is the user-facing class; `type` is the static
 * `rb_data_type_t`; `data` is the C pointer. */
#define TypedData_Wrap_Struct(klass, type, data) \
    rb_data_typed_object_wrap((klass), (data), (type))

/* Type-check + extract. Pointer-identity check on the descriptor;
 * mismatch is a programmer error and currently panics (TypeError
 * raise wiring is L3-B.1 follow-up). Returns the data pointer
 * stashed by the matching `rb_data_typed_object_wrap` call. */
void *rb_check_typeddata(VALUE obj, const rb_data_type_t *type);

/* TypedData_Get_Struct macro — same shape as CRuby. Casts the
 * extracted pointer to the user-named C struct type and assigns
 * to `sval`. Typical use:
 *
 *   Counter *c;
 *   TypedData_Get_Struct(self, Counter, &counter_type, c);
 *   c->count += 1;
 */
#define TypedData_Get_Struct(obj, type, data_type, sval) \
    ((sval) = (type *)rb_check_typeddata((obj), (data_type)))

/* ===== Spike L3-D: trivial macros + stubs for flori/json wedge =====
 *
 * Cluster 1 — type predicates (host-side type-tag inspection).
 * rubyrs has VALUE as opaque u64; we don't expose the tag layout
 * to C. These predicates approximate the CRuby semantics enough
 * for json-shape dispatch:
 *   - FIXNUM_P / FLONUM_P / SPECIAL_CONST_P → defer to host
 *     via a tiny helper rb_value_is_fixnum / rb_value_is_flonum,
 *     declared below. (CRuby implements them as bit checks; we
 *     can't because there's no exposed tag bit.)
 *   - RB_TYPE_P / rb_type → host helper returns a CRuby-shape
 *     `enum ruby_value_type` int. */
typedef int rb_value_type_t;
#define T_NONE     0
#define T_NIL      1
#define T_TRUE     2
#define T_FALSE    3
#define T_FIXNUM   4
#define T_FLOAT    5
#define T_STRING   6
#define T_ARRAY    7
#define T_HASH     8
#define T_SYMBOL   9
#define T_OBJECT  10
#define T_CLASS   11
#define T_MODULE  12
#define T_DATA    13
/* T_BIGNUM: CRuby's arbitrary-precision integer. rubyrs only has
 * fixed-width i64 inside Number, so no value ever returns T_BIGNUM
 * from rb_type — but the constant must exist so cext type switches
 * compile. */
#define T_BIGNUM  14
#define T_REGEXP  15
#define T_STRUCT  16
#define T_RATIONAL 17
#define T_COMPLEX 18

int rb_value_type(VALUE v);
#define rb_type(v) ((int)rb_value_type(v))
#define RB_TYPE_P(v, t) (rb_value_type(v) == (t))
#define RB_BUILTIN_TYPE(v) rb_value_type(v)

int rb_value_is_fixnum(VALUE v);
int rb_value_is_flonum(VALUE v);
int rb_value_is_special_const(VALUE v);
#define RB_FIXNUM_P(v)        rb_value_is_fixnum(v)
#define RB_FLONUM_P(v)        rb_value_is_flonum(v)
#define RB_SPECIAL_CONST_P(v) rb_value_is_special_const(v)
#define FIXNUM_P(v)           RB_FIXNUM_P(v)
#define FLONUM_P(v)           RB_FLONUM_P(v)
#define SPECIAL_CONST_P(v)    RB_SPECIAL_CONST_P(v)

/* Cluster 2 — conversion macros. rubyrs already has rb_int2num /
 * rb_long2num / rb_num2int / rb_num2long; these macros wire the
 * "FIX" / "ID" aliases CRuby code expects. */
#define FIX2INT(v)   rb_num2int(v)
#define FIX2LONG(v)  rb_num2long(v)
#define LONG2FIX(n)  rb_long2num((long)(n))
#define INT2FIX(n)   rb_int2num((int)(n))

/* SYM2ID / ID2SYM — Symbol is a CValue::Sym(ID) on the host
 * side; the macros here forward to small helpers. */
ID rb_sym2id(VALUE sym);
VALUE rb_id2sym(ID id);
#define SYM2ID(sym) rb_sym2id(sym)
#define ID2SYM(id)  rb_id2sym(id)

/* Cluster 3 — branch hints. Free in CRuby; we just forward to
 * the standard `__builtin_expect` ones. */
#define RB_LIKELY(x)   __builtin_expect(!!(x), 1)
#define RB_UNLIKELY(x) __builtin_expect(!!(x), 0)
#ifndef LIKELY
#define LIKELY(x)   RB_LIKELY(x)
#endif
#ifndef UNLIKELY
#define UNLIKELY(x) RB_UNLIKELY(x)
#endif

/* Cluster 4 — GC barriers. rubyrs has mark-and-sweep, NO
 * generational / incremental / movable GC. Every write-barrier
 * macro the CRuby world cares about collapses to a plain
 * assignment here. rb_gc_mark / rb_gc_mark_movable / rb_gc_location
 * are no-ops because rubyrs walks roots from the host side and
 * doesn't ask cexts to enumerate. */
#define RB_OBJ_WRITE(a, slot, v)   ((*(slot)) = (v))
#define RB_OBJ_WRITTEN(a, oldv, v) ((void)0)
#define OBJ_WRITE(a, slot, v)      RB_OBJ_WRITE((a), (slot), (v))
#define OBJ_WRITTEN(a, ov, v)      RB_OBJ_WRITTEN((a), (ov), (v))

void rb_gc_mark(VALUE v);
void rb_gc_mark_movable(VALUE v);
VALUE rb_gc_location(VALUE v);
void rb_gc_register_mark_object(VALUE v);
void rb_global_variable(VALUE *var);

/* Ractor safety declaration — CRuby uses this to opt a cext
 * into the parallel-Ractor execution model. rubyrs is
 * single-threaded across the cext boundary; the macro is a
 * no-op. */
void rb_ext_ractor_safe(int safe);
#define RB_EXT_RACTOR_SAFE(x) rb_ext_ractor_safe(x)

/* Cluster 4a — additional sentinels + TypedData flags + truthy.
 *
 * Qundef is CRuby's "uninitialized" sentinel (distinct from Qnil);
 * used as default arg for rb_scan_args / rb_hash_lookup fallback.
 * rubyrs uses Qnil for "absent" — Qundef aliases to a fresh
 * never-allocated handle value (high bit set, like rb_e* sentinels). */
#define Qundef ((VALUE)0xC000000000000000ULL)

/* RTEST: CRuby truthiness — everything except Qnil and Qfalse is
 * truthy. */
#define RTEST(v) (((v) != Qnil) && ((v) != Qfalse))

/* rb_data_type_t flags. RUBY_TYPED_FREE_IMMEDIATELY tells the
 * GC to invoke dfree on sweep rather than deferring; rubyrs
 * always frees immediately (we don't have generational GC), so
 * this flag is informational only. */
#define RUBY_TYPED_FREE_IMMEDIATELY  1
#define RUBY_TYPED_WB_PROTECTED      2
#define RUBY_TYPED_FROZEN_SHAREABLE  4

/* RUBY_TYPED_DEFAULT_FREE: CRuby sentinel value for "this TypedData
 * uses system free()". When a `rb_data_type_t`'s `dfree` slot is
 * set to this value, GC sweep should call `free(data_ptr)` directly.
 * rubyrs's heap-sweep treats it as a regular `unsafe extern "C" fn`
 * pointer — supplying the libc `free()` here keeps the contract
 * intact without a separate sentinel branch on the host side. */
#define RUBY_TYPED_DEFAULT_FREE ((void (*)(void *))free)

/* TypedData_Make_Struct: combined alloc + wrap (CRuby convenience).
 *   sval = malloc(sizeof(type)); memset(sval, 0, sizeof(type));
 *   return TypedData_Wrap_Struct(klass, type_ptr, sval);
 * Matches CRuby's macro shape; the alloc'd struct is zero-init. */
#define TypedData_Make_Struct(klass, type, type_ptr, sval) ( \
    (sval) = (type *)ruby_xcalloc(1, sizeof(type)), \
    rb_data_typed_object_wrap((klass), (sval), (type_ptr)) \
)

/* RTYPEDDATA_DATA: CRuby's macro is an lvalue used both to read
 * (`p = RTYPEDDATA_DATA(v)`) and assign (`RTYPEDDATA_DATA(v) = NULL`,
 * eg parser.rl:304). We expose a host helper returning a
 * `void **` slot; deref makes it lvalue-compatible. */
void **rb_typeddata_data_slot(VALUE obj);
#define RTYPEDDATA_DATA(obj) (*rb_typeddata_data_slot(obj))

/* OBJ_FREEZE: CRuby freezes the object so further mutation raises
 * FrozenError. rubyrs's wedge doesn't enforce freeze on cext-side
 * Values (would need parallel freeze flags on every CValue
 * variant); no-op for now. flori/json freezes lookup tables which
 * are read-only by construction. */
#define OBJ_FREEZE(v) ((void)(v))
#define OBJ_FROZEN(v) (1)  /* conservative: always-frozen */

/* Module / global sentinels. rb_mKernel is the Kernel module
 * handle returned by `rb_define_module("Kernel")` at bootstrap;
 * cexts use it as a method-attach target for top-level methods.
 * rb_cObject is already declared near the top of this header. */
extern VALUE rb_mKernel;
extern VALUE rb_mComparable;
extern VALUE rb_mEnumerable;

/* Cluster 5 — Array/Hash helpers used by flori/json. */
VALUE rb_ary_new_from_values(long n, const VALUE *values);
#define RARRAY_AREF(ary, i)  rb_ary_entry((ary), (i))
long RHASH_SIZE(VALUE h);
VALUE rb_hash_new_capa(long capa);
#define RB_HASH_NEW_CAPA(n) rb_hash_new_capa((long)(n))
/* hash_bulk_insert: CRuby fast path for bulk Hash construction.
 * `argv` points to alternating key/value pairs; `n` is the
 * count of values (so 2 entries per pair). flori/json uses this
 * to build result Hashes without per-pair rb_hash_aset. */
void rb_hash_bulk_insert(long n, const VALUE *argv, VALUE hash);
#define RB_HASH_BULK_INSERT rb_hash_bulk_insert
/* hash_foreach takes a C callback invoked per (k,v). Returning
 * ST_CONTINUE (0) advances; ST_STOP (1) exits. */
#define ST_CONTINUE 0
#define ST_STOP     1
typedef int (*rb_hash_foreach_func)(VALUE key, VALUE value, VALUE data);
void rb_hash_foreach(VALUE hash, rb_hash_foreach_func cb, VALUE arg);

/* Cluster 6 — Class / module helpers. */
VALUE rb_obj_class(VALUE obj);
VALUE rb_class_name(VALUE cls);
VALUE rb_class_new_instance(int argc, const VALUE *argv, VALUE klass);
VALUE rb_const_get(VALUE klass, ID id);
VALUE rb_path_to_class(VALUE pathname);
int rb_obj_is_kind_of(VALUE obj, VALUE klass);
int rb_respond_to(VALUE obj, ID id);
VALUE rb_define_module_under(VALUE outer, const char *name);
void rb_define_alias(VALUE klass, const char *new_name, const char *old_name);
void rb_define_private_method(VALUE klass, const char *name,
                              VALUE (*func)(ANYARGS), int arity);
typedef VALUE (*rb_alloc_func_t)(VALUE klass);
void rb_define_alloc_func(VALUE klass, rb_alloc_func_t fn);
VALUE rb_call_super(int argc, const VALUE *argv);
VALUE rb_ivar_set(VALUE obj, ID id, VALUE val);
VALUE rb_ivar_get(VALUE obj, ID id);

/* Cluster 7 — exception helpers. */
VALUE rb_exc_new_str(VALUE klass, VALUE msg);
__attribute__((noreturn))
void rb_exc_raise(VALUE exception);
VALUE rb_rescue(VALUE (*body)(VALUE), VALUE arg,
                VALUE (*rescue)(VALUE, VALUE), VALUE rescue_arg);

/* Cluster 8 — argument-parsing helpers. */
void rb_check_arity(int argc, int min, int max);
int rb_scan_args(int argc, const VALUE *argv, const char *fmt, ...);

/* Cluster 9 — misc IO + warning + require. */
void rb_warn(const char *fmt, ...);
void rb_category_warn(const char *category, const char *fmt, ...);
#define RB_WARN_CATEGORY_DEPRECATED "deprecated"
VALUE rb_io_flush(VALUE io);
VALUE rb_io_write(VALUE io, VALUE str);
VALUE rb_require(const char *feature);
VALUE rb_vsprintf(const char *fmt, va_list args);

/* Cluster 10a — memory helpers. CRuby exposes ALLOC_N / xfree /
 * etc. as part of the public API; rubyrs maps them straight to
 * libc since we don't track per-allocation telemetry. xmalloc /
 * xfree historically panic-on-OOM (CRuby raises NoMemError);
 * we mirror by aborting (real Trap propagation is L3-A.1 work). */
#include <stdlib.h>
#include <string.h>
static inline void *ruby_xmalloc(size_t size) {
    void *p = malloc(size);
    if (!p) abort();  /* L3-A.1 follow-up: raise NoMemError */
    return p;
}
static inline void *ruby_xrealloc(void *ptr, size_t size) {
    void *p = realloc(ptr, size);
    if (!p && size != 0) abort();
    return p;
}
static inline void *ruby_xcalloc(size_t n, size_t size) {
    void *p = calloc(n, size);
    if (!p && n * size != 0) abort();
    return p;
}
static inline void ruby_xfree(void *ptr) { free(ptr); }
#define xmalloc(s)        ruby_xmalloc(s)
#define xrealloc(p, s)    ruby_xrealloc((p), (s))
#define xcalloc(n, s)     ruby_xcalloc((n), (s))
#define xfree(p)          ruby_xfree(p)
#define ALLOC(type)       ((type *)ruby_xmalloc(sizeof(type)))
#define ALLOC_N(type, n)  ((type *)ruby_xmalloc(sizeof(type) * (size_t)(n)))
#define REALLOC_N(var, type, n) \
    ((var) = (type *)ruby_xrealloc((var), sizeof(type) * (size_t)(n)))
#define MEMCPY(dst, src, type, n) \
    memcpy((dst), (src), sizeof(type) * (size_t)(n))
#define MEMMOVE(dst, src, type, n) \
    memmove((dst), (src), sizeof(type) * (size_t)(n))
#define MEMZERO(dst, type, n) \
    memset((dst), 0, sizeof(type) * (size_t)(n))

/* Cluster 9a — Number conversions. */
VALUE rb_ll2num(long long n);
VALUE rb_dbl2num(double d);
#define LL2NUM(n)  rb_ll2num((long long)(n))
#define ULL2NUM(n) rb_ll2num((long long)(n))
#define DBL2NUM(d) rb_dbl2num((double)(d))
VALUE rb_cstr2inum(const char *str, int base);

/* rb_path_to_class takes a Ruby String (VALUE); rb_path2class
 * takes a C string (const char *). CRuby exposes both as
 * distinct entry points, not as macro aliases. */
VALUE rb_path2class(const char *path);

/* Check_Type asserts that v's type tag is T (raises TypeError
 * otherwise). Mirrors CRuby's macro. */
void rb_check_type(VALUE v, int t);
#define Check_Type(v, t) rb_check_type((v), (t))

/* StringValue: convert to String via implicit to_str dispatch,
 * raise TypeError on failure. Takes a pointer because CRuby's
 * version replaces *v with the converted value. */
VALUE rb_string_value(VALUE *v);
#define StringValue(v)     rb_string_value(&(v))

/* rb_eArgError: alias for rb_eArgumentError used by older code.
 * CRuby exposes both; we forward the alias. */
#define rb_eArgError rb_eArgumentError

/* Additional encoding indices used by flori/json's binary-vs-utf8
 * dispatch. */
int rb_ascii8bit_encindex(void);

/* RBASIC_CLASS: returns the class VALUE of any object. Generator
 * uses it for fast dispatch (compare against rb_cString / rb_cHash
 * before falling through to method-call dispatch). */
VALUE rb_basic_class(VALUE obj);
#define RBASIC_CLASS(obj) rb_basic_class(obj)

/* RFLOAT_VALUE: extract the C double from a Float VALUE. CRuby
 * provides this as a struct field accessor; we expose as a host
 * function call. */
double rb_float_value(VALUE v);
#define RFLOAT_VALUE(v) rb_float_value(v)

/* rb_utf8_str_new_lit: CRuby variant that takes a string literal
 * and uses sizeof - 1 for length. We collapse to rb_utf8_str_new
 * via __builtin_strlen so a constant arg gets folded. */
#define rb_utf8_str_new_lit(s) rb_utf8_str_new((s), (long)(sizeof(s) - 1))
#define rb_str_new_lit(s)      rb_str_new((s), (long)(sizeof(s) - 1))

/* PRIsVALUE: CRuby's printf conversion specifier that prints a
 * VALUE via rb_obj_as_string (object → its inspect representation).
 * rubyrs has no printf hook to intercept this, so we fall back to
 * printing the raw u64 handle in hex. Output isn't pretty (cext
 * uses this in error messages), but it compiles and the message
 * is still distinguishable. A proper fix would route rb_raise's
 * fmt through a custom formatter — beyond wedge scope. */
#define PRIsVALUE "llx"

/* CLASS_OF: returns the class VALUE of any object. Same surface
 * as RBASIC_CLASS — distinct CRuby names that we collapse. */
#define CLASS_OF(obj) rb_obj_class(obj)

/* Cluster 10 — String helpers. */
VALUE rb_str_buf_new(long capa);
VALUE rb_str_dup(VALUE str);
VALUE rb_str_freeze(VALUE str);
VALUE rb_str_intern(VALUE str);
void  rb_str_set_len(VALUE str, long len);
VALUE rb_str_substr(VALUE str, long beg, long len);
VALUE rb_sym2str(VALUE sym);
double rb_cstr_to_dbl(const char *p, int badcheck);
VALUE rb_convert_type(VALUE val, int type, const char *cname, const char *method);

/* RB_GC_GUARD — keeps a Value live until the macro's call site.
 * In CRuby this prevents the optimizer from dropping the binding
 * before its heap reference is used. rubyrs's `Vm::pinned`
 * mechanism does the same job from the host side, but cexts
 * still write `RB_GC_GUARD(v)` for portability — no-op here. */
#ifndef RB_GC_GUARD
#define RB_GC_GUARD(v) ((void)(v))
#endif

/* ========================================================
 * msgpack-ruby additions (L3-E spike).
 *
 * Surface beyond what flori/json's L3-D wedge needed:
 *   - Bignum accessors (rubyrs has no arbitrary precision —
 *     all integers are i64; >i64 values overflow at convert)
 *   - rb_num2dbl (Float coercion)
 *   - rb_class_of / rb_class_inherited_p (ancestry)
 *   - rb_hash_lookup (returns Qnil for missing, like rb_hash_aref;
 *     real CRuby distinguishes "missing" from "default value")
 *   - Encoding macros for ENCODING_GET/SET on strings
 *   - RUBY_FUNC_EXPORTED visibility attr
 * ========================================================
 */

/* Visibility attr: CRuby uses this on Init_<gem>. */
#define RUBY_FUNC_EXPORTED __attribute__((visibility("default")))

/* rb_class_of: alias for the existing rb_basic_class helper. */
#define rb_class_of(v) rb_basic_class(v)

/* rb_class_inherited_p(child, parent): is child a subclass of parent
 * (or equal to it)? rubyrs has no inheritance modeling at the spike
 * level — return Qtrue conservatively so dispatch fallthrough works,
 * matching the "respond_to / kind_of return permissive default"
 * shape used elsewhere. */
VALUE rb_class_inherited_p(VALUE child, VALUE parent);

/* rb_hash_lookup(h, key): same as rb_hash_aref. CRuby uses this to
 * skip the default-value path; rubyrs doesn't track defaults so
 * the two are equivalent. */
#define rb_hash_lookup(h, k) rb_hash_aref((h), (k))

/* Numeric coercion */
double rb_num2dbl(VALUE v);

/* Bignum surface. rubyrs's Number is fixed i64; values outside
 * the range overflow lossily. msgpack's packer uses these to
 * decide narrow vs wide integer encoding. */
size_t rb_absint_size(VALUE v, int *nlz_bits_ret);
unsigned long long rb_big2ull(VALUE v);
long long rb_big2ll(VALUE v);
int rb_bignum_positive_p(VALUE v);
#define RBIGNUM_POSITIVE_P(v) rb_bignum_positive_p(v)

/* Encoding macros. ENCODING_GET returns the encindex of a String;
 * ENCODING_SET tags a String with an encindex. rubyrs is UTF-8-
 * everywhere so both collapse to no-op shape (GET always returns
 * the UTF-8 index; SET is ignored). Distinct from the encoding.h
 * functional accessors so cext code using either form compiles. */
#define ENCODING_GET(v) rb_enc_get_index(v)
#define ENCODING_SET(v, idx) ((void)(idx))

/* ========================================================
 * msgpack-ruby round 2 additions.
 * ========================================================
 */

/* Conversions */
#define NUM2SIZET(v)   ((size_t)rb_num2long(v))
#define SIZET2NUM(n)   rb_long2num((long)(n))
#define NUM2UINT(v)    ((unsigned int)rb_num2long(v))
#define ULONG2NUM(n)   rb_long2num((long)(n))
VALUE rb_ull2inum(unsigned long long n);
/* ULL2NUM was defined earlier as rb_ll2num cast; redefine to the
 * unsigned-truncating path now that we have a dedicated symbol. */
#undef ULL2NUM
#define ULL2NUM(n) rb_ull2inum(n)
VALUE rb_float_new(double d);

/* Array varargs ctor: rb_ary_new3(n, v1, v2, ...). CRuby exposes
 * this as a variadic alias for rb_ary_new_from_args. rubyrs's
 * variadic surface is limited; emulate via the existing
 * rb_ary_new_from_values + caller staging the args into a stack
 * array. msgpack uses small Ns (typically 2-3); macro form
 * dispatches up to N=5. */
VALUE rb_ary_new3(long n, ...);

/* Hash mutation */
VALUE rb_hash_clear(VALUE h);
VALUE rb_hash_dup(VALUE h);
VALUE rb_hash_freeze(VALUE h);

/* String mutation */
VALUE rb_str_buf_cat(VALUE str, const char *ptr, long len);
VALUE rb_str_replace(VALUE dst, VALUE src);
VALUE rb_String(VALUE v);  /* coerce to String via to_s */

/* Exception flow control */
__attribute__((noreturn))
void rb_bug(const char *fmt, ...);
VALUE rb_errinfo(void);
__attribute__((noreturn))
void rb_jump_tag(int tag);
VALUE rb_protect(VALUE (*body)(VALUE), VALUE arg, int *state);
VALUE rb_rescue2(VALUE (*body)(VALUE), VALUE body_arg,
                 VALUE (*rescue)(VALUE, VALUE), VALUE rescue_arg, ...);
extern VALUE rb_eEOFError;
extern VALUE rb_eFrozenError;
extern VALUE rb_eEncCompatError;

/* Class / object */
void rb_define_const(VALUE klass, const char *name, VALUE val);
const char *rb_obj_classname(VALUE v);
VALUE rb_obj_freeze(VALUE v);
int rb_obj_frozen_p(VALUE v);

/* Symbols */
ID rb_intern3(const char *name, long len, void *enc);

/* Struct — rubyrs doesn't model Struct; stub returns a Class
 * sentinel that supports `.new(args) -> Array-shape Object`. */
VALUE rb_struct_define(const char *name, ...);
VALUE rb_struct_new(VALUE klass, ...);
#define RSTRUCT_GET(s, i) rb_struct_aref((s), (i))
VALUE rb_struct_aref(VALUE s, long i);

/* msgpack-ruby round 3 additions. */
#define FIX2ULONG(v) ((unsigned long)rb_num2long(v))
VALUE rb_ll2inum(long long n);
VALUE rb_check_string_type(VALUE v);
void rb_include_module(VALUE klass, VALUE mod);
VALUE rb_str_resize(VALUE str, long len);
void rb_undef_alloc_func(VALUE klass);
VALUE rb_yield(VALUE arg);

#ifdef __cplusplus
}
#endif

#endif /* RUBYRS_H */
