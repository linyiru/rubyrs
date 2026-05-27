//! BigInt arithmetic, comparison, dispatch, and the full Phase B
//! `Integer` surface for out-of-i64 magnitudes. CRuby analogue:
//! `bignum.c`. Pulled out of `vm.rs` so the arbitrary-precision
//! integer surface lives alongside its kin in `vm/`, following the
//! per-class compilation-unit convention documented in
//! `docs/ARCHITECTURE.md:53-80`. ADR 0018 BigInt placement.
//!
//! Phase B status (all landed):
//!   - **B.1** — base arithmetic / comparison / `to_s` / `inspect`
//!     / predicates. Auto-promotion from i64 on overflow; demote
//!     back to `Value::Int` whenever the result fits.
//!   - **B.2** — unary `-@` / `+@` / `abs` (with the `i64::MIN`
//!     auto-promote case so `-(i64::MIN) == 2**63` instead of
//!     wrapping).
//!   - **B.3** — bit ops `~`, `& | ^`, `<< >>` with two's-
//!     complement semantics for negatives (via num_bigint's own
//!     impls). Left-shift DoS cap mirrors `try_bigint_pow`'s
//!     estimator. `try_int_shl_lossless` in `numeric.rs` handles
//!     the Int×Int overflow promote path.
//!   - **B.4** — `to_s(radix)` + sprintf `'%d/%i/%b/%B/%o/%x/%X' % big`.
//!     Shared scaled-integer log2 estimator
//!     ([`bignum_digits_upper_bound`]) caps the pre-allocation
//!     bound to within ±1 char of the true digit count across
//!     radix 2..=36. `format_radix_any` does in-place sign/prefix
//!     prepend to keep peak memory at 1× the estimate.
//!   - **B.5** — `Integer#pow(exp[, mod])`. 1-arg aliases `**`;
//!     2-arg routes through `BigInt::modpow` for modular
//!     exponentiation. Plus `Integer#bit_length` and
//!     `Integer#digits([base])`.
//!   - **B.6** — block-form iteration `times` / `upto` / `downto`
//!     with BigInt operands. Counter held as native
//!     `num_bigint::BigInt`; yielded `Value` pinned across
//!     `step_block` to survive `invoke_block`'s rest-args GC
//!     window. Implementation lives in `iter.rs` to share the
//!     `collection_call_block` dispatch entry.
//!   - **B.7** — hash-key canonical equality: `Integer#eql?` /
//!     `Integer#hash` (Int + BigInt), `Object#equal?` BigInt arm
//!     (ObjId identity), shared `INT_HASH_TAG` so all Integer
//!     receivers share a hash domain disjoint from `FLOAT_HASH_TAG`.
//!
//! Canonical-BigInt invariant (every revision must preserve):
//!   - Any `Value::BigInt(id)` reaching dispatch satisfies
//!     `i64::try_from(heap.bigint(id)).is_err()` — i.e. the
//!     magnitude is strictly outside `i64::MIN..=i64::MAX`.
//!   - The single funnel that enforces this is `bigint_to_value`,
//!     which demotes to `Value::Int` whenever the result fits.
//!     Every arithmetic / bit-op / iteration arm that produces a
//!     `BigInt` result MUST route through it.
//!   - Debug-asserts in `try_bigint_unary` (the `+@` / `abs`
//!     identity short-circuits) catch any future cext/FFI path
//!     that bypasses the funnel.
//!
//! DoS-cap convention (shared with the rest of the codebase):
//! every arm that can produce arbitrarily large output traps
//! **before** the alloc when the estimated byte cost exceeds
//! `Config::max_value_bytes` (fallback: 1 MB, same as
//! `try_bigint_pow`'s original). Two flavours depending on what
//! the arm is about to allocate:
//!   - **BigInt-allocation caps** — `try_bigint_pow` (result of
//!     `base ** exp`) and `try_bigint_bit_shift` (result of
//!     `recv << n` / `recv >> n`). Estimate rounds up to u64
//!     limbs + 32-byte allocator header so the cap reflects
//!     actual heap storage, not just minimal bit count.
//!   - **String-formatting caps** — `check_bigint_to_s_cap`
//!     (BigInt#to_s output), `format_radix_any` (sprintf
//!     `%b/%B/%o/%x/%X` output), and the `%d/%i % big` path in
//!     `vm::sprintf::ruby_sprintf`. Estimate is the rendered
//!     character count (digits + sign byte + optional `0x`/`0b`
//!     prefix) — bounds the output String length, not the
//!     underlying BigInt storage.
//!
//! Structure (top to bottom):
//!   - `try_bigint_binop` — `Op::BinOp` cold path (arithmetic +
//!     comparison). Always compiled (no-op stub when `bignum` is
//!     off so step.rs can call it unconditionally).
//!   - `try_bigint_pow` / `bigint_to_f64_bounded` /
//!     `bigint_recv_to_f64_bounded` — `**` exponentiation with
//!     DoS cap and the bounded f64 coercion helpers. `bignum` only.
//!   - `try_bigint_unary` — `-@` / `+@` / `abs` / `~` with the
//!     `i64::MIN` promote case (B.2 + B.3a).
//!   - `try_bigint_bit_binop` / `try_bigint_bit_shift` /
//!     `bit_shift_collapse` — `& | ^` and `<< >>` two's-complement
//!     surface (B.3b/c).
//!   - `try_bigint_pow_method` — `Integer#pow(exp[, mod])` (B.5a).
//!   - `try_integer_digits` — `Integer#digits([base])` (B.5b).
//!   - `bigint_primitive` — BigInt method dispatch wrapper that
//!     fans out to the above plus per-method arms for `to_s` /
//!     `inspect` / predicates / `<=>` / `eql?` / `hash` (B.4 + B.7
//!     `eql?` / `hash`).
//!   - `check_bigint_to_s_cap` / `bignum_log2_per_digit_scaled` /
//!     `bignum_digits_upper_bound` — shared cap estimator for the
//!     `to_s` and sprintf base-N paths (B.4). Listed in file order;
//!     the `Vm::` method comes first because it's the call-site,
//!     the two free fns below are its helpers.
//!   - `bigint_to_value` / `as_bigint` / `as_bigint_ref` /
//!     `bigint_arith` — the lowest-level arithmetic surface.
//!     `bignum` only.

use crate::error::Trap;
use crate::value::Value;
use crate::vm::Vm;
#[cfg(feature = "bignum")]
use crate::error::RubyError;
#[cfg(feature = "bignum")]
use crate::vm::PinGuard;

/// CRuby-parity lossless equality between a BigInt and a Float.
/// Returns true iff `bigint` and `float` represent the same exact
/// integer value.
///
/// Without this, demoting the BigInt to f64 collapses values like
/// `2**64` and `2**64 + 1` onto the same Float bit pattern (the
/// ULP at that magnitude is 2^(64-52)=4096), making
/// `(2**64 + 1) == (2**64).to_f` wrongly return true. CRuby's
/// `rb_big_eq` short-circuits on NaN / infinity / non-integral
/// floats and otherwise compares against a losslessly-constructed
/// BigInt; mirror that.
#[cfg(feature = "bignum")]
fn bigint_equals_float_lossless(bigint: &num_bigint::BigInt, float: f64) -> bool {
    use num_traits::FromPrimitive;
    // NaN, +inf, -inf: never equal to a finite integer.
    if !float.is_finite() {
        return false;
    }
    // Float with a fractional part: never equal to an integer.
    // For huge floats whose magnitude exceeds 2^53 the fractional
    // bits are zero, so `f.fract() != 0.0` correctly bottoms out
    // for small fractional floats (1.5, 0.1, …) without
    // false-rejecting big integral floats.
    if float.fract() != 0.0 {
        return false;
    }
    // Integral finite float → exact BigInt conversion. `from_f64`
    // truncates toward zero, but the integral-float guard above
    // means there's nothing to truncate.
    match num_bigint::BigInt::from_f64(float) {
        Some(rhs) => *bigint == rhs,
        // `from_f64` returns `None` only for NaN / ±inf — both
        // already filtered above by `is_finite()`. Defensive
        // arm: a future num-bigint version that narrowed the
        // accepted range would land here, and "not equal" is
        // the safe default (BigInt itself can represent any
        // finite-f64 magnitude, since the largest finite f64
        // is ~1.8e308 which fits trivially in a BigInt).
        None => false,
    }
}

/// CRuby-parity lossless three-way comparison between a BigInt
/// and a Float.
///
/// Scope: BigInt × Float only. Int × Float (Fixnum range) still
/// demotes the Int to f64 in numeric.rs's Int×Float arm, so
/// e.g. `(2**62 + 1) <=> (2**62).to_f` currently returns 0
/// instead of CRuby's 1 — fixing that would require an Int-side
/// lossless path with i64-vs-f64 mantissa-bit reasoning, tracked
/// as a follow-up.
///
/// Returns:
/// - `None` for NaN (CRuby's `bigint <=> nan` returns `nil`;
///   `bigint < nan` / `> nan` / `<= nan` / `>= nan` all return
///   `false`).
/// - `Some(Less)` if `bigint < float` (bigint is more negative).
/// - `Some(Equal)` if exactly equal.
/// - `Some(Greater)` if `bigint > float`.
///
/// Without this, demoting the BigInt to f64 collapses values
/// within the same Float ULP onto the same bit pattern: f64 has
/// 53-bit precision, so above 2^53 the ULP is 2^(N-52) for
/// magnitude 2^N — e.g. ULP=2^12=4096 at 2^64. `(2**64 + 1)` is
/// closer to 2^64 than to 2^64+4096, so it rounds to exactly
/// 2^64; without the lossless path `(2**64 + 1) > (2**64).to_f`
/// wrongly returned false. Mirror `bigint_equals_float_lossless`
/// — finite floats convert losslessly via `from_f64` (truncates
/// toward zero) and the fractional sign disambiguates the tie.
#[cfg(feature = "bignum")]
fn bigint_cmp_float_lossless(
    bigint: &num_bigint::BigInt,
    float: f64,
) -> Option<std::cmp::Ordering> {
    use num_traits::FromPrimitive;
    use std::cmp::Ordering;
    if float.is_nan() {
        return None;
    }
    // ±inf: bigint is finite, so always strictly less than +inf
    // and strictly greater than -inf.
    if float == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if float == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    // Finite float: convert losslessly via truncation (toward
    // zero). The fractional sign then disambiguates the tie:
    //   f = 2.7 → trunc=2,  frac=+0.7 → if bigint==2 then bigint < f
    //   f = -2.7 → trunc=-2, frac=-0.7 → if bigint==-2 then bigint > f
    //   f = 2.0 → trunc=2,  frac=0     → if bigint==2 then equal
    // Defensive: from_f64 returns None only for NaN/±inf
    // (filtered above). A future num-bigint version that narrowed
    // the range would land here; "not comparable" is the safe
    // default.
    let trunc = num_bigint::BigInt::from_f64(float)?;
    let cmp = bigint.cmp(&trunc);
    if cmp != Ordering::Equal {
        return Some(cmp);
    }
    let frac = float - float.trunc();
    if frac == 0.0 {
        Some(Ordering::Equal)
    } else if frac > 0.0 {
        // f is between trunc and trunc+1; bigint == trunc < f
        Some(Ordering::Less)
    } else {
        // f is between trunc-1 and trunc; bigint == trunc > f
        Some(Ordering::Greater)
    }
}

