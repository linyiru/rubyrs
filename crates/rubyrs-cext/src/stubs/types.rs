//! Type predicate + numeric conversion stubs (L3-D bulk-stub batch).
//!
//! See `stubs/mod.rs`. These exist so dlopen resolves; many are
//! conservative (return 0 / Qnil for variants rubyrs doesn't model
//! like Float / Symbol / Bignum).

use std::ffi::{c_char, c_double, c_int, c_long, c_longlong};

use crate::{with_state, CValue, Qfalse, Qnil, Qtrue, Value, ID};

// T_* constants — must match rubyrs.h.
const T_NIL: c_int = 1;
const T_TRUE: c_int = 2;
const T_FALSE: c_int = 3;
const T_FIXNUM: c_int = 4;
const T_STRING: c_int = 6;
const T_ARRAY: c_int = 7;
const T_HASH: c_int = 8;
const T_CLASS: c_int = 11;
const T_DATA: c_int = 13;

unsafe extern "C" {
    fn strtoll(s: *const c_char, end: *mut *mut c_char, base: c_int) -> c_longlong;
}

/// Map a CValue variant to the matching CRuby T_* tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_value_type(v: Value) -> c_int {
    with_state(|st| match st.resolve(v) {
        CValue::Nil => T_NIL,
        CValue::True => T_TRUE,
        CValue::False => T_FALSE,
        CValue::Int(_) => T_FIXNUM,
        CValue::Str(_) => T_STRING,
        CValue::Array(_) => T_ARRAY,
        CValue::Hash(_) => T_HASH,
        CValue::Class(_) => T_CLASS,
        CValue::HeapRef(_) => T_DATA,
    })
}

/// 1 if v resolves to a fixnum-style integer, else 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_value_is_fixnum(v: Value) -> c_int {
    with_state(|st| matches!(st.resolve(v), CValue::Int(_)) as c_int)
}

/// rubyrs has no Float variant; always 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_value_is_flonum(_v: Value) -> c_int {
    0
}

/// 1 if v is an immediate-like singleton (nil/true/false) or a fixnum.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_value_is_special_const(v: Value) -> c_int {
    if v == Qnil || v == Qtrue || v == Qfalse {
        return 1;
    }
    with_state(|st| matches!(st.resolve(v), CValue::Int(_)) as c_int)
}

/// LL2NUM — intern as CValue::Int.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ll2num(n: c_longlong) -> Value {
    // `c_longlong` is guaranteed by the C standard to be at least
    // 64 bits — i64 on every target rubyrs supports — so no widening
    // cast is needed (and clippy::unnecessary_cast flags one).
    with_state(|st| st.intern(CValue::Int(n)))
}

/// rubyrs has no Float CValue; return Qnil.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_dbl2num(_d: c_double) -> Value {
    Qnil
}

/// Parse a C string to an integer via strtoll; base==0 treated as 10.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_cstr2inum(str: *const c_char, base: c_int) -> Value {
    if str.is_null() {
        return Qnil;
    }
    let b = if base == 0 { 10 } else { base };
    let n = unsafe { strtoll(str, std::ptr::null_mut(), b) };
    with_state(|st| st.intern(CValue::Int(n as i64)))
}

/// rubyrs has no Float; always 0.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_cstr_to_dbl(_str: *const c_char, _badcheck: c_int) -> c_double {
    0.0
}

/// rubyrs has no Float CValue; always 0.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_float_value(_v: Value) -> c_double {
    0.0
}

/// rubyrs has no Symbol CValue; stub for dlopen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_id2sym(_id: ID) -> Value {
    Qnil
}

/// rubyrs has no Symbol CValue; stub for dlopen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_sym2id(_v: Value) -> ID {
    0
}

/// rubyrs has no Symbol CValue; stub for dlopen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_sym2str(_v: Value) -> Value {
    Qnil
}

/// Verify v's type matches t; panic on mismatch (spike: real impl would raise TypeError).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_check_type(v: Value, t: c_int) {
    let actual = unsafe { rb_value_type(v) };
    assert!(actual == t, "rb_check_type: expected {}, got {}", t, actual);
}

/// Verify argc in [min, max]; max == -1 means unbounded.
///
/// PR #42 review #6 fix: header declares return as `void`. Previously
/// returned `c_int` — ABI mismatch (caller may interpret garbage as
/// the return value across the FFI boundary). CRuby's macro form
/// `rb_check_arity(argc, min, max)` discards the value anyway.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_check_arity(argc: c_int, min: c_int, max: c_int) {
    assert!(argc >= min, "rb_check_arity: argc {} < min {}", argc, min);
    assert!(max == -1 || argc <= max, "rb_check_arity: argc {} > max {}", argc, max);
}

/// Spike: trust caller, return v unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_convert_type(
    v: Value,
    _type: c_int,
    _tname: *const c_char,
    _method: *const c_char,
) -> Value {
    v
}

/// Hash entry count; 0 for non-Hash.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn RHASH_SIZE(v: Value) -> c_long {
    with_state(|st| match st.resolve(v) {
        CValue::Hash(entries) => entries.len() as c_long,
        _ => 0,
    })
}
