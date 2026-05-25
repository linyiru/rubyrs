//! GC / warn / IO no-op stubs (L3-D bulk-stub batch).
//!
//! See `stubs/mod.rs`. These collapse to no-op (rubyrs GC is not
//! cooperative with cext-side mark/location, and warn/IO route
//! through different paths).

use std::ffi::{c_char, c_int, c_void, CStr};

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_category_warn(_category: c_int, fmt: *const c_char) {
    if fmt.is_null() {
        return;
    }
    let s = unsafe { CStr::from_ptr(fmt) }.to_bytes();
    eprintln!("[rb_category_warn] {}", String::from_utf8_lossy(s));
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
