//! GC / warn / IO no-op stubs (L3-D bulk-stub batch).
//!
//! See `stubs/mod.rs`. These collapse to no-op (rubyrs GC is not
//! cooperative with cext-side mark/location, and warn/IO route
//! through different paths).

use std::ffi::{c_char, c_int, c_long, c_void, CStr};

use crate::{Qnil, Value};

/// No-op: rubyrs GC marking is host-side.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_gc_mark(_v: Value) {}

/// No-op: no compaction, no movable mark.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_gc_mark_movable(_v: Value) {}

/// No compaction; objects don't move.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_gc_location(v: Value) -> Value {
    v
}

/// No-op: rubyrs roots are managed elsewhere.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_gc_register_mark_object(_v: Value) {}

/// No-op: accept the call but don't actually track the static.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_global_variable(_var: *mut Value) {}

/// No-op: rubyrs has no ractors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ext_ractor_safe(_safe: c_int) {}

/// Print fmt as a raw string; varargs intentionally ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_warn(fmt: *const c_char) {
    if fmt.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(fmt) }.to_bytes();
    eprintln!("[rb_warn] {}", String::from_utf8_lossy(s));
}

/// Same as rb_warn but with a category prefix; varargs ignored.
///
/// PR #42 review #1 fix: header declares `category` as
/// `const char *`, matching CRuby (cf RB_WARN_CATEGORY_DEPRECATED =
/// "deprecated" string literal). Previously typed as `c_int` —
/// flori/json's caller would pass the string pointer, the stub
/// would interpret its low bits as an int → ABI mismatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_category_warn(category: *const c_char, fmt: *const c_char) {
    if fmt.is_null() {
        return;
    }
    let cat = if category.is_null() {
        std::borrow::Cow::Borrowed("?")
    } else {
        let s = unsafe { CStr::from_ptr(category) }.to_bytes();
        String::from_utf8_lossy(s)
    };
    let s = unsafe { CStr::from_ptr(fmt) }.to_bytes();
    eprintln!("[rb_warn:{}] {}", cat, String::from_utf8_lossy(s));
}

/// Spike: return Qnil (callers using this for rb_raise get a generic error).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_vsprintf(_fmt: *const c_char, _ap: *mut c_void) -> Value {
    Qnil
}

/// No-op flush.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_io_flush(_io: Value) -> Value {
    Qnil
}

/// No-op write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_io_write(_io: Value, _str: Value) -> Value {
    Qnil
}

/// Log and ignore — Ruby-side hasn't loaded the feature yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_require(name: *const c_char) -> Value {
    if name.is_null() {
        return Qnil;
    }
    let s = unsafe { CStr::from_ptr(name) }.to_bytes();
    eprintln!("rb_require: ignoring {}", String::from_utf8_lossy(s));
    Qnil
}

// ===== msgpack-ruby additions =====

// Fatal "ruby bug" abort. Print fmt as-is (varargs dropped) and abort.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_bug(fmt: *const c_char) -> ! {
    if !fmt.is_null() {
        let s = unsafe { CStr::from_ptr(fmt) }.to_bytes();
        eprintln!("[rb_bug] {}", String::from_utf8_lossy(s));
    }
    std::process::abort()
}

// $! — exception in current rescue. Spike doesn't model dynamic
// exception state from cext side; always Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_errinfo() -> Value { Qnil }

// Re-raise the tag set by a prior rb_protect. Spike has no protect
// state to re-raise.
//
// PR #46 review #1: must NOT use panic!() — unwinding across an
// extern "C" boundary is undefined behavior under the default
// panic=unwind strategy. Route to stderr + std::process::abort()
// for deterministic behavior across panic strategies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_jump_tag(tag: c_int) -> ! {
    eprintln!("[rb_jump_tag] called with tag={} — not implemented at spike scope; aborting", tag);
    std::process::abort()
}

// rb_protect(body, arg, &state): invoke body; set *state = 0 on
// success or non-zero on exception. Spike: just call body, no
// exception catch. *state = 0 always (no exception).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_protect(
    body: extern "C" fn(Value) -> Value,
    arg: Value,
    state: *mut c_int,
) -> Value {
    if !state.is_null() {
        unsafe { *state = 0; }
    }
    body(arg)
}

// rb_rescue2 — like rb_rescue but takes a variadic list of exception
// classes after rescue_arg. Spike forwards body without rescue
// (variadic list ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_rescue2(
    body: extern "C" fn(Value) -> Value,
    body_arg: Value,
    _rescue: extern "C" fn(Value, Value) -> Value,
    _rescue_arg: Value,
) -> Value {
    body(body_arg)
}

// rb_yield(arg): yield arg to the caller's block. Spike doesn't
// expose cext-side reentry into Ruby blocks — return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_yield(_arg: Value) -> Value { Qnil }

// Intern with explicit length + encoding. Spike: encoding ignored.
//
// PR #46 review #2: previously returned 0 (Qnil-as-ID) when
// len == 0. CRuby allows interning the empty string and msgpack
// calls `rb_intern3("", 0, ...)`; returning 0 made downstream
// "is this a valid ID?" checks fail. Only short-circuit on null
// pointer or negative length; len == 0 delegates to a normal
// empty-string intern.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_intern3(name: *const c_char, len: c_long, _enc: *mut c_void) -> u64 {
    if name.is_null() || len < 0 { return 0; }
    let slice = unsafe { std::slice::from_raw_parts(name as *const u8, len as usize) };
    let s = String::from_utf8_lossy(slice);
    let cs = std::ffi::CString::new(s.as_ref()).unwrap_or_default();
    unsafe { crate::rb_intern(cs.as_ptr()) }
}
