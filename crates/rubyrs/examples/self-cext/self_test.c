/* self_test.c — regression coverage for PR #2 review comment #1.
 *
 * Verifies that `rb_define_singleton_method`-registered callbacks
 * receive the *class object* as `self`, not `Qnil`, matching CRuby
 * — and that `rb_define_global_function`-registered callbacks still
 * receive `Qnil` (top-level functions are conceptually attached to
 * the main object, which extensions universally treat as opaque).
 *
 * The three checks below produce different output depending on
 * which branch the cext dispatcher took:
 *
 *   - `SelfCheck.from_module`       — singleton on module → self should NOT be Qnil
 *   - `SelfCheck::Inner.from_class` — singleton on class  → self should NOT be Qnil
 *   - `from_global`                 — top-level function  → self should be Qnil
 *
 * The Rust integration test pins each line of expected output, so
 * a regression to "always pass Qnil" or to "always pass class"
 * fails the test loudly with a diff that names the failing branch.
 */

#include "rubyrs.h"

static VALUE module_check(VALUE self) {
    if (self == Qnil) return rb_str_new_cstr("FAIL: module singleton received Qnil as self");
    return rb_str_new_cstr("ok: module singleton self is not Qnil");
}

static VALUE class_check(VALUE self) {
    if (self == Qnil) return rb_str_new_cstr("FAIL: class singleton received Qnil as self");
    return rb_str_new_cstr("ok: class singleton self is not Qnil");
}

static VALUE global_check(VALUE self) {
    if (self == Qnil) return rb_str_new_cstr("ok: global function self is Qnil");
    return rb_str_new_cstr("FAIL: global function received non-Qnil self");
}

void Init_self_test(void) {
    VALUE mod = rb_define_module("SelfCheck");
    VALUE cls = rb_define_class_under(mod, "Inner", rb_cObject);
    rb_define_singleton_method(mod, "from_module", RUBY_METHOD_FUNC(module_check), 0);
    rb_define_singleton_method(cls, "from_class",  RUBY_METHOD_FUNC(class_check),  0);
    rb_define_global_function( "from_global",      RUBY_METHOD_FUNC(global_check), 0);
}
