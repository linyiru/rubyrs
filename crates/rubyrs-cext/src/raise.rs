//! Spike L3-A: `rb_raise` + `longjmp`-based exception propagation
//! across the C ABI boundary.
//!
//! See `c/setjmp_shim.c`'s header for the overall design (why
//! setjmp lives in C, the nested jmp-buf stack, the pending-
//! exception slot). This module is the Rust-facing surface:
//!
//! - [`call_with_raise`] — wraps a Rust closure in `rubyrs_jmp_call`
//!   so that any `rb_raise` fired from inside the C call
//!   (transitively through any `extern "C"` work the closure does)
//!   is caught, not aborted. Returns the raised class id + message
//!   instead of the closure's normal `Value`. Used by
//!   `vm::cext_dispatch`.
//!
//! ## Built-in exception class sentinels
//!
//! C extensions reference exception classes as global VALUEs
//! (`rb_eArgumentError` etc.). In CRuby these resolve to actual
//! `Class` objects on the heap; in our Option-A opaque-handle
//! model we instead reserve a small range of high u64 sentinels
//! and export them as `#[no_mangle] pub static` constants. C
//! extensions link them at dlopen time (same `extern VALUE`
//! mechanism as the `rb_e*` constants in MRI's `ruby.h`).
//!
//! When `rb_raise` fires, the class sentinel travels through
//! `rubyrs_jmp_raise` → `Raised::Raised { class, .. }`; the host
//! `vm.rs` maps the sentinel back to a rubyrs `RubyError` variant
//! ([`exception_class_name_for_sentinel`]) and constructs the
//! corresponding Trap. The sentinels sit in the top bits of the
//! u64 namespace so they can never collide with a regular
//! per-`CExtState` handle index (which is bottom-up from 3,
//! reserving 0/1/2 for Qnil/Qtrue/Qfalse).

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

// === Built-in exception class sentinels ===
//
// High u64 range, one per class. The actual numeric values are
// arbitrary; only their distinctness matters. Bump 0xE000... range
// to leave room for future families.
//
// `#[no_mangle] pub static` exports the symbol under the exact
// CRuby name so C extensions written against ruby.h link without
// modification. `#[used]` would matter for a cdylib (LTO might
// strip otherwise-unreferenced statics); since we're rlib + the
// host binary references each one via its own `#[used]` static
// (see crates/rubyrs/src/lib.rs), that's already handled.

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eRuntimeError:    super::Value = 0xE000_0000_0000_0001;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eArgumentError:   super::Value = 0xE000_0000_0000_0002;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eTypeError:       super::Value = 0xE000_0000_0000_0003;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eRangeError:      super::Value = 0xE000_0000_0000_0004;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eStandardError:   super::Value = 0xE000_0000_0000_0005;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eNoMethodError:   super::Value = 0xE000_0000_0000_0006;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eIOError:         super::Value = 0xE000_0000_0000_0007;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eNameError:       super::Value = 0xE000_0000_0000_0008;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eZeroDivError:    super::Value = 0xE000_0000_0000_0009;

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static rb_eNotImpError:     super::Value = 0xE000_0000_0000_000A;

/// Map a raised class sentinel back to the rubyrs exception-class
/// name string used by `RubyError::*::class_name()`. Returns
/// `"RuntimeError"` as the fallback for any unknown / future
/// sentinel — matches CRuby's behaviour when an unrecognised
/// VALUE is passed to `rb_raise`.
pub fn exception_class_name_for_sentinel(class: super::Value) -> &'static str {
    match class {
        x if x == rb_eRuntimeError    => "RuntimeError",
        x if x == rb_eArgumentError   => "ArgumentError",
        x if x == rb_eTypeError       => "TypeError",
        x if x == rb_eRangeError      => "RangeError",
        x if x == rb_eStandardError   => "StandardError",
        x if x == rb_eNoMethodError   => "NoMethodError",
        x if x == rb_eIOError         => "IOError",
        x if x == rb_eNameError       => "NameError",
        x if x == rb_eZeroDivError    => "ZeroDivisionError",
        x if x == rb_eNotImpError     => "NotImplementedError",
        _ => "RuntimeError",
    }
}
