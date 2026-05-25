/* counter_ext.c — Spike L3-B acceptance: TypedData wrap + dfree.
 *
 * Defines a `Counter` class whose instances wrap a tiny C struct
 * `{ long count; }` on the heap. The struct is malloc'd by C and
 * registered with rubyrs's TypedData ABI; when the wrapping Ruby
 * Object is garbage-collected, the host fires `counter_free` on
 * the data pointer, which `free()`s the struct.
 *
 * Three exports cover the load-bearing properties:
 *
 *   - `Counter.create`       — singleton constructor; mallocs the
 *                              C struct, wraps it via TypedData,
 *                              returns the new Counter Object.
 *   - `Counter.inc(c)`       — singleton method taking a Counter
 *                              Object as arg; bumps c.count.
 *   - `Counter.value(c)`     — reads c.count back to Ruby as Int.
 *   - `Counter.free_count`   — global counter (in C static) of
 *                              how many times counter_free has
 *                              fired. The acceptance test reads
 *                              this to verify that GC actually
 *                              ran the dfree callback when an
 *                              unreferenced Counter was collected.
 *
 * API shape: L3-B shipped singleton-only methods (`Counter.inc(c)`)
 * because `rb_define_method` wasn't wired yet. L3-C added it; this
 * file now exposes BOTH:
 *
 *   Singleton (kept for the original L3-B acceptance test):
 *     Counter.create  / Counter.inc(c)  / Counter.value(c)
 *
 *   Instance methods (new in L3-C — exercised by
 *   tests/cext_instance_method.rs):
 *     c.bump   — instance-side equivalent of Counter.inc(c)
 *     c.peek   — instance-side equivalent of Counter.value(c)
 *
 * Different names so the existing L3-B acceptance test continues
 * to assert what it was meant to assert, without ambiguity over
 * which dispatch path it's actually exercising.
 */

#include <stdlib.h>
#include "rubyrs.h"

typedef struct {
    long count;
} Counter;

/* Global counter of dfree invocations. Read back to Ruby via
 * the `Counter.free_count` accessor; the acceptance test asserts
 * this rises after a GC sweep. */
static long g_free_count = 0;

static void counter_free(void *p) {
    g_free_count += 1;
    free(p);
}

/* The CRuby-shape type descriptor. Static + const so its pointer
 * (which the host identity-compares on rb_check_typeddata) is
 * stable for the program's lifetime. */
static const rb_data_type_t counter_type = {
    "Counter",                      /* wrap_struct_name */
    { NULL, counter_free, NULL, { NULL, NULL } },
    NULL,                           /* parent */
    NULL,                           /* data */
    0,                              /* flags */
};

/* Counter_klass cache REMOVED (review #6): rubyrs-cext's VALUE is
 * a per-dispatch handle into the current CExtState, not a stable
 * class object. Caching one in a C static from Init_ leaves a
 * stale handle by the time Counter.create runs.
 *
 * Use `self` instead — singleton methods receive the class itself
 * as `self`, freshly interned into the calling CExtState. */
static VALUE counter_create(VALUE self) {
    Counter *c = malloc(sizeof(Counter));
    if (!c) {
        rb_raise(rb_eRuntimeError, "Counter.create: malloc failed");
    }
    c->count = 0;
    return TypedData_Wrap_Struct(self, &counter_type, c);
}

static VALUE counter_inc(VALUE self, VALUE obj) {
    (void)self;
    Counter *c;
    TypedData_Get_Struct(obj, Counter, &counter_type, c);
    c->count += 1;
    return rb_long2num(c->count);
}

static VALUE counter_value(VALUE self, VALUE obj) {
    (void)self;
    Counter *c;
    TypedData_Get_Struct(obj, Counter, &counter_type, c);
    return rb_long2num(c->count);
}

static VALUE counter_free_count(VALUE self) {
    (void)self;
    return rb_long2num(g_free_count);
}

/* === L3-C: instance methods (rb_define_method dispatch path) ===
 *
 * Same TypedData backing as counter_inc/counter_value above, but
 * `self` IS the receiver (already a Counter Object), not a
 * separately-passed arg. The cext_instance_methods dispatch table
 * in vm/dispatch.rs routes c.bump / c.peek through these. */
static VALUE counter_bump(VALUE self) {
    Counter *c;
    TypedData_Get_Struct(self, Counter, &counter_type, c);
    c->count += 1;
    return rb_long2num(c->count);
}

static VALUE counter_peek(VALUE self) {
    Counter *c;
    TypedData_Get_Struct(self, Counter, &counter_type, c);
    return rb_long2num(c->count);
}

void Init_counter_ext(void) {
    VALUE klass = rb_define_class_under(rb_cObject, "Counter", rb_cObject);
    rb_define_singleton_method(klass, "create",     RUBY_METHOD_FUNC(counter_create),     0);
    rb_define_singleton_method(klass, "inc",        RUBY_METHOD_FUNC(counter_inc),        1);
    rb_define_singleton_method(klass, "value",      RUBY_METHOD_FUNC(counter_value),      1);
    rb_define_singleton_method(klass, "free_count", RUBY_METHOD_FUNC(counter_free_count), 0);

    /* L3-C: instance methods. Dispatched via vm/dispatch.rs's
     * Value::Object arm consulting cext_instance_methods. */
    rb_define_method(klass, "bump", RUBY_METHOD_FUNC(counter_bump), 0);
    rb_define_method(klass, "peek", RUBY_METHOD_FUNC(counter_peek), 0);

    /* `klass` falls out of scope with the Init_ frame; no global
     * cache needed — `self` parameter on each method gives us a
     * fresh handle to the same class. */
}
