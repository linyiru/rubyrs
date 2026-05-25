/* hello.c — minimal CRuby-shape "hello world" C extension.
 *
 * Compiled into a shared library (hello.so / hello.bundle / hello.dll)
 * and loaded by rubyrs via `require "/path/to/hello"`. Once loaded,
 * Ruby code can call `hello` to receive the string "hello from C".
 *
 * Source-compatible with CRuby's documented hello-world style — the
 * point of the Level 0 spike is that this file is not customised for
 * rubyrs in any way.
 */

#include "rubyrs.h"

static VALUE hello(VALUE self) {
    (void)self; /* unused — global function, self is Qnil */
    return rb_str_new_cstr("hello from C");
}

void Init_hello(void) {
    rb_define_global_function("hello", RUBY_METHOD_FUNC(hello), 0);
}