/// Dispatch helper: tries Int/BigInt arithmetic or comparison
/// for the `Op::BinOp` cold path (operands include at least one
/// BigInt, or are non-Int shapes that this method declines).
/// With `bignum` off this is a no-op that always returns `None`,
/// so the caller falls through to `primitive_call` exactly as
/// before.
impl Vm {
    pub(crate) fn try_bigint_binop(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        a: &Value,
        b: &Value,
    ) -> Result<Option<Value>, Trap> {
        #[cfg(not(feature = "bignum"))]
        {
            let _ = (kind, a, b);
            Ok(None)
        }
        #[cfg(feature = "bignum")]
        {
            use crate::bytecode::BinOpKind;
            // Decline unless at least one operand is a BigInt
            // (Int×Int went through the fast path above already).
            if !matches!(a, Value::BigInt(_)) && !matches!(b, Value::BigInt(_)) {
                return Ok(None);
            }
            // Float ↔ BigInt mixed: coerce the BigInt to f64 (lossy
            // at extreme magnitudes — matches CRuby's "Float wins
            // on mix" rule and Integer#to_f's documented precision
            // loss past 2^53). Without this branch, `2.0 + big`
            // raised NoMethodError because primitive_call's Float
            // arms only handle Int/Float rhs.
            if matches!(a, Value::Float(_)) || matches!(b, Value::Float(_)) {
                // Eq/Ne take a separate lossless path — `(2**64 + 1)
                // == (2**64).to_f` must return false (CRuby), but
                // demoting both sides to f64 collapses them onto the
                // same Float bit pattern. Compare the BigInt against
                // a BigInt converted FROM the float (exact when the
                // float is integral; pre-rejected when fractional).
                //
                // Lt/Le/Gt/Ge take the same lossless treatment via
                // `bigint_cmp_float_lossless` — NaN yields false for
                // every ordering operator (CRuby parity), finite
                // floats compare via truncation + fractional sign.
                if matches!(
                    kind,
                    BinOpKind::Lt | BinOpKind::Le | BinOpKind::Gt | BinOpKind::Ge,
                ) {
                    use std::cmp::Ordering;
                    let (big_id, float_v, big_is_lhs) = match (a, b) {
                        (Value::BigInt(id), Value::Float(f)) => (*id, *f, true),
                        (Value::Float(f), Value::BigInt(id)) => (*id, *f, false),
                        _ => unreachable!("BigInt × Float invariant"),
                    };
                    let cmp = bigint_cmp_float_lossless(
                        self.heap.bigint(big_id),
                        float_v,
                    );
                    // Flip Ordering if BigInt sits on the RHS so the
                    // operator interpretation stays in lhs-vs-rhs
                    // direction.
                    let cmp = cmp.map(|o| if big_is_lhs { o } else { o.reverse() });
                    let result = match (cmp, kind) {
                        // NaN → all four ordering ops are false.
                        (None, _) => false,
                        (Some(Ordering::Less), BinOpKind::Lt) => true,
                        (Some(Ordering::Less), BinOpKind::Le) => true,
                        (Some(Ordering::Equal), BinOpKind::Le) => true,
                        (Some(Ordering::Equal), BinOpKind::Ge) => true,
                        (Some(Ordering::Greater), BinOpKind::Gt) => true,
                        (Some(Ordering::Greater), BinOpKind::Ge) => true,
                        _ => false,
                    };
                    return Ok(Some(Value::Bool(result)));
                }
                if matches!(kind, BinOpKind::Eq | BinOpKind::Ne) {
                    // The outer "at least one is BigInt" guard plus
                    // this "at least one is Float" guard mean exactly
                    // one of {a, b} is a BigInt and the other is a
                    // Float — Float×Int and Int×Float already
                    // declined above (neither side is BigInt).
                    let (big_id, float_v) = match (a, b) {
                        (Value::BigInt(id), Value::Float(f))
                        | (Value::Float(f), Value::BigInt(id)) => (*id, *f),
                        _ => unreachable!("BigInt × Float invariant"),
                    };
                    let eq = bigint_equals_float_lossless(
                        self.heap.bigint(big_id),
                        float_v,
                    );
                    let result = match kind {
                        BinOpKind::Eq => eq,
                        BinOpKind::Ne => !eq,
                        _ => unreachable!(),
                    };
                    return Ok(Some(Value::Bool(result)));
                }

                let to_f = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(n) => Some(*n as f64),
                        Value::BigInt(id) => self.heap.bigint(*id).to_string().parse::<f64>().ok(),
                        _ => None,
                    }
                };
                let (af, bf) = match (to_f(a), to_f(b)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => return Ok(None),
                };
                let result = match kind {
                    BinOpKind::Add => Value::Float(af + bf),
                    BinOpKind::Sub => Value::Float(af - bf),
                    BinOpKind::Mul => Value::Float(af * bf),
                    BinOpKind::Div => Value::Float(af / bf),
                    BinOpKind::Mod => Value::Float(af.rem_euclid(bf)),
                    // Eq/Ne handled above via bigint_equals_float_lossless;
                    // Lt/Le/Gt/Ge via bigint_cmp_float_lossless.
                    BinOpKind::Eq
                    | BinOpKind::Ne
                    | BinOpKind::Lt
                    | BinOpKind::Le
                    | BinOpKind::Gt
                    | BinOpKind::Ge => unreachable!("comparison ops handled via lossless paths above"),
                };
                return Ok(Some(result));
            }
            // Both operands must be integers (Int or BigInt); if
            // not, decline and let primitive_call try (e.g. for
            // String * BigInt later). Use `as_bigint_ref` to
            // borrow heap-side BigInts rather than cloning — only
            // Int→BigInt coercions allocate, and comparison ops
            // run entirely from refs.
            let ax_cow = match self.as_bigint_ref(a) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bx_cow = match self.as_bigint_ref(b) {
                Some(v) => v,
                None => return Ok(None),
            };
            // Comparison ops return Bool directly (run against the
            // borrowed BigInts via Cow's Deref impl — no clones).
            match kind {
                BinOpKind::Lt => return Ok(Some(Value::Bool(*ax_cow < *bx_cow))),
                BinOpKind::Le => return Ok(Some(Value::Bool(*ax_cow <= *bx_cow))),
                BinOpKind::Gt => return Ok(Some(Value::Bool(*ax_cow > *bx_cow))),
                BinOpKind::Ge => return Ok(Some(Value::Bool(*ax_cow >= *bx_cow))),
                BinOpKind::Eq => return Ok(Some(Value::Bool(*ax_cow == *bx_cow))),
                BinOpKind::Ne => return Ok(Some(Value::Bool(*ax_cow != *bx_cow))),
                _ => {}
            }
            drop(ax_cow);
            drop(bx_cow);
            // Arithmetic: delegate to bigint_arith which handles
            // zero-division traps and CRuby-style floor div / mod.
            match self.bigint_arith(kind, a, b) {
                Some(res) => Ok(Some(res?)),
                None => Ok(None),
            }
        }
    }
}

