/* callback_ext.c — Level 2 spike wedge: rb_funcall round trip.
 *
 * Proves the load-bearing Level 2 hypothesis: a C extension can
 * call back into Ruby via `rb_funcall(v)` mid-flight, get the
 * result, and return it as the C ext's own value.
 *
 * Three callbacks exercise different return shapes:
 *
 *   - `apply_upcase(s)`    — dispatch `s.upcase` from C → String result
 *   - `string_length(s)`   — dispatch `s.length` from C → Integer result
 *   - `nil_check(v)`       — dispatch `v.nil?`  from C → Bool result
 *
 * The IDs for "upcase" / "length" / "nil?" are cached at `Init_` time,
 * mirroring how every real CRuby C ext (json, oj, sqlite3, ...) does
 * it — that exercises `rb_intern`'s contract that IDs survive past the
 * Init call.
 *
 * Each callback's return value crosses the FFI boundary twice (Ruby →
 * C in via rb_funcall, C → Ruby out via our wrapper), so any
 * regression in handle translation, per-call CExtState lifecycle, or
 * VM re-entrance shows up as wrong output.
 */

#include "rubyrs.h"

static ID id_upcase;
static ID id_length;
static ID id_nil_q;

static VALUE apply_upcase(VALUE self, VALUE s) {
    (void)self;
    return rb_funcallv(s, id_upcase, 0, NULL);
}

static VALUE string_length(VALUE self, VALUE s) {
    (void)self;
    return rb_funcallv(s, id_length, 0, NULL);
}

static VALUE nil_check(VALUE self, VALUE v) {
    (void)self;
    return rb_funcallv(v, id_nil_q, 0, NULL);
}

/* === L2-3 (Array + Hash builders from C) === */

/* Build [1, 2, 3, 4, 5] from C and return to Ruby. Exercises
 * rb_ary_new + rb_ary_push + Int interning across the FFI. */
static VALUE build_list(VALUE self) {
    (void)self;
    VALUE a = rb_ary_new();
    for (int i = 1; i <= 5; i++) {
        rb_ary_push(a, rb_int2num(i));
    }
    return a;
}

/* Build {"name"=>name, "len"=>name.length} from C. Demonstrates
 * Hash construction with mixed key/value types (Str/Int) AND
 * rb_funcall reentrance inside a Hash builder (length comes from
 * Ruby-side String#length). This mirrors how flori/json's
 * generator computes nested object structure. */
static VALUE build_pair(VALUE self, VALUE name) {
    (void)self;
    VALUE h = rb_hash_new();
    rb_hash_aset(h, rb_str_new_cstr("name"), name);
    VALUE len = rb_funcallv(name, id_length, 0, NULL);
    rb_hash_aset(h, rb_str_new_cstr("len"), len);
    return h;
}

/* Build [{"lang"=>"ruby"}, {"lang"=>"rust"}] — a nested
 * Array-of-Hashes shaped exactly like a JSON document fragment.
 * Verifies the recursive translator (CValue::Array containing
 * CValue::Hash handles) round-trips into a Vm Value::Array of
 * Value::Hash objects on the heap. */
static VALUE build_records(VALUE self) {
    (void)self;
    VALUE outer = rb_ary_new();
    const char *langs[] = { "ruby", "rust" };
    for (int i = 0; i < 2; i++) {
        VALUE h = rb_hash_new();
        rb_hash_aset(h, rb_str_new_cstr("lang"), rb_str_new_cstr(langs[i]));
        rb_ary_push(outer, h);
    }
    return outer;
}

void Init_callback_ext(void) {
    id_upcase = rb_intern("upcase");
    id_length = rb_intern("length");
    id_nil_q  = rb_intern("nil?");

    rb_define_global_function("apply_upcase",  RUBY_METHOD_FUNC(apply_upcase),  1);
    rb_define_global_function("string_length", RUBY_METHOD_FUNC(string_length), 1);
    rb_define_global_function("nil_check",     RUBY_METHOD_FUNC(nil_check),     1);
    rb_define_global_function("build_list",    RUBY_METHOD_FUNC(build_list),    0);
    rb_define_global_function("build_pair",    RUBY_METHOD_FUNC(build_pair),    1);
    rb_define_global_function("build_records", RUBY_METHOD_FUNC(build_records), 0);
}
