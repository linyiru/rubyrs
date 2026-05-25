/* raise.c — Spike L3-A: rb_raise C-ABI surface.
 *
 * `rb_raise(class, fmt, ...)` is variadic, which is awkward to
 * express from Rust without a stable ABI for `...`. Easiest: put
 * the variadic shim in C, use vsnprintf to format the message,
 * then call rubyrs_jmp_raise (from setjmp_shim.c) to do the
 * longjmp.
 *
 * Buffer is sized at 1024 bytes — matches CRuby's RUBY_FATAL
 * cap for raise messages. Messages longer than that get
 * truncated, same as CRuby. A C ext writing a multi-KB raise
 * message is already misusing the API.
 *
 * Companion to setjmp_shim.c. Stays in C so the variadic call
 * is portable across SysV / AAPCS / Win64; Rust's C variadic
 * support (`std::ffi::VaList`) is still nightly-only.
 */

#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>

/* Forward decl — implemented in setjmp_shim.c. */
extern void rubyrs_jmp_raise(uint64_t class_id, const char *msg);

void rb_raise(uint64_t exc_class, const char *fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    rubyrs_jmp_raise(exc_class, buf);
}
