/* setjmp_shim.c — Spike L3-A: rb_raise / longjmp across the FFI.
 *
 * CRuby C extensions raise exceptions via `rb_raise(rb_eFoo, fmt,
 * ...)`, which internally does a `longjmp` back to the topmost
 * `setjmp` installed by the interpreter at the C-ext entry point.
 * The interpreter then converts the stashed exception back into
 * its own exception-propagation machinery.
 *
 * In rubyrs that interpreter is in Rust. Rust's `extern "C"`
 * functions are `nounwind` (panic = abort across the boundary), so
 * we can't use Rust panics to model raise. We also can't put
 * `setjmp` in Rust frames safely — longjmp must return to the
 * exact frame that called setjmp, and that frame would have to be
 * the one calling the C extension (otherwise Rust RAII drops in
 * intermediate frames are skipped). The cleanest fix is to do the
 * setjmp / call / check dance in a single C function and route the
 * Rust caller's actual work through a function-pointer callback.
 *
 * That's this file:
 *
 *   - `rubyrs_jmp_call(cb, userdata, &out_class, &out_msg)`:
 *     installs a setjmp, calls `cb(userdata)`, returns the call's
 *     u64 result OR — if rb_raise fires from inside cb — sets
 *     out_class/out_msg to the stashed exception class+message and
 *     returns 0.
 *
 *   - `rubyrs_jmp_raise(class_id, msg)`: stashes the exception in
 *     thread-locals and longjmps to the topmost installed setjmp.
 *     Called by `rb_raise` (which lives in raise.c, vsnprintf's
 *     the variadic args, then calls this).
 *
 * Nesting matters: Ruby → C → Ruby → C → rb_raise should land at
 * the innermost C entry point, not the outermost. So we maintain a
 * tiny stack of jmp_bufs in a thread-local. Realistic nesting is
 * shallow (a few levels for callback bridges); we cap at 64 to
 * keep the static allocation small and fail loudly if anything
 * tries to go deeper.
 *
 * Thread-locality: matches the existing thread-local model in
 * `rubyrs-cext/src/lib.rs` (STATE, FUNCALL_CB, CURRENT_VM_PTR,
 * INTERN). rubyrs is single-Ruby-thread at the cext boundary; if
 * that ever changes, this needs the same mutex treatment as
 * INTERN's docstring forewarns.
 */

#include <setjmp.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define RUBYRS_JMP_MAX_NEST 64

typedef struct {
    int top;  /* -1 when empty; otherwise index of the most-recently-pushed buf */
    jmp_buf bufs[RUBYRS_JMP_MAX_NEST];
} jmp_stack_t;

static __thread jmp_stack_t g_jmps = { -1, {{0}} };

/* Pending exception slot — populated by rubyrs_jmp_raise just
 * before longjmp, consumed by rubyrs_jmp_call's raised branch. */
static __thread uint64_t g_pending_class = 0;
static __thread char *g_pending_msg = NULL;

uint64_t rubyrs_jmp_call(uint64_t (*cb)(void *),
                         void *userdata,
                         uint64_t *out_raised_class,
                         char **out_raised_msg) {
    if (g_jmps.top + 1 >= RUBYRS_JMP_MAX_NEST) {
        /* Programmer error — the host should not be nesting this
         * deep. Abort rather than silently corrupting the stack. */
        abort();
    }
    g_jmps.top += 1;
    int raised = setjmp(g_jmps.bufs[g_jmps.top]);
    if (raised == 0) {
        uint64_t result = cb(userdata);
        /* Normal return — pop the buf and clear the out-params. */
        g_jmps.top -= 1;
        *out_raised_class = 0;
        *out_raised_msg = NULL;
        return result;
    } else {
        /* Longjmp landed here from rubyrs_jmp_raise. Pop the buf
         * and hand the pending exception to the Rust caller. */
        g_jmps.top -= 1;
        *out_raised_class = g_pending_class;
        *out_raised_msg = g_pending_msg;
        g_pending_class = 0;
        g_pending_msg = NULL;
        return 0;  /* meaningless; caller checks *out_raised_class */
    }
}

void rubyrs_jmp_raise(uint64_t class_id, const char *msg) {
    if (g_jmps.top < 0) {
        /* No setjmp installed → cannot longjmp anywhere. This is a
         * programmer error in the rubyrs integration (some path
         * called into C without going through rubyrs_jmp_call).
         * Abort rather than silently dropping the raise. */
        abort();
    }
    g_pending_class = class_id;
    /* Copy msg into a thread-local owned buffer — the caller may
     * have allocated `msg` on its own stack via vsnprintf. */
    if (g_pending_msg) {
        free(g_pending_msg);
        g_pending_msg = NULL;
    }
    if (msg) {
        size_t n = strlen(msg);
        g_pending_msg = malloc(n + 1);
        if (g_pending_msg) {
            memcpy(g_pending_msg, msg, n + 1);
        }
    }
    longjmp(g_jmps.bufs[g_jmps.top], 1);
}