/// `**` exponentiation with BigInt promotion and DoS cap.
/// Returns:
/// - `Some(v)` for any Int/BigInt × {Int (non-negative), Float,
///   negative Int, BigInt-when-|base|≤1} where we can produce a
///   value. Float / negative-Int exponents on Int receivers are
///   normally handled by numeric_call BEFORE reaching this fn;
///   we cover them here only when the receiver is a BigInt
///   (otherwise NoMethodError despite `respond_to?(:**)` being
///   true) and for the |base|≤1 short-circuit.
/// - `Err(...)` for BigInt exponents with |base|>1 — the result
///   would need at least 2^63 bits of storage so we trap
///   `ResourceExhausted` rather than attempting to compute or
///   silently falling through.
/// - `None` for operand shapes outside this branch's scope
///   (non-integer recv, or Int recv + Float/negative exp where
///   numeric_call handles it); the caller falls through.
///
/// DoS protection: result bit count is approximately
/// `bit_length(base) * exp` (tight as `(bit_length-1) * exp + 1`
/// when |base| is a power of two). A few bytes of input can ask
/// for many GB of output, so we pre-estimate and trap
/// `ResourceExhausted` before calling `BigInt::pow`. The estimate
/// rounds up to the BigInt limb size (u64 = 8 bytes) plus a small
/// allocator-header overhead so the cap reflects actual heap
/// storage, not just the minimal bit count. Honours
/// `Config::max_value_bytes` (same cap that bounds String /
/// Array growth); falls back to a 1 MB safety ceiling when no
/// cap is configured.
#[cfg(feature = "bignum")]
impl Vm {
    pub(crate) fn try_bigint_pow(
        &mut self,
        recv: &Value,
        exp_arg: &Value,
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::{BigInt, Sign};
        let recv_is_bigint = matches!(recv, Value::BigInt(_));
        let exp_is_bigint = matches!(exp_arg, Value::BigInt(_));
        // Float / negative-exp paths need to fire here whenever
        // either operand is BigInt, since numeric_call only covers
        // pure Int×Int. Without this, `2 ** -(2**100)` or
        // `1 ** -(2**100)` (Int recv + negative BigInt exp) would
        // fall through to NoMethodError.
        let need_float_handling = recv_is_bigint || exp_is_bigint;
        // Read base sign + bit-length via borrowed Cow — avoids
        // the O(n) magnitude clone `as_bigint` would do for BigInt
        // receivers. The Cow borrow ends with the block; later
        // &mut self calls (`trap`, `bigint_to_value`) are free to
        // re-borrow. The full base is re-borrowed only at the
        // single `pow` site below.
        let (base_sign, base_bits) = {
            let base_cow = match self.as_bigint_ref(recv) {
                Some(v) => v,
                None => return Ok(None),
            };
            (base_cow.sign(), base_cow.bits())
        };
        // `base_is_pow2` is only consulted by the positive-exp
        // DoS estimator below. Defer the O(n) `count_ones()` scan
        // until we know we're in that branch so Float / negative-
        // exp / short-circuit paths don't pay for it.
        // Compute parity / sign / zero of the exponent up front so
        // every branch below dispatches on one vocabulary.
        let (exp_is_negative, exp_is_zero, exp_is_odd, exp_is_float) = match exp_arg {
            Value::Int(n) => (*n < 0, *n == 0, *n & 1 != 0, false),
            Value::BigInt(id) => {
                let big = self.heap.bigint(*id);
                let s = big.sign();
                (s == Sign::Minus, s == Sign::NoSign, big.bit(0), false)
            }
            Value::Float(_) => (false, false, false, true),
            _ => return Ok(None),
        };
        // Float exponent: coerce base to f64 (bounded) and use
        // powf. Int receivers go through numeric_call's Int×Float
        // arm BEFORE reaching here, so this fires only for BigInt
        // receivers (where the alternative would be NoMethodError
        // despite `respond_to?(:**)` returning true).
        if exp_is_float {
            if !recv_is_bigint { return Ok(None); }
            if let Value::Float(f) = exp_arg {
                let base_f = self.bigint_recv_to_f64_bounded(recv);
                return Ok(Some(Value::Float(base_f.powf(*f))));
            }
            unreachable!("exp_is_float ⇒ Value::Float(_)");
        }
        // Short-circuit |base| ≤ 1 — constant-size results,
        // dispatch only on sign + parity, safe for any exp shape.
        if base_bits <= 1 {
            match base_sign {
                Sign::NoSign => {
                    // base == 0. 0**0 == 1; 0**n (n>0) == 0;
                    // 0**n (n<0) raises ZeroDivisionError in CRuby
                    // — match that for ALL operand shapes. The
                    // previous behaviour returned `Float::INFINITY`
                    // for BigInt-flavoured operands (and Int recv
                    // × Int neg exp deferred to numeric.rs's powf,
                    // which silently produced inf too). Both paths
                    // now raise so the error surfaces explicitly
                    // instead of poisoning downstream arithmetic.
                    if exp_is_negative {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let r = if exp_is_zero { BigInt::from(1) } else { BigInt::from(0) };
                    return Ok(Some(self.bigint_to_value(r)?));
                }
                Sign::Plus => {
                    // base == 1: always 1. Negative exp → Float(1.0)
                    // for BigInt-flavoured operands (Int×Int still
                    // defers to numeric.rs's parity-preserving ±1
                    // arm).
                    if exp_is_negative {
                        if need_float_handling {
                            return Ok(Some(Value::Float(1.0)));
                        }
                        return Ok(None);
                    }
                    return Ok(Some(self.bigint_to_value(BigInt::from(1))?));
                }
                Sign::Minus => {
                    // base == -1: parity decides sign. Negative
                    // exponent: |result| = 1, sign from parity.
                    if exp_is_negative {
                        if need_float_handling {
                            return Ok(Some(Value::Float(if exp_is_odd { -1.0 } else { 1.0 })));
                        }
                        return Ok(None);
                    }
                    let r = if exp_is_odd { BigInt::from(-1) } else { BigInt::from(1) };
                    return Ok(Some(self.bigint_to_value(r)?));
                }
            }
        }
        // |base| > 1 from here on.
        // Negative Int / BigInt exp: Float reciprocal. Pure
        // Int×Int neg-exp goes through numeric.rs first; we cover
        // every other shape here (BigInt recv with any neg exp,
        // OR Int recv with negative BigInt exp) so dispatch
        // doesn't NoMethodError on `2 ** -(2**100)` and friends.
        if exp_is_negative {
            if need_float_handling {
                let exp_f = match exp_arg {
                    Value::Int(n) => *n as f64,
                    // BigInt-negative exp: result tends toward 0
                    // for |base|>1. Coerce via the bounded helper
                    // (caps the intermediate string at f64-range,
                    // ~310 bytes max).
                    Value::BigInt(id) => {
                        let big = self.heap.bigint(*id);
                        Self::bigint_to_f64_bounded(big)
                    }
                    _ => unreachable!(),
                };
                // Compute on |base| so a negative base + non-
                // integer / non-finite exp can't NaN out of
                // libm's powf. Re-apply the sign from the
                // already-computed base_sign + exp_is_odd, which
                // preserve parity from the original i64 / BigInt
                // rather than the f64 round.
                let base_f = self.bigint_recv_to_f64_bounded(recv);
                let mag = base_f.abs().powf(exp_f);
                let signed = if base_sign == Sign::Minus && exp_is_odd { -mag } else { mag };
                return Ok(Some(Value::Float(signed)));
            }
            return Ok(None);
        }
        // Exponent identities — return cheap results before the
        // DoS estimator (which itself adds a 32-byte header to
        // est_bytes, so `big ** 0` under an aggressively tight
        // `max_value_bytes` would otherwise trap even though the
        // correct answer is the immediate `Int(1)`). Skip the pow
        // allocation entirely for `** 0` and `** 1`.
        if exp_is_zero {
            return Ok(Some(Value::Int(1)));
        }
        if matches!(exp_arg, Value::Int(1)) {
            return Ok(Some(recv.clone()));
        }
        // Positive exp from here on. BigInt exponent with |base|>1
        // → trap (would need ≥ 2**63 bits).
        if matches!(exp_arg, Value::BigInt(_)) {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: "integer ** BigInt exponent exceeds u32::MAX".to_string(),
            }));
        }
        let exp_i64 = match exp_arg {
            Value::Int(n) => *n, // ≥ 2 (0, 1, negative handled above)
            _ => unreachable!("non-Int/BigInt/Float exp returned earlier"),
        };
        let exp_u32: u32 = match u32::try_from(exp_i64) {
            Ok(v) => v,
            Err(_) => {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!("integer exponent {} exceeds u32::MAX", exp_i64),
                }));
            }
        };
        // Estimate result size and trap before allocating GBs.
        // The true bit-length of `base ** exp` is
        // `floor(exp * log2(|base|)) + 1`. For a power-of-two
        // base, `log2(|base|) == base_bits - 1` exactly, so the
        // tight bound is `(base_bits - 1) * exp + 1` — using
        // `base_bits * exp` here would overshoot 2× on the
        // canonical `2 ** n` shape (e.g. `2 ** 10_000_000`
        // really is ~1.25MB but a `2 * 10_000_000 = 20M-bit`
        // estimate would falsely trap a 2MB cap). For non-pow2
        // bases we fall back to `base_bits * exp` as a safe
        // upper bound (log2(base) < base_bits for any base).
        // Ceil-div in u64; compare against `cap as u64` so the
        // check doesn't silently truncate on 32-bit targets.
        // Compute power-of-two flag lazily here — earlier paths
        // (Float exp, negative exp, |base|≤1 short-circuit) all
        // return before reaching the estimator, so they avoid
        // the O(n) `count_ones()` scan over the BigInt magnitude.
        let base_is_pow2 = {
            let base_cow = match self.as_bigint_ref(recv) {
                Some(v) => v,
                None => unreachable!("recv shape validated at fn entry"),
            };
            base_cow.magnitude().count_ones() == 1
        };
        let est_bits: u64 = if base_is_pow2 {
            (base_bits.saturating_sub(1))
                .saturating_mul(exp_u32 as u64)
                .saturating_add(1)
        } else {
            base_bits.saturating_mul(exp_u32 as u64)
        };
        // Round up to BigInt limb storage (u64 limbs = 8 bytes each)
        // plus a small allocator-header overhead so the cap reflects
        // actual heap storage rather than just the minimal bit count.
        // This keeps `max_value_bytes` semantically aligned with the
        // Array/String paths (which count backing-storage bytes) and
        // closes a small word-boundary bypass on inputs that landed
        // just under the previous min-bytes estimate.
        const BIGINT_HEADER_BYTES: u64 = 32;
        let est_limbs: u64 = est_bits.saturating_add(63) / 64;
        let est_bytes: u64 = est_limbs.saturating_mul(8).saturating_add(BIGINT_HEADER_BYTES);
        let cap = self.max_value_bytes.unwrap_or(1 << 20);
        if est_bytes > cap as u64 {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!(
                    "integer ** exp would need ~{} bytes, exceeding cap {}",
                    est_bytes, cap
                ),
            }));
        }
        // Borrow base once more for the actual pow; `(&BigInt).pow`
        // returns an owned BigInt without consuming the receiver,
        // so a BigInt-receiver path computes pow against a
        // borrowed magnitude rather than a clone.
        let result = match self.as_bigint_ref(recv) {
            Some(c) => c.pow(exp_u32),
            None => unreachable!("recv shape validated earlier"),
        };
        Ok(Some(self.bigint_to_value(result)?))
    }

    /// BigInt → f64 with the intermediate decimal string bounded
    /// by a bits()-based pre-check. f64::MAX ≈ 2^1024, so any
    /// BigInt past that is already out of f64 range — return ±∞
    /// without materialising a string. Below the threshold the
    /// decimal form is at most ~310 digits, well under any
    /// `max_value_bytes` cap we care about. Centralises the
    /// dispatch.rs Range coercion pattern in one place that the
    /// `**` Float / negative-exp paths can share without
    /// allocating O(magnitude) strings on a hostile big input.
    pub(crate) fn bigint_to_f64_bounded(b: &num_bigint::BigInt) -> f64 {
        use num_bigint::Sign;
        if b.bits() > 1024 {
            return if b.sign() == Sign::Minus { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        b.to_string().parse::<f64>().unwrap_or(f64::NAN)
    }

    /// Receiver-side helper around [`Self::bigint_to_f64_bounded`]:
    /// borrows the BigInt out of the heap via `as_bigint_ref`,
    /// then defers to the bounded coercion. Returns `NaN` if the
    /// receiver isn't an integer (caller already validated this;
    /// the NaN is a defensive fallback, not a reachable path).
    pub(crate) fn bigint_recv_to_f64_bounded(&self, recv: &Value) -> f64 {
        match self.as_bigint_ref(recv) {
            Some(c) => Self::bigint_to_f64_bounded(&c),
            None => f64::NAN,
        }
    }

    /// Unary `-@` / `+@` / `abs` for BigInt receivers, plus the
    /// Int(i64::MIN) auto-promotion case (where the i64 cannot
    /// represent its own negation or absolute value, so numeric.rs
    /// declines and we materialise the BigInt 2^63 here). For Int
    /// receivers other than i64::MIN we return `None` so dispatch
    /// stays on numeric.rs's existing wrapping arms. `+@` on
    /// BigInt is a no-op clone; on Int it shouldn't even reach
    /// here (numeric.rs handles it) — included for completeness.
    pub(crate) fn try_bigint_unary(
        &mut self,
        recv: &Value,
        name: &str,
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::{BigInt, Sign};
        match recv {
            Value::BigInt(id) => {
                // Compute the owned result in a borrow scope, then
                // drop the borrow before calling bigint_to_value
                // (&mut self). `+@` just hands back the receiver
                // unchanged — no demote needed. The identity
                // shortcut is sound ONLY because every
                // `Value::BigInt(id)` is allocated through
                // `bigint_to_value`, which demotes any in-i64
                // magnitude to `Value::Int(n)` — see the
                // `debug_assert!` below. If a future cext/FFI path
                // ever bypasses `bigint_to_value` and stores an
                // in-i64 magnitude as `HeapObj::BigInt`, this
                // shortcut would leak a non-canonical
                // `Value::BigInt(small)` whose dispatch semantics
                // drift from `Value::Int(small)`.
                if name == "+@" {
                    debug_assert!(
                        i64::try_from(self.heap.bigint(*id)).is_err(),
                        "non-canonical BigInt reached try_bigint_unary +@: \
                         magnitude fits i64 but wasn't demoted by bigint_to_value",
                    );
                    return Ok(Some(recv.clone()));
                }
                // `abs` on an already-non-negative BigInt is the
                // identity: skip both the BigInt clone and the
                // bigint_to_value allocation by handing back
                // `recv` unchanged (same shape as `+@`). Only the
                // Minus branch needs a fresh BigInt + demote-on-fit
                // funnel.
                if name == "abs" {
                    let sign = self.heap.bigint(*id).sign();
                    if sign != Sign::Minus {
                        debug_assert!(
                            i64::try_from(self.heap.bigint(*id)).is_err(),
                            "non-canonical BigInt reached try_bigint_unary abs: \
                             magnitude fits i64 but wasn't demoted by bigint_to_value",
                        );
                        return Ok(Some(recv.clone()));
                    }
                }
                let result = {
                    let b = self.heap.bigint(*id);
                    match name {
                        "-@" => -b,
                        "abs" => -b, // sign == Minus from check above
                        // `~big` two's-complement bitwise NOT.
                        // Identity: `~b == -(b + 1)`. num_bigint
                        // impls `Not` for both owned and borrowed
                        // BigInt (the `&BigInt` form returns a fresh
                        // owned BigInt without consuming the
                        // reference) — same shape as the `-b` arms
                        // above, no clone needed. The two's-
                        // complement conversion happens internally.
                        // `bigint_to_value` demotes-on-fit so
                        // `~(2**63) == -(2**63 + 1)` stays BigInt
                        // (just past i64::MIN) but `~(2**63 - 1) ==
                        // -(2**63)` demotes to Int(i64::MIN). The
                        // Int receiver `~n` path is unchanged in
                        // numeric.rs because `!i64::MIN == i64::MAX`
                        // fits without promotion.
                        "~" => !b,
                        // `succ` / `next` / `pred` on BigInt — `b + 1`
                        // / `b - 1` through the demote-on-fit funnel.
                        // `(2**63).pred == 2**63 - 1 == i64::MAX` is
                        // the canonical demote case (BigInt(2^63) is
                        // one past i64::MAX; subtracting 1 lands on
                        // i64::MAX). `(-2**63 - 1).succ == i64::MIN`
                        // is the symmetric demote.
                        "succ" | "next" => b + 1,
                        "pred" => b - 1,
                        _ => return Ok(None),
                    }
                };
                Ok(Some(self.bigint_to_value(result)?))
            }
            Value::Int(n) if *n == i64::MIN => {
                // i64::MIN.abs() and -i64::MIN both overflow i64 by
                // exactly one (the magnitude is 2^63, one past
                // i64::MAX). Promote via BigInt — bigint_to_value
                // will keep it as BigInt since it doesn't fit.
                // `pred` lands one past on the negative side
                // (i64::MIN - 1 = -(2^63 + 1)).
                match name {
                    "abs" | "-@" => {
                        let promoted = -BigInt::from(i64::MIN);
                        Ok(Some(self.bigint_to_value(promoted)?))
                    }
                    "pred" => {
                        let promoted = BigInt::from(i64::MIN) - 1;
                        Ok(Some(self.bigint_to_value(promoted)?))
                    }
                    "+@" => Ok(Some(Value::Int(i64::MIN))),
                    _ => Ok(None),
                }
            }
            Value::Int(n) if *n == i64::MAX => {
                // Symmetric to the i64::MIN arm above for the
                // succ/next path: i64::MAX + 1 = 2^63, which lands
                // exactly on the smallest BigInt magnitude.
                // Other unary ops on i64::MAX don't overflow
                // (`-i64::MAX == -9223372036854775807` fits,
                // `i64::MAX.abs() == i64::MAX` fits), so numeric.rs
                // handles them without our help — only succ/next
                // need the promote.
                match name {
                    "succ" | "next" => {
                        let promoted = BigInt::from(i64::MAX) + 1;
                        Ok(Some(self.bigint_to_value(promoted)?))
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// Bitwise binary ops `&` / `|` / `^` on Integer operands with
    /// at least one BigInt. num_bigint's `BitAnd` / `BitOr` /
    /// `BitXor` impls perform unbounded two's-complement
    /// conversion for negatives before applying the op and convert
    /// back — so `(-1) & 0xff == 0xff` and `(-256) & 0xff == 0`
    /// match CRuby without any sign-extension bookkeeping on our
    /// side.
    ///
    /// Returns:
    /// - `Some(v)` for Integer × Integer — result funnelled
    ///   through `bigint_to_value` for demote-on-fit (e.g.
    ///   `(2**100) & 0xff` demotes to Int).
    /// - `Ok(None)` when the receiver is not an Integer (caller
    ///   falls through; not our concern).
    /// - `Err(TypeError)` when the receiver IS an Integer but the
    ///   arg is not — CRuby raises "no implicit conversion of X
    ///   into Integer" here, not NoMethodError. Pre-fix the helper
    ///   returned Ok(None) for non-Integer args and the
    ///   fall-through landed at NoMethodError, diverging from both
    ///   CRuby and from the sibling bignum arithmetic paths which
    ///   raise TypeError on the same shape.
    ///
    /// Fires for both `(BigInt, op, [_])` and `(Int, op, [BigInt])`
    /// shapes — the recv-or-arg-is-BigInt guard in
    /// `bigint_primitive` is what gates entry. Int × Int is owned
    /// by numeric.rs's existing `(Int, op, [Int])` bit-op arm and
    /// never reaches here, EXCEPT when arg is non-Integer (Float,
    /// String, …) — that case falls through to this helper so the
    /// TypeError raise applies uniformly across Int and BigInt
    /// receivers.
    pub(crate) fn try_bigint_bit_binop(
        &mut self,
        recv: &Value,
        name: &str,
        arg: &Value,
    ) -> Result<Option<Value>, Trap> {
        if !matches!(recv, Value::Int(_) | Value::BigInt(_)) { return Ok(None); }
        // Arg-type guard: non-Integer raises TypeError matching
        // CRuby's coerce-error wording (sibling to the unified
        // `Integer#to_s(non_integer)` arm in numeric.rs).
        if !matches!(arg, Value::Int(_) | Value::BigInt(_)) {
            return Err(self.trap(RubyError::TypeError {
                msg: format!(
                    "no implicit conversion of {} into Integer",
                    crate::vm::numeric::type_name_for_coerce(arg),
                ),
            }));
        }
        let result = {
            // Borrow scope: both sides borrowed as Cow<BigInt>
            // (Int wraps via owned `BigInt::from(n)`, BigInt is
            // borrowed from the heap with no clone). Drop before
            // calling bigint_to_value (&mut self).
            let ax = match self.as_bigint_ref(recv) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bx = match self.as_bigint_ref(arg) {
                Some(v) => v,
                None => return Ok(None),
            };
            // Owned-by-ref op — num_bigint defines `&BigInt op
            // &BigInt`, so no clone of either operand. The result
            // is a fresh owned BigInt that outlives the borrow.
            match name {
                "&" => &*ax & &*bx,
                "|" => &*ax | &*bx,
                "^" => &*ax ^ &*bx,
                _ => return Ok(None),
            }
        };
        Ok(Some(self.bigint_to_value(result)?))
    }

    /// Bitwise shifts `<<` / `>>` with BigInt promotion.
    ///
    /// CRuby semantics (two's-complement, arbitrary precision):
    /// - `recv << n` where `n < 0` means `recv >> (-n)` (direction
    ///   swap), and vice versa for `>>`. Magnitude-only after the
    ///   swap.
    /// - Right-shifting any value by ≥ its bit-length collapses
    ///   to 0 (non-negative recv) or -1 (negative recv,
    ///   sign-extended two's-complement).
    /// - Left-shifts can produce arbitrarily large results; cap
    ///   via `max_value_bytes` (same convention as `**`).
    /// - BigInt shift count is allowed but by the canonical-
    ///   BigInt invariant any `Value::BigInt` arg is outside i64,
    ///   so any actual-left-shift by a BigInt count traps (would
    ///   need > 2^63 bits); actual-right-shift collapses to 0/-1.
    ///
    /// Fires for both `(BigInt, op, [_])` and `(Int, op, [_])` —
    /// the Int×Int overflow path (`1 << 64`) returns None from
    /// numeric.rs's arm so we can promote here. The cond hook in
    /// `bigint_primitive` lives ahead of the recv-or-arg-is-BigInt
    /// guard so the Int×Int overflow path isn't filtered out.
    pub(crate) fn try_bigint_bit_shift(
        &mut self,
        recv: &Value,
        name: &str,
        arg: &Value,
    ) -> Result<Option<Value>, Trap> {
        let left = match name { "<<" => true, ">>" => false, _ => return Ok(None) };
        if !matches!(recv, Value::Int(_) | Value::BigInt(_)) { return Ok(None); }
        // Arg-type guard FIRST so we don't accidentally accept
        // `0 << 1.5` as `0`. Pre-fix the `Value::Int(0)` recv
        // shortcut below ran ahead of this check and swallowed the
        // TypeError that a non-Integer arg should raise. Raises
        // TypeError directly (same shape as `try_bigint_bit_binop`)
        // rather than Ok(None)+NoMethodError-fallthrough — CRuby
        // raises "no implicit conversion of X into Integer" here.
        if !matches!(arg, Value::Int(_) | Value::BigInt(_)) {
            return Err(self.trap(RubyError::TypeError {
                msg: format!(
                    "no implicit conversion of {} into Integer",
                    crate::vm::numeric::type_name_for_coerce(arg),
                ),
            }));
        }
        // Zero receiver: `0 << anything == 0` and `0 >> anything ==
        // 0` regardless of count sign or magnitude. Short-circuit
        // ahead of the BigInt-count trap and the DoS cap estimator
        // so `0 << 1_000_000` (or `0 << (2**100)`) doesn't
        // false-trap under a tight `max_value_bytes`. The canonical-
        // BigInt invariant means `Value::BigInt(_)` can never be
        // zero (any in-i64 magnitude demotes), so this check only
        // fires for `Value::Int(0)`. Lives AFTER the arg-type guard
        // so non-Integer args are still rejected.
        if matches!(recv, Value::Int(0)) {
            return Ok(Some(Value::Int(0)));
        }
        // Resolve shift magnitude + actual direction.
        // - Int arg: direction may flip if negative; magnitude
        //   stored as u64. `i64::MIN` negation overflows, so handle
        //   it explicitly as `1u64 << 63`.
        // - BigInt arg: magnitude is by invariant > i64::MAX. If
        //   the actual direction is left we trap immediately
        //   (would need > 2^63 bits); if right, collapse via
        //   `bit_shift_collapse`.
        let (shift_mag, actual_left): (u64, bool) = match arg {
            Value::Int(n) => {
                if *n == 0 { return Ok(Some(recv.clone())); }
                if *n > 0 { (*n as u64, left) }
                else if *n == i64::MIN { (1u64 << 63, !left) }
                else { ((-n) as u64, !left) }
            }
            Value::BigInt(id) => {
                let sign = self.heap.bigint(*id).sign();
                let actual_left = if sign == num_bigint::Sign::Minus { !left } else { left };
                if actual_left {
                    // Shift count is a Bignum, so by the canonical-
                    // BigInt invariant its magnitude > i64::MAX.
                    // Actual-left-shift by that count would need
                    // ≥ 2^63 bits of storage — well past any sane
                    // `max_value_bytes`. Frame the trap around the
                    // would-be result size rather than the count's
                    // u32::MAX boundary so it reads in the same
                    // shape as the DoS-cap trap below.
                    return Err(self.trap(RubyError::ResourceExhausted {
                        msg: "integer shift result exceeds max representable size (Bignum shift count)".to_string(),
                    }));
                }
                return Ok(Some(self.bit_shift_collapse(recv)));
            }
            _ => return Ok(None),
        };
        // Recv bit-length — exact for both Int and BigInt. The
        // earlier conservative `64` for Int over-counted on small
        // magnitudes, which (after rounding to limbs + 32-byte
        // header) could false-trap the DoS cap for shifts where
        // the rendered result actually fit (`5 << shift` with a
        // tight `max_value_bytes` near `bit_length(5) + shift`
        // bytes was the canonical bad case). For i64 the exact
        // bit_length of the magnitude is `64 - unsigned_abs().leading_zeros()`
        // with the zero-magnitude case clamping to 0 (matches
        // CRuby's `bit_length(0) == 0`).
        let recv_bits: u64 = match recv {
            Value::Int(n) => {
                let mag = n.unsigned_abs();
                if mag == 0 { 0 } else { 64 - mag.leading_zeros() as u64 }
            }
            Value::BigInt(id) => self.heap.bigint(*id).bits(),
            _ => unreachable!("guarded above"),
        };
        // Right-shift by ≥ recv_bits: collapse to 0 / -1 without
        // touching num_bigint (avoids large-shift allocation).
        if !actual_left && shift_mag >= recv_bits {
            return Ok(Some(self.bit_shift_collapse(recv)));
        }
        // Left-shift DoS cap. Estimate the result's storage from
        // est_bits = recv_bits + shift_mag, rounded up to u64 limbs
        // plus the same 32-byte allocator header used by
        // `try_bigint_pow`'s estimator. Honour `max_value_bytes`
        // with the same 1 MB fallback as the other bignum guards.
        if actual_left {
            const BIGINT_HEADER_BYTES: u64 = 32;
            let est_bits = recv_bits.saturating_add(shift_mag);
            let est_limbs = est_bits.saturating_add(63) / 64;
            let est_bytes = est_limbs.saturating_mul(8).saturating_add(BIGINT_HEADER_BYTES);
            let cap = self.max_value_bytes.unwrap_or(1 << 20) as u64;
            if est_bytes > cap {
                return Err(self.trap(RubyError::ResourceExhausted {
                    msg: format!(
                        "integer shift result ~{} bytes > cap {}",
                        est_bytes, cap,
                    ),
                }));
            }
        }
        // Apply the shift in the borrow scope, then drop before
        // demote-on-fit. usize::try_from is mostly a no-op on
        // 64-bit; on 32-bit it would trap on shift counts > 4 GB
        // which the cap above already excludes for any sane
        // max_value_bytes.
        let result = {
            let r = match self.as_bigint_ref(recv) {
                Some(v) => v,
                None => return Ok(None),
            };
            let usz: usize = match usize::try_from(shift_mag) {
                Ok(v) => v,
                Err(_) => {
                    return Err(self.trap(RubyError::ResourceExhausted {
                        msg: format!("integer shift count {} exceeds usize::MAX", shift_mag),
                    }));
                }
            };
            if actual_left { (&*r) << usz } else { (&*r) >> usz }
        };
        Ok(Some(self.bigint_to_value(result)?))
    }

    /// Collapse result for a right-shift that consumes all bits.
    /// Non-negative recv → 0; negative recv → -1 (two's-complement
    /// sign extension). Used by the early-exit in
    /// `try_bigint_bit_shift` to avoid allocating a giant BigInt
    /// just to immediately shift it down to a constant.
    fn bit_shift_collapse(&self, recv: &Value) -> Value {
        let neg = match recv {
            Value::Int(n) => *n < 0,
            Value::BigInt(id) => self.heap.bigint(*id).sign() == num_bigint::Sign::Minus,
            _ => false,
        };
        Value::Int(if neg { -1 } else { 0 })
    }

    /// `Integer#pow(exp[, mod])`. 1-arg form is exactly `recv ** exp`
    /// — delegated to `try_bigint_pow`. 2-arg form is modular
    /// exponentiation: computes `(recv ** exp) mod modulus` without
    /// materialising the intermediate (so the DoS cap that bounds
    /// the plain `**` path is unnecessary here — the result is
    /// already bounded by `|modulus|`).
    ///
    /// CRuby semantics for the 2-arg form:
    /// - `modulus == 0` → ZeroDivisionError.
    /// - `exp < 0` → RangeError (modular inverse may not exist; we
    ///   don't compute it).
    /// - Otherwise the result follows Ruby's floor-mod convention
    ///   (same sign as `modulus`). `num_bigint::BigInt::modpow`
    ///   already returns a value with the same sign as the modulus,
    ///   matching this convention exactly — no post-adjustment.
    /// - `exp` and `modulus` must both be Integer (Int / BigInt);
    ///   Float / String etc. raise TypeError.
    pub(crate) fn try_bigint_pow_method(
        &mut self,
        recv: &Value,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::Sign;
        // 1-arg form ≡ `recv ** exp`. Reuse try_bigint_pow's full
        // shape handling (Float exp, negative exp, BigInt exp,
        // DoS cap, identity short-circuits, ZeroDivisionError on
        // 0**-n, etc.). Non-numeric exponents (String, Symbol,
        // nil, …) raise TypeError matching CRuby — `try_bigint_pow`
        // would otherwise decline (`Ok(None)`) and dispatch would
        // surface NoMethodError, which is the wrong error class.
        // Mirrors the Int-receiver guard in numeric.rs::pow.
        if args.len() == 1 {
            let arg = &args[0];
            let acceptable = matches!(arg, Value::Int(_) | Value::Float(_) | Value::BigInt(_));
            if !acceptable {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "{} can't be coerced into Integer",
                        crate::vm::numeric::type_name_for_coerce(arg),
                    ),
                }));
            }
            return self.try_bigint_pow(recv, arg);
        }
        // 2-arg form: pow(exp, mod). Validate shapes first using
        // immutable borrows (no clones). The error paths short-
        // circuit before the modpow allocation; the success path
        // borrows the three BigInts via `as_bigint_ref` (Cow) and
        // runs `modpow` inside the borrow scope so BigInt operands
        // don't pay an O(n) clone before the computation.
        //
        // All Cow-dependent work (shape checks, sign reads, modpow)
        // runs inside one labelled block so each operand is borrowed
        // exactly once. Int operands still pay one `BigInt::from(n)`
        // alloc per `as_bigint_ref` call (unavoidable); BigInt
        // operands stay as `Cow::Borrowed` (no clone). The block
        // exits with Ok(Some(result)) / Ok(None) (decline) / Err
        // (trap). Trap construction (which needs `&mut self`)
        // happens AFTER the borrows expire, when the block returns.
        let pre: Result<Option<num_bigint::BigInt>, RubyError> = 'classify: {
            let Some(base) = self.as_bigint_ref(recv) else {
                // Non-Integer recv → decline so dispatch falls
                // through to NoMethodError (Float etc. have no
                // `.pow(exp, mod)`).
                break 'classify Ok(None);
            };
            // Match CRuby's exact TypeError message text so user
            // code pattern-matching on `e.message` keeps working.
            let Some(exp) = self.as_bigint_ref(&args[0]) else {
                break 'classify Err(RubyError::TypeError {
                    msg: "Integer#pow() 2nd argument not allowed unless a 1st argument is integer".to_string(),
                });
            };
            let Some(modulus) = self.as_bigint_ref(&args[1]) else {
                break 'classify Err(RubyError::TypeError {
                    msg: "Integer#pow() 2nd argument not allowed unless all arguments are integers".to_string(),
                });
            };
            // Sign checks read the held Cows directly (no extra
            // borrow / no extra `BigInt::from(n)` for Int operands).
            if modulus.sign() == Sign::NoSign {
                break 'classify Err(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                });
            }
            if exp.sign() == Sign::Minus {
                break 'classify Err(RubyError::RangeError {
                    msg: "Integer#pow() 1st argument cannot be negative when 2nd argument specified".to_string(),
                });
            }
            // BigInt::modpow returns a value with the same sign as
            // modulus — matches Ruby's floor-mod semantics exactly,
            // no post-adjustment.
            Ok(Some(base.modpow(&exp, &modulus)))
        };
        // Borrows expired with the block. Safe to call &mut self.
        match pre {
            Ok(None) => Ok(None),
            Ok(Some(result)) => Ok(Some(self.bigint_to_value(result)?)),
            Err(err) => Err(self.trap(err)),
        }
    }

    /// `Integer#digits([base = 10])` — array of digits in the given
    /// base, least-significant first. Returns `Some(Value::Array)`
    /// for BigInt receivers; `Ok(None)` for Int receivers (so the
    /// i64 fast path in `vm/dispatch.rs::Integer#digits` runs
    /// instead — keeps small Int×Int#digits off the BigInt
    /// arithmetic path) and for non-Integer recv (lets dispatch
    /// fall through to NoMethodError). Traps:
    /// - Negative receiver → ArgumentError "out of domain"
    ///   (CRuby raises Math::DomainError; the established subset
    ///   pattern uses ArgumentError as the substitute since
    ///   Math::DomainError isn't modelled — same convention as
    ///   the Range #cover? / numeric-out-of-domain arms in
    ///   `Vm::do_call`).
    /// - Base < 0 → ArgumentError "negative radix".
    /// - Base < 2 → ArgumentError "invalid radix N".
    /// - Non-Integer base → TypeError "no implicit conversion of
    ///   X into Integer".
    /// - Result-array estimate exceeds the active cap → trap
    ///   ResourceExhausted before allocation. The cap is
    ///   `Config::max_value_bytes` when set, otherwise a 1 MB
    ///   safety ceiling (same fallback as `try_bigint_pow`'s
    ///   estimator — so hostless / default-config users still get
    ///   a bound on this allocation path). The bound itself uses
    ///   an integer approximation:
    ///   `est_count = floor((recv_bits - 1) / log2_lower) + 1`,
    ///   where `log2_lower = max(1, base.bits() - 1)` is a lower
    ///   bound on `log2(base)` (since
    ///   `base >= 2^(base.bits() - 1)`). Dividing by a smaller log
    ///   gives a safe upper bound on the count without floating-
    ///   point. Multiply by `size_of::<Value>()` for bytes.
    pub(crate) fn try_integer_digits(
        &mut self,
        recv: &Value,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::{BigInt, Sign};
        // BigInt receivers only — Int receivers route through
        // `dispatch.rs`'s existing i64 fast path (no BigInt
        // arithmetic for small Int×Int). Non-Integer recv: decline
        // so dispatch can fall through to NoMethodError.
        // Returning `Ok(None)` for Int recv lets `bigint_primitive`
        // continue through the arity guard (which still fires for
        // `args.len() > 1` regardless of recv type) and then
        // through to `dispatch.rs`'s Int#digits handler. The Int
        // fast path now shares error message text with this BigInt
        // path (see the matching dispatch.rs edits).
        let (recv_bits, recv_sign) = match recv {
            Value::BigInt(id) => {
                let b = self.heap.bigint(*id);
                (b.bits(), b.sign())
            }
            _ => return Ok(None),
        };
        // Negative receiver: out of domain.
        if recv_sign == Sign::Minus {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "out of domain".to_string(),
            }));
        }
        // Resolve base. Default 10; reject non-Integer args; reject
        // <2 (with CRuby's two distinct messages).
        let base: BigInt = match args.first() {
            None => BigInt::from(10),
            Some(Value::Int(r)) => {
                if *r < 0 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: "negative radix".to_string(),
                    }));
                }
                if *r < 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("invalid radix {}", r),
                    }));
                }
                BigInt::from(*r)
            }
            Some(Value::BigInt(id)) => {
                let b = self.heap.bigint(*id);
                // BigInt radix is always > i64::MAX > 1, so >= 2.
                // Negative BigInt radix would have been demoted to
                // Int by bigint_to_value if it fit. For BigInts
                // outside i64 range, we know sign from b.sign().
                if b.sign() == Sign::Minus {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: "negative radix".to_string(),
                    }));
                }
                b.clone()
            }
            Some(other) => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        crate::vm::numeric::type_name_for_coerce(other),
                    ),
                }));
            }
        };
        // Pre-estimate array length to avoid building a multi-GB
        // Vec on hostile input. The exact digit count is
        // `floor(log_base(recv)) + 1`; rewriting via base-2:
        // `floor((recv_bits - 1) / log2(base)) + 1` (since
        // `log2(recv) ≈ recv_bits - 1` for recv > 0). We use the
        // integer lower bound `log2(base) >= base.bits() - 1`
        // (since `base >= 2^(base.bits() - 1)`); dividing by a
        // smaller log gives a safe upper bound on the count
        // without floating-point.
        //
        // Base = 2:   log2_lower = 1, est = recv_bits (exact).
        // Base = 10:  log2_lower = 3, est ≈ recv_bits/3 + 1.
        // Base = 256: log2_lower = 8, est ≈ recv_bits/8 + 1.
        //
        // recv_bits == 0 case (`Sign::NoSign`) sets est_count = 1
        // explicitly below — the cap check still runs but is
        // trivially satisfied for any non-pathological cap (a
        // single-Value array is `size_of::<Value>()` bytes).
        const VALUE_BYTES: u64 = std::mem::size_of::<Value>() as u64;
        let log2_lower: u64 = base.bits().saturating_sub(1).max(1);
        let est_count: u64 = if recv_bits == 0 {
            1
        } else {
            // ceil-form: `(recv_bits - 1) / log2_lower + 1`.
            // Previous form `recv_bits / log2_lower + 1`
            // overcounted by 1 for base = 2 (recv_bits = N gave
            // est = N+1 instead of N) and similarly off-by-one
            // for any base where `recv_bits % log2_lower == 0`.
            (recv_bits - 1) / log2_lower + 1
        };
        let est_bytes: u64 = est_count.saturating_mul(VALUE_BYTES);
        let cap = self.max_value_bytes.unwrap_or(1 << 20) as u64;
        if est_bytes > cap {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!(
                    "Integer#digits would need ~{} bytes, exceeding cap {}",
                    est_bytes, cap
                ),
            }));
        }
        // Build the digit array. Clone the heap BigInt as the
        // working value; we mutate `n` via repeated `n = &n / &base`
        // in the loop below, so an owned BigInt is required.
        let mut n: BigInt = match recv {
            Value::BigInt(id) => self.heap.bigint(*id).clone(),
            _ => unreachable!("recv shape narrowed to BigInt at fn entry"),
        };
        // GC rooting: every `bigint_to_value` call below invokes
        // `maybe_gc()`. For Int radix (the common case) rem is
        // always small and demotes to `Value::Int`, no rooting
        // needed. For BigInt radix, rem can be a heap-backed
        // `Value::BigInt(id)`; without pinning, an iteration N+1
        // GC could sweep the BigInts pushed during 1..N before
        // the Array allocation roots them, leaving dangling
        // ObjIds in the returned Array. Pin every Value::BigInt
        // digit as it's produced; the PinGuard drops after the
        // Array is allocated (heap.alloc itself triggers the
        // final GC walk, which now sees both the pinned digits
        // and the freshly-allocated Array as reachable).
        let mut guard = PinGuard::new(self);
        // Pre-reserve up to `est_count` (already capped against
        // `max_value_bytes` above, so safe to truncate to usize).
        // Avoids the geometric reallocation pattern Vec would
        // otherwise use during the loop on large digit arrays.
        let cap_count = est_count.min(usize::MAX as u64) as usize;
        let mut digits: Vec<Value> = Vec::with_capacity(cap_count);
        if recv_sign == Sign::NoSign {
            digits.push(Value::Int(0));
        } else {
            use num_integer::Integer;
            while n.sign() != Sign::NoSign {
                // `div_rem` returns (quotient, remainder) in a
                // single division step — half the per-iteration
                // BigInt work vs separate `&n / &base` + `&n %
                // &base`. `Integer` is impl'd for `BigInt` by
                // num-bigint. rem fits i64 when base fits i64;
                // for BigInt base we go through bigint_to_value
                // so the demote-on-fit funnel handles either.
                let (quot, rem) = n.div_rem(&base);
                n = quot;
                let digit_val = guard.vm.bigint_to_value(rem)?;
                if matches!(digit_val, Value::BigInt(_)) {
                    guard.pin(digit_val.clone());
                }
                digits.push(digit_val);
            }
        }
        guard.vm.maybe_gc();
        guard.vm.check_alloc()?;
        let arr_id = guard.vm.heap.alloc(crate::heap::HeapObj::Array(digits));
        // `guard` drops here, unpinning the digits — but the
        // Array now holds them as roots, so the next GC walk
        // still sees them as reachable.
        Ok(Some(Value::Array(arr_id)))
    }
}

