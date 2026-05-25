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
const T_FLOAT: c_int = 5;
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
        CValue::Float(_) => T_FLOAT,
    })
}

/// 1 if v resolves to a fixnum-style integer, else 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_value_is_fixnum(v: Value) -> c_int {
    with_state(|st| matches!(st.resolve(v), CValue::Int(_)) as c_int)
}

/// PR #63 review #2: now that CValue::Float is a real variant
/// (post-L3-I), this should return 1 for Float values so cexts
/// using FLONUM_P / RB_FLONUM_P see the same answer as
/// rb_value_type's T_FLOAT branch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_value_is_flonum(v: Value) -> c_int {
    with_state(|st| matches!(st.resolve(v), CValue::Float(_)) as c_int)
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
pub unsafe extern "C" fn rb_dbl2num(d: c_double) -> Value {
    with_state(|st| st.intern(CValue::Float(d)))
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

/// Parse a C string to double. flori-json calls this for every
/// float literal it parses (parser.rl:1734). Pre-L3-I returned
/// 0.0 unconditionally, collapsing all JSON floats.
///
/// PR #63 review #3: libc `strtod` is locale-dependent —
/// `1.5` parses as `1` (stop at `.`) under e.g. de_DE.UTF-8
/// where `,` is the decimal separator. JSON and Ruby float
/// literals are locale-invariant (always `.`). Use Rust's
/// `str::parse::<f64>` instead, which honours `.` regardless of
/// the process locale. Trailing garbage parses as 0.0 (matches
/// CRuby's behaviour with `badcheck=0`); `badcheck=1`'s strict-
/// error mode is documented as a spike gap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_cstr_to_dbl(str: *const c_char, _badcheck: c_int) -> c_double {
    if str.is_null() {
        return 0.0;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(str) }
        .to_str()
        .unwrap_or("");
    // CRuby strips leading whitespace before parsing.
    let s = s.trim_start();
    // Manually pick the longest valid f64 prefix — Rust's
    // str::parse::<f64>() requires the WHOLE string to be a
    // valid number, while strtod stops at the first invalid
    // char. Walk forward consuming sign/digits/decimal/exponent.
    let bytes = s.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    while end < bytes.len() {
        let c = bytes[end];
        if c.is_ascii_digit() {
            seen_digit = true;
            end += 1;
        } else if c == b'.' && !seen_dot && !seen_exp {
            seen_dot = true;
            end += 1;
        } else if (c == b'e' || c == b'E') && !seen_exp && seen_digit {
            seen_exp = true;
            end += 1;
            if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                end += 1;
            }
        } else {
            break;
        }
    }
    if !seen_digit {
        return 0.0;
    }
    s[..end].parse::<f64>().unwrap_or(0.0)
}

/// RFLOAT_VALUE — extract the f64 from a Float VALUE. L3-I: now
/// reads through CValue::Float; non-Float resolves to 0.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_float_value(v: Value) -> c_double {
    with_state(|st| match st.resolve(v) {
        CValue::Float(f) => *f,
        _ => 0.0,
    })
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

// ===== msgpack-ruby additions =====

/// long long -> Integer. rubyrs's Number is i64; identical to rb_ll2num.
/// Rust's `std::ffi::c_longlong` is defined as `i64` (per the Rust
/// ABI types — the C standard only mandates ≥64 bits, but the Rust
/// alias picks the exact-i64 contract), so no cast is needed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ll2inum(n: c_longlong) -> Value {
    with_state(|st| st.intern(CValue::Int(n)))
}

/// unsigned long long -> Integer. Truncates to i64 (rubyrs has no
/// arbitrary precision); >i64::MAX values overflow into negatives.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_ull2inum(n: u64) -> Value {
    with_state(|st| st.intern(CValue::Int(n as i64)))
}

/// VALUE -> double. CValue::Float reads through; CValue::Int
/// promotes (matches CRuby's Integer-to-Float coercion). Other
/// variants return 0.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_num2dbl(v: Value) -> c_double {
    with_state(|st| match st.resolve(v) {
        CValue::Float(f) => *f,
        CValue::Int(n) => *n as c_double,
        _ => 0.0,
    })
}

/// double -> VALUE Float. L3-I: real `CValue::Float` slot now.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_float_new(d: c_double) -> Value {
    with_state(|st| st.intern(CValue::Float(d)))
}

/// Bignum byte size. rubyrs's Int is fixed i64 so the absolute value
/// fits in 8 bytes; report ceil-of-bytes for the magnitude.
/// `nlz_bits_ret` (number of leading zero bits in the top byte)
/// gets a conservative 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_absint_size(v: Value, nlz_bits_ret: *mut c_int) -> usize {
    if !nlz_bits_ret.is_null() {
        unsafe { *nlz_bits_ret = 0; }
    }
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => {
            let abs = n.unsigned_abs();
            // bytes needed to represent abs.
            if abs == 0 { 1 } else { (8 - abs.leading_zeros() / 8) as usize }
        }
        _ => 0,
    })
}

/// Bignum -> u64. rubyrs's Int is always i64; reinterpret.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_big2ull(v: Value) -> u64 {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as u64,
        _ => 0,
    })
}

/// Bignum -> i64. Same shape as rb_big2ull.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_big2ll(v: Value) -> c_longlong {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => *n as c_longlong,
        _ => 0,
    })
}

/// Bignum positive? rubyrs Int treated as signed i64.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rb_bignum_positive_p(v: Value) -> c_int {
    with_state(|st| match st.resolve(v) {
        CValue::Int(n) => if *n >= 0 { 1 } else { 0 },
        _ => 1,
    })
}
