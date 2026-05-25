//! String + encoding stubs (L3-D bulk-stub batch).
//!
//! See `stubs/mod.rs` for the why (dlopen symbol presence).
//! These are minimal: most return Qnil or delegate to existing
//! rb_str_new variants. Encoding APIs collapse to UTF-8-only.

use std::ffi::{c_char, c_int, c_long, c_void};

use crate::{with_state, CValue, Qnil, Value};

// `rb_raise` is provided by `c/raise.c` as a C-ABI variadic noreturn.
// We declare it here so `rb_enc_raise` can forward to it.
unsafe extern "C" {
    fn rb_raise(exc_class: Value, fmt: *const c_char, ...) -> !;
}

// Allocate an empty String; capacity hint is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_buf_new(_capa: c_long) -> Value {
    with_state(|st| st.intern(CValue::str_from_bytes(&[])))
}

// Clone a String handle; non-String inputs return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_dup(v: Value) -> Value {
    with_state(|st| {
        let bytes = match st.resolve(v) {
            CValue::Str(b) => {
                // Strip the trailing sentinel NUL before re-wrapping.
                let logical = if b.last() == Some(&0) { &b[..b.len() - 1] } else { &b[..] };
                logical.to_vec()
            }
            _ => return Qnil,
        };
        st.intern(CValue::str_from_bytes(&bytes))
    })
}

// Freeze: spike does not track frozenness; no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_freeze(v: Value) -> Value {
    v
}

// String -> Symbol. rubyrs has no Symbol value; return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_intern(_v: Value) -> Value {
    Qnil
}

// Truncate a String to `len` bytes. No-op safe on non-String.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_set_len(v: Value, len: c_long) {
    with_state(|st| {
        if let CValue::Str(b) = st.resolve_mut(v) {
            let new_len = (len as usize).min(b.len().saturating_sub(1));
            b.truncate(new_len);
            b.push(0);
        }
    })
}

// Substring of a String; returns Qnil on bad inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_substr(v: Value, beg: c_long, len: c_long) -> Value {
    with_state(|st| {
        let slice = match st.resolve(v) {
            CValue::Str(b) => {
                let logical_len = b.len().saturating_sub(1);
                let beg = if beg < 0 {
                    (logical_len as c_long + beg).max(0) as usize
                } else {
                    beg as usize
                };
                if beg > logical_len || len < 0 {
                    return Qnil;
                }
                let end = (beg + len as usize).min(logical_len);
                b[beg..end].to_vec()
            }
            _ => return Qnil,
        };
        st.intern(CValue::str_from_bytes(&slice))
    })
}

// StringValue(v): coerce *v to a String. Spike: assume *v already is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_string_value(v: *mut Value) -> Value {
    if v.is_null() {
        return Qnil;
    }
    unsafe { *v }
}

// UTF-8 String constructor: delegate (UTF-8 is rubyrs' only encoding).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_utf8_str_new(buf: *const c_char, len: c_long) -> Value {
    unsafe { crate::rb_str_new(buf, len) }
}

// UTF-8 NUL-terminated String constructor: delegate to rb_str_new_cstr.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_utf8_str_new_cstr(s: *const c_char) -> Value {
    unsafe { crate::rb_str_new_cstr(s) }
}

// US-ASCII String constructor: delegate (encoding tag ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_usascii_str_new(buf: *const c_char, len: c_long) -> Value {
    unsafe { crate::rb_str_new(buf, len) }
}

// Encoded String constructor: delegate (encoding ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_str_new(
    buf: *const c_char,
    len: c_long,
    _enc: *const c_void,
) -> Value {
    unsafe { crate::rb_str_new(buf, len) }
}

// Encoded interned String constructor: delegate (no fstring table here).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_interned_str(
    buf: *const c_char,
    len: c_long,
    _enc: *const c_void,
) -> Value {
    unsafe { crate::rb_str_new(buf, len) }
}

// Encoding singletons: stable non-null sentinels (cext only compares ==).
// `without_provenance_mut(N)` is the idiomatic way to fabricate a
// pointer from an integer with no provenance — same numeric value
// as `N as *mut c_void` but doesn't trip clippy's manual-dangling-ptr.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_utf8_encoding() -> *mut c_void {
    std::ptr::without_provenance_mut(1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ascii8bit_encoding() -> *mut c_void {
    std::ptr::without_provenance_mut(2)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_usascii_encoding() -> *mut c_void {
    std::ptr::without_provenance_mut(3)
}

// Encoding indices: arbitrary but stable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_utf8_encindex() -> c_int {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_usascii_encindex() -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ascii8bit_encindex() -> c_int {
    2
}

// Associate encoding with String: no-op (all strings are UTF-8).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_associate_index(v: Value, _idx: c_int) -> Value {
    v
}