/// BigInt method dispatch — covers the calls `primitive_call`
/// can't satisfy (it's stateless; BigInt needs heap access for
/// the decimal-string read). Hooked from `Vm::do_call` after
/// the regular primitive paths. Phase A surface:
///
/// - `to_s` / `inspect` — heap-read paths handled inline.
/// - Operator method-call shape (`big.+(x)`, `big.send(:==, y)`)
///   — name parsed by `BinOpKind::from_op_name`, then routed
///   through `try_bigint_binop` so the answer matches the
///   `Op::BinOp` path exactly.
///
/// The expression-form arithmetic (`big + 1` compiled as
/// `Op::BinOp`) still goes through `try_bigint_binop` directly
/// without entering this helper.
#[cfg(feature = "bignum")]
impl Vm {
    pub(crate) fn bigint_primitive(
        &mut self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        // Entry conditions, in order of precedence:
        // 1. `**` exponentiation fires for ANY Int/BigInt operand
        //    combo, including Int×Int — numeric_call's `**` arm
        //    declines on i64 overflow so we get the chance to
        //    promote here (`2 ** 100`). Handled before the guard
        //    below so the Int×Int overflow case isn't filtered out.
        // 2. Unary `-@` / `+@` / `abs` — fires for BigInt recv OR
        //    Int(i64::MIN) recv. numeric_call declines on i64::MIN
        //    under `bignum` so this arm can promote to the
        //    BigInt 2^63. Also sits ahead of the recv-or-arg-is-
        //    BigInt guard for the same reason as `**`.
        // 3. `pow(exp[, mod])` method form — 1-arg aliases `**`;
        //    2-arg routes through `BigInt::modpow` for modular
        //    exponentiation. Fires for any Integer recv (including
        //    Int×Int×Int), so it sits ahead of the recv-or-arg
        //    guard. No DoS cap on the 2-arg form: modpow never
        //    materialises the intermediate, and the result is
        //    bounded by |mod|.
        // 4. `digits([base])` — produces a `Value::Array` so it
        //    needs `&mut Vm` (can't live in stateless numeric_call).
        //    Two sub-checks fire ahead of the dispatch in CRuby
        //    precedence order: negative recv → ArgumentError "out
        //    of domain" (Math::DomainError substitute), then arity
        //    guard for >1 args → ArgumentError. The dispatch
        //    itself narrows the helper to BigInt receivers; Int
        //    receivers fall through to dispatch.rs's i64 fast
        //    path. Sits ahead of the recv-or-arg guard so Int
        //    receivers don't get filtered out.
        // 5. Recv is BigInt: covers `big.to_s`, `big.+(x)`, etc.
        // 6. Recv is Int AND a BigInt is among args: covers the
        //    inverse-receiver operator method-call shape
        //    `1.+(2**63)`, which goes through the Int-side
        //    dispatch path and would otherwise miss BigInt
        //    arithmetic entirely (the expression form `1 + big`
        //    works because Op::BinOp already routes via
        //    try_bigint_binop on either-operand-is-BigInt).
        //
        // When adding a new entry path that needs to fire for
        // Int receivers without a BigInt arg (e.g. another auto-
        // promotion shape), place it BEFORE the
        // `recv_is_bigint || arg_is_bigint` guard below.
        //
        // Fall through to the rest of bigint_primitive when
        // `try_bigint_pow` declines. Decline cases narrow to
        // Int recv × Int (positive) exp where `numeric_call`
        // already produced a value, or operand shapes that aren't
        // integer at all (the latter never reaches bigint_primitive
        // in practice — `primitive_call`'s Int arm would have
        // matched first). Float and negative-Int exponents are
        // handled inside `try_bigint_pow` itself for BigInt-
        // flavoured operands; Int×Int Float/neg-exp is owned by
        // `numeric_call` before we get here.
        if args.len() == 1 && name == "**"
            && let Some(v) = self.try_bigint_pow(recv, &args[0])?
        {
            return Ok(Some(v));
        }
        // Cond 2 — see entry-conditions doc above. `~` joins the
        // arity-0 unary group: numeric.rs's `(Int, "~", [])` arm
        // handles Int receivers (no promotion — `!i64::MIN` fits
        // in i64), but BigInt receivers need the two's-complement
        // `-(b + 1)` form via try_bigint_unary. `succ`/`next`/`pred`
        // join the same group: numeric.rs handles the in-range Int
        // path but declines at the i64::MAX/MIN boundaries so this
        // hook promotes (i64::MAX.succ → BigInt(2^63),
        // i64::MIN.pred → BigInt(-(2^63+1))), plus the BigInt-recv
        // case for any BigInt.succ / BigInt.pred call.
        if args.is_empty() && matches!(name, "-@" | "+@" | "abs" | "~" | "succ" | "next" | "pred")
            && let Some(v) = self.try_bigint_unary(recv, name)?
        {
            return Ok(Some(v));
        }
        // Bitwise binary `&` / `|` / `^` on Integer × Integer where
        // at least one operand is BigInt. numeric.rs's `(Int, op,
        // [Int])` arm handles the pure Int × Int case; this fires
        // for the mixed shapes (`big & 0xff`, `5 & (2**100)`,
        // `big & big`). Sits ahead of the recv-or-arg guard below
        // because the Int-recv-with-BigInt-arg shape is exactly
        // what the guard is gating in.
        if args.len() == 1 && matches!(name, "&" | "|" | "^")
            && let Some(v) = self.try_bigint_bit_binop(recv, name, &args[0])?
        {
            return Ok(Some(v));
        }
        // Bitwise shifts `<<` / `>>`. Fires for any Integer recv +
        // any Integer arg, including the Int×Int overflow path
        // (`1 << 64`) that numeric.rs declined under bignum. Sits
        // ahead of the recv-or-arg guard for the same reason as
        // `**`: the Int×Int-with-overflow shape isn't gated by
        // recv-or-arg-is-BigInt and needs an explicit path.
        if args.len() == 1 && matches!(name, "<<" | ">>")
            && let Some(v) = self.try_bigint_bit_shift(recv, name, &args[0])?
        {
            return Ok(Some(v));
        }
        // `pow(exp[, mod])` method form — 1-arg is an alias for `**`,
        // 2-arg is modular exponentiation via BigInt::modpow. Fires
        // ahead of the recv-or-arg guard so Int×Int×Int shapes work
        // too. No DoS cap needed for the 2-arg form: modpow never
        // materialises the intermediate, and the result is bounded
        // by |mod|.
        if name == "pow" && (args.len() == 1 || args.len() == 2)
            && let Some(v) = self.try_bigint_pow_method(recv, args)?
        {
            return Ok(Some(v));
        }
        // CRuby precedence: a negative receiver for `Integer#digits`
        // raises `Math::DomainError: out of domain` BEFORE any
        // arity / base validation. Match that ordering by checking
        // recv sign first, ahead of the arity guard and digits
        // dispatch below. The Math::DomainError substitute is
        // ArgumentError (same convention as other numeric-out-of-
        // domain arms in Vm::do_call). Concrete examples (CRuby vs
        // pre-fix rubyrs): `(-5).digits(10, 2)` should raise
        // "out of domain", not the arity error;
        // `(-5).digits("foo")` should raise "out of domain", not
        // a TypeError on the base; etc.
        if name == "digits" {
            let neg_recv = match recv {
                Value::Int(n) => *n < 0,
                Value::BigInt(id) => self.heap.bigint(*id).sign() == num_bigint::Sign::Minus,
                _ => false,
            };
            if neg_recv {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "out of domain".to_string(),
                }));
            }
        }
        // `Integer#digits` produces a `Value::Array`, which needs
        // heap allocation — can't live in stateless `numeric_call`.
        // Fires for ANY Int/BigInt receiver (recv-side check is in
        // the helper, which now narrows to BigInt only — Int
        // receivers continue through and hit dispatch.rs's i64
        // fast path). Sits ahead of the recv-or-arg guard so Int
        // receivers don't get filtered out. By the time we reach
        // here, `recv` is non-negative (the precedence check above
        // already trapped the negative case).
        if name == "digits" && (args.is_empty() || args.len() == 1)
            && let Some(v) = self.try_integer_digits(recv, args)?
        {
            return Ok(Some(v));
        }
        // Arity guard for `digits` — CRuby raises ArgumentError
        // ("wrong number of arguments (given N, expected 0..1)")
        // for arities outside {0, 1}. Without this, `5.digits(10, 2)`
        // falls through to NoMethodError despite `respond_to?(:digits)`
        // being true. Fires for any Int/BigInt receiver.
        if name == "digits"
            && matches!(recv, Value::Int(_) | Value::BigInt(_))
            && args.len() > 1
        {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len(),
                ),
            }));
        }
        // Arity guard for BigInt-receiver `pow` — numeric.rs's
        // arity guard only catches Int×*, so `big.pow` /
        // `big.pow(1,2,3)` would otherwise fall through to
        // NoMethodError despite `respond_to?(:pow)` being true.
        // Match CRuby's exact ArgumentError message text.
        if name == "pow" && matches!(recv, Value::BigInt(_)) && args.len() != 2 && args.len() != 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 1..2)",
                    args.len(),
                ),
            }));
        }
        // `Integer#to_s(radix)` — 1-arg form for BigInt receivers.
        // Symmetric with the Int side (numeric_call's `to_s(radix)`
        // arm). Validates radix ∈ 2..=36, then defers to
        // num_bigint's `to_str_radix` (which handles negative sign
        // and digits >= 10 as lowercase).
        //
        // PRE-allocation cap check: `to_str_radix(2)` on a 1M-bit
        // BigInt allocates ~1 MB before we get a chance to inspect
        // its length. Estimate the rendered length first via
        // `bits()` and trap before the alloc. See
        // [`Vm::check_bigint_to_s_cap`] for the bound — it
        // delegates to [`bignum_digits_upper_bound`], which uses a
        // scaled-integer `floor(log2(radix) * 64)` lower bound on
        // `log2(base)` (power-of-two exact path + f64 fallback for
        // radices 3/5/6/7/…/36) plus a 1-byte sign accounting.
        // Mirrored by the 0-arg arm so both paths share the
        // protection.
        // Arity guards for `BigInt#to_s` and `BigInt#inspect` —
        // sibling to numeric.rs's guards on the Int recv side.
        // `to_s` accepts 0..1 (0-arg via the empty-args branch
        // below, 1-arg via the radix arm); `inspect` accepts
        // exactly 0. Without these guards 2+-arg / any-arg
        // `inspect` falls through to NoMethodError despite
        // `respond_to?` returning true.
        if name == "to_s" && args.len() > 1
            && matches!(recv, Value::BigInt(_))
        {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len(),
                ),
            }));
        }
        if name == "inspect" && !args.is_empty()
            && matches!(recv, Value::BigInt(_))
        {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len(),
                ),
            }));
        }
        if name == "to_s" && args.len() == 1
            && let Value::BigInt(id) = recv
        {
            let radix: u32 = match &args[0] {
                Value::Int(r) => {
                    if !(2..=36).contains(r) {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("invalid radix {}", r),
                        }));
                    }
                    *r as u32
                }
                // Canonical-BigInt invariant: any Value::BigInt is
                // out of i64 range, so it can never be in 2..=36. But
                // BigInt IS an Integer — matching it via `other`
                // produces TypeError "no implicit conversion of
                // Integer into Integer" (self-referential nonsense).
                // CRuby raises `RangeError: bignum too big to convert
                // into 'long'` for `big.to_s(2**100)`; match that.
                Value::BigInt(_) => {
                    return Err(self.trap(RubyError::RangeError {
                        msg: "bignum too big to convert into `long'".to_string(),
                    }));
                }
                other => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into Integer",
                            crate::vm::numeric::type_name_for_coerce(other),
                        ),
                    }));
                }
            };
            self.check_bigint_to_s_cap(*id, radix)?;
            let s = self.heap.bigint(*id).to_str_radix(radix);
            return Ok(Some(Value::new_str(s)));
        }
        let recv_is_bigint = matches!(recv, Value::BigInt(_));
        let arg_is_bigint = args.iter().any(|a| matches!(a, Value::BigInt(_)));
        if !recv_is_bigint && !arg_is_bigint {
            return Ok(None);
        }
        // Phase A heap-read operations — only meaningful on a BigInt
        // receiver (Int#to_s already handled by numeric_call).
        if recv_is_bigint && args.is_empty()
            && let Value::BigInt(id) = recv
        {
            use num_bigint::Sign;
            let b = self.heap.bigint(*id);
            match name {
                    "to_s" | "inspect" => {
                        // BigInt decimal can grow arbitrarily (consider
                        // `n = 2 ** 1_000_000; n.to_s`), so the
                        // String materialised here must obey the same
                        // `Config::max_value_bytes` cap that other
                        // primitive_call arms enforce. Pre-allocation
                        // estimate via `bits()` traps BEFORE the
                        // `to_string()` call — otherwise the host
                        // could OOM on a 1 MB string before we get a
                        // chance to check `s.len()`. Shared helper
                        // with the 1-arg `to_s(radix)` arm above.
                        // `check_bigint_to_s_cap` takes `&self`, so
                        // the heap borrow on `b` can stay live across
                        // the call without an explicit drop dance.
                        self.check_bigint_to_s_cap(*id, 10)?;
                        let s = b.to_string();
                        return Ok(Some(Value::new_str(s)));
                    }
                    // Pure read-only predicates — fit cleanly in
                    // Phase A because they don't need heap mutation.
                    // (CRuby Integer uniformity: any predicate the
                    // i64 Int receiver supports should work on the
                    // unified Integer class regardless of magnitude.)
                    "to_i" => return Ok(Some(recv.clone())),
                    "to_f" => {
                        // Lossy at extreme magnitudes; matches CRuby.
                        return Ok(Some(Value::Float(
                            b.to_string().parse::<f64>().unwrap_or(f64::INFINITY)
                        )));
                    }
                    "zero?" => return Ok(Some(Value::Bool(b.sign() == Sign::NoSign))),
                    "positive?" => return Ok(Some(Value::Bool(b.sign() == Sign::Plus))),
                    "negative?" => return Ok(Some(Value::Bool(b.sign() == Sign::Minus))),
                    "even?" => return Ok(Some(Value::Bool((b & num_bigint::BigInt::from(1)) == num_bigint::BigInt::from(0)))),
                    "odd?" => return Ok(Some(Value::Bool((b & num_bigint::BigInt::from(1)) != num_bigint::BigInt::from(0)))),
                    // `Integer#bit_length` on BigInt. For non-
                    // negatives: bit position of the highest set
                    // bit (== `bits()`). For negatives: CRuby's
                    // two's-complement convention gives the bit
                    // position of the highest 0-bit, equivalent to
                    // `bit_length(~n) = bit_length(-n - 1) =
                    // bits(|n| - 1)`. `bits()` returns u64; cap at
                    // i64::MAX in case of pathological 2^63-bit
                    // BigInts (unreachable under our DoS caps, but
                    // future-proofs the cast).
                    "bit_length" => {
                        let bits: u64 = match b.sign() {
                            Sign::NoSign => 0,
                            Sign::Plus => b.bits(),
                            Sign::Minus => {
                                // |n| - 1 in BigInt land, then bit count.
                                (b.magnitude() - 1u32).bits()
                            }
                        };
                        let n = i64::try_from(bits).unwrap_or(i64::MAX);
                        return Ok(Some(Value::Int(n)));
                    }
                    _ => {}
            }
        }
        // Operator method-call shape — `big.+(1)`, `1.+(big)`,
        // `big.send(:==, x)`. Route through `try_bigint_binop` so
        // the answer matches the `Op::BinOp` path exactly (same
        // arithmetic / floor-div semantics, same comparison Bool,
        // same overflow-promotion-then-demote rule).
        if args.len() == 1
            && let Some(kind) = crate::bytecode::BinOpKind::from_op_name(name)
            && let Some(v) = self.try_bigint_binop(kind, recv, &args[0])?
        {
            return Ok(Some(v));
        }
        // `Integer#eql?(other)` on BigInt receiver — type-strict
        // equality. Only true when `other` is also a BigInt AND the
        // two magnitudes match (separately-allocated BigInts of
        // equal value pass). The canonical-BigInt invariant
        // guarantees Int and BigInt can't share a value, so
        // `(2**64).eql?(some_int)` is always false. Mirrors
        // numeric.rs's Int-receiver arm.
        if args.len() == 1 && name == "eql?"
            && let Value::BigInt(id) = recv
        {
            let same = match &args[0] {
                Value::BigInt(other_id) => {
                    *id == *other_id || self.heap.bigint(*id) == self.heap.bigint(*other_id)
                }
                _ => false,
            };
            return Ok(Some(Value::Bool(same)));
        }
        // `Integer#hash` on BigInt receiver — same domain tag as
        // numeric.rs's Int arm so the Integer hash domain stays
        // disjoint from Float's. Hashes via [`fnv1a_64`] (see
        // numeric.rs's Int arm for the FNV-1a vs DefaultHasher
        // rationale: cross-rustc-stable digest).
        //
        // `to_signed_bytes_le` returns the two's-complement
        // representation, so positive and negative are
        // distinguished and same-value pairs across allocs hash
        // identically.
        //
        // The Hash collection itself uses linear scan via
        // ruby_eq (no hashing) but this method exists for the
        // user-facing protocol and for pure-Ruby code that does
        // its own bookkeeping.
        if args.is_empty() && name == "hash"
            && let Value::BigInt(id) = recv
        {
            let b = self.heap.bigint(*id);
            let mag_bytes = b.to_signed_bytes_le();
            let mut bytes = Vec::with_capacity(1 + mag_bytes.len());
            bytes.push(crate::vm::numeric::INT_HASH_TAG);
            bytes.extend_from_slice(&mag_bytes);
            return Ok(Some(Value::Int(crate::vm::numeric::fnv1a_64(&bytes) as i64)));
        }
        // `<=>` — universal three-way comparison. Not in BinOpKind
        // (it returns Int not Bool, so the BinOp machinery doesn't
        // model it), so we handle it here for Int/BigInt operands.
        // CRuby's Integer#<=> returns nil for incomparable rhs
        // (e.g. `1 <=> "foo"`); we do the same by deferring to the
        // numeric_call path via None.
        if args.len() == 1 && name == "<=>" {
            // BigInt × Float (either direction): use the lossless
            // path so e.g. `(2**64 + 1) <=> (2**64).to_f` returns
            // 1 instead of 0. NaN → nil; ±inf → ∓1.
            if let Some((big_id, float_v, big_is_lhs)) = match (recv, &args[0]) {
                (Value::BigInt(id), Value::Float(f)) => Some((*id, *f, true)),
                (Value::Float(f), Value::BigInt(id)) => Some((*id, *f, false)),
                _ => None,
            } {
                let cmp = bigint_cmp_float_lossless(
                    self.heap.bigint(big_id),
                    float_v,
                );
                let cmp = cmp.map(|o| if big_is_lhs { o } else { o.reverse() });
                let v = match cmp {
                    None => Value::Nil,
                    Some(std::cmp::Ordering::Less) => Value::Int(-1),
                    Some(std::cmp::Ordering::Equal) => Value::Int(0),
                    Some(std::cmp::Ordering::Greater) => Value::Int(1),
                };
                return Ok(Some(v));
            }
            if let (Some(ax), Some(bx)) = (
                self.as_bigint_ref(recv),
                self.as_bigint_ref(&args[0]),
            ) {
                let ord = ax.cmp(&bx);
                let n = match ord {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                return Ok(Some(Value::Int(n)));
            }
        }
        Ok(None)
    }

    /// Pre-allocation cap check for `BigInt#to_s` / `to_s(radix)`.
    /// `BigInt::to_str_radix(2)` on a 10M-bit input allocates a
    /// 10 MB+ string in one go — without this check the host can
    /// OOM (or hit the allocator's panic-on-fail path) before we
    /// get a chance to inspect `s.len()`. Estimate the rendered
    /// length from the BigInt's bit count + per-digit bit yield:
    ///   `ceil(bits * SCALE / log2_per_digit_scaled) + sign_byte`
    /// where `log2_per_digit_scaled = floor(log2(radix) * SCALE)`
    /// is a tight integer lower bound on `log2(base)` (see
    /// [`bignum_log2_per_digit_scaled`]). Earlier revisions used
    /// the integer `floor(log2(radix))` which over-estimated the
    /// digit count by ~10% for radix 10 and ~38% for radix 3 —
    /// enough to false-trap rendered values that would actually
    /// fit under a tightly-configured `max_value_bytes`.
    /// `sign_byte = 1` iff the BigInt is negative, else 0.
    /// `max_value_bytes` falls back to the same 1 MB safety
    /// ceiling that `try_bigint_pow` uses when no host cap is
    /// configured.
    #[cfg(feature = "bignum")]
    pub(crate) fn check_bigint_to_s_cap(&self, id: crate::value::ObjId, radix: u32) -> Result<(), crate::error::Trap> {
        use num_bigint::Sign;
        let b = self.heap.bigint(id);
        let bits = b.bits();
        let sign_byte: u64 = if b.sign() == Sign::Minus { 1 } else { 0 };
        let digits_est = bignum_digits_upper_bound(bits, radix);
        let est: u64 = digits_est.saturating_add(sign_byte);
        let cap = self.max_value_bytes.unwrap_or(1 << 20) as u64;
        if est > cap {
            return Err(self.trap(RubyError::ResourceExhausted {
                msg: format!("value size ~{} bytes > cap {}", est, cap),
            }));
        }
        Ok(())
    }
}

