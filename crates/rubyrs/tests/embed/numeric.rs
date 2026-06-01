//! `Integer` / `BigInt` arithmetic + dispatch + DoS-cap
//! enforcement. Largest single embed sub-module; collects the
//! BigInt phase-A/B work surface.
//!
//!   - `bigint_*` — arithmetic, bit ops, shifts, unary, to_s
//!     radix conversion, DoS caps.
//!   - `bigint_iter_*` / `bigint_times_upto_downto_*` —
//!     iteration block surface (Phase B.6).
//!   - `pow_*` / `bigint_pow_*` — `**` and `Integer#pow(exp[,
//!     mod])`. Full surface: estimator DoS cap, identity
//!     short-circuits, Float coercion, parity preservation,
//!     no-bignum profile error shapes, modular exponentiation.
//!   - `sprintf_*` — sprintf %d / %x radix through BigInt +
//!     alt-form zero suppression.
//!   - `digits_*` — `Integer#digits([base])` BigInt and Int
//!     paths, estimator log2 correctness, negative-recv
//!     precedence over arity / base errors.
//!   - `integer_to_s_*` — Integer#to_s(radix) error class
//!     consistency between Int and BigInt receivers.
//!   - `int_min_abs_*` / `int_shift_*` — i64::MIN edge cases.
//!   - `integer_bit_ops_*` / `bit_length_*` — `&`/`|`/`^`
//!     TypeError on non-Integer arg; two's-complement
//!     `Integer#bit_length` on BigInt.
//!
//! ResourceExhausted caps cross-cut with `tests/embed/resource_caps.rs`
//! (the cap machinery itself). Cap-related tests here exercise
//! cap enforcement specifically *through* a BigInt code path,
//! so they stay with the rest of the BigInt surface.

use rubyrs::Value;

