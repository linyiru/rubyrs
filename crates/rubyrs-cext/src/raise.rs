//! Spike L3-A: `rb_raise` + `longjmp`-based exception propagation
//! across the C ABI boundary.
//!
//! See `c/setjmp_shim.c`'s header for the design (why setjmp lives
//! in C, the nested jmp-buf stack, the pending-exception slot).
//! This module is just the Rust-facing surface:
//!
//! - [`call_with_raise`] — wraps a Rust closure in `rubyrs_jmp_call`
//!   so that any `rb_raise` from inside the C call (transitively
//!   through any `extern "C"` work the closure does) is caught,
//!   not aborted. Returns the raised class id + message instead of
//!   the closure's normal `Value`. Used by `vm::cext_dispatch`.
//!
//! The actual `rb_raise` Rust-side export, plus the
//! `rb_eArgumentError` etc. class-handle constants, are added in
//! the next commit alongside the dispatch wire-up.

use std::ffi::{CStr, c_char, c_void};

unsafe extern "C" {
    /// See c/setjmp_shim.c. Returns the closure's u64 on normal
    /// return; on a raised exception, writes the class id +
    /// (heap-owned) message into the out-params and returns 0.
    fn rubyrs_jmp_call(
        cb: unsafe extern "C" fn(*mut c_void) -> u64,
        userdata: *mut c_void,
        out_raised_class: *mut u64,
        out_raised_msg: *mut *mut c_char,
    ) -> u64;
}

/// Outcome of a protected call into a C extension function.
///
/// `Returned(value)` — the C function returned normally with that
/// VALUE. `Raised { class, msg }` — `rb_raise` fired; the class
/// is whatever VALUE the C code passed as the first arg
/// (typically one of the `rb_e*` class-handle constants), and msg
/// is the formatted exception message (post-vsnprintf, owned).
#[derive(Debug)]
pub enum Raised {
    Returned(u64),
    Raised { class: u64, msg: String },
}

/// Invoke `f` under a setjmp protected frame. Any `rb_raise` call
/// that fires while `f` is on the call stack (including
/// transitively through other C → Ruby → C bridges) will be
/// caught and surfaced as [`Raised::Raised`] instead of aborting.
///
/// # Safety
///
/// `f` must not retain any Rust references to thread-local state
/// (`STATE` / `FUNCALL_CB` / `CURRENT_VM_PTR` / pending pin set)
/// across the call — on the raised path, RAII drops of any guards
/// the *caller* holds across this function ARE skipped (longjmp
/// fundamentally bypasses Rust's drop machinery). The host's
/// `cext_dispatch` defends by snapshotting + truncating those
/// stacks on both the normal and raised return paths, see
/// vm::cext_dispatch for the cleanup protocol.
pub fn call_with_raise<F: FnOnce() -> u64>(f: F) -> Raised {
    // Heap-box the closure so its FnOnce machinery survives the
    // round trip through C via a `*mut c_void` userdata pointer.
    // FnOnce can't be invoked through a bare fn-ptr without
    // assistance — the trampoline below pulls the Box back out
    // and calls the closure exactly once.
    struct Bundle<F: FnOnce() -> u64> {
        f: Option<F>,
    }

    unsafe extern "C" fn trampoline<F: FnOnce() -> u64>(ud: *mut c_void) -> u64 {
        let bundle = unsafe { &mut *(ud as *mut Bundle<F>) };
        let f = bundle.f.take().expect("ICE: call_with_raise trampoline fired twice");
        f()
    }

    let mut bundle = Bundle::<F> { f: Some(f) };
    let mut out_class: u64 = 0;
    let mut out_msg: *mut c_char = std::ptr::null_mut();
    let raw_result = unsafe {
        rubyrs_jmp_call(
            trampoline::<F>,
            (&raw mut bundle) as *mut c_void,
            &mut out_class as *mut u64,
            &mut out_msg as *mut *mut c_char,
        )
    };
    if out_class == 0 {
        Raised::Returned(raw_result)
    } else {
        // out_msg was malloc'd by the shim; we own it. Copy into a
        // Rust String and free immediately to avoid leaking on the
        // raised path.
        let msg = if out_msg.is_null() {
            String::new()
        } else {
            unsafe {
                let s = CStr::from_ptr(out_msg).to_string_lossy().into_owned();
                libc_free(out_msg as *mut c_void);
                s
            }
        };
        Raised::Raised { class: out_class, msg }
    }
}

unsafe fn libc_free(p: *mut c_void) {
    unsafe extern "C" {
        fn free(p: *mut c_void);
    }
    unsafe { free(p) }
}