/// Returns `floor(log2(radix) * SCALE)` as an integer lower
/// bound on `log2(radix)`. Shared by the `to_s(radix)` cap in
/// [`Vm::check_bigint_to_s_cap`] and the `'%b/%o/%x' % bignum`
/// pre-allocation cap in [`super::sprintf::format_radix_any`].
///
/// `SCALE = 64` keeps the table-free f64 computation within
/// f64's ~15.95 decimal-digit precision for the 2..=36 radix
/// domain we care about (the largest value here is
/// `floor(log2(36) * 64) = 330` which rounds exactly) while
/// giving enough resolution that the resulting digit estimate
/// is within +1 of the true value across the supported radix
/// range. Power-of-two radices short-circuit to an exact
/// integer multiply to sidestep f64 rounding entirely.
#[cfg(feature = "bignum")]
pub(crate) fn bignum_log2_per_digit_scaled(radix: u32) -> u64 {
    const SCALE: u64 = 64;
    if radix >= 2 && radix.is_power_of_two() {
        return (radix.trailing_zeros() as u64) * SCALE;
    }
    let v = (radix as f64).log2() * SCALE as f64;
    (v.floor() as u64).max(1)
}

/// Returns an upper bound on the number of characters
/// `BigInt::to_str_radix(radix)` produces for a value with the
/// given `bits()`. Clamps to ≥ 1 so that `BigInt(0)` (whose
/// `bits()` is 0 but whose rendered form is `"0"`) costs at
/// least one byte against `max_value_bytes` — callers compare
/// `est > cap`, so `Some(0)` still traps even on the zero value
/// (consistent with the pre-tightening behaviour). Uses `u128`
/// intermediates to avoid overflow when `bits` approaches
/// `u64::MAX / SCALE` (≈ 2^58 bits ≈ 32 PB).
#[cfg(feature = "bignum")]
pub(crate) fn bignum_digits_upper_bound(bits: u64, radix: u32) -> u64 {
    const SCALE: u128 = 64;
    let log2_scaled = bignum_log2_per_digit_scaled(radix) as u128;
    let scaled_bits = (bits as u128).saturating_mul(SCALE);
    let digits_est = scaled_bits.div_ceil(log2_scaled);
    let digits_est_u64 = if digits_est > u64::MAX as u128 {
        u64::MAX
    } else {
        digits_est as u64
    };
    digits_est_u64.max(1)
}

