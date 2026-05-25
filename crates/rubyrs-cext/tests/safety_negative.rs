//! Negative tests for the rubyrs-cext FFI safety contract.
//!
//! Each test feeds a deliberately-bad input to an `rb_*` entry
//! point and asserts the documented behaviour: forged handles
//! resolve to Qnil / 0 / wrong-but-defined results, never UB or
//! panic.
//!
//! See `docs/CEXT_SAFETY.md` for the three contract classes;
//! this file is the executable witness for "Class A" (handle-only
//! entry points).
//!
//! Each test calls `enter()` to push a fresh per-call state and
//! `leave()` to discard it (mirroring the cext_dispatch lifecycle).

use rubyrs_cext::{
    enter, leave,
    rb_ary_entry, rb_ary_new, rb_ary_push, RARRAY_LEN,
    rb_hash_aref, rb_hash_aset, rb_hash_new,
    rb_int2num, rb_long2num, rb_num2int, rb_num2long, rb_num2ulong,
    rb_str_new_cstr, RSTRING_LEN, RSTRING_PTR,
    Qfalse, Qnil, Qtrue, Value,
};

/// Push a fresh CExtState, run `body`, drop it.
fn with_cext<F: FnOnce()>(body: F) {
    enter();
    body();
    let _ = leave();
}

// ===== Class A: handle-only =====

#[test]
fn rb_num2long_on_forged_handle_returns_zero() {
    with_cext(|| {
        // Handle 999999 was never interned in this state; resolve
        // to Nil, num2long returns 0.
        let v: Value = 999_999;
        let n = unsafe { rb_num2long(v) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rb_num2long_on_qnil_returns_zero() {
    with_cext(|| {
        let n = unsafe { rb_num2long(Qnil) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rb_num2ulong_on_forged_handle_returns_zero() {
    with_cext(|| {
        let v: Value = 999_999;
        let n = unsafe { rb_num2ulong(v) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rb_num2int_on_forged_handle_returns_zero() {
    with_cext(|| {
        let v: Value = 999_999;
        let n = unsafe { rb_num2int(v) };
        assert_eq!(n, 0);
    });
}

#[test]
fn int_roundtrip_via_long2num_then_num2long() {
    // Positive case: confirm the well-defined path still works
    // alongside the negative ones, so a regression that broke the
    // happy path would show up here too.
    with_cext(|| {
        let v = unsafe { rb_long2num(42) };
        let n = unsafe { rb_num2long(v) };
        assert_eq!(n, 42);
    });
}

#[test]
fn int_roundtrip_via_int2num_then_num2int() {
    with_cext(|| {
        let v = unsafe { rb_int2num(-7) };
        let n = unsafe { rb_num2int(v) };
        assert_eq!(n, -7);
    });
}

#[test]
fn rstring_len_on_forged_handle_returns_zero() {
    with_cext(|| {
        let n = unsafe { RSTRING_LEN(999_999) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rstring_len_on_qnil_returns_zero() {
    with_cext(|| {
        let n = unsafe { RSTRING_LEN(Qnil) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rstring_ptr_on_forged_handle_returns_null() {
    with_cext(|| {
        let p = unsafe { RSTRING_PTR(999_999) };
        assert!(p.is_null());
    });
}

#[test]
fn rstring_ptr_on_bool_returns_null() {
    // Qtrue / Qfalse aren't strings; defined fall-through is null.
    with_cext(|| {
        assert!(unsafe { RSTRING_PTR(Qtrue) }.is_null());
        assert!(unsafe { RSTRING_PTR(Qfalse) }.is_null());
    });
}

#[test]
fn rarray_len_on_forged_handle_returns_zero() {
    with_cext(|| {
        let n = unsafe { RARRAY_LEN(999_999) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rarray_len_on_qnil_returns_zero() {
    with_cext(|| {
        let n = unsafe { RARRAY_LEN(Qnil) };
        assert_eq!(n, 0);
    });
}

#[test]
fn rb_ary_entry_negative_index_wraps_from_end() {
    with_cext(|| {
        let ary = unsafe { rb_ary_new() };
        let a = unsafe { rb_long2num(10) };
        let b = unsafe { rb_long2num(20) };
        let c = unsafe { rb_long2num(30) };
        unsafe { rb_ary_push(ary, a); rb_ary_push(ary, b); rb_ary_push(ary, c); }

        // -1 → last element. CRuby semantics.
        let last = unsafe { rb_ary_entry(ary, -1) };
        assert_eq!(unsafe { rb_num2long(last) }, 30);

        // -3 → first element.
        let first = unsafe { rb_ary_entry(ary, -3) };
        assert_eq!(unsafe { rb_num2long(first) }, 10);
    });
}

#[test]
fn rb_ary_entry_out_of_range_returns_qnil() {
    with_cext(|| {
        let ary = unsafe { rb_ary_new() };
        let v = unsafe { rb_long2num(1) };
        unsafe { rb_ary_push(ary, v); }

        // Past the end.
        assert_eq!(unsafe { rb_ary_entry(ary, 5) }, Qnil);
        // Far-negative — would underflow into a giant positive
        // index without the checked_add guard added in the
        // L2-3 hardening pass.
        assert_eq!(unsafe { rb_ary_entry(ary, i64::MIN) }, Qnil);
    });
}

#[test]
fn rb_ary_entry_on_forged_handle_returns_qnil() {
    with_cext(|| {
        // Non-Array handle.
        let v = unsafe { rb_ary_entry(999_999, 0) };
        assert_eq!(v, Qnil);
    });
}

#[test]
fn rb_hash_aref_missing_key_returns_qnil() {
    with_cext(|| {
        let h = unsafe { rb_hash_new() };
        let key = unsafe { rb_long2num(42) };
        let v = unsafe { rb_hash_aref(h, key) };
        assert_eq!(v, Qnil);
    });
}

#[test]
fn rb_hash_aref_on_forged_handle_returns_qnil() {
    with_cext(|| {
        let key = unsafe { rb_long2num(1) };
        let v = unsafe { rb_hash_aref(999_999, key) };
        assert_eq!(v, Qnil);
    });
}

#[test]
fn hash_aset_then_aref_roundtrip() {
    with_cext(|| {
        let h = unsafe { rb_hash_new() };
        let key = unsafe { rb_long2num(7) };
        let value = unsafe { rb_long2num(100) };
        unsafe { rb_hash_aset(h, key, value); }

        let got = unsafe { rb_hash_aref(h, key) };
        assert_eq!(unsafe { rb_num2long(got) }, 100);
    });
}

#[test]
fn rb_str_new_cstr_on_known_cstring_then_rstring_len() {
    // Mixed positive/negative — exercises the happy path that
    // underlies the forged-handle tests above.
    let c_string = b"hello\0";
    with_cext(|| {
        let v = unsafe { rb_str_new_cstr(c_string.as_ptr() as *const i8) };
        // RSTRING_LEN excludes the sentinel NUL.
        assert_eq!(unsafe { RSTRING_LEN(v) }, 5);
    });
}
