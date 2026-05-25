/* raise_ext.c — Spike L3-A acceptance: rb_raise across the FFI.
 *
 * Three exports cover the load-bearing properties of rb_raise:
 *
 *   - `raise_argument_error(arg)`: unconditionally raises
 *     `rb_eArgumentError` with a fmt'd message. Proves the typed-
 *     variant mapping (rb_eArgumentError → RubyError::ArgumentError →
 *     `rescue ArgumentError`).
 *
 *   - `raise_runtime_error(arg)`: same shape but rb_eRuntimeError.
 *     Proves the rescue dispatcher matches the second-most-common
 *     class.
 *
 *   - `raise_unless_positive(n)`: conditional raise. Returns n
 *     when n > 0, otherwise raises rb_eArgumentError. Exercises
 *     the NORMAL exit path through the same dispatch entry point —
 *     this is the "rb_raise didn't fire, returned normally" case
 *     that has to keep working alongside the raise path. Without
 *     this, regression in the normal path could pass undetected as
 *     long as the raise tests themselves pass.
 *
 * Every raise message uses a %s / %ld format string so the test
 * also verifies the vsnprintf path in rb_raise's variadic shim.
 */

#include "rubyrs.h"

static VALUE raise_argument_error(VALUE self, VALUE arg) {
    (void)self; (void)arg;
    rb_raise(rb_eArgumentError, "bogus arg: %s", "expected positive");
    /* unreachable — rb_raise is __attribute__((noreturn)) */
}

static VALUE raise_runtime_error(VALUE self, VALUE arg) {
    (void)self; (void)arg;
    rb_raise(rb_eRuntimeError, "runtime boom in %s", "raise_runtime_error");
}

static VALUE raise_unless_positive(VALUE self, VALUE n_val) {
    (void)self;
    long n = NUM2LONG(n_val);
    if (n <= 0) {
        rb_raise(rb_eArgumentError, "expected positive, got %ld", n);
    }
    /* Round-trip via rb_long2num: NUM2LONG returned `long`, so
     * downcasting to `int` would silently truncate on 64-bit
     * platforms when the caller passes a value outside int range
     * (review #3). */
    return rb_long2num(n);
}

void Init_raise_ext(void) {
    rb_define_global_function("raise_argument_error",   RUBY_METHOD_FUNC(raise_argument_error),   1);
    rb_define_global_function("raise_runtime_error",    RUBY_METHOD_FUNC(raise_runtime_error),    1);
    rb_define_global_function("raise_unless_positive",  RUBY_METHOD_FUNC(raise_unless_positive),  1);
}