/// BigInt arithmetic surface — shared by the i64-overflow promotion
/// path in `Op::BinOp` / `Op::BinOpInt` and by the cold-path
/// dispatch for already-BigInt operands. Cfg-gated on `bignum`
/// alongside the `Value::BigInt` variant. ADR 0018 BigInt placement.
#[cfg(feature = "bignum")]
impl Vm {
    /// Wraps a `BigInt` as a `Value`, demoting to `Value::Int` if
    /// it fits in i64. Every arithmetic path that can produce a
    /// BigInt result funnels through here so that
    /// post-overflow-shrink results land as `Int(n)` (matching
    /// CRuby's `Fixnum == Bignum` equality on the natural Int
    /// path) rather than `BigInt(n)` with a different ObjId per
    /// computation.
    pub(crate) fn bigint_to_value(&mut self, b: num_bigint::BigInt) -> Result<Value, Trap> {
        if let Ok(n) = i64::try_from(&b) {
            return Ok(Value::Int(n));
        }
        self.maybe_gc();
        self.check_alloc()?;
        Ok(Value::BigInt(self.heap.alloc(crate::heap::HeapObj::BigInt(b))))
    }

    /// Resolves an Int / BigInt operand to its `num_bigint::BigInt`
    /// form. Non-integer Values return `None` so the caller can
    /// fall through to the regular dispatch path (e.g. method-missing
    /// for `String + BigInt`). Owned form — clones the heap-side
    /// BigInt because the caller will consume it (arithmetic moves).
    /// For comparisons / read-only paths prefer `as_bigint_ref`.
    pub(crate) fn as_bigint(&self, v: &Value) -> Option<num_bigint::BigInt> {
        match v {
            Value::Int(n) => Some(num_bigint::BigInt::from(*n)),
            Value::BigInt(id) => Some(self.heap.bigint(*id).clone()),
            _ => None,
        }
    }

