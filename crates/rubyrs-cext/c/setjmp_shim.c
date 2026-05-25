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
 * setjmp / call / check dance in a single C function — AND to do
 * the arity-specific cast + cext call from the same C function too,
 * so there are zero Rust frames between setjmp and the cext fn.
 *
 * That's this file:
 *
 *   - `rubyrs_jmp_invoke(func, arity, args, &out_class, &out_msg)`:
 *     installs a setjmp, dispatches on `arity` to cast the opaque
 *     `func` to the right signature, calls it with the args in
 *     `args[0..=arity]` (args[0] is self), returns the cext fn's
 *     u64 result OR — if rb_raise fires from inside the call — sets
 *     out_class/out_msg to the stashed exception class+message and
 *     returns 0. Caller (Rust `cext_dispatch`) builds the args
 *     array and validates `arity` before invoking.
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
 * before longjmp, consumed by rubyrs_jmp_invoke's raised branch. */
static __thread uint64_t g_pending_class = 0;
static __thread char *g_pending_msg = NULL;

/* Arity-specific function-pointer types for the cext dispatch. The
 * Rust host transmutes the registered `OpaqueFn` to one of these
 * based on the arity captured at `rb_define_*_function` time.
 * Done here in C (rather than via a generic Rust callback) so that
 * longjmp from rb_raise unwinds ONLY C frames between setjmp and
 * the cext call. Longjmp across a Rust frame would skip its RAII
 * `Drop`s and is implementation-defined; closing that gap was the
 * point of review #7 / #8 on PR #14. */
typedef uint64_t (*rubyrs_arity0_fn)(uint64_t);
typedef uint64_t (*rubyrs_arity1_fn)(uint64_t, uint64_t);
typedef uint64_t (*rubyrs_arity2_fn)(uint64_t, uint64_t, uint64_t);
typedef uint64_t (*rubyrs_arity3_fn)(uint64_t, uint64_t, uint64_t, uint64_t);
typedef uint64_t (*rubyrs_arity4_fn)(uint64_t, uint64_t, uint64_t, uint64_t, uint64_t);
typedef uint64_t (*rubyrs_arity5_fn)(uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t);
/* L3-H: variadic dispatch. CRuby's `rb_define_method(..., -1)` shape
 * is `VALUE func(int argc, const VALUE *argv, VALUE self)` — argc
 * and argv are separate from self. Used by any cext with a
 * `def initialize(*args)`-style entry point (msgpack's
 * Unpacker_initialize, flori/json's cParser_initialize, etc.).
 *
 * Calling convention used by case -1 below: invoke receives the
 * full caller args array as `[self, arg0, arg1, ...]` plus the
 * length `nargs` (separate parameter). The dispatch computes
 * argc = nargs - 1 and argv = &args[1], then invokes
 * `func(argc, argv, self)`. argc is NOT stored inside the args
 * array. */
typedef uint64_t (*rubyrs_arityN1_fn)(int, const uint64_t *, uint64_t);

/* Invoke the C extension function under a setjmp protected frame.
 *
 *   `func`  — the OpaqueFn registered by rb_define_*_function.
 *   `arity` — 0..5 fixed-arity, or -1 for variadic.
 *   `nargs` — total length of `args` (= fixed-arity + 1, or argc+1
 *             for variadic).
 *   `args`  — pointer to an array of length `nargs`; args[0] is the
 *             `self` handle, args[1..] are the call args.
 *
 * Returns the C function's u64 return value on normal return; on a
 * raised exception writes (class, msg) into the out-params and
 * returns 0 (meaningless — caller checks `*out_raised_class`).
 *
 * The whole call chain from `setjmp` to `func(...)` lives in C
 * frames, so a longjmp from rb_raise never unwinds a Rust frame. */
uint64_t rubyrs_jmp_invoke(void (*func)(void),
                           int arity,
                           int nargs,
                           const uint64_t *args,
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
        uint64_t result;
        switch (arity) {
            case 0:
                result = ((rubyrs_arity0_fn)func)(args[0]);
                break;
            case 1:
                result = ((rubyrs_arity1_fn)func)(args[0], args[1]);
                break;
            case 2:
                result = ((rubyrs_arity2_fn)func)(args[0], args[1], args[2]);
                break;
            case 3:
                result = ((rubyrs_arity3_fn)func)(args[0], args[1], args[2], args[3]);
                break;
            case 4:
                result = ((rubyrs_arity4_fn)func)(args[0], args[1], args[2], args[3], args[4]);
                break;
            case 5:
                result = ((rubyrs_arity5_fn)func)(args[0], args[1], args[2], args[3], args[4], args[5]);
                break;
            case -1:
                /* Variadic: args[0] is self; args[1..nargs] are the
                 * user args; argc = nargs - 1. */
                result = ((rubyrs_arityN1_fn)func)(nargs - 1, &args[1], args[0]);
                break;
            default:
                /* Caller (Rust cext_dispatch) validates arity before
                 * reaching us, so anything outside {-1, 0..5} is a
                 * host bug. */
                abort();
        }
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
         * called into C without going through rubyrs_jmp_invoke).
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