// Get encoding index of a String: always UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_get_index(_v: Value) -> c_int {
    1
}

// Coderange: always VALID (we assume well-formed UTF-8).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_str_coderange(_v: Value) -> c_int {
    2
}

// Raise-with-encoding: drop the encoding tag and forward to rb_raise.
//
// PR #42 review #2 fix: callers pass format strings containing `%s`
// (flori/json: `"unexpected token at '%s'"`). Forwarding fmt directly
// to rb_raise made vsnprintf read absent varargs → UB / crash on any
// parse error. Treat the caller's fmt as already-rendered text by
// wrapping it in `"%s"` and passing the original ptr as the sole arg.
// Loses CRuby's format-with-actual-args behavior, but that fidelity
// requires va_list pass-through which isn't available in stable Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn rb_enc_raise(
    _enc: *mut c_void,
    exc: Value,
    fmt: *const c_char,
) -> ! {
    unsafe { rb_raise(exc, c"%s".as_ptr(), fmt) }
}

// ===== msgpack-ruby additions =====

// Append bytes to a String. Returns the (possibly relocated) string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_buf_cat(str: Value, ptr: *const c_char, len: c_long) -> Value {
    if ptr.is_null() || len <= 0 { return str; }
    let slice = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
    with_state(|st| {
        if let CValue::Str(b) = st.resolve_mut(str) {
            // Strip trailing NUL sentinel before extending, re-append.
            if b.last() == Some(&0) { b.pop(); }
            b.extend_from_slice(slice);
            b.push(0);
        }
    });
    str
}

// Copy contents from src into dst. Returns dst.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_replace(dst: Value, src: Value) -> Value {
    let bytes: Vec<u8> = with_state(|st| match st.resolve(src) {
        CValue::Str(b) => b.clone(),
        _ => Vec::new(),
    });
    with_state(|st| {
        if let CValue::Str(b) = st.resolve_mut(dst) {
            *b = bytes;
            if b.last() != Some(&0) { b.push(0); }
        }
    });
    dst
}

// Resize a String's content buffer to `len` bytes. Truncate or
// extend with zeros; keep the sentinel NUL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_resize(str: Value, len: c_long) -> Value {
    let target = if len < 0 { 0 } else { len as usize };
    with_state(|st| {
        if let CValue::Str(b) = st.resolve_mut(str) {
            // Drop sentinel before resize so it doesn't count.
            if b.last() == Some(&0) { b.pop(); }
            b.resize(target, 0);
            b.push(0);
        }
    });
    str
}

// Encode str into target encoding. rubyrs is UTF-8 only; return str
// unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_str_encode(str: Value, _to: Value, _ecflags: c_int, _ecopts: Value) -> Value {
    str
}

// Try-coerce to String via to_str. Returns Qnil if not stringy.
// rubyrs spike: only direct CValue::Str works.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_check_string_type(v: Value) -> Value {
    with_state(|st| match st.resolve(v) {
        CValue::Str(_) => v,
        _ => Qnil,
    })
}

// rb_String(v): coerce to String. Equivalent to Kernel#String, calls
// .to_s. Spike: if already String, return; else return Qnil (lossy).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_String(v: Value) -> Value {
    with_state(|st| match st.resolve(v) {
        CValue::Str(_) => v,
        _ => Qnil,
    })
}

// Encoding accessors. Same singletons returned by rb_*_encoding().
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_asciicompat(_enc: *mut c_void) -> c_int { 1 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_from_index(idx: c_int) -> *mut c_void {
    // Match the singletons in rb_utf8_encoding / rb_ascii8bit_encoding
    // / rb_usascii_encoding.
    // `without_provenance_mut(N)` matches the encoding-singleton
    // pattern used by rb_utf8_encoding et al. above.
    match idx {
        0 => std::ptr::without_provenance_mut(3), // usascii
        2 => std::ptr::without_provenance_mut(2), // ascii8bit
        _ => std::ptr::without_provenance_mut(1), // utf8 default
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_enc_from_encoding(_enc: *mut c_void) -> Value {
    // CRuby returns the per-encoding Encoding instance VALUE; rubyrs
    // has no Encoding object — return Qnil.
    Qnil
}