    /// Borrowed form of `as_bigint`. BigInt operands flow as
    /// `Cow::Borrowed(&BigInt)` (no clone); Int operands wrap
    /// in `Cow::Owned(BigInt::from(n))` because we have to
    /// materialise the conversion somewhere. The borrowed result
    /// is tied to `&self.heap`, so the caller must drop it before
    /// any `&mut self` calls. Used by `try_bigint_binop` for
    /// comparison ops where both sides run from refs.
    pub(crate) fn as_bigint_ref<'a>(
        &'a self,
        v: &'a Value,
    ) -> Option<std::borrow::Cow<'a, num_bigint::BigInt>> {
        use std::borrow::Cow;
        match v {
            Value::Int(n) => Some(Cow::Owned(num_bigint::BigInt::from(*n))),
            Value::BigInt(id) => Some(Cow::Borrowed(self.heap.bigint(*id))),
            _ => None,
        }
    }

    /// Performs Add/Sub/Mul/Div/Mod on Int/BigInt operands in
    /// arbitrary precision. Returns `None` for operands that
    /// aren't integers (the caller dispatches normally). Div/Mod
    /// by zero returns `Some(Err(...))` for the trap.
    pub(crate) fn bigint_arith(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        a: &Value,
        b: &Value,
    ) -> Option<Result<Value, Trap>> {
        use crate::bytecode::BinOpKind;
        use num_bigint::BigInt;
        let ax = self.as_bigint(a)?;
        let bx = self.as_bigint(b)?;
        let result: BigInt = match kind {
            BinOpKind::Add => ax + bx,
            BinOpKind::Sub => ax - bx,
            BinOpKind::Mul => ax * bx,
            BinOpKind::Div | BinOpKind::Mod => {
                use num_bigint::Sign;
                if bx.sign() == Sign::NoSign {
                    return Some(Err(self.trap(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    })));
                }
                // CRuby's Integer#/ floors toward negative infinity
                // (BigInt's default `Div` truncates toward zero).
                // Same correction for `%`: result has rhs's sign.
                let (q, r) = (&ax / &bx, &ax % &bx);
                let needs_correction = (r.sign() == Sign::Minus && bx.sign() == Sign::Plus)
                    || (r.sign() == Sign::Plus && bx.sign() == Sign::Minus);
                if matches!(kind, BinOpKind::Div) {
                    if needs_correction { q - 1 } else { q }
                } else {
                    if needs_correction { r + &bx } else { r }
                }
            }
            // Comparison ops are handled inline in
            // `try_bigint_binop` (which returns Bool directly via
            // BigInt's PartialOrd/PartialEq); they never reach this
            // arithmetic match.
            _ => return None,
        };
        Some(self.bigint_to_value(result))
    }
}
