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
 * NOTE on API shape: we use singleton-only methods here (not
 * `c.inc` instance dispatch) because `rb_define_method` isn't
 * wired in the spike — that's a small follow-up commit. The point
 * being proved is the dfree + TypedData mechanism, not method
 * dispatch sugar.
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

static VALUE Counter_klass;         /* captured in Init_; used by .create */

static VALUE counter_create(VALUE self) {
    (void)self;
    Counter *c = malloc(sizeof(Counter));
    if (!c) {
        rb_raise(rb_eRuntimeError, "Counter.create: malloc failed");
    }
    c->count = 0;
    return TypedData_Wrap_Struct(Counter_klass, &counter_type, c);
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

void Init_counter_ext(void) {
    Counter_klass = rb_define_class_under(rb_cObject, "Counter", rb_cObject);
    rb_define_singleton_method(Counter_klass, "create",     RUBY_METHOD_FUNC(counter_create),     0);
    rb_define_singleton_method(Counter_klass, "inc",        RUBY_METHOD_FUNC(counter_inc),        1);
    rb_define_singleton_method(Counter_klass, "value",      RUBY_METHOD_FUNC(counter_value),      1);
    rb_define_singleton_method(Counter_klass, "free_count", RUBY_METHOD_FUNC(counter_free_count), 0);
}