use super::SharedBuf;

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_respects_max_value_bytes_cap() {
    // Regression cover for PR #103 cycle 13. BigInt#to_s/#inspect
    // produce a decimal-digit string that grows arbitrarily with
    // the magnitude (`(2 ** 1_000_000).to_s` is ~300 KB), so the
    // bigint_primitive path must enforce Config::max_value_bytes
    // the same way primitive_call arms do — otherwise a script
    // could DoS the host by stringifying a huge integer.
    let cfg = rubyrs::Config { max_value_bytes: Some(64), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        r#"
        n = 1
        100.times { n = n * 1_000_000 }   # n has ~600 decimal digits
        n.to_s
        "#,
        "bigint_to_s_size_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted trap from BigInt#to_s exceeding max_value_bytes, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_caps_huge_result() {
    // Phase B.1: `**` with a huge exponent estimates result bits
    // and traps ResourceExhausted before allocating GBs. Default
    // ceiling (no max_value_bytes) is 1 MB; `2 ** 10_000_000`
    // would need ~1.25 MB so it traps.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "2 ** 10_000_000",
        "pow_huge.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted trap from 2**10_000_000, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_honors_max_value_bytes() {
    // The DoS cap respects Config::max_value_bytes when set —
    // a tight 64-byte cap rejects `2 ** 1000`. The estimator
    // bounds the binary magnitude (~126 bytes here; the decimal
    // form would be 302 digits but the cap is on the storable
    // value, not its rendered string).
    let cfg = rubyrs::Config { max_value_bytes: Some(64), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "2 ** 1000",
        "pow_tight_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted under max_value_bytes=64, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_negative_exponent_returns_float() {
    // CRuby returns Rational `(1/4)` for `2 ** -2`; rubyrs uses
    // Float because there's no Rational in the subset
    // (documented SUBSET.md divergence). Pin the Float path here
    // since diff_cruby can't compare the formats.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts (2 ** -2)", "pow_neg.rb").expect("Float reciprocal path");
    assert_eq!(buf.snapshot().trim(), "0.25");
}

#[cfg(feature = "bignum")]
#[test]
fn pow_int_int_identity_bases_skip_numeric_u32_clamp() {
    // 0/±1 bases produce trivial results regardless of exponent
    // size — numeric.rs's `**` arm short-circuits via parity
    // BEFORE the `(*b as u64).min(u32::MAX as u64) as u32`
    // clamp it would otherwise apply. Without those short-
    // circuits `(-1) ** (u32::MAX + 2)` would clamp to the
    // u32::MAX exponent (odd) and silently flip sign for an
    // even input. The inputs here are all Int×Int, so dispatch
    // is owned by numeric.rs and never reaches
    // `Vm::try_bigint_pow` — the BigInt-exponent equivalent of
    // this guarantee lives in
    // `bigint_pow_identity_bases_with_bigint_exponent` below.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let huge = (u32::MAX as i64) + 1; // 4_294_967_296
    rt.eval(
        &format!("puts 1 ** {h}\nputs 0 ** {h}\nputs (-1) ** {h}\nputs (-1) ** ({h} + 1)",
            h = huge),
        "pow_identity_huge.rb",
    ).expect("identity bases must skip the u32 clamp");
    assert_eq!(buf.snapshot().trim(), "1\n0\n1\n-1");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_int_receiver_negative_bigint_exponent_returns_float() {
    // Int receiver + NEGATIVE BigInt exponent had no handler:
    // numeric.rs only covers Int×Int, and try_bigint_pow's
    // recv_is_bigint gate skipped Int receivers — so
    // `2 ** -(2**100)` raised NoMethodError despite
    // `respond_to?(:**)` being true. With the gate widened to
    // `recv OR exp is BigInt`, dispatch produces a Float
    // (which underflows toward 0 for |base|>1 since the BigInt
    // exponent is past f64 range — the helper coerces it to
    // -Inf, and `2 ** -Inf` = 0.0).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Build a negative BigInt via subtraction (BigInt unary
        // `-@` is unshipped Phase B.2).
        "neg_big = 0 - (2 ** 100)\n\
         puts (2 ** neg_big).zero?\n\
         puts (1 ** neg_big)\n\
         puts ((-1) ** neg_big)",
        "int_recv_neg_bigint_exp.rb",
    ).expect("Int recv + negative BigInt exp must not NoMethodError");
    // 2**-2**100 underflows to 0.0; 1**-big = 1.0 exactly;
    // (-1)**-big: big = 2^100 is even, so parity → 1.0.
    assert_eq!(buf.snapshot().trim(), "true\n1.0\n1.0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_bigint_receiver_negative_exponent_returns_float() {
    // BigInt receiver + negative Int exp must not NoMethodError —
    // respond_to?(:**) is true for BigInt, so the dispatch path
    // has to produce *something*. We pick Float (matches the
    // documented Rational divergence for `Int ** -n`).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    // (2 ** 100) ** -2 → 2**-200 ≈ 6.22e-61: a tiny but non-zero
    // Float (well above the smallest f64 subnormal at ~5e-324).
    rt.eval("puts ((2 ** 100) ** -2)", "bigint_pow_neg.rb")
        .expect("BigInt ** negative-Int must return a Float, not NoMethodError");
    let out = buf.snapshot();
    let v: f64 = out.trim().parse().expect("output must parse as Float");
    assert!(v > 0.0 && v < 1e-50, "expected tiny positive Float ~6e-61, got {}", v);
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_bigint_receiver_float_exponent_returns_float() {
    // BigInt receiver + Float exp must also return a Float, not
    // NoMethodError. `(2 ** 100) ** 0.5` ≈ 2**50 ≈ 1.126e15.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts ((2 ** 100) ** 0.5)", "bigint_pow_float_exp.rb")
        .expect("BigInt ** Float must return a Float, not NoMethodError");
    let out = buf.snapshot();
    let v: f64 = out.trim().parse().expect("output must parse as Float");
    let expected = (2.0_f64).powi(50);
    let rel = ((v - expected) / expected).abs();
    assert!(rel < 1e-6, "expected ~{}, got {} (rel error {})", expected, v, rel);
}

#[cfg(feature = "bignum")]
#[test]
fn int_min_abs_promotes_to_bigint() {
    // `i64::MIN.abs` overflows i64 by exactly one (magnitude is
    // 2^63, one past i64::MAX). numeric.rs's `abs` arm now
    // declines under `bignum`, bigint_primitive's unary path
    // materialises the BigInt 2^63 and keeps it as BigInt (since
    // it doesn't fit i64). Same expectation for `-i64::MIN`.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts((-9_223_372_036_854_775_808).abs)\n\
         puts(-(-9_223_372_036_854_775_808))",
        "int_min_unary.rb",
    ).expect("i64::MIN unary must promote, not wrap");
    assert_eq!(buf.snapshot().trim(), "9223372036854775808\n9223372036854775808");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn int_min_abs_wraps_without_bignum() {
    // Without the bignum feature, `i64::MIN.abs` stays as
    // `i64::MIN` (wrapping_abs) — there's no BigInt fallback. Pin
    // the historical behaviour so a future no-bignum build can't
    // silently flip semantics.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts((-9_223_372_036_854_775_808).abs)",
        "int_min_unary_no_bignum.rb",
    ).expect("eval must succeed (wraps to i64::MIN)");
    assert_eq!(buf.snapshot().trim(), "-9223372036854775808");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_unary_plus_returns_same_value_id() {
    // `+@` on BigInt is a no-op clone — the resulting Value is
    // a `Value::BigInt(id)` pointing at the SAME heap entry as
    // the receiver. Numeric `==` would also pass if `+@` silently
    // re-allocated, so capture both values into a 2-element
    // Array and assert on the `Value::BigInt` ids directly.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval(
        "big = 2 ** 100\n[big, +big]",
        "bigint_unary_plus.rb",
    ).expect("+@ on BigInt must produce a Value");
    let elems = rt.resolve_array(&v).expect("expected Value::Array");
    assert_eq!(elems.len(), 2);
    match (&elems[0], &elems[1]) {
        (Value::BigInt(a), Value::BigInt(b)) => assert_eq!(
            a, b,
            "+@ must return a Value::BigInt pointing at the same heap id",
        ),
        other => panic!("expected (Value::BigInt, Value::BigInt), got {:?}", other),
    }
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_unary_neg_demotes_when_result_fits_int() {
    // `-big` where `big` after negation fits i64 must demote to
    // `Value::Int`. `2 ** 63` is exactly i64::MAX + 1
    // (9223372036854775808); negating gives i64::MIN exactly,
    // which fits. Demote-on-fit should produce
    // `Value::Int(i64::MIN)`. Numeric `==` would silently pass
    // even if the result stayed `Value::BigInt`, so assert
    // directly on the Value variant.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval(
        "big = 2 ** 63\n-big",
        "bigint_unary_neg_demote.rb",
    ).expect("eval must succeed");
    assert!(
        matches!(v, Value::Int(i64::MIN)),
        "expected Value::Int(i64::MIN), got {:?}",
        v,
    );
}

#[cfg(not(feature = "bignum"))]
#[test]
fn pow_method_works_under_no_bignum_profile() {
    // Both 1-arg and 2-arg `Integer#pow` must work on the no-bignum
    // profile too — `respond_to?(:pow)` is whitelisted
    // unconditionally, so dispatch needs to match. 1-arg delegates
    // to `**` (numeric.rs alias). 2-arg uses an i128 square-and-
    // multiply since BigInt isn't available.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 5.pow(3)\n\
         puts 5.pow(3, 7)\n\
         puts 7.pow(8, 5)\n\
         puts (-5).pow(3, 7)\n\
         puts 5.pow(3, -7)",
        "pow_no_bignum.rb",
    ).expect("pow must work without bignum");
    // 5³=125; 125 mod 7 = 6; 7⁸ mod 5 = 1; (-5)³=-125, -125 floor-mod 7 = 1
    // (since -125 = 7*-18 + 1); 125 floor-mod -7 = -1.
    assert_eq!(buf.snapshot().trim(), "125\n6\n1\n1\n-1");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn digits_no_bignum_arity_guard_raises_argument_error() {
    // Under no-bignum, `bigint_primitive`'s arity guard doesn't
    // exist — the dispatch.rs Int fast path needs its own guard
    // so `5.digits(10, 2)` raises ArgumentError matching CRuby
    // instead of falling through to NoMethodError despite
    // `respond_to?(:digits)` being true.
    let mut rt = rubyrs::Runtime::new();
    for (script, n) in [
        ("5.digits(10, 2)", 2),
        ("5.digits(10, 2, 3)", 3),
        ("5.digits(10, 2, 3, 4)", 4),
    ] {
        let err = rt.eval(script, "no_bignum_digits_arity.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("wrong number of arguments (given {}, expected 0..1)", n),
            "wrong message for {:?}", script,
        );
    }
}

#[cfg(not(feature = "bignum"))]
#[test]
fn digits_int_path_error_semantics_match_bignum_profile() {
    // Cross-profile parity: the no-bignum Int#digits path
    // (dispatch.rs) must surface the same error class +
    // message text as the bignum BigInt path
    // (Vm::try_integer_digits). Pin the dispatch.rs error arms
    // so a future refactor that flips one side doesn't silently
    // diverge.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_msg) in [
        ("(-5).digits",     "out of domain"),
        ("5.digits(-2)",    "negative radix"),
        ("5.digits(1)",     "invalid radix 1"),
        ("5.digits(0)",     "invalid radix 0"),
    ] {
        let err = rt.eval(script, "no_bignum_digits.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(msg, expected_msg, "wrong message for {:?}", script);
    }
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_radix_bigint_traps_via_pre_alloc_cap() {
    // `'%b' % (2 ** N)` allocates ~N bytes during
    // `to_str_radix`. The post-format cap check in `Kernel#sprintf`
    // / `String#%` only sees the already-allocated result string
    // and can't unwind a host OOM. Pre-alloc cap in
    // `format_radix_any` must trap based on `bits()` BEFORE the
    // alloc runs.
    //
    // Set a 64 KB cap large enough for `2 ** 100_000` to exist
    // as a BigInt (~12.5 KB magnitude) but small enough that
    // its base-2 sprintf form (~100 KB) trips. Pin the trap.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "'%b' % (2 ** 100_000)",
        "sprintf_pre_alloc_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_decimal_bigint_traps_via_pre_alloc_cap() {
    // Companion to `sprintf_radix_bigint_traps_via_pre_alloc_cap`:
    // `'%d' % big` used to call `to_string()` directly with no
    // pre-allocation cap, leaving the most common integer
    // format-spec exposed to the host-OOM scenario the base-N
    // pre-alloc helper defends against.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    // `(2 ** 1_000_000)` is ~301_030 decimal digits — well above
    // the 64 KB cap, well below any reasonable host RAM ceiling
    // (~120 KB of BigInt magnitude). Pre-alloc check must trap
    // before `to_string()` materialises the 300 KB decimal string.
    let err = rt.eval(
        "'%d' % (2 ** 1_000_000)",
        "sprintf_decimal_pre_alloc_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_radix_cap_does_not_false_trap_decimal_at_exact_length() {
    // Regression for cycle 10: earlier the cap estimator used
    // integer `floor(log2(radix))` as the per-digit bit yield,
    // which over-estimated digit count by ~10% for radix 10.
    // `(10 ** 100).to_s` is exactly 101 chars ("1" + 100 "0"s);
    // pre-fix estimate was ceil(333 bits / 3) = 111, so a cap
    // of 105 would have false-trapped despite the rendered
    // value fitting. Post-fix estimate is 101, matching reality.
    let cfg = rubyrs::Config { max_value_bytes: Some(105), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (10 ** 100).to_s",
        "to_s_cap_tight.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let expected = format!("1{}", "0".repeat(100));
    assert_eq!(out.trim(), expected);
}

#[test]
fn sprintf_alt_form_suppresses_prefix_for_zero_value() {
    // CRuby suppresses the alt-form prefix when the value is
    // zero: `'%#x' % 0` → `"0"`, not `"0x0"`. Same for
    // `'%#o' % 0` (`"0"`, not `"00"`), `'%#b' % 0` (`"0"`,
    // not `"0b0"`). All literals here take the Int(0) path;
    // non-zero alt rendering pinned as the negative half of
    // the contract.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%#o' % 0\n\
         puts '%#x' % 0\n\
         puts '%#X' % 0\n\
         puts '%#b' % 0\n\
         puts '%#B' % 0\n\
         puts '%#o' % 7\n\
         puts '%#x' % 255",
        "sprintf_alt_zero.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Zero values: no prefix.
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "0");
    assert_eq!(lines[2], "0");
    assert_eq!(lines[3], "0");
    assert_eq!(lines[4], "0");
    // Non-zero: prefix present.
    assert_eq!(lines[5], "07");
    assert_eq!(lines[6], "0xff");
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_alt_form_zero_via_bignum_arithmetic_still_suppressed() {
    // Regression guard for the bignum profile: expressions
    // that route through the BigInt arithmetic path but reduce
    // to zero (`(2 ** 100) % (2 ** 100)`) demote to Int(0) per
    // the canonical-BigInt invariant, so the formatter sees
    // Int(0) and the alt prefix must still be suppressed.
    // The BigInt(0) formatting arm itself isn't reachable from
    // user code (demote-on-fit), but the `b.sign() != NoSign`
    // guard in `format_radix_any` defends against hand-built
    // BigInt(0) values from FFI / preamble paths; that guard
    // is exercised structurally rather than dynamically here.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%#x' % ((2 ** 100) % (2 ** 100))\n\
         puts '%#o' % ((2 ** 100) % (2 ** 100))\n\
         puts '%#b' % ((2 ** 100) % (2 ** 100))",
        "sprintf_alt_bignum_arith_zero.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert_eq!(out.trim(), "0\n0\n0");
}

#[test]
fn sprintf_alt_form_with_zero_pad_keeps_prefix_before_zeros() {
    // Regression guard: pre-fix `'%#08x' % 255` produced
    // `00000xff` (zero-pad inserted before the `0x` prefix);
    // CRuby produces `0x0000ff` (zeros go between prefix and
    // digits). Same for `%#08X`, `%#08b`, `%#08B`. Octal's `0`
    // alt prefix happens to behave identically under
    // unconditional zero-padding (`'%#08o' % 7` → `00000007`
    // either way), so no special handling there.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%#08x' % 255\n\
         puts '%#08X' % 255\n\
         puts '%#08b' % 7\n\
         puts '%#08B' % 7\n\
         puts '%#08o' % 7\n\
         puts '%#08x' % (2 ** 60)",
        "sprintf_alt_zero_pad.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0x0000ff");
    assert_eq!(lines[1], "0X0000FF");
    assert_eq!(lines[2], "0b000111");
    assert_eq!(lines[3], "0B000111");
    assert_eq!(lines[4], "00000007");
    assert_eq!(lines[5], "0x1000000000000000"); // body > width, no pad
}

#[test]
fn sprintf_radix_int_min_does_not_panic() {
    // Regression guard: `format_radix_int` used to compute the
    // magnitude of a negative i64 via `(-n) as u64`, which panics
    // in debug builds for `n == i64::MIN` (-i64::MIN overflows
    // i64). `'%x' % i64::MIN` is a legitimate Ruby call. Switch
    // to `unsigned_abs()` so the path stays panic-free; pin all
    // four base specifiers at the i64::MIN cell.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "imin = -9_223_372_036_854_775_808\n\
         puts '%x' % imin\n\
         puts '%X' % imin\n\
         puts '%o' % imin\n\
         puts '%b' % imin",
        "sprintf_imin.rb",
    ).expect("i64::MIN sprintf must not panic");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Documented divergence: we render `-<unsigned magnitude>`,
    // CRuby renders the `..f`-prefixed two's-complement form.
    assert_eq!(lines[0], "-8000000000000000");
    assert_eq!(lines[1], "-8000000000000000");
    assert_eq!(lines[2], "-1000000000000000000000");
    assert_eq!(lines[3], "-1000000000000000000000000000000000000000000000000000000000000000");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_times_upto_downto_iterate_with_demote_on_fit() {
    // Phase B.6: block-form iteration over BigInt operands.
    // Counter lives as a native num_bigint::BigInt; each
    // yielded Value is demoted to `Value::Int` when it fits i64
    // (`(big - 5).upto(big)` yields five BigInts but
    // `(2**65).times { |i| break if i >= 3 }` yields Int(0..3)
    // because the in-range counts fit i64 fine).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // BigInt#times: break early — yields Int because the
        // visited values fit i64.
        "arr = []\n\
         (2 ** 65).times { |i| arr << i; break if i >= 3 }\n\
         puts arr.inspect\n\
         puts arr[0].class.name\n\
         # BigInt#upto: small range across the i64 boundary —\n\
         # all yielded values are BigInt (> i64::MAX).\n\
         out = []\n\
         (2 ** 70).upto(2 ** 70 + 3) { |i| out << i.to_s }\n\
         puts out.inspect\n\
         # BigInt#downto: same but decreasing.\n\
         out2 = []\n\
         (2 ** 70).downto(2 ** 70 - 3) { |i| out2 << i.to_s }\n\
         puts out2.inspect\n\
         # Int recv + BigInt stop: start in-range, break early.\n\
         out3 = []\n\
         5.upto(2 ** 100) { |i| out3 << i; break if i >= 10 }\n\
         puts out3.inspect\n\
         # Negative BigInt#times → 0 iterations (CRuby).\n\
         calls = 0\n\
         (-(2 ** 65)).times { |i| calls += 1 }\n\
         puts \"neg=#{calls}\"\n\
         # Return value: recv when no break, break-value when break.\n\
         ret = (2 ** 65).downto(2 ** 65 - 2) { |_| }\n\
         puts \"ret_class=#{ret.class.name}\"\n\
         br = (2 ** 65).times { |i| break :early if i >= 1 }\n\
         puts \"break=#{br}\"\n\
         # respond_to? gates true for the new methods.\n\
         b = 2 ** 70\n\
         puts b.respond_to?(:times)\n\
         puts b.respond_to?(:upto)\n\
         puts b.respond_to?(:downto)",
        "bigint_iter.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "[0, 1, 2, 3]");
    assert_eq!(lines[1], "Integer"); // demoted
    assert_eq!(
        lines[2],
        "[\"1180591620717411303424\", \"1180591620717411303425\", \"1180591620717411303426\", \"1180591620717411303427\"]"
    );
    assert_eq!(
        lines[3],
        "[\"1180591620717411303424\", \"1180591620717411303423\", \"1180591620717411303422\", \"1180591620717411303421\"]"
    );
    assert_eq!(lines[4], "[5, 6, 7, 8, 9, 10]");
    assert_eq!(lines[5], "neg=0");
    assert_eq!(lines[6], "ret_class=Integer");
    assert_eq!(lines[7], "break=early");
    assert_eq!(lines[8], "true");
    assert_eq!(lines[9], "true");
    assert_eq!(lines[10], "true");
}

#[test]
fn int_iter_arity_and_coerce_errors_match_cruby() {
    // Sibling fix to bigint_iter_arity_and_coerce_errors_match_cruby
    // (PR #174 cycle 2): the Int-recv side of times/upto/downto
    // had the same gap. Pre-fix `5.upto(3.14)` and `5.times(99)`
    // both raised NoMethodError; CRuby raises TypeError /
    // ArgumentError respectively, and `respond_to?(:times|:upto|
    // :downto)` already returns true on Int via the lookup.rs
    // whitelist — so the divergence is observable from rescue
    // clauses.
    //
    // Int recv + BigInt arg (e.g. `5.upto(2**100)`) is handled by
    // the BigInt arm and exercised separately.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_class, expected_msg) in [
        // Float endpoint is accepted (yields up to floor / down to
        // ceil) — covered separately. Non-numeric endpoints raise
        // ArgumentError, matching CRuby's "comparison of Integer
        // with X failed" wording (the upto/downto loop uses `<=>`
        // internally, so the comparison failure surfaces as
        // ArgumentError rather than TypeError).
        ("5.upto(\"x\") { }",  "ArgumentError", "comparison of Integer with String failed"),
        ("5.upto(nil) { }",    "ArgumentError", "comparison of Integer with nil failed"),
        ("5.upto { }",         "ArgumentError", "wrong number of arguments (given 0, expected 1)"),
        ("5.upto(1, 2) { }",   "ArgumentError", "wrong number of arguments (given 2, expected 1)"),
        ("5.downto(\"x\") { }", "ArgumentError", "comparison of Integer with String failed"),
        ("5.downto { }",       "ArgumentError", "wrong number of arguments (given 0, expected 1)"),
        ("5.times(99) { }",    "ArgumentError", "wrong number of arguments (given 1, expected 0)"),
        ("5.times(1, 2) { }",  "ArgumentError", "wrong number of arguments (given 2, expected 0)"),
    ] {
        let err = rt.eval(script, "int_iter_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            other => panic!("expected Uncaught {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_iter_arity_and_coerce_errors_match_cruby() {
    // Phase B.6 review cycle 2: pre-fix wrong-arity / non-Integer
    // arg calls bypassed the loop arms and fell through to
    // NoMethodError — diverging from CRuby's ArgumentError /
    // TypeError. \`respond_to?\` answers true for these methods
    // (the lookup.rs whitelist gates by name only), so user
    // code's \`rescue ArgumentError\` keys on the wrong class
    // without explicit guards.
    let mut rt = rubyrs::Runtime::new();
    let big = "(2 ** 70)";
    for (script, expected_class, expected_msg) in [
        (
            format!("{}.times(99) {{ }}", big),
            "ArgumentError",
            "wrong number of arguments (given 1, expected 0)",
        ),
        (
            format!("{}.times(1, 2) {{ }}", big),
            "ArgumentError",
            "wrong number of arguments (given 2, expected 0)",
        ),
        (
            format!("{}.upto {{ }}", big),
            "ArgumentError",
            "wrong number of arguments (given 0, expected 1)",
        ),
        (
            format!("{}.upto(1, 2) {{ }}", big),
            "ArgumentError",
            "wrong number of arguments (given 2, expected 1)",
        ),
        // Float endpoint for BigInt receiver — the Int Float arm
        // is not extended to BigInt receivers (BigInt iteration
        // beyond i64 isn't wired through `f.floor() as i64`).
        // The general non-numeric arm catches it and raises
        // ArgumentError "comparison of Integer with Float failed",
        // which matches CRuby for the BigInt-vs-Float case (both
        // bottom out in `<=>` for the loop bound). Pin the
        // boundary so a future refactor that widens the Int Float
        // arm to BigInt (or returns TypeError instead) trips this.
        (
            format!("{}.upto(3.14) {{ }}", big),
            "ArgumentError",
            "comparison of Integer with Float failed",
        ),
        (
            format!("{}.downto(3.14) {{ }}", big),
            "ArgumentError",
            "comparison of Integer with Float failed",
        ),
        // Sibling case for non-numeric arg shifted to ArgumentError
        // to match the Int-recv side.
        (
            format!("{}.downto(\"x\") {{ }}", big),
            "ArgumentError",
            "comparison of Integer with String failed",
        ),
        (
            format!("{}.downto(nil) {{ }}", big),
            "ArgumentError",
            "comparison of Integer with nil failed",
        ),
    ] {
        let err = rt.eval(&script, "iter_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            other => panic!("expected Uncaught {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_iter_yield_pinned_across_rest_param_gc_window() {
    // Regression for PR #174 cycle 1: `invoke_block` builds the
    // rest-args Array via heap.alloc, which runs maybe_gc with
    // only the Block pinned — leaving any freshly-allocated
    // yielded Value reachable only from the local args Vec,
    // which GC doesn't see. Without the per-iteration
    // `vm.pinned.push(yield_val)` fix, this would sweep the
    // yielded BigInt and either panic or read garbage into
    // the rest-Array.
    //
    // Reproducer: BigInt counter (`(big - 5).upto(big)` yields
    // five separately-allocated BigInts), block with `|*args|`
    // rest param, allocations inside the block to trigger GC.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // `|*args|` triggers the rest-args allocation path in
        // invoke_block. The body allocates strings to pressure GC.
        // The yielded BigInt must survive into the rest-Array so
        // `args[0].to_s` produces the right value.
        "out = []\n\
         (2 ** 80).upto(2 ** 80 + 4) do |*args|\n\
           50.times { |k| _ = \"alloc#{k}\".dup }\n\
           out << args[0].to_s\n\
         end\n\
         puts out.size\n\
         puts out.first\n\
         puts out.last",
        "bigint_iter_rest_gc.rb",
    ).expect("eval");
    let lines: Vec<String> = buf.snapshot().trim().split('\n').map(String::from).collect();
    assert_eq!(lines[0], "5");
    assert_eq!(lines[1], "1208925819614629174706176"); // 2^80
    assert_eq!(lines[2], "1208925819614629174706180"); // 2^80 + 4
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_iter_survives_gc_inside_block() {
    // GC stress: the yielded BigInt sits in the block-arg slot
    // (a Ruby stack root) during invocation, but the block may
    // allocate strings that trigger maybe_gc. Verify the heap
    // entry stays reachable across collection cycles, with the
    // BigInt recv pinned via PinGuard so it survives too.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // 6 iterations, each allocating 50 small Strings to
        // pressure the heap. If the counter BigInt got swept
        // mid-iteration the to_s call would panic / read garbage.
        "out = []\n\
         (2 ** 80).upto(2 ** 80 + 5) do |i|\n\
           50.times { |k| _ = \"alloc#{k}\".dup }\n\
           out << i.to_s\n\
         end\n\
         puts out.size\n\
         puts out.first\n\
         puts out.last",
        "bigint_iter_gc.rb",
    ).expect("eval");
    let lines: Vec<String> = buf.snapshot().trim().split('\n').map(String::from).collect();
    assert_eq!(lines[0], "6");
    assert_eq!(lines[1], "1208925819614629174706176"); // 2^80
    assert_eq!(lines[2], "1208925819614629174706181"); // 2^80 + 5
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_succ_pred_promote_at_i64_boundary_and_demote_back() {
    // Closes the subset gap surfaced by integer_bit_length_spec's
    // commented `.succ`/`.pred` lines. Covers four invariants:
    //
    // 1. BigInt#succ / #pred: `+1` / `-1` through bigint_to_value.
    //    `(1 << 100).succ.bit_length == 101` (no demote — value
    //    stays > i64::MAX), `(1 << 100).pred.bit_length == 100`
    //    (one off the round number → narrower).
    //
    // 2. Int → BigInt promotion at the i64 boundary:
    //    `i64::MAX.succ` returns BigInt(2^63), `i64::MIN.pred`
    //    returns BigInt(-(2^63 + 1)). Pre-fix the wrapping
    //    `wrapping_add(1)` / `wrapping_sub(1)` in numeric.rs
    //    wrapped to i64::MIN / i64::MAX respectively, breaking
    //    Ruby semantics.
    //
    // 3. BigInt → Int demote-on-fit: `(2 ** 63).pred` lands on
    //    i64::MAX which fits, so bigint_to_value demotes back to
    //    `Value::Int(i64::MAX)`.
    //
    // 4. respond_to? whitelist updated so `(2 ** 100).respond_to?(:succ)`
    //    returns true (previously fell back to method-missing
    //    answer since the whitelist hadn't listed succ/pred yet).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (1 << 100).succ.bit_length\n\
         puts (1 << 100).pred.bit_length\n\
         puts (9223372036854775807).succ\n\
         puts (9223372036854775807).succ.class.name\n\
         puts (-9223372036854775808).pred\n\
         puts (-9223372036854775808).pred.class.name\n\
         puts (2 ** 63).pred\n\
         puts (2 ** 63).pred.class.name\n\
         puts 5.succ\n\
         puts 5.next\n\
         puts 5.pred\n\
         puts (2 ** 100).respond_to?(:succ)\n\
         puts (2 ** 100).respond_to?(:next)\n\
         puts (2 ** 100).respond_to?(:pred)",
        "succ_pred.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "101");
    assert_eq!(lines[1], "100");
    assert_eq!(lines[2], "9223372036854775808");
    assert_eq!(lines[3], "Integer"); // BigInt prints as Integer
    assert_eq!(lines[4], "-9223372036854775809");
    assert_eq!(lines[5], "Integer");
    assert_eq!(lines[6], "9223372036854775807"); // demoted to i64::MAX
    assert_eq!(lines[7], "Integer");
    assert_eq!(lines[8], "6");
    assert_eq!(lines[9], "6");
    assert_eq!(lines[10], "4");
    assert_eq!(lines[11], "true");
    assert_eq!(lines[12], "true");
    assert_eq!(lines[13], "true");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_bitwise_not_uses_twos_complement_identity() {
    // Phase B.3: BigInt bit ops. `~big` is two's-complement
    // bitwise NOT — equivalent to `-(big + 1)` for any sign.
    // Numeric.rs's `(Int, "~", [])` arm handles Int receivers
    // (since `!i64::MIN == i64::MAX` fits without promotion),
    // but BigInt receivers need bigint_primitive's path.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // - `~(2**100)` = -(2^100 + 1) — stays BigInt.
        // - `~(-(2**100))` = -(-(2^100) + 1) = 2^100 - 1 — stays BigInt.
        // - `~(2**63)` = -(2^63 + 1) — one past i64::MIN, stays BigInt.
        // - `~(2**63 - 1)` = -(2^63) = i64::MIN — demotes to Int via
        //   bigint_to_value's demote-on-fit. Pins that the demote
        //   funnel runs for `~` results too (catches a regression
        //   where the bit-op path bypassed bigint_to_value).
        // - `~~big == big` round-trip (involution).
        "puts (~(2 ** 100)).to_s\n\
         puts (~(-(2 ** 100))).to_s\n\
         puts (~(2 ** 63)).to_s\n\
         puts (~(2 ** 63 - 1)).to_s\n\
         puts (~(2 ** 63 - 1)).class.name\n\
         puts (~~(2 ** 100)).to_s == (2 ** 100).to_s",
        "bigint_bitnot.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "-1267650600228229401496703205377");
    assert_eq!(lines[1], "1267650600228229401496703205375");
    assert_eq!(lines[2], "-9223372036854775809");
    assert_eq!(lines[3], "-9223372036854775808");
    assert_eq!(lines[4], "Integer");
    assert_eq!(lines[5], "true");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_bitwise_and_or_xor_two_complement_semantics() {
    // Phase B.3b: `&` / `|` / `^` with at least one BigInt operand.
    // CRuby uses unbounded two's-complement representation for
    // negatives in bitwise ops. num_bigint's BitAnd/Or/Xor impls
    // perform the conversion internally so we just route through
    // them — but pin the expected results to catch any future
    // regression in either the num_bigint contract or our hook.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Magnitude masks: `(2**100) & 0xff == 0` (low 8 bits of
        // 2^100 are all 0), demotes to Int.
        // Sign extension: `(-1) & (2**100) == 2**100` (-1 is
        // all-ones in two's-complement).
        // Sign extension: `(-256) & 0xff == 0` (low 8 bits of
        // two's-complement -256 are clear).
        // OR with low bit: `(2**100) | 1` lights bit 0 — full
        // BigInt result.
        // Self-XOR: cancels to 0 (Int via demote).
        // Inverse receiver: `5 & (2**100)` — Int recv + BigInt arg,
        // exercises the recv-or-arg guard path.
        // Mixed sign: `(-(2**100)) & 0xff == 0` (bit 0..7 of
        // -(2^100) in two's-complement are 0).
        "puts ((2 ** 100) & 0xff)\n\
         puts ((2 ** 100) & 0xff).class.name\n\
         puts ((-1) & (2 ** 100))\n\
         puts ((-256) & 0xff)\n\
         puts ((2 ** 100) | 1)\n\
         puts ((2 ** 100) ^ (2 ** 100))\n\
         puts ((2 ** 100) ^ (2 ** 100)).class.name\n\
         puts (5 & (2 ** 100))\n\
         puts ((-(2 ** 100)) & 0xff)",
        "bigint_bitops.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "Integer");
    assert_eq!(lines[2], "1267650600228229401496703205376");
    assert_eq!(lines[3], "0");
    assert_eq!(lines[4], "1267650600228229401496703205377");
    assert_eq!(lines[5], "0");
    assert_eq!(lines[6], "Integer");
    assert_eq!(lines[7], "0");
    assert_eq!(lines[8], "0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_left_right_promote_and_collapse() {
    // Phase B.3c: `<<` / `>>` with BigInt-flavoured operands.
    // Covers:
    // - Int recv overflow promote: `1 << 64` was Int 0 pre-fix
    //   (wrapping_shl clamped to 63), now BigInt 2^64.
    // - BigInt magnitude: `1 << 100` produces 2^100.
    // - BigInt recv right-shift: `(2**100) >> 50` = 2^50 (Int demote).
    // - Right-shift collapse: shifting past bit-length returns 0
    //   (non-neg) or -1 (neg) via the early-exit, not a giant alloc.
    // - Negative shift count: `5 << -1 == 5 >> 1 == 2`.
    // - Demote-on-fit: `(2**100) << -100 == 1`.
    // - Identity short-circuit: `1 << 0 == 1` returns recv unchanged.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (1 << 64)\n\
         puts (1 << 64).class.name\n\
         puts (1 << 100)\n\
         puts ((2 ** 100) >> 50)\n\
         puts ((2 ** 100) >> 50).class.name\n\
         puts ((2 ** 100) >> 1000)\n\
         puts ((-(2 ** 100)) >> 1000)\n\
         puts (5 << -1)\n\
         puts ((2 ** 100) << -100)\n\
         puts ((2 ** 100) << -100).class.name\n\
         puts (5 >> 100)\n\
         puts ((-1) >> 100)",
        "bigint_shifts.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "18446744073709551616");
    assert_eq!(lines[1], "Integer");
    assert_eq!(lines[2], "1267650600228229401496703205376");
    assert_eq!(lines[3], "1125899906842624");
    assert_eq!(lines[4], "Integer");
    assert_eq!(lines[5], "0");
    assert_eq!(lines[6], "-1");
    assert_eq!(lines[7], "2");
    assert_eq!(lines[8], "1");
    assert_eq!(lines[9], "Integer");
    assert_eq!(lines[10], "0");
    assert_eq!(lines[11], "-1");
}

#[cfg(feature = "bignum")]
#[test]
fn int_shift_left_promotes_on_value_overflow_not_just_count_overflow() {
    // Regression for PR #159 cycle 1: `i64::checked_shl` only
    // detects shift-count overflow (≥ 64), not value overflow.
    // Pre-fix, `1 << 63` returned `i64::MIN` (sign bit set,
    // wrapping into negative space) instead of promoting to
    // BigInt(2^63). Round-trip check `(a << s) >> s == a`
    // catches bit-loss exactly so these subtler overflow cases
    // promote like the count-overflow path already did for
    // `1 << 64`.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // - `1 << 62` is exactly the largest positive i64 (sign
        //   bit clear) — must stay Int, no false promote.
        // - `1 << 63` is +2^63 in Ruby (positive Bignum), not
        //   `i64::MIN`. Must promote.
        // - `5 << 61` overflows into the sign bit (5 takes 3
        //   bits, +61 = bit 63 set) — must promote.
        // - `1 >> -63` == `1 << 63` via direction swap — same
        //   value-overflow path, must promote.
        // - `(-1) << 1` == -2 stays Int (sign-preserving, no
        //   bit-loss).
        "puts (1 << 62)\n\
         puts (1 << 62).class.name\n\
         puts (1 << 63)\n\
         puts (1 << 63).class.name\n\
         puts (5 << 61)\n\
         puts (5 << 61).class.name\n\
         puts (1 >> -63)\n\
         puts (1 >> -63).class.name\n\
         puts ((-1) << 1)\n\
         puts ((-1) << 1).class.name",
        "int_shift_value_overflow.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "4611686018427387904");
    assert_eq!(lines[1], "Integer");
    assert_eq!(lines[2], "9223372036854775808"); // +2^63, NOT i64::MIN
    assert_eq!(lines[3], "Integer");
    assert_eq!(lines[4], "11529215046068469760"); // 5 * 2^61
    assert_eq!(lines[5], "Integer");
    assert_eq!(lines[6], "9223372036854775808"); // 1 >> -63 == 1 << 63
    assert_eq!(lines[7], "Integer");
    assert_eq!(lines[8], "-2");
    assert_eq!(lines[9], "Integer");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_left_dos_cap_uses_exact_int_bit_length() {
    // Regression for PR #159 cycle 1: `recv_bits` over-counted
    // Int receivers as 64 bits, so small-magnitude shifts under a
    // tight `max_value_bytes` could false-trap even when the
    // rendered BigInt fit. With exact bit-length for Ints, the
    // cap estimator matches the actual storage.
    //
    // `5 << 1_000_000` produces a ~125 KB BigInt. Pre-fix recv_bits
    // = 64 → est_bits = 1_000_064 → est_bytes ≈ 125_040. With
    // a cap of 125_064 bytes (just above the true est) pre-fix
    // would still trap because the 64-bit Int width over-counted
    // by ~61 bits. Post-fix recv_bits = bit_length(5) = 3 →
    // est_bits = 1_000_003 → est_bytes ≈ 125_032, passes.
    let cfg = rubyrs::Config { max_value_bytes: Some(125_064), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // `class` returns Integer for both Int and Bignum, so check
        // a deterministic property: bit_length matches the shift.
        "puts (5 << 1_000_000).bit_length",
        "shift_dos_exact_bits.rb",
    ).expect("eval");
    // `bit_length(5) == 3`, so `(5 << 1_000_000).bit_length == 1_000_003`.
    // Ruby prints integers without underscores.
    assert_eq!(buf.snapshot().trim(), "1000003");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_responds_to_bit_op_names_matches_dispatch() {
    // Regression for PR #159 cycle 2: `Vm::responds_to`'s BigInt
    // whitelist must include every method `bigint_primitive` can
    // dispatch — otherwise `big.respond_to?(:<<)` returns false
    // even though the call succeeds, breaking pure-Ruby code that
    // gates on respond_to?. Phase B.3 adds `~`, `& | ^`, `<< >>`.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "b = 2 ** 100\n\
         puts b.respond_to?(:~)\n\
         puts b.respond_to?(:&)\n\
         puts b.respond_to?(:|)\n\
         puts b.respond_to?(:^)\n\
         puts b.respond_to?(:<<)\n\
         puts b.respond_to?(:>>)",
        "bigint_responds_to_bit_ops.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert_eq!(out.trim(), "true\ntrue\ntrue\ntrue\ntrue\ntrue");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn int_shift_i64_min_count_does_not_panic_under_no_bignum() {
    // Regression for the no-bignum `<<` / `>>` arms in
    // numeric.rs: pre-fix `(-b) as u32` overflowed when
    // `b == i64::MIN` (debug builds panicked with "attempt to
    // negate with overflow"; release silently wrapped to a
    // 63-bit shift via two-step wrap). Pin clamp semantics so
    // both profiles agree on the result for this corner.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    // `5 << i64::MIN` == `5 >> |i64::MIN|` == `5 >> 63` == 0.
    // `(-1) << i64::MIN` == `(-1) >> |i64::MIN|` == -1 (sign-ext).
    // `5 >> i64::MIN` == `5 << |i64::MIN|` clamped to 63 bits;
    //   `5.wrapping_shl(63)` produces `i64::MIN`-relative bit
    //   pattern (5 << 63 wraps), but the saturating-shift
    //   semantics under no-bignum just want no-panic + matching
    //   the existing wrapping behaviour. Pin the result so
    //   future refactors don't accidentally change it.
    rt.eval(
        "x = -9223372036854775807 - 1\n\
         puts (5 << x)\n\
         puts ((-1) << x)",
        "shift_i64_min_no_bignum.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "-1");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn int_bit_ops_raise_typeerror_on_non_integer_arg_no_bignum() {
    // Sibling guard to `integer_bit_ops_raise_typeerror_on_non_integer_arg`
    // (bignum-on profile) — under no-bignum the BigInt-side
    // helpers don't exist, so without the Int-side coerce arm
    // in numeric.rs `3 & 3.4` would fall through to
    // NoMethodError instead of CRuby's TypeError. Pin the
    // Int-side guard added in B.6 bit-ops spec batch (sibling
    // to PR #186's iter-method guards).
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_arg_type) in [
        ("3 & 3.4", "Float"),
        ("3 | 3.4", "Float"),
        ("3 ^ 3.4", "Float"),
        ("3 << 3.4", "Float"),
        ("3 >> 3.4", "Float"),
        ("3 & nil", "nil"),
        ("3 << \"4\"", "String"),
        ("3 >> :sym", "Symbol"),
    ] {
        let err = rt.eval(script, "int_bit_op_typeerr.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "TypeError", "for {:?}", script);
                assert_eq!(
                    message,
                    format!("no implicit conversion of {} into Integer", expected_arg_type),
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught TypeError for {:?}, got {:?}", script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn integer_bit_ops_raise_typeerror_on_non_integer_arg() {
    // Phase B.3 follow-up: pre-fix `try_bigint_bit_binop` and
    // `try_bigint_bit_shift` returned `Ok(None)` when the arg
    // wasn't an Integer, falling through to NoMethodError. CRuby
    // raises TypeError "no implicit conversion of X into Integer"
    // — same shape as the BigInt-arith coerce errors and as the
    // unified `Integer#to_s(non_integer)` arm. Pin that both
    // Int and BigInt receivers route through the same TypeError
    // for every bit-op selector. Covers:
    // - all 5 bit-op selectors (& | ^ << >>)
    // - all 4 non-Integer arg types (Float, String, nil, Symbol)
    // - both Int and BigInt receivers
    // - the special `Int(0)` recv case (which used to short-circuit
    //   ahead of the arg-type guard)
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_arg_type) in [
        // BigInt recv, every selector × Float arg
        ("(2 ** 100) & 1.5", "Float"),
        ("(2 ** 100) | 1.5", "Float"),
        ("(2 ** 100) ^ 1.5", "Float"),
        ("(2 ** 100) << 1.5", "Float"),
        ("(2 ** 100) >> 1.5", "Float"),
        // Int recv, non-Integer args
        ("5 & 1.5", "Float"),
        ("5 << 1.5", "Float"),
        ("5 >> \"foo\"", "String"),
        ("5 << nil", "nil"),
        ("5 << :sym", "Symbol"),
        // Int(0) recv: regression for the swallow-TypeError fix.
        ("0 << 1.5", "Float"),
        ("0 >> :sym", "Symbol"),
        ("0 << nil", "nil"),
    ] {
        let err = rt.eval(script, "bit_op_nonint_arg.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "TypeError", "for {:?}", script);
                assert_eq!(
                    message,
                    format!("no implicit conversion of {} into Integer", expected_arg_type),
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught TypeError for {:?}, got {:?}", script, other),
        }
    }
}

#[test]
fn int_bit_ops_raise_argumenterror_on_bad_arity() {
    // Arity guard sibling to `pow`'s in numeric.rs: `respond_to?`
    // returns true for `:& :| :^ :<< :>>` on Integer, so
    // `5.send(:&, 1, 2)` / `5.send(:&)` must raise ArgumentError
    // (CRuby) instead of falling through to NoMethodError. Pin
    // exact message + count for both 0-arg and 2-arg shapes
    // across every bit-op selector. Lives outside the cfg-gated
    // typeerror tests because the same guard runs on both profiles.
    let mut rt = rubyrs::Runtime::new();
    for (script, given) in [
        ("5.send(:&)", 0),
        ("5.send(:|)", 0),
        ("5.send(:^)", 0),
        ("5.send(:<<)", 0),
        ("5.send(:>>)", 0),
        ("5.send(:&, 1, 2)", 2),
        ("5.send(:|, 1, 2)", 2),
        ("5.send(:^, 1, 2)", 2),
        ("5.send(:<<, 1, 2)", 2),
        ("5.send(:>>, 1, 2)", 2),
    ] {
        let err = rt.eval(script, "int_bit_op_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
                assert_eq!(
                    message,
                    format!("wrong number of arguments (given {}, expected 1)", given),
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught ArgumentError for {:?}, got {:?}", script, other),
        }
    }
}

#[test]
fn integer_to_s_raises_argumenterror_on_bad_arity() {
    // Arity guards for `Integer#to_s` / `Integer#inspect`.
    // `respond_to?` returns true for both selectors on Int (and
    // BigInt under bignum), so wrong-arity shapes must raise
    // ArgumentError per CRuby, not NoMethodError. `to_s` accepts
    // 0..1 args; `inspect` accepts exactly 0. Sibling arity
    // guards already cover bit ops (PR #211) and iter methods
    // (PR #186). Happy paths are tested by the spec micro-runner.
    let mut rt = rubyrs::Runtime::new();
    for (script, given, expected_range) in [
        ("5.send(:to_s, 10, 20)", 2, "0..1"),
        ("5.send(:to_s, 10, 20, 30)", 3, "0..1"),
        ("5.send(:inspect, 1)", 1, "0"),
        ("5.send(:inspect, 1, 2)", 2, "0"),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:to_s, 10, 20)", 2, "0..1"),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:to_s, 10, 20, 30)", 3, "0..1"),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:inspect, 1)", 1, "0"),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:inspect, 1, 2)", 2, "0"),
    ] {
        let err = rt.eval(script, "int_to_s_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
                assert_eq!(
                    message,
                    format!("wrong number of arguments (given {}, expected {})", given, expected_range),
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught ArgumentError for {:?}, got {:?}", script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_case_compare_float_is_lossless_via_ruby_eq() {
    // Pin the `ruby_eq` (heap.rs) BigInt × Float lossless path.
    // Used by `===` (case/when), Array#include?, and `==`/`!=`
    // dispatch fast-path. (Hash key lookup goes through `ruby_eql`
    // — eql?-based, type-strict — not ruby_eq, so it's NOT
    // affected by this fix; tracked separately if/when Hash gets
    // a `==`-style include? variant.) Pre-fix this PR, ruby_eq
    // had no BigInt × Float arm — comparisons fell through to
    // the catch-all `_ => false`. Now routes through the same
    // `bigint_equals_float_lossless` helper as the BinOp `==`
    // path, so `===` returns the right answer in both directions
    // and Array#include? finds float-shaped duplicates of BigInt
    // members.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "nan = 0.0 / 0.0\n\
         inf = 1.0 / 0.0\n\
         # === — both directions\n\
         puts ((2**64) === (2**64).to_f)             # true (exact)\n\
         puts ((2**64 + 1) === (2**64).to_f)         # false (precision)\n\
         puts ((2**64).to_f === (2**64))             # true (symmetric)\n\
         puts ((2**64) === 1.5)                      # false (fractional)\n\
         puts ((2**64) === nan)                      # false\n\
         puts ((2**64) === inf)                      # false\n\
         puts ((2**64) === -inf)                     # false\n\
         puts ((-(2**64)) === -(2**64).to_f)         # true (negative exact)\n\
         # Array#include? — uses ruby_eq too\n\
         puts [2**64, 5].include?((2**64).to_f)      # true\n\
         puts [2**64 + 1, 5].include?((2**64).to_f)  # false (precision preserved)\n\
         # Int × Float precision via ruby_eq — sibling to the\n\
         # BigInt path above. The demote-to-f64 bug existed on\n\
         # both Int and BigInt sides of ruby_eq; both arms now\n\
         # route through their respective lossless helpers.\n\
         puts ((2**62 + 1) === (2**62).to_f)         # false (|i| > 2^53)\n\
         puts ((2**62) === (2**62).to_f)             # true (exact)\n\
         puts [2**62 + 1].include?((2**62).to_f)     # false",
        "bigint_eq_float_ruby_eq.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines, vec![
        "true", "false", "true", "false",   // === precision + symmetric + fractional
        "false", "false", "false",          // NaN / ±inf
        "true",                              // negative exact
        "true", "false",                     // Array#include? precision (BigInt)
        "false", "true", "false",           // Int × Float precision + include?
    ]);
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_eq_float_is_lossless() {
    // Pin the BigInt × Float `==` lossless path
    // (bigint_equals_float_lossless in bignum.rs). Pre-fix the arm
    // demoted BigInt to f64 for the compare, so values within the
    // same Float ULP wrongly compared equal. Example: f64 ULP at
    // 2^64 is 2^(64-52)=4096, so 2**64+1 rounds to exactly 2**64,
    // and the pre-fix BigInt-side also collapsed to 2**64 — both
    // sides end up at the same Float bit pattern.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Each puts produces one line; the closing block pins
        // the exact 14-line transcript so a regression in any
        // single arm fails loudly with the wrong line.
        "nan = 0.0 / 0.0\n\
         inf = 1.0 / 0.0\n\
         puts (2**64) == (2**64).to_f         # true (Float-exact)\n\
         puts (2**64 + 1) == (2**64).to_f     # false (precision)\n\
         puts (2**64) == (2**64 + 1).to_f     # true (RHS rounds: f64 ULP at 2^64 is 2^(64-52)=4096; 2**64+1 is far closer to 2**64 than to 2**64+4096, so it rounds to exactly 2**64)\n\
         puts (2**64).to_f == (2**64 + 1)     # false (Float side is 2**64, not 2**64+1)\n\
         puts (2**64) == nan                  # false (NaN)\n\
         puts (2**64) == inf                  # false (+inf)\n\
         puts (2**64) == -inf                 # false (-inf)\n\
         puts (2**64) == 1.5                  # false (fractional)\n\
         puts (2**64) == 0.0                  # false (nonzero BigInt vs zero Float)\n\
         puts (-(2**64)) == -(2**64).to_f     # true (negative exact)\n\
         puts (2**100) == (2**100).to_f       # true (2^100 exact in f64)\n\
         puts (2**64 + 1) != (2**64).to_f     # true (negation)\n\
         puts (2**64) != (2**64).to_f         # false (negation)\n\
         puts nan != (2**64)                  # true (NaN ne)",
        "bigint_eq_float.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines, vec![
        "true", "false", "true", "false",   // 2**64 ± 1 cases (note: (2**64+1).to_f rounds to 2**64)
        "false", "false", "false",          // NaN / ±inf
        "false", "false",                   // fractional / zero
        "true", "true",                     // negative-exact / 2^100-exact
        "true", "false", "true",            // != cases
    ]);
}

#[test]
fn integer_ceil_floor_round_truncate_basic() {
    // Pin the 4 new Integer rounding methods. Spec coverage lives in
    // spec/ruby/integer_{ceil,floor,round,truncate}_spec.rb; this is
    // the cross-profile embed guard for arity/TypeError dispatch and
    // a few key negative-precision answers.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        // 0-arg: returns self
        ("10.ceil.inspect", "10"),
        ("(-15).floor.inspect", "-15"),
        ("10.round.inspect", "10"),
        ("10.truncate.inspect", "10"),
        // Positive precision: returns self
        ("123.ceil(10).inspect", "123"),
        ("123.floor(10).inspect", "123"),
        ("(-123).round(5).inspect", "-123"),
        ("123.truncate(7).inspect", "123"),
        // Negative precision
        ("123.ceil(-1).inspect", "130"),
        ("123.ceil(-2).inspect", "200"),
        ("(-123).ceil(-1).inspect", "-120"),
        ("123.floor(-1).inspect", "120"),
        ("(-123).floor(-1).inspect", "-130"),
        ("250.round(-2).inspect", "300"),
        ("(-250).round(-2).inspect", "-300"),
        ("249.round(-2).inspect", "200"),
        ("1832.truncate(-2).inspect", "1800"),
        ("(-1832).truncate(-2).inspect", "-1800"),
        // BigInt precision — accepted as no-op (positive sign;
        // canonical-BigInt invariant means |x| > i64::MAX so any
        // BigInt is far past the 38-digit i128 ceiling the
        // Int-precision path uses).
        #[cfg(feature = "bignum")]
        ("123.round(2**64).inspect", "123"),
        #[cfg(feature = "bignum")]
        ("123.ceil(2**64).inspect", "123"),
        #[cfg(feature = "bignum")]
        ("(2**100).floor(2**64).inspect", "1267650600228229401496703205376"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(&format!("puts {}", script), "round.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // Errors
    for (script, expected_class) in [
        ("42.round(\"4\")", "TypeError"),
        ("42.round(nil)", "TypeError"),
        ("42.round(1, 2)", "ArgumentError"),
        ("42.ceil(:sym)", "TypeError"),
    ] {
        let err = rt.eval(script, "round_err.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
            }
            other => panic!("expected {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
}

#[test]
fn round_half_kwarg_dispatch() {
    // End-to-end pin for the new kwarg routing infra:
    // `Op::CallKw` emitted for `foo(a, half: :up)` sugar →
    // `do_call_kw` resolves the `:half` Symbol → dispatches
    // `int_round_with_half` / `float_round_with_half`.
    //
    // Distinction from a positional Hash: `25.round(-1, {half:
    // :up})` (explicit braces) is positional, hits the normal
    // round arm, and the Hash is ignored — that's the today
    // behaviour and is intentional for the MVP. Pin only the
    // kwarg-sugar form here.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        // Int receiver, default + each mode at half boundary.
        ("puts 25.round(-1, half: :up)",   "30"),
        ("puts 25.round(-1, half: :down)", "20"),
        ("puts 25.round(-1, half: :even)", "20"),
        ("puts 35.round(-1, half: :even)", "40"),
        ("puts (-25).round(-1, half: :up)",   "-30"),
        ("puts (-25).round(-1, half: :down)", "-20"),
        // Float receiver, no-precision form.
        ("puts 2.5.round(half: :up)",   "3"),
        ("puts 2.5.round(half: :down)", "2"),
        ("puts 2.5.round(half: :even)", "2"),
        ("puts 3.5.round(half: :even)", "4"),
        // Float receiver, negative-precision form.
        ("puts 25.0.round(-1, half: :down)", "20"),
        // Positional Hash WITHOUT kwarg sugar should NOT be
        // routed through the kwarg path. `25.round(-1)` defaults
        // to :up.
        ("puts 25.round(-1)", "30"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "round_half.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // Error shapes: unknown rounding mode + unknown kwarg key.
    for (script, expected_class, expected_msg) in [
        ("25.round(-1, half: :weird)", "ArgumentError", "invalid rounding mode: weird"),
        ("25.round(-1, foo: :bar)",    "ArgumentError", "unknown keyword: :foo"),
    ] {
        let err = rt.eval(script, "round_half_err.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected {} for {:?}, got {:?}", expected_class, script, other),
        }
    }

    // Fall-through cases — do_call_kw must NOT intercept these
    // shapes; the regular round arm / user-method dispatch must
    // still fire so existing arity / TypeError guards aren't
    // bypassed.
    //
    // (a) Unsupported arity — `25.round(1, 2, half: :up)` should
    //     surface CRuby's ArgumentError, not NoMethodError.
    //     `:sym` precision falls back to the regular arm too —
    //     it surfaces ArgumentError ("wrong number of arguments")
    //     in the current MVP because the fallback path treats the
    //     kwargs Hash as a second positional arg; a fully kwarg-
    //     aware shape would need to peel the Hash off first and
    //     surface TypeError for the bad precision. Acceptable for
    //     MVP — both shapes are loud, both rescue under
    //     `StandardError`.
    for (script, expected_class) in [
        ("25.round(1, 2, half: :up)", "ArgumentError"),
        ("25.round(:sym, half: :up)", "ArgumentError"),
    ] {
        let err = rt.eval(script, "round_half_fall.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
            }
            ref other => panic!("expected {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
    // (b) User-defined `C#round(half:)` must reach the user
    //     method, not be shadowed by the primitive kwarg path.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "class C; def round(half:); \"user-#{half}\"; end; end; puts C.new.round(half: :down)",
        "round_half_user.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot().trim(), "user-down");

    // (post-#284 polish) |n| > 38 returns 0, not the original
    // receiver — matches the regular `Integer#round(-100)` arm.
    for (script, expected) in [
        ("puts 123.round(-100, half: :up)",   "0"),
        ("puts 123.round(-100, half: :down)", "0"),
        ("puts 123.round(-100, half: :even)", "0"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "round_half_large_n.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // (post-#284 polish) non-Symbol/String :half value reports
    // the inspect shape rather than the class name.
    for (script, expected_msg) in [
        ("25.round(-1, half: 0)",   "invalid rounding mode: 0"),
        ("25.round(-1, half: nil)", "invalid rounding mode: nil"),
        ("25.round(-1, half: 1.5)", "invalid rounding mode: 1.5"),
    ] {
        let err = rt.eval(script, "round_half_inspect.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected ArgumentError for {:?}, got {:?}", script, other),
        }
    }
    // (c) CRuby parity — String value for the `:half` kwarg is
    //     accepted the same as the Symbol form.
    for (script, expected) in [
        ("puts 25.round(-1, half: \"up\")",   "30"),
        ("puts 25.round(-1, half: \"down\")", "20"),
        ("puts 35.round(-1, half: \"even\")", "40"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "round_half_str.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }

    // (d) BigInt promotion on i64::MIN overflow — under the
    //     bignum profile, `(-2**63).round(-1, half: :up)` produces
    //     `-9223372036854775810` (doesn't fit i64). Pre-polish:
    //     silently returned i64::MIN unchanged.
    #[cfg(feature = "bignum")]
    {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(
            "puts ((-(2**63)).round(-1, half: :up)).inspect",
            "round_half_bignum.rb",
        ).expect("eval");
        assert_eq!(buf.snapshot().trim(), "-9223372036854775810");
    }
    // (e) Under no-bignum, the same overflow raises RangeError
    //     instead of silently truncating.
    #[cfg(not(feature = "bignum"))]
    {
        let err = rt.eval(
            "(-(2**63)).round(-1, half: :up)",
            "round_half_nobignum.rb",
        ).unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, .. } => {
                assert_eq!(class_name, "RangeError");
            }
            ref other => panic!("expected RangeError, got {:?}", other),
        }
    }
}

#[test]
fn integer_divmod_fdiv_gcd_lcm_basic() {
    // Quick happy-path + error pin for the 4 methods landed in this
    // batch. Spec coverage lives in spec/ruby/integer_{divmod,fdiv,
    // gcd,lcm}_spec.rb; this is the cross-profile guard for the
    // dispatch wiring (arity, TypeError, ZeroDivisionError).
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_inspect) in [
        // divmod (Int × Int floor + Float result)
        ("13.divmod(4).inspect", "[3, 1]"),
        ("(-13).divmod(4).inspect", "[-4, 3]"),
        ("13.divmod(4.0).inspect", "[3, 1.0]"),
        // fdiv
        ("8.fdiv(9.0).inspect", (8.0_f64 / 9.0).to_string().as_str()),
        ("1.fdiv(0).infinite?.inspect", "1"),
        ("(-1).fdiv(0).infinite?.inspect", "-1"),
        ("1.fdiv(0.0/0.0).nan?.inspect", "true"),
        // gcd/lcm
        ("10.gcd(5).inspect", "5"),
        ("(-12).gcd(-6).inspect", "6"),
        ("200.lcm(2001).inspect", "400200"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(&format!("puts {}", script), "div_fdiv_gcd_lcm.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected_inspect, "for {:?}", script);
    }
    // Errors
    for (script, expected_class) in [
        ("13.divmod(0)", "ZeroDivisionError"),
        ("13.divmod(0.0)", "ZeroDivisionError"),
        ("13.divmod(\"10\")", "TypeError"),
        ("13.divmod", "ArgumentError"),
        ("13.divmod(1, 2)", "ArgumentError"),
        ("1.fdiv(\"x\")", "TypeError"),
        ("1.fdiv", "ArgumentError"),
        ("12.gcd", "ArgumentError"),
        ("12.gcd(30, 20)", "ArgumentError"),
        ("39.gcd(3.8)", "TypeError"),
    ] {
        let err = rt.eval(script, "errors.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
            }
            other => panic!("expected {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
}

#[test]
fn integer_div_mod_floor_semantics() {
    // Pin CRuby floor-division semantics for `/` and `%`. Pre-fix
    // rubyrs used Rust's truncating-toward-zero `/` and `%`, so
    // `(-13) / 4` returned -3 (CRuby: -4) and `(-13) % 4`
    // returned -1 (CRuby: 3). Both the BinOp fast path
    // (`BinOpKind::apply_int`) and the cold method-call path
    // (numeric_call's `/` and `%` arms) route through the same
    // `floor_div_i64` / `floor_mod_i64` helpers, so the cases
    // below cover both entry points (literal-shape uses the
    // fast path; `send(...)` exercises the method-call shape).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "# Int×Int — every sign combination\n\
         puts (13 / 4)                    # 3\n\
         puts (13 % 4)                    # 1\n\
         puts ((-13) / 4)                 # -4 (floor, not -3)\n\
         puts ((-13) % 4)                 # 3 (sign follows divisor)\n\
         puts (13 / (-4))                 # -4\n\
         puts (13 % (-4))                 # -3\n\
         puts ((-13) / (-4))              # 3\n\
         puts ((-13) % (-4))              # -1\n\
         # Method-call shape — cold path through numeric.rs\n\
         puts (13.send(:/, 4))            # 3\n\
         puts ((-13).send(:%, 4))         # 3\n\
         # i64::MIN / -1 overflow promotion. The expression form\n\
         # works via apply_int's None → bigint_arith fallback;\n\
         # the method-call shape goes through numeric_call which\n\
         # also returns None, so bigint_primitive needs an explicit\n\
         # promotion hook (added per /code-review on PR #254).\n\
         puts ((-9223372036854775808) / -1).inspect           # 9223372036854775808\n\
         puts ((-9223372036854775808).send(:/, -1)).inspect   # 9223372036854775808 (lock-step)\n\
         # Float × Float — same floor semantics\n\
         puts ((-13.0) % 4.0)             # 3.0\n\
         puts (13.0 % (-4.0))             # -3.0\n\
         # Int × Float / Float × Int — same\n\
         puts ((-13) % 4.0)               # 3.0\n\
         puts ((-13.0) % 4)               # 3.0\n\
         # Infinity edge case (cycle 1 review): 1.0 % Infinity\n\
         # should be 1.0, not NaN. CRuby parity.\n\
         puts (1.0 % (1.0/0.0))           # 1.0",
        "div_mod_floor.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Under bignum, i64::MIN/-1 promotes to BigInt 2^63. Under
    // no-bignum, both paths wrap to i64::MIN per the documented
    // wrapping-on-overflow convention (floor_div_i64's doc).
    #[cfg(feature = "bignum")]
    let imin_div_expected = "9223372036854775808";
    #[cfg(not(feature = "bignum"))]
    let imin_div_expected = "-9223372036854775808";
    assert_eq!(lines, vec![
        "3", "1", "-4", "3", "-4", "-3", "3", "-1",  // Int×Int sign combos
        "3", "3",                                      // method-call shape
        imin_div_expected, imin_div_expected,          // i64::MIN/-1 (promote or wrap)
        "3.0", "-3.0", "3.0", "3.0",                   // Float-involved
        "1.0",                                         // Infinity
    ]);
}

#[test]
fn int_cmp_float_is_lossless() {
    // Sibling to bigint_cmp_float_is_lossless. The Int×Float
    // arm in numeric.rs pre-fix demoted the i64 to f64 for the
    // compare; for |i| > 2^53 the cast loses bits, so e.g.
    // `(2**62 + 1) > (2**62).to_f` returned false. Runs on
    // BOTH profiles because the helper is pure i64+f64 (no
    // BigInt required).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "nan = 0.0 / 0.0\n\
         inf = 1.0 / 0.0\n\
         # <=> (Int × Float, both directions)\n\
         puts ((2**62 + 1) <=> (2**62).to_f).inspect    # 1\n\
         puts ((2**62) <=> (2**62).to_f).inspect        # 0\n\
         puts ((2**62 - 1) <=> (2**62).to_f).inspect    # -1\n\
         puts ((2**62).to_f <=> (2**62 + 1)).inspect    # -1 (Float × Int reverses)\n\
         puts (1 <=> nan).inspect                       # nil\n\
         puts (1 <=> inf).inspect                       # -1\n\
         puts (1 <=> -inf).inspect                      # 1\n\
         # Ordering operators\n\
         puts ((2**62 + 1) > (2**62).to_f)              # true\n\
         puts ((2**62 + 1) < (2**62).to_f)              # false\n\
         puts ((2**62 + 1) <= (2**62).to_f)             # false\n\
         puts ((2**62 + 1) >= (2**62).to_f)             # true\n\
         puts ((2**62).to_f < (2**62 + 1))              # true (Float × Int direction)\n\
         # NaN: all four ordering ops are false (CRuby parity)\n\
         puts (1 < nan)                                 # false\n\
         puts (1 > nan)                                 # false\n\
         puts (1 <= nan)                                # false\n\
         puts (1 >= nan)                                # false\n\
         # Float-exact happy paths (under 2^53 — must still work)\n\
         puts (1 == 1.0)                                # true\n\
         puts (5 < 5.5)                                 # true\n\
         puts ((-3) == -3.0)                            # true\n\
         # i64::MIN boundary — i64::MIN.to_f = -2^63 is exact;\n\
         # the lower-bound check in int_cmp_float_lossless uses\n\
         # `<` (not `<=`) so equal-to-i64::MIN cases fall into\n\
         # the integer-compare branch rather than the sign-only\n\
         # short-circuit.\n\
         puts ((-9223372036854775808) <=> (-9223372036854775808).to_f).inspect  # 0\n\
         puts ((-9223372036854775808) <=> -1e30).inspect                        # 1 (f more neg)\n\
         puts ((-9223372036854775808) <=> -(2.0 ** 63 + 2048)).inspect          # 1 (next ulp below i64::MIN)\n\
         # Frac-sign disambiguation at zero\n\
         puts (0 <=> (-0.5)).inspect                                           # 1\n\
         puts (0 <=> 0.5).inspect                                              # -1",
        "int_cmp_float.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines, vec![
        "1", "0", "-1", "-1",                  // <=> precision + symmetric
        "nil", "-1", "1",                      // <=> NaN/±inf
        "true", "false", "false", "true",      // Lt/Le/Gt/Ge precision
        "true",                                 // Float × Int direction
        "false", "false", "false", "false",    // NaN ordering
        "true", "true", "true",                // Float-exact happy paths
        "0", "1", "1",                         // i64::MIN boundary
        "1", "-1",                              // frac-sign disambig at 0
    ]);
}

#[cfg(feature = "bignum")]
#[test]
fn aggregate_cmp_int_float_is_lossless() {
    // Sibling pin to int_cmp_float_is_lossless / bigint_cmp_float_is_lossless,
    // but for the aggregator path in vm/util.rs::value_cmp_v_heap_inner.
    // Used by Array#<=>, Array#sort, Array#min/max — pre-cycle-1
    // it still demoted Int→f64 (and had no BigInt×Float arm at
    // all, returning nil), so the direct operator and the
    // aggregate path diverged after the previous PRs in this
    // series. Both routes now use the same lossless helpers.
    // Gated on bignum because half the assertions use 2**64
    // literals which saturate to i64::MAX under no-bignum.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts ([2**62 + 1] <=> [(2**62).to_f]).inspect    # 1\n\
         puts ([2**62] <=> [(2**62).to_f]).inspect        # 0\n\
         puts ([2**62 - 1] <=> [(2**62).to_f]).inspect    # -1\n\
         puts ([(2**62).to_f] <=> [2**62 + 1]).inspect    # -1\n\
         # BigInt × Float (pre-fix returned nil — no arm at all)\n\
         puts ([2**64 + 1] <=> [(2**64).to_f]).inspect    # 1\n\
         puts ([2**64] <=> [(2**64).to_f]).inspect        # 0\n\
         puts ([(2**64).to_f] <=> [2**64 + 1]).inspect    # -1",
        "agg_cmp_int_float.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines, vec![
        "1", "0", "-1", "-1",   // Int × Float (both directions)
        "1", "0", "-1",         // BigInt × Float (both directions)
    ]);
}

#[test]
fn by_aggregators_cmp_int_float_is_lossless() {
    // Sibling to aggregate_cmp_int_float_is_lossless but for the
    // `_by` family (`min_by`, `max_by`, `sort_by`), which uses
    // the heap-less `value_cmp_v` aggregator. Pre-fix Int×Float
    // keys returned `None` (incomparable) and bubbled up as
    // NoMethodError; now routes through int_cmp_float_lossless.
    // BigInt×Float still out of scope here (value_cmp_v has no
    // heap access; tracked as a follow-up).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts ([1, 2, 3].min_by { |x| x == 2 ? 1.5 : x }).inspect    # 1\n\
         puts ([1, 2, 3].max_by { |x| x == 2 ? 1.5 : x }).inspect    # 3\n\
         puts ([3, 1, 2.5].sort_by { |x| x }).inspect                # [1, 2.5, 3]\n\
         puts ([2**62 + 1, 2**62 - 1].min_by { |x| x.to_f }).inspect # 4611686018427387903 (both keys round to 2**62; min_by returns first)",
        "by_cmp.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines, vec![
        "1",
        "3",
        "[1, 2.5, 3]",
        "4611686018427387905",  // first element of [2**62+1, 2**62-1]
    ]);
}

#[test]
fn integer_bit_predicates_arity_typeerror() {
    // Pin the `allbits?` / `anybits?` / `nobits?` arity guards
    // and the non-Integer-arg TypeError, sibling to the bit-op
    // guards landed in PR #211. Covers Int recv on both profiles;
    // BigInt recv pinned where the bignum feature is on.
    let mut rt = rubyrs::Runtime::new();
    // Happy paths first — ensure all 3 selectors dispatch.
    for (script, expected) in [
        ("p 42.allbits?(42)", "true"),
        ("p 42.anybits?(42)", "true"),
        ("p 42.nobits?(42)", "false"),
        ("p 0b0100_0101.nobits?(0b1010_1010)", "true"),
        ("p (-42).allbits?(-42)", "true"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "predicates.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // Arity: 0 args + 2+ args → ArgumentError. Cover both Int
    // recv (numeric.rs's guard) and BigInt recv (bignum.rs's
    // sibling guard added per PR #241 cycle 3 review — previously
    // BigInt fell through to NoMethodError).
    for (script, given) in [
        ("5.send(:allbits?)", 0),
        ("5.send(:anybits?, 1, 2)", 2),
        ("5.send(:nobits?, 1, 2, 3)", 3),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:allbits?)", 0),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:anybits?, 1, 2)", 2),
        #[cfg(feature = "bignum")]
        ("(2**64).send(:nobits?, 1, 2, 3)", 3),
    ] {
        let err = rt.eval(script, "predicates_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
                assert_eq!(
                    message,
                    format!("wrong number of arguments (given {}, expected 1)", given),
                    "for {:?}", script,
                );
            }
            other => panic!("expected ArgumentError for {:?}, got {:?}", script, other),
        }
    }
    // Non-Integer arg → TypeError.
    for script in ["13.allbits?(\"10\")", "13.anybits?(:sym)", "13.nobits?(3.5)"] {
        let err = rt.eval(script, "predicates_type.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "TypeError", "for {:?}", script);
                assert!(
                    message.starts_with("no implicit conversion of "),
                    "for {:?}: {:?}", script, message,
                );
            }
            other => panic!("expected TypeError for {:?}, got {:?}", script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_cmp_float_is_lossless() {
    // Sibling to bigint_eq_float_is_lossless. Pre-fix the
    // BigInt × Float Lt/Le/Gt/Ge arm demoted both sides to f64,
    // and the `<=>` arm returned nil (because the existing
    // arm required both sides to be BigInt-castable). Both
    // collapse values within the same f64 ULP onto the
    // same bit pattern (ULP at 2^64 is 2^(64-52)=4096), so e.g.
    // `(2**64 + 1) > (2**64).to_f` returned false. Pin the
    // lossless path via bigint_cmp_float_lossless.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "nan = 0.0 / 0.0\n\
         inf = 1.0 / 0.0\n\
         # <=> (BigInt × Float, both directions)\n\
         puts ((2**64 + 1) <=> (2**64).to_f).inspect    # 1\n\
         puts ((2**64) <=> (2**64).to_f).inspect        # 0\n\
         puts ((2**64 - 1) <=> (2**64).to_f).inspect    # -1\n\
         puts ((2**64).to_f <=> (2**64 + 1)).inspect    # -1\n\
         puts ((2**64) <=> nan).inspect                 # nil\n\
         puts ((2**64) <=> inf).inspect                 # -1\n\
         puts ((2**64) <=> -inf).inspect                # 1\n\
         # Ordering operators (Lt/Le/Gt/Ge)\n\
         puts ((2**64 + 1) > (2**64).to_f)              # true\n\
         puts ((2**64 + 1) < (2**64).to_f)              # false\n\
         puts ((2**64 + 1) <= (2**64).to_f)             # false\n\
         puts ((2**64 + 1) >= (2**64).to_f)             # true\n\
         puts ((2**64).to_f < (2**64 + 1))              # true (Float × BigInt)\n\
         # NaN: all four ordering ops are false (CRuby parity)\n\
         puts ((2**64) < nan)                           # false\n\
         puts ((2**64) > nan)                           # false\n\
         puts ((2**64) <= nan)                          # false\n\
         puts ((2**64) >= nan)                          # false",
        "bigint_cmp_float.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines, vec![
        "1", "0", "-1", "-1",               // <=> precision + symmetric
        "nil", "-1", "1",                   // <=> NaN/±inf
        "true", "false", "false", "true",   // Lt/Le/Gt/Ge precision
        "true",                             // Float × BigInt direction
        "false", "false", "false", "false", // NaN ordering
    ]);
}

#[cfg(feature = "bignum")]
#[test]
fn int_shift_zero_receiver_never_traps_regardless_of_count() {
    // Regression for PR #159 cycle 2: `0 << anything == 0` and
    // `0 >> anything == 0` in Ruby — should never allocate, never
    // trap on the DoS cap, never trap on the BigInt-count "shift
    // exceeds u32::MAX" guard. Pre-fix `0 << 1_000_000` under a
    // 1024-byte cap would trap because the cap estimator computed
    // `est_bits = 0 + 1_000_000` → 125 KB which exceeds 1 KB.
    let cfg = rubyrs::Config { max_value_bytes: Some(1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Tight cap, huge shift counts: all should return 0
        // without touching the DoS estimator or the BigInt-count
        // trap.
        "puts (0 << 1_000_000)\n\
         puts (0 << (2 ** 100))\n\
         puts (0 >> 1_000_000)\n\
         puts (0 >> -(2 ** 100))",
        "zero_shift.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "0");
    assert_eq!(lines[2], "0");
    assert_eq!(lines[3], "0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_left_traps_dos_via_max_value_bytes() {
    // Left-shift DoS cap: `1 << 1_000_000` would allocate
    // ~125 KB. With a 64 KB `max_value_bytes`, the pre-cap
    // estimator must trap before BigInt::shl touches the
    // allocator. Honours `max_value_bytes` with the same 1 MB
    // fallback as `try_bigint_pow`.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "1 << 1_000_000",
        "shift_dos.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_by_bigint_count_left_traps_right_collapses() {
    // BigInt shift count: by canonical invariant any BigInt is
    // outside i64, so:
    // - actual-left-shift by BigInt count → trap (would need
    //   > 2^63 bits of storage).
    // - actual-right-shift by BigInt count → collapse to 0 / -1
    //   without touching num_bigint (avoids the impossible alloc).
    let mut rt = rubyrs::Runtime::new();
    // Right-shift by BigInt count: collapses.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts ((2 ** 100) >> (2 ** 100))\n\
         puts ((-(2 ** 100)) >> (2 ** 100))",
        "shift_by_bigint_right.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "-1");
    // Left-shift by BigInt count: traps regardless of cap.
    let err = rt.eval(
        "1 << (2 ** 100)",
        "shift_by_bigint_left.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_radix_negative_uses_minus_magnitude_form() {
    // Two distinct CRuby behaviours for negative integers in
    // non-decimal bases:
    //   - `Integer#to_s(radix)` returns `-<magnitude>`:
    //     `(-256).to_s(16) == "-100"`. We match this exactly.
    //   - `sprintf '%x' % -256` returns `"..f00"` (CRuby's
    //     two's-complement infinite-ones notation). We diverge
    //     here and render `-<magnitude>` instead — documented
    //     in the sibling
    //     `sprintf_bigint_radix_negative_uses_minus_magnitude_divergence`
    //     test and in `format_radix_int`'s source comment.
    //
    // This test pins the `to_s` half — Int and BigInt receivers
    // both produce `-<magnitude>` for negative inputs, matching
    // CRuby byte-for-byte.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (-256).to_s(16)\n\
         puts (0 - (2 ** 100)).to_s(16)\n\
         puts (0 - (2 ** 64)).to_s(2).start_with?(\"-1\")",
        "bigint_to_s_neg.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "-100");
    // 2^100 in hex = 0x10000000000000000000000000 (1 followed by 25 zeros)
    assert!(lines[1].starts_with("-1") && lines[1].len() == 27,
        "expected -10000... (27 chars), got {:?}", lines[1]);
    assert_eq!(lines[2], "true");
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_bigint_radix_negative_uses_minus_magnitude_divergence() {
    // Documented divergence shared with the Int sprintf path:
    // CRuby renders `'%x' % -256` as `..f00` (two's-complement
    // infinite-ones notation), we render `-100`. Same shape for
    // negative BigInt. Pin our behaviour so a future "fix" that
    // adds CRuby compat is an opt-in upgrade rather than a silent
    // regression.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%x' % (0 - 256)\n\
         puts '%x' % (0 - (2 ** 100))\n\
         puts '%b' % (0 - (2 ** 16))",
        "sprintf_bigint_neg.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "-100");
    assert!(lines[1].starts_with("-1") && lines[1].len() == 27);
    assert!(lines[2].starts_with("-1"));
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_radix_traps_under_max_value_bytes() {
    // Like the 0-arg to_s arm, the radix form's string output must
    // be capped against `max_value_bytes` to prevent a hostile
    // script from DoSing the host via `(2 ** 1_000_000).to_s(2)`.
    // `(2 ** 10_000).to_s(2)` is exactly 10_001 chars; pin under
    // a 4 KB cap so the trap fires.
    let cfg = rubyrs::Config { max_value_bytes: Some(4 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "(2 ** 10_000).to_s(2)",
        "to_s_radix_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[test]
fn integer_to_s_non_integer_radix_raises_typeerror_on_int_path() {
    // Regression for cycle 13: the BigInt arm of `Integer#to_s(radix)`
    // raised `TypeError` for non-Integer radix, but the Int arm only
    // matched `Value::Int(radix)` and fell through to `NoMethodError`,
    // diverging from CRuby and from the BigInt path. Pin parity on
    // both sides — the unified `Integer#to_s` API should raise the
    // same `TypeError` regardless of receiver size.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.to_s(\"x\")", "int_to_s_typeerr.rb").unwrap_err();
    match err.err {
        rubyrs::RubyError::Uncaught { class_name, message } => {
            assert_eq!(class_name, "TypeError");
            assert_eq!(message, "no implicit conversion of String into Integer");
        }
        other => panic!("expected Uncaught TypeError, got {:?}", other),
    }
    // `Float` should error the same way (matches BigInt-path coercion).
    let err = rt.eval("5.to_s(1.0)", "int_to_s_typeerr_float.rb").unwrap_err();
    assert!(matches!(
        err.err,
        rubyrs::RubyError::Uncaught { ref class_name, .. } if class_name == "TypeError"
    ));
}

#[cfg(feature = "bignum")]
#[test]
fn integer_to_s_bigint_radix_raises_rangeerror_not_self_referential_typeerror() {
    // Pre-fix the catch-all `(Value::Int(_), "to_s", [other])` arm
    // intercepted `5.to_s(2**100)` (BigInt radix) and emitted
    // TypeError "no implicit conversion of Integer into Integer"
    // — `type_name_for_coerce` maps BigInt → "Integer" so the
    // wording was self-referential nonsense. CRuby raises
    // `RangeError: bignum too big to convert into 'long'` for this
    // shape (any BigInt is by canonical-BigInt invariant outside
    // i64, hence outside the 2..=36 radix range, but it IS an
    // Integer so TypeError is the wrong error class).
    let mut rt = rubyrs::Runtime::new();
    for script in ["5.to_s(2 ** 100)", "(2 ** 100).to_s(2 ** 100)"] {
        let err = rt.eval(script, "to_s_bigint_radix.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "RangeError", "for {:?}", script);
                assert_eq!(
                    message, "bignum too big to convert into `long'",
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught RangeError for {:?}, got {:?}", script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn integer_to_s_non_integer_radix_typeerror_message_matches_bigint_path() {
    // Cross-check the parity guard above against the BigInt path
    // so future drift between the two arms is caught immediately.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "(2 ** 100).to_s(\"x\")",
        "bigint_to_s_typeerr.rb",
    ).unwrap_err();
    match err.err {
        rubyrs::RubyError::Uncaught { class_name, message } => {
            assert_eq!(class_name, "TypeError");
            assert_eq!(message, "no implicit conversion of String into Integer");
        }
        other => panic!("expected Uncaught TypeError, got {:?}", other),
    }
}

#[test]
fn digits_negative_recv_takes_precedence_over_arity_and_base_errors() {
    // CRuby precedence: a negative `Integer#digits` receiver
    // raises Math::DomainError BEFORE any arity / base validation.
    // Pre-fix rubyrs checked arity / base type / base sign / base
    // < 2 first, so each shape surfaced a different error class.
    // Match CRuby's precedence so user code's `rescue ArgumentError`
    // catches the negative-recv path regardless of the other args'
    // shapes. Substitute is ArgumentError "out of domain" (same
    // convention as other numeric-out-of-domain arms in
    // Vm::do_call). Runs in both profiles.
    let mut rt = rubyrs::Runtime::new();
    for script in [
        "(-5).digits(10, 2)",     // would have been arity error
        "(-5).digits(-2)",        // would have been "negative radix"
        "(-5).digits(\"foo\")",   // would have been TypeError
        "(-5).digits(1)",         // would have been "invalid radix 1"
        "(-5).digits",            // pure negative-recv, no other badness
    ] {
        let err = rt.eval(script, "digits_precedence.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError (out-of-domain substitute) for {:?}, got {:?}",
            script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg, "out of domain",
            "wrong message for {:?} — expected the negative-recv check to fire first",
            script,
        );
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_negative_recv_raises_argument_error_substitute() {
    // CRuby raises `Math::DomainError: out of domain` for
    // `(-5).digits` (and the same shape for negative BigInt).
    // The established subset pattern (same convention as other
    // numeric-out-of-domain arms in Vm::do_call) substitutes
    // `ArgumentError` because `Math::DomainError` isn't modelled.
    // Pin the divergence so a future Math::DomainError addition
    // is an opt-in upgrade rather than a silent regression.
    let mut rt = rubyrs::Runtime::new();
    for script in [
        "(-5).digits",
        "(0 - (2 ** 100)).digits",
        "(-1).digits(16)",
    ] {
        let err = rt.eval(script, "digits_neg.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(msg, "out of domain", "wrong message for {:?}", script);
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_bigint_radix_survives_stress_gc() {
    // GC rooting regression guard. For a BigInt radix (e.g. base
    // = 2 ** 70), each digit produced is itself a heap-backed
    // `Value::BigInt(id)`. Every `bigint_to_value` call inside
    // the loop invokes `maybe_gc()`; without PinGuard rooting,
    // a sweep mid-loop could deallocate already-pushed digits,
    // leaving dangling ObjIds in the returned Array. Run under
    // forced GC (`stress_gc: true`) so every alloc triggers a
    // full mark — pre-fix this test panicked / produced wrong
    // values; with PinGuard around the loop it stays sound.
    let cfg = rubyrs::Config { stress_gc: true, ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let v = rt.eval(
        // (2 ** 200).digits(2 ** 70) — 3 digits, each potentially
        // BigInt-backed (top digit fits below 2^60 → demotes; the
        // other two could be BigInts). Verify all elements are
        // valid Integer values (no dangling refs / no panic).
        "(2 ** 200).digits(2 ** 70).map { |d| d.bit_length }",
        "digits_stress_gc.rb",
    ).expect("BigInt-radix digits must survive STRESS_GC");
    let elems = rt.resolve_array(&v).expect("expected Value::Array");
    // Each element is bit_length of a digit; values bounded by
    // log2(2^70) = 70. Just confirm we have a populated array of
    // small Ints — exact values are an implementation detail.
    assert!(!elems.is_empty(), "expected non-empty digits array");
    for e in &elems {
        match e {
            rubyrs::Value::Int(n) => assert!(*n >= 0 && *n <= 70, "bit_length out of range: {}", n),
            other => panic!("expected Value::Int (bit_length), got {:?}", other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_estimator_uses_log2_base_not_just_bits() {
    // Tighter estimator (`(recv_bits - 1) / (base.bits() - 1) + 1`)
    // means a `recv` whose base-2 expansion would exceed the cap
    // can still succeed in base-10 / base-16 — the actual digit
    // count for those bases is far smaller. Pin this so a future
    // refactor that drops the log-2 division and reverts to a
    // base-independent bound fails immediately.
    //
    // `(2 ** 1000).digits` is 302 decimal digits; at 16 B per
    // Value that's ~4.8 KB, well under an 8 KB cap. The base-2
    // form of the same recv would estimate 1001 elements
    // (~16 KB) and would correctly TRAP the 8 KB cap — exactly
    // the shape the sibling `digits_huge_bigint_in_base_2_traps_under_tight_cap`
    // test pins. So this test exercises the estimator's
    // base-awareness rather than its trap path.
    let cfg = rubyrs::Config { max_value_bytes: Some(8 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let v = rt.eval(
        "(2 ** 1000).digits.length",
        "digits_base10_fits.rb",
    ).expect("base-10 estimate must fit 8 KB cap for 2**1000");
    match v {
        rubyrs::Value::Int(n) => {
            // 2**1000 has 302 decimal digits.
            assert_eq!(n, 302, "expected 302 decimal digits, got {}", n);
        }
        other => panic!("expected Value::Int, got {:?}", other),
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_huge_bigint_in_base_2_traps_under_tight_cap() {
    // `(2 ** 100_000).digits(2)` would produce a 100_001-element
    // array (~1.6 MB at 16 B per Value). Under a tight cap, the
    // helper traps ResourceExhausted before allocating. Pin the
    // pre-allocation bound so a future refactor that drops the
    // estimator-trip fails immediately.
    let cfg = rubyrs::Config { max_value_bytes: Some(16 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "(2 ** 100_000).digits(2)",
        "digits_huge.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn digits_returns_value_array_with_int_elements() {
    // Embedding-facing contract: result is `Value::Array` of
    // `Value::Int` digits (each digit fits i64 since base fits
    // i64). Lock the public-API shape rather than just the
    // printed form.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval("12345.digits", "digits_shape.rb").expect("eval");
    let elems = rt.resolve_array(&v).expect("expected Value::Array");
    let nums: Vec<i64> = elems.iter().map(|e| match e {
        rubyrs::Value::Int(n) => *n,
        other => panic!("expected Value::Int, got {:?}", other),
    }).collect();
    assert_eq!(nums, vec![5, 4, 3, 2, 1]);
}

#[cfg(feature = "bignum")]
#[test]
fn bit_length_bigint_two_complement_semantics() {
    // Embedding-facing contract: `bit_length` on BigInt returns
    // `Value::Int`. Verify both signs across boundary cases.
    let mut rt = rubyrs::Runtime::new();
    let cases: &[(&str, i64)] = &[
        ("(2 ** 100).bit_length", 101),
        ("(2 ** 200).bit_length", 201),
        ("(0 - (2 ** 100)).bit_length", 100),  // bit_length(-2^100) = 100
        ("(0 - (2 ** 100) - 1).bit_length", 101),  // bit_length(-2^100 - 1) = bit_length(2^100) = 101
    ];
    for (script, expected) in cases {
        let v = rt.eval(script, "bit_length.rb").expect(script);
        match v {
            rubyrs::Value::Int(n) => assert_eq!(n, *expected, "{} → {}", script, n),
            other => panic!("expected Value::Int, got {:?}", other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn pow_arity_guard_fires_for_bigint_receiver() {
    // numeric.rs's arity guard only catches Int receivers — BigInt
    // receivers go through bigint_primitive's separate dispatch
    // path. Mirror the guard there so `big.pow` / `big.pow(1,2,3)`
    // raise CRuby's exact ArgumentError instead of NoMethodError.
    let mut rt = rubyrs::Runtime::new();
    for (script, n) in [
        ("big = 2 ** 100; big.pow", 0),
        ("big = 2 ** 100; big.pow(1, 2, 3)", 3),
    ] {
        let err = rt.eval(script, "bigint_pow_arity.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("wrong number of arguments (given {}, expected 1..2)", n),
            "wrong message for {:?}", script,
        );
    }
}

#[test]
fn pow_one_arg_non_numeric_raises_type_error() {
    // CRuby: `5.pow("x")` raises `TypeError: String can't be
    // coerced into Integer`. Pre-fix the 1-arg pow alias
    // recursed unconditionally to `**`, which (separately) only
    // surfaces NoMethodError for non-numeric args — so pow's
    // delegate inherited that wrong error class. Validate the
    // arg type at the pow boundary and raise TypeError directly.
    let mut rt = rubyrs::Runtime::new();
    for (script, class_name) in [
        ("5.pow(\"x\")", "String"),
        ("5.pow(nil)", "nil"),
        ("5.pow(true)", "true"),
        ("5.pow([1])", "Array"),
        ("5.pow({a: 1})", "Hash"),
    ] {
        let err = rt.eval(script, "pow_typeerr.rb").unwrap_err();
        assert!(
            err.err.is("TypeError"),
            "expected TypeError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::TypeError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("{} can't be coerced into Integer", class_name),
            "wrong message for {:?}", script,
        );
    }
}

#[cfg(feature = "bignum")]
#[test]
fn pow_one_arg_non_numeric_raises_type_error_for_bigint_receiver() {
    // Same fix on the BigInt receiver path — `(2 ** 100).pow("x")`
    // routes through `try_bigint_pow_method`'s 1-arg branch, which
    // mirrors the Int-side guard.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "(2 ** 100).pow(\"x\")",
        "bigint_pow_typeerr.rb",
    ).unwrap_err();
    assert!(err.err.is("TypeError"), "got {:?}", err.err);
    let msg = match &err.err {
        rubyrs::RubyError::TypeError { msg } => msg.clone(),
        rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
        _ => unreachable!(),
    };
    assert_eq!(msg, "String can't be coerced into Integer");
}

#[test]
fn pow_arity_zero_or_too_many_args_raise_argument_error() {
    // CRuby: `5.pow` and `5.pow(1, 2, 3)` raise ArgumentError
    // ("wrong number of arguments (given N, expected 1..2)").
    // Without the explicit arity guard those shapes fall through
    // to NoMethodError despite `respond_to?(:pow)` being true.
    let mut rt = rubyrs::Runtime::new();
    for (script, n) in [("5.pow", 0), ("5.pow(1, 2, 3)", 3), ("5.pow(1, 2, 3, 4, 5)", 5)] {
        let err = rt.eval(script, "pow_arity.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("wrong number of arguments (given {}, expected 1..2)", n),
            "wrong message for {:?}", script,
        );
    }
}

#[test]
fn pow_one_arg_accepts_float_exponent() {
    // `5.pow(1.5)` must mirror `5 ** 1.5` — both routes through
    // the same `**` arm. Previously the `pow` alias only fired
    // for `[Int]` exponents, so Float exp NoMethodErrored despite
    // being supported by `**`. Pin across both profiles.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 5.pow(1.5)\nputs 9.pow(0.5)",
        "pow_float_exp.rb",
    ).expect("Int#pow(Float) must work");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    let a: f64 = lines[0].parse().expect("Float output");
    let b: f64 = lines[1].parse().expect("Float output");
    // 5^1.5 ≈ 11.180339887; 9^0.5 = 3.0.
    assert!((a - 11.180_339_887).abs() < 1e-6);
    assert!((b - 3.0).abs() < 1e-12);
}

#[cfg(not(feature = "bignum"))]
#[test]
fn pow_no_bignum_two_arg_distinguishes_exp_vs_mod_type_errors() {
    // CRuby uses two distinct TypeError messages depending on
    // which arg is non-Integer: "...1st argument is integer" when
    // the exp is non-Int, "...all arguments are integers" when the
    // mod is non-Int. The no-bignum 2-arg path must match exactly
    // (the bignum path already does).
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(1.5, 7)", "exp_float.rb").unwrap_err();
    let msg = match &err.err {
        rubyrs::RubyError::TypeError { msg } => msg.clone(),
        rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
        other => panic!("expected TypeError, got {:?}", other),
    };
    assert!(
        msg.contains("a 1st argument is integer"),
        "wrong message for non-Int exp: {}",
        msg,
    );
    let err = rt.eval("5.pow(3, 1.5)", "mod_float.rb").unwrap_err();
    let msg = match &err.err {
        rubyrs::RubyError::TypeError { msg } => msg.clone(),
        rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
        other => panic!("expected TypeError, got {:?}", other),
    };
    assert!(
        msg.contains("all arguments are integers"),
        "wrong message for non-Int mod: {}",
        msg,
    );
}

#[cfg(not(feature = "bignum"))]
#[test]
fn pow_no_bignum_error_shapes_match_cruby() {
    let mut rt = rubyrs::Runtime::new();
    assert!(
        rt.eval("5.pow(-1, 7)", "no_bignum_neg_exp.rb").unwrap_err().err.is("RangeError"),
    );
    assert!(
        rt.eval("5.pow(3, 0)", "no_bignum_zero_mod.rb").unwrap_err().err.is("ZeroDivisionError"),
    );
    assert!(
        rt.eval("5.pow(1.5, 7)", "no_bignum_float_exp.rb").unwrap_err().err.is("TypeError"),
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_huge_exponent_skips_dos_cap() {
    // `2.pow(huge_exp, mod)` must succeed even when `2 ** huge_exp`
    // would blow far past any reasonable max_value_bytes — modpow
    // never materialises the intermediate, so the cap on the
    // pre-modulo `**` path doesn't apply. Pin under a tight 1 KB
    // cap that `2 ** 100_000` would trip (12.5 KB real magnitude).
    let cfg = rubyrs::Config { max_value_bytes: Some(1024), ..Default::default() };
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 2.pow(100_000, 1_000_000_007)",
        "pow_mod_huge.rb",
    ).expect("modpow must not trip the unmodulated `**` DoS cap");
    let v: i64 = buf.snapshot().trim().parse().expect("result must be Int");
    assert!((0..1_000_000_007).contains(&v),
        "result {} not in [0, mod)", v);
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_negative_exponent_raises_range_error() {
    // CRuby: `5.pow(-1, 7)` raises RangeError. Modular inverse may
    // not exist and we don't compute it — match by raising rather
    // than silently producing an unrelated value.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(-1, 7)", "pow_neg_exp_with_mod.rb").unwrap_err();
    assert!(
        err.err.is("RangeError"),
        "expected RangeError, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_zero_modulus_raises_zero_division() {
    // CRuby: `5.pow(3, 0)` raises ZeroDivisionError ("divided by 0").
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(3, 0)", "pow_zero_mod.rb").unwrap_err();
    assert!(
        err.err.is("ZeroDivisionError"),
        "expected ZeroDivisionError, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_non_integer_args_raise_type_error() {
    // CRuby: `5.pow(1.5, 7)` raises TypeError. Same for
    // `5.pow(3, 1.5)`. Pin the type-shape contract.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(1.5, 7)", "pow_float_exp.rb").unwrap_err();
    assert!(err.err.is("TypeError"), "expected TypeError, got {:?}", err.err);
    let err = rt.eval("5.pow(3, 1.5)", "pow_float_mod.rb").unwrap_err();
    assert!(err.err.is("TypeError"), "expected TypeError, got {:?}", err.err);
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_result_demotes_when_fits_int() {
    // The result is always strictly bounded by |mod|. When |mod|
    // fits i64, the result fits too — `bigint_to_value` should
    // demote so the embedding-facing `Value` is `Value::Int`, not
    // `Value::BigInt`. Pins demote-on-fit through the modpow path.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval(
        "(2 ** 100).pow(50, 1_000_000_007)",
        "pow_mod_demote.rb",
    ).expect("eval must succeed");
    assert!(
        matches!(v, Value::Int(_)),
        "expected Value::Int (mod fits i64), got {:?}", v,
    );
}

#[test]
fn pow_zero_to_negative_exponent_raises_zero_division() {
    // CRuby: `0 ** -1` raises `ZeroDivisionError: divided by 0`
    // because the reciprocal of 0 is undefined. Previous rubyrs
    // routed through `(0_u64 as f64).powf(-1.0) = +Infinity` and
    // silently returned `Float::INFINITY`, poisoning downstream
    // arithmetic. Match CRuby and raise instead.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("0 ** -1", "pow_zero_neg.rb").unwrap_err();
    assert!(
        err.err.is("ZeroDivisionError"),
        "expected ZeroDivisionError (direct or Uncaught-wrapped), got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_zero_to_negative_bigint_exponent_raises_zero_division() {
    // Same divergence fix on the BigInt-flavoured path: when the
    // exponent is a (negative) BigInt and recv is Int(0), dispatch
    // goes through try_bigint_pow's |base|≤1 short-circuit. That
    // arm previously returned `Float::INFINITY` for BigInt-flavoured
    // operands. Now it raises ZeroDivisionError uniformly with the
    // Int×Int path.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "neg_big = 0 - (2 ** 100); 0 ** neg_big",
        "pow_zero_neg_bigint.rb",
    ).unwrap_err();
    assert!(
        err.err.is("ZeroDivisionError"),
        "expected ZeroDivisionError (direct or Uncaught-wrapped), got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_zero_and_one_exponent_skip_estimator() {
    // `big ** 0` must always return 1 and `big ** 1` must return
    // the receiver, regardless of cap. With the previous flow the
    // estimator added a 32-byte BigInt-header overhead to
    // est_bytes, so a sub-32-byte cap would trap `big ** 0` even
    // though no allocation is actually needed. Pin both shapes
    // under a minimal 16-byte cap.
    let cfg = rubyrs::Config { max_value_bytes: Some(16), ..Default::default() };
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    // Build a `big` BigInt under a larger cap-free runtime first
    // would change scope; instead use a small Int receiver where
    // the demoted result still hits the identity short-circuits.
    rt.eval(
        "puts 7 ** 0\nputs 7 ** 1\nputs (-3) ** 0\nputs (-3) ** 1",
        "pow_exp_identities.rb",
    ).expect("** 0 and ** 1 must short-circuit before the cap check");
    assert_eq!(buf.snapshot().trim(), "1\n7\n1\n-3");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_pow2_estimator_avoids_2x_overshoot() {
    // The DoS estimator must use `(base_bits - 1) * exp + 1` for
    // power-of-two bases, not `base_bits * exp` — otherwise a
    // factor-of-2 overestimate falsely rejects allocations that
    // fit. `2 ** 100_000` produces ~12.5 KB of magnitude; the
    // tight bound estimates ~12.5 KB and fits under a 16 KB cap.
    // The old `base_bits * exp` would have estimated ~25 KB and
    // trapped, even though the real value fits comfortably.
    let cfg = rubyrs::Config { max_value_bytes: Some(16 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.eval("2 ** 100_000", "pow2_tight_estimate.rb")
        .expect("tight pow-of-2 estimate must allow values that fit the cap");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_huge_bigint_float_coercion_skips_string_alloc() {
    // BigInt → f64 must NOT materialise a decimal string for
    // BigInts past f64 range — Copilot flagged that a script
    // could trigger an unbounded allocation via `huge ** 0.5`.
    // The bits()-based pre-check (> 1024 ⇒ ±∞ directly) caps
    // any intermediate string at ~310 digits. Build a BigInt
    // far past 2**1024, then exercise the Float and negative-Int
    // exp paths. Both must produce ±∞ Floats without trapping.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    // 2 ** 5000 ≈ 625 bytes of magnitude, fits the 64 KB cap; its
    // bits() == 5001 puts it well past the 1024 f64 threshold.
    rt.eval(
        "big = 2 ** 5000\n\
         puts (big ** 0.5).infinite?\n\
         puts (big ** -1).zero?",
        "bigint_huge_to_f64.rb",
    ).expect("must not trap or NoMethodError");
    // 0.5 of +∞ is still +∞; -1 reciprocal of +∞ is 0.0.
    assert_eq!(buf.snapshot().trim(), "1\ntrue");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_identity_bases_with_bigint_exponent() {
    // |base| ≤ 1 must not trap on BigInt exponents — results are
    // constant-size. Pin `1 ** big`, `0 ** big`, `(-1) ** big`
    // (even and odd via parity-preserving bit(0)).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "big_even = 2 ** 100\n\
         big_odd  = big_even + 1\n\
         puts 1 ** big_even\n\
         puts 0 ** big_even\n\
         puts (-1) ** big_even\n\
         puts (-1) ** big_odd",
        "pow_bigint_exp_identity.rb",
    ).expect("identity bases must accept BigInt exponents");
    assert_eq!(buf.snapshot().trim(), "1\n0\n1\n-1");
}

#[cfg(feature = "bignum")]
#[test]
fn pow_neg_exponent_negative_base_preserves_parity_via_abs_powf() {
    // Negative-base + large-magnitude negative-exp must keep
    // the sign decided by i64 parity rather than relying on
    // f64-rounded `powf(neg, non-int-as-int)` which can NaN
    // (or flip sign) on some libm impls. `(-2) ** -3` is a
    // small enough case to assert exactly: -1/8 = -0.125.
    // Then `(-2) ** -(2**60 + 1)` (odd huge) — past 2**53
    // f64-mantissa — must stay non-positive (underflows to
    // -0.0 or a tiny negative Float).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let odd_huge = (1_i64 << 60) | 1;
    rt.eval(
        &format!("puts (-2) ** -3\nv = (-2) ** -{odd}\nputs v <= 0.0\nputs !v.nan?",
            odd = odd_huge),
        "pow_neg_base_parity.rb",
    ).expect("negative-base negative-exp must not NaN");
    assert_eq!(buf.snapshot().trim(), "-0.125\ntrue\ntrue");
}

#[cfg(feature = "bignum")]
#[test]
fn pow_neg_exponent_minus_one_preserves_parity_beyond_f64_mantissa() {
    // (-1) ** (-huge_odd) must remain -1.0; casting the i64
    // exponent through f64 loses parity past 2**53, so the
    // negative-exp arm has to short-circuit ±1 bases before powf.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let odd = (1_i64 << 60) | 1; // 2**60 + 1: way past f64 mantissa
    rt.eval(
        &format!("puts (-1) ** (-{odd})\nputs (-1) ** (-({odd} - 1))", odd = odd),
        "pow_neg_exp_parity.rb",
    ).expect("parity must survive f64 cast");
    assert_eq!(buf.snapshot().trim(), "-1.0\n1.0");
}

#[test]
fn integer_chr_basic() {
    // Pin the 0..255 byte-form Integer#chr surface; spec coverage
    // lives in spec/ruby/integer_chr_spec.rb. Cross-profile guard
    // for the (a) happy path round-trips, (b) RangeError shape for
    // out-of-range receivers, (c) TypeError for the unsupported
    // 1-arg `chr(encoding)` form, (d) BigInt-recv routing through
    // bignum_primitive's chr arm.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        ("65.chr", "A"),
        ("0.chr.bytes.inspect", "[0]"),
        ("127.chr.bytes.inspect", "[127]"),
        ("128.chr.bytes.inspect", "[128]"),
        ("255.chr.bytes.inspect", "[255]"),
        ("65.chr.length.inspect", "1"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(&format!("puts {}", script), "chr.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    for (script, expected_class) in [
        ("(-1).chr", "RangeError"),
        ("256.chr", "RangeError"),
        ("1000.chr", "RangeError"),
        ("65.chr(\"UTF-8\")", "TypeError"),
        ("65.chr(nil)", "TypeError"),
        // Regression guard: an Integer arg used to be silently
        // shadowed by the broad `(Int, op, [Int])` arm and
        // surface NoMethodError despite respond_to?(:chr) being
        // true. Pin TypeError so the shadow doesn't re-emerge.
        ("65.chr(0)", "TypeError"),
        ("65.chr(-1)", "TypeError"),
        // 2+-arg arity guard — without it 65.chr falls through
        // to NoMethodError instead of CRuby's ArgumentError.
        ("65.chr(\"UTF-8\", \"extra\")", "ArgumentError"),
        #[cfg(feature = "bignum")]
        ("(2**64).chr", "RangeError"),
        #[cfg(feature = "bignum")]
        ("(-(2**64)).chr", "RangeError"),
        // BigInt-recv arity/1-arg coherence with respond_to?
        // — previously `(2**64).chr("UTF-8")` fell through
        // bignum_primitive and raised NoMethodError.
        #[cfg(feature = "bignum")]
        ("(2**64).chr(\"UTF-8\")", "TypeError"),
        #[cfg(feature = "bignum")]
        ("(2**64).chr(nil)", "TypeError"),
        #[cfg(feature = "bignum")]
        ("(2**64).chr(1, 2)", "ArgumentError"),
    ] {
        let err = rt.eval(script, "chr_err.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
            }
            other => panic!("expected {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
    // CRuby uses the literal "bignum out of char range" message
    // for BigInt-recv `chr`. Pin the message text — interpolating
    // the BigInt's decimal expansion would diverge from CRuby AND
    // bypass `check_bigint_to_s_cap`, so this also serves as the
    // DoS-shape regression guard.
    #[cfg(feature = "bignum")]
    {
        let err = rt.eval("(2**64).chr", "chr_msg.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message, .. } => {
                assert_eq!(class_name, "RangeError");
                assert_eq!(message, "bignum out of char range");
            }
            other => panic!("expected RangeError, got {:?}", other),
        }
    }
    // respond_to? should report true on both Int and BigInt
    // receivers (whitelist guard in lookup.rs).
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts 65.respond_to?(:chr)", "rt_int.rb").expect("eval");
    assert_eq!(buf.snapshot().trim(), "true");
    #[cfg(feature = "bignum")]
    {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval("puts (2**64).respond_to?(:chr)", "rt_big.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), "true");
    }
}

#[test]
fn rational_phase_c1_construction_and_readers() {
    // Phase C.1 surface — `Kernel#Rational(n, d)` constructor +
    // .numerator / .denominator / .to_s / .inspect / .to_i / .to_f
    // / .to_r. Arithmetic + comparison lands in Phase C.2.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        // Construction + normalization invariants.
        ("puts Rational(3, 4).inspect",         "(3/4)"),
        ("puts Rational(6, 4).inspect",         "(3/2)"),
        ("puts Rational(3, -4).inspect",        "(-3/4)"),
        ("puts Rational(-3, -4).inspect",       "(3/4)"),
        ("puts Rational(5).inspect",            "(5/1)"),
        ("puts Rational(0, 7).inspect",         "(0/1)"),
        // class chain — Rational < Numeric < Object. Integer
        // and Float are also re-opened to chain through Numeric
        // so the whole numeric tower matches CRuby:
        // `5.is_a?(Numeric)` and `5.0.is_a?(Numeric)` both true,
        // `5.class.ancestors` includes Numeric.
        ("puts Rational(1, 2).class",           "Rational"),
        ("puts Rational(1, 2).is_a?(Numeric)",  "true"),
        ("puts Rational(1, 2).is_a?(Object)",   "true"),
        ("puts 5.is_a?(Numeric)",               "true"),
        ("puts 5.0.is_a?(Numeric)",             "true"),
        ("puts 5.class.ancestors.inspect",      "[Integer, Numeric, Object, Kernel, BasicObject]"),
        ("puts 5.0.class.ancestors.inspect",    "[Float, Numeric, Object, Kernel, BasicObject]"),
        ("puts Rational(1, 2).class.ancestors.inspect",
         "[Rational, Numeric, Object, Kernel, BasicObject]"),
        // to_s drops the parens; inspect keeps them.
        ("puts Rational(3, 4).to_s",            "3/4"),
        // Readers.
        ("puts Rational(3, 4).numerator",       "3"),
        ("puts Rational(3, 4).denominator",     "4"),
        ("puts Rational(-3, 4).numerator",      "-3"),
        ("puts Rational(-3, 4).denominator",    "4"),
        // Conversions. to_i truncates toward zero (NOT floor).
        ("puts Rational(7, 2).to_i",            "3"),
        ("puts Rational(-7, 2).to_i",           "-3"),
        ("puts Rational(3, 4).to_f",            "0.75"),
        ("puts Rational(-3, 4).to_f",           "-0.75"),
        ("puts Rational(3, 4).to_r.inspect",    "(3/4)"),
        // Structural equality (Phase C.1 — independent of Phase
        // C.2 arithmetic). gcd-normalize + sign-normalize at
        // construction make canonical form an invariant, so
        // `(num, den)` equality IS value equality. Without this
        // Rationals couldn't be used as Hash keys / Set members /
        // Array#include? args.
        ("puts (Rational(1, 2) == Rational(1, 2))",         "true"),
        ("puts (Rational(1, 2) == Rational(2, 4))",         "true"),
        ("puts (Rational(1, 2) == Rational(3, 7))",         "false"),
        ("puts (Rational(-3, 4) == Rational(3, -4))",       "true"),  // both normalize to (-3, 4)
        // eql? mirrors == for same-typed Rational (numeric
        // strictness doesn't apply since both sides are Rational).
        ("puts Rational(1, 2).eql?(Rational(1, 2))",        "true"),
        // hash invariant: a == b ⇒ a.hash == b.hash. Needed for
        // Hash key lookup.
        ("puts (Rational(1, 2).hash == Rational(2, 4).hash)", "true"),
        ("puts ({Rational(1, 2) => :half}[Rational(2, 4)])",   "half"),
        // Builtin Rational wins over user `def Rational` — without
        // adding "Rational" to `is_builtin_name`, the toplevel fast
        // path would cache the user def and silently shadow the
        // builtin Kernel function. CRuby's "builtin always wins"
        // dispatch order applies the same way for Integer/Float/etc.
        (
            "def Rational(n, d=1); 'user-shadow' end; puts Rational(1, 2).inspect",
            "(1/2)",
        ),
        // respond_to?
        ("puts Rational(1, 2).respond_to?(:numerator)",  "true"),
        ("puts Rational(1, 2).respond_to?(:denominator)","true"),
        ("puts Rational(1, 2).respond_to?(:to_r)",       "true"),
        // Arithmetic / comparison whitelist (Phase C.2 — the
        // operator method-call arms in dispatch.rs's Rational
        // block plus `try_rational_binop` at the Op::BinOp site).
        ("puts Rational(1, 2).respond_to?(:+)",          "true"),
        ("puts Rational(1, 2).respond_to?(:<=>)",        "true"),
        ("puts Rational(1, 2).respond_to?(:coerce)",     "true"),
        ("puts Rational(1, 2).respond_to?(:==)",         "true"),  // Object#==
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "rational_c1.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // Error shapes.
    for (script, expected_class, expected_msg) in [
        // Denominator zero → ZeroDivisionError.
        ("Rational(1, 0)", "ZeroDivisionError", "divided by 0"),
        // Non-Integer arg → TypeError.
        ("Rational(\"x\")",    "TypeError",    "can't convert String into Rational"),
        ("Rational(1, nil)",   "TypeError",    "can't convert NilClass into Rational"),
        ("Rational(1.5)",      "TypeError",    "can't convert Float into Rational"),
        // Arity.
        ("Rational()",         "ArgumentError","wrong number of arguments (given 0, expected 1..2)"),
        ("Rational(1, 2, 3)",  "ArgumentError","wrong number of arguments (given 3, expected 1..2)"),
    ] {
        let err = rt.eval(script, "rational_c1_err.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected Uncaught {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
    // Reader arity guard.
    for script in [
        "Rational(1, 2).numerator(99)",
        "Rational(1, 2).denominator(99)",
        "Rational(1, 2).to_i(99)",
    ] {
        let err = rt.eval(script, "rational_c1_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, .. } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
            }
            ref other => panic!("expected ArgumentError for {:?}, got {:?}", script, other),
        }
    }
}

#[test]
fn rational_phase_c2_arithmetic_and_comparison() {
    // Phase C.2 surface — `+ - * / <=>` + comparisons on Rational
    // operands, plus cross-type promotion (Int and Float) routed
    // through `try_rational_binop` at the Op::BinOp site and the
    // method-call arms in dispatch.rs's Rational block.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        // Rational × Rational.
        ("puts (Rational(1, 2) + Rational(1, 3)).inspect", "(5/6)"),
        ("puts (Rational(3, 4) - Rational(1, 2)).inspect", "(1/4)"),
        ("puts (Rational(2, 3) * Rational(3, 4)).inspect", "(1/2)"),
        ("puts (Rational(1, 2) / Rational(1, 3)).inspect", "(3/2)"),
        // Rational result that reduces back to an integer-valued
        // Rational stays a Rational (CRuby parity).
        ("puts (Rational(1, 2) + Rational(1, 2)).inspect", "(1/1)"),
        // Rational × Int (and reverse) via Op::BinOp's
        // try_rational_binop.
        ("puts (Rational(1, 2) + 1).inspect",  "(3/2)"),
        ("puts (1 + Rational(1, 2)).inspect",  "(3/2)"),
        ("puts (2 * Rational(1, 4)).inspect",  "(1/2)"),
        ("puts (Rational(1, 2) - 1).inspect",  "(-1/2)"),
        ("puts (Rational(1, 2) / 2).inspect",  "(1/4)"),
        // Rational × Float (Float dominates).
        ("puts (Rational(1, 2) + 0.5).inspect", "1.0"),
        ("puts (0.5 + Rational(1, 4)).inspect", "0.75"),
        // Float div by 0.0 follows IEEE-754 / CRuby — `(r/0.0)`
        // is `±Infinity`, NOT ZeroDivisionError. Matches the
        // existing `1.0 / 0.0 == Infinity` semantics so all
        // Float-dominant ops stay consistent.
        ("puts (Rational(1, 2) / 0.0).inspect",   "Infinity"),
        ("puts (Rational(-1, 2) / 0.0).inspect",  "-Infinity"),
        ("puts (0.0 / Rational(1, 2)).inspect",   "0.0"),
        // Comparison operators (Rational × Rational and cross-type).
        ("puts (Rational(1, 2) < Rational(2, 3))", "true"),
        ("puts (Rational(2, 3) > Rational(1, 2))", "true"),
        ("puts (Rational(1, 2) <= Rational(1, 2))", "true"),
        ("puts (Rational(1, 2) == Rational(2, 4))", "true"),
        ("puts (Rational(1, 2) != Rational(1, 3))", "true"),
        // <=> across Rational / Int / Float.
        ("puts (Rational(1, 2) <=> Rational(2, 3))", "-1"),
        ("puts (Rational(1, 2) <=> Rational(1, 2))", "0"),
        ("puts (Rational(2, 3) <=> Rational(1, 2))", "1"),
        ("puts (Rational(1, 2) <=> 1)",              "-1"),
        ("puts (1 <=> Rational(1, 2))",              "1"),
        ("puts (Rational(1, 2) <=> 0.5)",            "0"),
        ("puts (0.5 <=> Rational(2, 3))",            "-1"),
        // Cross-type equality (`Int == Rational` / `Float ==
        // Rational`) flows through the same path.
        ("puts (1 == Rational(1, 1))",   "true"),
        ("puts (Rational(1, 1) == 1)",   "true"),
        ("puts (0.5 == Rational(1, 2))", "true"),
        ("puts (Rational(1, 2) == 0.5)", "true"),
        // Non-numeric arg returns nil from <=>.
        ("puts (Rational(1, 2) <=> \"x\").inspect", "nil"),
        // send(:==, ...) consistency — the BinOp == fast path and
        // the method-call (Object#== via ruby_eq) path must agree
        // on cross-type Rational × Int / Float. Pre-fix
        // `1.send(:==, Rational(1, 1))` returned false because
        // ruby_eq had no cross-type arms.
        ("puts Rational(1, 1).send(:==, 1)",          "true"),
        ("puts 1.send(:==, Rational(1, 1))",          "true"),
        ("puts Rational(1, 2).send(:==, 0.5)",        "true"),
        ("puts 0.5.send(:==, Rational(1, 2))",        "true"),
        ("puts Rational(1, 2).send(:==, Rational(2, 4))", "true"),
        // Hash key lookup goes through ruby_eql which falls
        // through to ruby_eq for cross-types — pin that Rational
        // keys still resolve when looked up by their canonical
        // equivalent.
        ("puts ({Rational(1, 2) => :half}[Rational(2, 4)])", "half"),
        // CRuby numeric strictness for `eql?`: even though
        // `Rational(1, 1) == 1` is true (Phase C.2 cross-type
        // arithmetic), `eql?` requires identical Ruby class.
        // Mirrors `1.eql?(1.0) == false`. Hash#uniq / Array#uniq
        // depend on this strictness to distinguish mixed numeric
        // collections.
        ("puts Rational(1, 1).eql?(1)",                    "false"),
        ("puts Rational(1, 2).eql?(0.5)",                  "false"),
        ("puts 1.eql?(Rational(1, 1))",                    "false"),
        ("puts 0.5.eql?(Rational(1, 2))",                  "false"),
        ("puts Rational(1, 2).eql?(Rational(2, 4))",       "true"),
        #[cfg(feature = "bignum")]
        ("puts Rational(1, 1).eql?(2**64)",                "false"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "rational_c2.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // Error shapes.
    for (script, expected_class, expected_msg) in [
        ("Rational(1, 2) / Rational(0, 1)", "ZeroDivisionError", "divided by 0"),
        ("Rational(1, 2) / 0",              "ZeroDivisionError", "divided by 0"),
        ("Rational(1, 2) + \"x\"",          "TypeError",         "String can't be coerced into Rational"),
        ("Rational(1, 2) - nil",            "TypeError",         "nil can't be coerced into Rational"),
        // eql? arity guard (universal Object#eql? is gated out
        // for Rational receivers, so the Rational-specific arm
        // must surface the ArgumentError itself).
        ("Rational(1, 1).eql?",             "ArgumentError",     "wrong number of arguments (given 0, expected 1)"),
        ("Rational(1, 1).eql?(1, 2)",       "ArgumentError",     "wrong number of arguments (given 2, expected 1)"),
    ] {
        let err = rt.eval(script, "rational_c2_err.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected Uncaught {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
}

#[test]
fn rational_survives_stress_gc() {
    // Regression guard for the visit_value mark hole Copilot
    // flagged on PR #297: without an arm for `Value::Rational`,
    // any live Rational's backing HeapObj slot fails to mark
    // during sweep and gets reused for the next allocation,
    // corrupting subsequent reads via `heap.rational(*id)`.
    //
    // Stress GC trips a collect on every alloc, so the bug is
    // reliably observable here when present. Bind a Rational to
    // a local, allocate several other heap objects (each forces
    // a sweep), then read the Rational back via #inspect /
    // #numerator. Without the fix the backing slot's RationalRepr
    // bytes are overwritten by whatever the intervening alloc
    // stored there.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "r = Rational(355, 113); \
         100.times { _ = [1, 2, 3]; _ = {a: 1, b: 2}; _ = \"alloc\" }; \
         puts r.inspect; puts r.numerator; puts r.denominator",
        "rational_stress_gc.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot().trim(), "(355/113)\n355\n113");
}

#[test]
fn numeric_coerce_basic() {
    // `Numeric#coerce(other)` — Tier-2 protocol entry point;
    // returns `[other_promoted, self_promoted]`. Spec coverage
    // lives in spec/ruby/integer_coerce_spec.rb; this is the
    // cross-profile embed guard for happy paths + error shapes.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        // Integer × Integer — pass-through (both already Integer).
        ("puts 1.coerce(2).inspect",          "[2, 1]"),
        ("puts 10.coerce(20).inspect",        "[20, 10]"),
        // Integer × Float — promote both to Float.
        ("puts 1.coerce(2.5).inspect",        "[2.5, 1.0]"),
        ("puts 5.coerce(-3.14).inspect",      "[-3.14, 5.0]"),
        // Float × Integer — promote both to Float.
        ("puts 2.5.coerce(1).inspect",        "[1.0, 2.5]"),
        // Float × Float — pass-through.
        ("puts 3.7.coerce(1.5).inspect",      "[1.5, 3.7]"),
        // BigInt × Integer / BigInt × BigInt — both Integer subclass.
        #[cfg(feature = "bignum")]
        ("puts (2**64).coerce(1).inspect",          "[1, 18446744073709551616]"),
        #[cfg(feature = "bignum")]
        ("puts 1.coerce(2**64).inspect",            "[18446744073709551616, 1]"),
        #[cfg(feature = "bignum")]
        ("puts (2**64).coerce(2**70).inspect",      "[1180591620717411303424, 18446744073709551616]"),
        // BigInt × Float — promote both to Float.
        #[cfg(feature = "bignum")]
        ("puts (2**64).coerce(2.5).inspect",        "[2.5, 1.8446744073709552e19]"),
        // Over-magnitude BigInt × Float — `bigint_to_f64_sign_preserving`
        // saturates to ±Infinity with the original BigInt's sign.
        // Pinned here at the user boundary so a future num-bigint
        // upgrade can't quietly flip negative-Inf to +Inf.
        #[cfg(feature = "bignum")]
        ("puts (2**2000).coerce(1.0).inspect",      "[1.0, Infinity]"),
        #[cfg(feature = "bignum")]
        ("puts (-(2**2000)).coerce(1.0).inspect",   "[1.0, -Infinity]"),
        #[cfg(feature = "bignum")]
        ("puts 1.0.coerce(-(2**2000)).inspect",     "[-Infinity, 1.0]"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "coerce.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), expected, "for {:?}", script);
    }
    // Non-Numeric arg → TypeError with CRuby-shape "X can't be
    // coerced into <recv_class>".
    for (script, expected_msg) in [
        ("5.coerce(:sym)", "Symbol can't be coerced into Integer"),
        ("5.coerce(nil)",  "nil can't be coerced into Integer"),
        ("5.coerce(\"x\")","String can't be coerced into Integer"),
        ("1.5.coerce(nil)","nil can't be coerced into Float"),
        ("1.5.coerce(\"x\")","String can't be coerced into Float"),
        #[cfg(feature = "bignum")]
        ("(2**64).coerce(:sym)", "Symbol can't be coerced into Integer"),
        // Rational arg surfaces "Rational" (not the `Object`
        // fallback) once `type_name_for_coerce` knows the variant.
        // Pin both Float and Integer recv sides; Phase C.2 will
        // turn these into successful coercions, at which point the
        // expected behaviour flips from TypeError to a Rational
        // result and these asserts move into the happy-path block.
        ("1.5.coerce(Rational(1, 2))", "Rational can't be coerced into Float"),
        ("5.coerce(Rational(1, 2))",   "Rational can't be coerced into Integer"),
    ] {
        let err = rt.eval(script, "coerce_err.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, "TypeError", "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected Uncaught TypeError for {:?}, got {:?}", script, other),
        }
    }
    // Arity guard.
    for (script, expected_class) in [
        ("5.coerce()",       "ArgumentError"),
        ("5.coerce(1, 2)",   "ArgumentError"),
        ("1.5.coerce(1, 2)", "ArgumentError"),
    ] {
        let err = rt.eval(script, "coerce_arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, .. } => {
                assert_eq!(class_name, expected_class, "for {:?}", script);
            }
            ref other => panic!("expected {} for {:?}, got {:?}", expected_class, script, other),
        }
    }
    // respond_to? true for Int + Float (+ BigInt under bignum).
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts 5.respond_to?(:coerce); puts 1.5.respond_to?(:coerce)", "rt_coerce.rb").expect("eval");
    assert_eq!(buf.snapshot().trim(), "true\ntrue");
    #[cfg(feature = "bignum")]
    {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval("puts (2**64).respond_to?(:coerce)", "rt_big_coerce.rb").expect("eval");
        assert_eq!(buf.snapshot().trim(), "true");
    }
}

#[cfg(feature = "bignum")]
#[test]
fn numeric_coerce_pass_through_bigint_survives_stress_gc() {
    // Regression guard for the GC root hole Copilot flagged on
    // PR #289: when `coerce` returns existing BigInt values
    // unchanged (e.g. `1.coerce(2**64)` / `(2**64).coerce(1)`),
    // the BigInt ObjIds live only as Rust locals between the
    // stack drain and the result Array allocation. Without a
    // PinGuard, the maybe_gc fired inside that window swept
    // the BigInt before it was rooted in the Array.
    //
    // Stress GC trips a collect on every allocation, so the bug
    // is reliably observable under this config when present.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 1.coerce(2**64).inspect; \
         puts (2**64).coerce(1).inspect; \
         puts (2**64).coerce(2**70).inspect",
        "coerce_stress_gc.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "[18446744073709551616, 1]\n\
         [1, 18446744073709551616]\n\
         [1180591620717411303424, 18446744073709551616]",
    );
}

#[cfg(feature = "bignum")]
#[test]
fn integer_divmod_bigint_result_survives_stress_gc() {
    // Sibling to numeric_coerce_pass_through_bigint_survives_stress_gc.
    // Pre-existing GC root hole noted during PR #289's /code-review:
    // for BigInt divmod, `q` and `r` are freshly-allocated BigInt
    // ObjIds returned by `bigint_arith` — their only live root
    // between the (q, r) tuple binding and the `Array(vec![q, r])`
    // alloc is the Rust local. Without a PinGuard, the maybe_gc
    // fired in that window sweeps both BigInts before the result
    // Array is rooted, leaving the Array with dangling slots.
    //
    // Stress GC trips a collect on every allocation, so the bug
    // is reliably observable under this config when present.
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // (a) BigInt recv × Int divisor — both q and r are BigInt
        //     (q stays BigInt because 2**100 / 3 > i64::MAX; r
        //     is a small Int after the modulo and demotes via
        //     bigint_to_value, so only q is the GC-hazardous one).
        // (b) BigInt recv × BigInt divisor — both q and r demote
        //     to Int, no BigInt in result — safe but exercise the
        //     code path.
        // (c) BigInt recv × small Int with negative result — q is
        //     a fresh BigInt (negative, magnitude > i64::MAX).
        "puts (2**100).divmod(3).inspect; \
         puts (2**100).divmod(2**99).inspect; \
         puts (-(2**100)).divmod(3).inspect",
        "divmod_stress_gc.rb",
    ).expect("eval");
    let snap = buf.snapshot();
    let lines: Vec<&str> = snap.lines().collect();
    assert_eq!(lines.len(), 3);
    // q = floor(2**100 / 3), r = 2**100 mod 3.
    assert_eq!(
        lines[0],
        "[422550200076076467165567735125, 1]",
    );
    // 2**100 / 2**99 == 2, r == 0.
    assert_eq!(lines[1], "[2, 0]");
    // CRuby floor: -(2**100) / 3 = -422550200076076467165567735126, r = 2
    //   (floor towards -∞, so quotient is one less in magnitude than
    //   truncated-toward-zero, and remainder is positive).
    assert_eq!(
        lines[2],
        "[-422550200076076467165567735126, 2]",
    );
}

#[test]
fn float_domain_error_class_and_rescue_chain() {
    // FloatDomainError sits at FloatDomainError < RangeError <
    // StandardError < Exception. Verify (a) the class is exposed
    // to Ruby code (preamble loaded), (b) the ancestor chain is
    // correct, (c) `rescue FloatDomainError`, `rescue RangeError`,
    // and a bare `rescue` all catch a NaN-divmod trap, (d)
    // `Float::INFINITY.to_i` / `Float::NAN.to_i` raise it (and
    // not the silent `as i64` clamp), (e) the embed host sees
    // the `Uncaught { class_name: "FloatDomainError" }` shape.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts FloatDomainError.ancestors.inspect",
        "fde_chain.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "[FloatDomainError, RangeError, StandardError, Exception]",
    );

    for (script, rescue_class, expected) in [
        ("begin; 13.divmod(0.0/0.0); rescue FloatDomainError => e; puts e.class; end",
         "FloatDomainError", "FloatDomainError"),
        ("begin; 13.divmod(0.0/0.0); rescue RangeError => e; puts e.class; end",
         "RangeError", "FloatDomainError"),
        ("begin; 13.divmod(0.0/0.0); rescue => e; puts e.class; end",
         "bare", "FloatDomainError"),
    ] {
        let buf = SharedBuf::new();
        rt.set_stdout(Box::new(buf.clone()));
        rt.eval(script, "fde_rescue.rb").expect("eval");
        assert_eq!(
            buf.snapshot().trim(),
            expected,
            "rescue {} should catch FloatDomainError",
            rescue_class,
        );
    }

    // Float#to_i / floor / ceil / round / truncate on NaN / ±Inf.
    for (script, expected_msg) in [
        ("(0.0/0.0).to_i",     "NaN"),
        ("(1.0/0.0).to_i",     "Infinity"),
        ("(-1.0/0.0).to_i",    "-Infinity"),
        ("(0.0/0.0).floor",    "NaN"),
        ("(1.0/0.0).ceil",     "Infinity"),
        ("(-1.0/0.0).round",   "-Infinity"),
        ("(0.0/0.0).truncate", "NaN"),
        // Precision-arg form: previously bypassed the guard
        // because the trap arm only matched `[]`, so
        // `Float::NAN.round` raised but `Float::NAN.round(0)`
        // silently returned 0 (the f64-NaN-to-i64 cast). Pin both
        // the n == 0 and the negative-n branches.
        ("(0.0/0.0).round(0)",      "NaN"),
        ("(0.0/0.0).round(-2)",     "NaN"),
        ("(1.0/0.0).round(0)",      "Infinity"),
        ("(1.0/0.0).round(-2)",     "Infinity"),
        ("(-1.0/0.0).truncate(0)",  "-Infinity"),
        ("(-1.0/0.0).truncate(-1)", "-Infinity"),
        // BigInt-precision form — under bignum any BigInt sits
        // outside i64, so without a dedicated arm the call
        // surfaces NoMethodError. Pin both positive and negative
        // BigInt-precision: NaN/Inf trap unconditionally.
        #[cfg(feature = "bignum")]
        ("(0.0/0.0).round(2**70)",     "NaN"),
        #[cfg(feature = "bignum")]
        ("(1.0/0.0).truncate(2**70)",  "Infinity"),
        #[cfg(feature = "bignum")]
        ("(-1.0/0.0).round(-(2**70))", "-Infinity"),
    ] {
        let err = rt.eval(script, "fde_to_i.rb").unwrap_err();
        // At the eval boundary the dispatcher always re-shapes a
        // typed trap into `Uncaught { class_name, message }` via
        // `trap_to_exception` + `unwind_with_exception` (see
        // vm/step.rs:289 — only ResourceExhausted / Uncaught /
        // SyntaxError bypass that conversion). Pin the boundary
        // shape AND that the class_name is exactly
        // "FloatDomainError" so a regression that downgrades to
        // a generic RangeError-shaped Uncaught fails loudly.
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, "FloatDomainError", "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected Uncaught FloatDomainError for {:?}, got {:?}", script, other),
        }
    }

    // sprintf %d / %b / %o / %x with NaN/±Infinity — CRuby
    // raises FloatDomainError; previously rubyrs silently
    // `as i64`-clamped. Mirror the unified surface.
    for (script, expected_msg) in [
        ("sprintf(\"%d\", 0.0/0.0)",  "NaN"),
        ("sprintf(\"%d\", 1.0/0.0)",  "Infinity"),
        ("sprintf(\"%d\", -1.0/0.0)", "-Infinity"),
        ("sprintf(\"%b\", 0.0/0.0)",  "NaN"),
        ("sprintf(\"%x\", 1.0/0.0)",  "Infinity"),
    ] {
        let err = rt.eval(script, "fde_sprintf.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, "FloatDomainError", "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected Uncaught FloatDomainError for {:?}, got {:?}", script, other),
        }
    }

    // Kernel#Integer parity — `Integer(Float::NAN)` and
    // `Integer(Float::INFINITY)` previously raised TypeError,
    // divergent from CRuby AND inconsistent with `Float#to_i`
    // (which this PR routes through FloatDomainError). Pin
    // the unified shape so the two surfaces stay aligned.
    for (script, expected_msg) in [
        ("Integer(0.0/0.0)",   "NaN"),
        ("Integer(1.0/0.0)",   "Infinity"),
        ("Integer(-1.0/0.0)",  "-Infinity"),
    ] {
        let err = rt.eval(script, "fde_kernel_integer.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { ref class_name, ref message, .. } => {
                assert_eq!(class_name, "FloatDomainError", "for {:?}", script);
                assert_eq!(message, expected_msg, "for {:?}", script);
            }
            ref other => panic!("expected Uncaught FloatDomainError for {:?}, got {:?}", script, other),
        }
    }

    // Positive-precision branch returns a Float and propagates
    // NaN/Inf cleanly — matches CRuby (e.g. `Float::NAN.round(2)`
    // returns NaN, not a trap). Pin so a future "trap on any
    // NaN/Inf precision" over-correction is caught.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (0.0/0.0).round(2); puts (1.0/0.0).truncate(3)",
        "fde_pos_prec.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot().trim(), "NaN\nInfinity");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_bigint_exponent_traps() {
    // `2 ** (2**63)` (BigInt exponent) must trap ResourceExhausted
    // instead of falling through to NoMethodError. The doc comment
    // promises a clean error.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "big = 2 ** 100; 2 ** big",
        "pow_bigint_exp.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted for BigInt exponent, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_oversize_exponent_traps_for_real_bases() {
    // For bases with |a| > 1, an exponent that doesn't fit u32
    // must trap (the result would be astronomically large) —
    // verifies numeric_call declines on u32-overflow so
    // bigint_primitive can issue the trap.
    let mut rt = rubyrs::Runtime::new();
    let huge = (u32::MAX as i64) + 1;
    let err = rt.eval(
        &format!("2 ** {}", huge),
        "pow_oversize_exp.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted for u32-overflow exp, got {:?}",
        err.err,
    );
}

