//! Method dispatch and call setup. Mirrors the call-handling
//! machinery CRuby keeps in `vm_eval.c` / `vm_insnhelper.c` —
//! finding the target method on a receiver, pushing a frame,
//! threading args/block through, and routing to host fns or
//! to interpreter bodies.
//!
//! Contents:
//!   - `do_call` / `do_call_block` — the Op::Call entry points
//!     called from the opcode loop.
//!   - `invoke_method` / `invoke_method_with_block` — frame
//!     setup once the target Method has been resolved.
//!   - `invoke_block` — re-enter a captured block.
//!   - `cext_invoke_method` — bridge for C-ext re-entering the
//!     Ruby side via `rb_funcallv`.
//!   - `try_method_missing` — fallback dispatch path when the
//!     name lookup fails.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::intern::SymId;
use crate::value::{Class, Instance, Method, ObjId, Value, Visibility};

#[cfg(any(
    all(feature = "cext", not(target_os = "wasi")),
    feature = "_http_server",
    feature = "_fiber",
    feature = "_json_native",
    feature = "_yaml_native",
    feature = "_liquid_native",
    feature = "_sqlite",
))]
use super::with_vm_ptr_set;
use super::{
    primitive_call, value_cmp_v_heap, vec_nil, visibility_from_name, Frame, HostFnSlot, PinGuard, Vm,
};
use crate::HostCtx;

/// A `(local-slot, value)` binding produced while rooting a block's
/// rest-array / keyword-rest Hash through the GC fence in
/// [`Vm::invoke_block`] (see the combined `PinGuard` block there).
type SlotBinding = Option<(u16, Value)>;

/// A frame's locals storage cell — the shared `Rc<RefCell<Vec<Value>>>`
/// used by `Locals::Shared` frames and the block-locals helpers.
type LocalsCell = Rc<RefCell<Vec<Value>>>;
/// A block frame's `block_writeback`: the outer-scope cell plus the
/// `param_start` boundary. `None` on the share-direct path.
type BlockWriteback = Option<(LocalsCell, u16)>;

/// Inline capacity of [`ArgsBuf`]'s stack-resident array. Sized at
/// 3 because the overwhelming majority of method calls in real Ruby
/// pass 0–3 positional args (`arr.push(x)`, `h[k] = v`, `a.insert(i,
/// x)`, etc.); argc above this spills to a heap `Vec`. Bump with
/// care — `[Value; N]` is `N * size_of::<Value>()` bytes on every
/// `do_call` stack frame.
const ARGS_INLINE: usize = 3;

/// Owned, `&mut self`-independent container for the positional args
/// of a single `do_call` / `do_call_block`, drained off the operand
/// stack at the dispatch boundary.
///
/// Why it exists: the args must be **owned** (the `&mut self`
/// primitive handlers — `primitive_call`, `array_collection_call`,
/// `hash_collection_call`, … — run while `self.stack` is otherwise
/// borrowed, so they can't read the args back off the stack) AND, on
/// the hot small-argc path (`arr.push(i)`, `h[k] = v`, `s << t`),
/// they must **not** heap-allocate (profiling showed the per-call
/// `stack.drain(..).collect::<Vec<_>>()` — `from_iter` + the matching
/// free/drop — was ~10–12 % of top-of-stack samples in primitive-arg-
/// heavy loops). `Inline` holds up to [`ARGS_INLINE`] args in a
/// stack-resident array with no allocation; `Heap` falls back to a
/// `Vec` for larger argc (and is also how the dispatch helpers hand
/// the args back when they don't consume them).
///
/// It `Deref`s to `&[Value]`, so every read-only use in the dispatch
/// body (`args.len()`, `args[0]`, `args.iter()`, `&args`,
/// `args.as_slice()`, …) is unchanged. By-value consumers (the
/// `invoke_method` / `invoke_block` user-method paths, the `for a in
/// args` re-push loops, `args.to_vec()`) call [`ArgsBuf::into_vec`]
/// / iterate via `IntoIterator`. The dispatch decision sequence is
/// **identical** to the previous all-`Vec` shape — only the args
/// *container* changes.
enum ArgsBuf {
    /// `len` valid args in `buf[..len]`; `buf[len..]` is `Value::Nil`
    /// filler (never read — `Deref` slices to `..len`). `len <=
    /// ARGS_INLINE` always.
    Inline { buf: [Value; ARGS_INLINE], len: usize },
    Heap(Vec<Value>),
}

impl ArgsBuf {
    /// Drain the top `argc` operand-stack slots into an `ArgsBuf`,
    /// preserving their order (`[..., a1, …, aN]` → `[a1, …, aN]`).
    /// argc ≤ [`ARGS_INLINE`] stays on the stack (no allocation);
    /// larger argc uses the same `drain(..).collect()` as before.
    #[inline]
    fn drain_from(stack: &mut Vec<Value>, argc: usize) -> Self {
        let split = stack.len() - argc;
        if argc <= ARGS_INLINE {
            // Fill an inline array bottom-up by popping the stack
            // top (which yields aN first), writing into slot
            // argc-1 down to 0 — restoring source order. `Value`
            // isn't `Copy`, so seed with `Nil` then overwrite.
            let mut buf: [Value; ARGS_INLINE] = std::array::from_fn(|_| Value::Nil);
            for slot in (0..argc).rev() {
                // `pop` can't underflow: callers guarantee
                // `stack.len() >= argc` (the receiver, if any,
                // sits below these slots).
                if let Some(v) = stack.pop() {
                    buf[slot] = v;
                }
            }
            ArgsBuf::Inline { buf, len: argc }
        } else {
            ArgsBuf::Heap(stack.drain(split..).collect())
        }
    }

    /// Consume into an owned `Vec<Value>` for the by-value dispatch
    /// paths (user-method invoke, send-arg re-push, `to_h`-style
    /// `args.to_vec()` callers). The `Inline` case allocates here —
    /// but those paths already needed a `Vec` (or were cold), so the
    /// hot primitive path that never calls this stays allocation-free.
    #[inline]
    fn into_vec(self) -> Vec<Value> {
        match self {
            ArgsBuf::Heap(v) => v,
            // `buf` owns ARGS_INLINE Values (`buf[len..]` are `Nil`
            // filler); consume by-value and keep the first `len`.
            ArgsBuf::Inline { buf, len } => {
                let mut v = Vec::with_capacity(len);
                v.extend(buf.into_iter().take(len));
                v
            }
        }
    }
}

impl std::ops::Deref for ArgsBuf {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        match self {
            ArgsBuf::Inline { buf, len } => &buf[..*len],
            ArgsBuf::Heap(v) => v.as_slice(),
        }
    }
}

impl IntoIterator for ArgsBuf {
    type Item = Value;
    type IntoIter = std::vec::IntoIter<Value>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a> IntoIterator for &'a ArgsBuf {
    type Item = &'a Value;
    type IntoIter = std::slice::Iter<'a, Value>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        // Borrows the args via `Deref` — `for a in &args` yields
        // `&Value` exactly as it did over the previous `Vec`.
        self.iter()
    }
}

/// Outcome of [`Vm::try_dispatch_send_bypass`].
///
/// `Handled(r)` means the helper has already done the work
/// (parsed target sym from `args[0]`, set
/// `bypass_visibility_once`, pushed args/recv onto the stack,
/// and recursed into `do_call`); the caller should propagate
/// `r` immediately.
///
/// `NotHandled { args, recv_opt }` means this isn't a `send`
/// call, or it's a `send` with a user-defined override on the
/// surrounding self / explicit receiver (CRuby's reserved-name
/// rule applies only to `__send__`, never `send`); the helper
/// has moved `args` and `recv_opt` back out so the caller can
/// continue dispatch with them intact.
enum SendBypass {
    Handled(Result<(), Trap>),
    NotHandled {
        args: ArgsBuf,
        recv_opt: Option<Value>,
    },
}

/// Outcome of [`Vm::try_dispatch_callable_intrinsics`].
///
/// `Handled` means the helper dispatched (Block.call /
/// `method(:name)` capture / BoundMethod-or-UnboundMethod-or-
/// CurriedProc arm); the caller `do_call` should
/// `return Ok(())` immediately. Any trap raised by an inner
/// arm bubbles through the helper's outer `Result<_, Trap>`.
///
/// `NotHandled { args, recv }` returns the inputs intact so
/// the caller continues with the rest of dispatch.
enum CallableOutcome {
    Handled,
    NotHandled {
        args: ArgsBuf,
        recv: Value,
    },
}

/// Outcome of [`Vm::try_dispatch_class_intrinsics`].
///
/// Same shape as [`CallableOutcome`] — `Handled` means the
/// helper fired one of the class-receiver arms (`Hash[]` /
/// `cls.new` / `cls.allocate` / `cls.include` / etc.) and
/// pushed the result; caller returns `Ok(())` immediately.
/// `NotHandled { args, recv }` returns inputs intact so the
/// caller continues with the rest of dispatch.
enum ClassOutcome {
    Handled,
    NotHandled {
        args: ArgsBuf,
        recv: Value,
    },
}

/// Mode selector for `Vm::float_to_rational_value` — distinguishes the
/// three Float→Rational paths surfaced by Phase C.4.3.
pub(crate) enum FloatToRationalMode {
    /// `Float#to_r` and `Kernel#Rational(f)` — exact IEEE-754
    /// decomposition, no rounding.
    Lossless,
    /// `Float#rationalize(eps)` — Stern-Brocot search within ±|eps|.
    /// The held `Value` is the validated Numeric (Int / BigInt /
    /// Float / Rational) eps from the caller. Read only by the
    /// bignum path of `float_to_rational_value`; no-bignum falls
    /// back to lossless.
    #[allow(dead_code)]
    EpsArg(Value),
    /// Bare `Float#rationalize` — half-ULP search returning the
    /// simplest Rational that round-trips to this Float.
    DefaultUlp,
}

impl Vm {
    /// Allocate a canonical-form `Value::Rational` from raw i64
    /// (num, den). gcd-normalizes and sign-normalizes (`den > 0`)
    /// at the boundary so every `HeapObj::Rational` slot satisfies
    /// the canonical invariants every reader assumes.
    /// `den == 0` → ZeroDivisionError.
    ///
    /// Under `bignum`: widens to BigInt before normalization — the
    /// i64::MIN edge / 2**64 receiver paths flagged as Phase C.4
    /// follow-ups in PR #310 no longer trap. Under no-bignum:
    /// preserves Phase C.1–C.3 behavior, including the i64::MIN
    /// RangeError shortcut (since `.abs()` would panic in debug).
    ///
    /// Numeric values that produce small results (e.g. 4/2 = 2/1
    /// equivalent to Integer 2) STAY as a Value::Rational — CRuby
    /// also keeps the type tag distinct from Integer.
    pub(crate) fn make_rational(&mut self, num: i64, den: i64) -> Result<Value, Trap> {
        if den == 0 {
            return Err(self.trap(RubyError::ZeroDivisionError {
                msg: "divided by 0".to_string(),
            }));
        }
        #[cfg(feature = "bignum")]
        {
            use num_bigint::BigInt;
            self.make_rational_bigint(BigInt::from(num), BigInt::from(den))
        }
        #[cfg(not(feature = "bignum"))]
        {
            if num == i64::MIN || den == i64::MIN {
                return Err(self.trap(RubyError::RangeError {
                    msg: "Rational components must fit in i64".to_string(),
                }));
            }
            let (mut num, mut den) = (num, den);
            if den < 0 { num = -num; den = -den; }
            let g = crate::vm::numeric::gcd_i64(num.abs(), den);
            if g > 1 { num /= g; den /= g; }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Rational(
                crate::heap::RationalRepr { num, den },
            ));
            Ok(Value::Rational(id))
        }
    }

    /// BigInt-arg entry point for `make_rational`. Performs the
    /// same canonical-form normalization (gcd reduce, `den > 0`)
    /// on arbitrary-precision operands. Only available under
    /// `bignum`; Phase C.4.2+ callers (Integer#to_r with BigInt
    /// receiver, Float#to_r, etc.) use this directly.
    ///
    /// `den == 0` → ZeroDivisionError. The guard is a real
    /// runtime check (not just `debug_assert`) so release-build
    /// callers that forget the precheck still surface as a Ruby
    /// exception rather than silently constructing a malformed
    /// `Rational(?, 0)` that later panics inside num-bigint's
    /// division.
    #[cfg(feature = "bignum")]
    pub(crate) fn make_rational_bigint(
        &mut self,
        mut num: num_bigint::BigInt,
        mut den: num_bigint::BigInt,
    ) -> Result<Value, Trap> {
        use num_bigint::Sign;
        use num_integer::Integer;
        use num_traits::{One, Zero};
        if den.is_zero() {
            return Err(self.trap(RubyError::ZeroDivisionError {
                msg: "divided by 0".to_string(),
            }));
        }
        if den.sign() == Sign::Minus {
            num = -num;
            den = -den;
        }
        // `Integer::gcd` is always non-negative; canonical form needs
        // gcd(|num|, den) but BigInt's gcd already takes magnitudes.
        let g = num.gcd(&den);
        if !g.is_one() {
            num /= &g;
            den /= &g;
        }
        self.maybe_gc();
        self.check_alloc()?;
        let id = self.heap.alloc(HeapObj::Rational(
            crate::heap::RationalRepr { num, den },
        ));
        Ok(Value::Rational(id))
    }

    /// Build the Rational representation of a finite `f64` per
    /// `mode`:
    ///   - `Lossless` (used by `Float#to_r` / `Kernel#Rational(f)`):
    ///     exact `f = sign * mantissa * 2^exp` Rational.
    ///   - `EpsArg(v)` (used by `Float#rationalize(eps)`): simplest
    ///     fraction in `[f - |eps|, f + |eps|]`. The interval is
    ///     computed in f64 arithmetic when `eps` is a Float
    ///     (matching CRuby's f_sub/f_add behavior) and in exact
    ///     arithmetic for Integer / Rational eps.
    ///   - `DefaultUlp` (used by bare `Float#rationalize`): simplest
    ///     fraction in `[(2m-1)/2^(1-exp), (2m+1)/2^(1-exp)]` — the
    ///     half-ULP interval covering all reals that round-trip back
    ///     to this Float. Matches CRuby's default-precision behavior
    ///     (`0.1.rationalize == (1/10)`).
    ///
    /// NaN / ±Inf are filtered by the caller — this method assumes
    /// `f.is_finite()`.
    pub(crate) fn float_to_rational_value(
        &mut self,
        f: f64,
        mode: FloatToRationalMode,
    ) -> Result<Value, Trap> {
        debug_assert!(f.is_finite(), "float_to_rational_value: NaN/Inf must be filtered upstream");
        let (sign, mantissa, exp) =
            crate::vm::numeric::float_decompose(f).expect("finite per debug_assert");
        if mantissa == 0 {
            return self.make_rational(0, 1);
        }
        #[cfg(feature = "bignum")]
        {
            use num_bigint::BigInt;
            use num_traits::{One, Zero};
            // Build the lossless (num, den) pair on demand — the
            // DefaultUlp branch below doesn't need it.
            let lossless_pair = |sign: i64, mantissa: u64, exp: i32| -> (BigInt, BigInt) {
                let mant_big = BigInt::from(mantissa);
                let signed = if sign < 0 { -mant_big } else { mant_big };
                if exp >= 0 {
                    (signed << exp as usize, BigInt::one())
                } else {
                    (signed, BigInt::one() << (-exp) as usize)
                }
            };
            let eps_v = match &mode {
                FloatToRationalMode::Lossless => {
                    let (num, den) = lossless_pair(sign, mantissa, exp);
                    return self.make_rational_bigint(num, den);
                }
                FloatToRationalMode::DefaultUlp => {
                    // Half-ULP interval (exact, BigInt):
                    //   a = (2m - 1) * 2^(exp-1)
                    //   b = (2m + 1) * 2^(exp-1)
                    // For exp >= 1 the denominator is 1; for exp <= 0
                    // the numerator carries (2m±1) and den = 2^(1-exp).
                    // Stern-Brocot only handles positive intervals;
                    // negate the result for negative `f`.
                    let mant_a = BigInt::from(2 * mantissa - 1);
                    let mant_b = BigInt::from(2 * mantissa + 1);
                    let (a_num, b_num, common_den) = if exp >= 1 {
                        let shift = (exp - 1) as usize;
                        (mant_a << shift, mant_b << shift, BigInt::one())
                    } else {
                        let shift = (1 - exp) as usize;
                        (mant_a, mant_b, BigInt::one() << shift)
                    };
                    let (p, q) = stern_brocot_simplest(
                        a_num, common_den.clone(), b_num, common_den,
                    );
                    let p = if sign < 0 { -p } else { p };
                    return self.make_rational_bigint(p, q);
                }
                FloatToRationalMode::EpsArg(v) => v,
            };
            // rationalize(eps) — Stern-Brocot search within ±|eps|.
            // CRuby computes the interval endpoints `f - eps` and
            // `f + eps` in f64 arithmetic when eps is a Float, then
            // runs Stern-Brocot on the resulting Float-derived
            // Rationals. Replicating that path so spec/CRuby agree
            // (e.g. `3.14.rationalize(0.01) == (22/7)`,
            // `3.14.rationalize(0.001) == (135/43)`). Non-Float
            // eps stays in exact arithmetic, matching CRuby's
            // promote-to-Rational behavior for Integer / Rational
            // eps.
            //
            // NaN / ±Inf eps (or overflow in `f ± eps`) → CRuby
            // raises FloatDomainError; we match. Pre-validate the
            // eps Float and the interval endpoints so the
            // `float_decompose(..).expect("finite")` invariant
            // inside `float_to_rational_pair_signed` holds.
            let eps_f_opt: Option<f64> = match eps_v {
                Value::Float(g) => {
                    if !g.is_finite() {
                        return Err(self.trap(RubyError::FloatDomainError {
                            msg: crate::vm::numeric::float_domain_label(*g).to_string(),
                        }));
                    }
                    Some(g.abs())
                }
                _ => None,
            };
            let (common_den, a_num, b_num) = if let Some(eps_f) = eps_f_opt {
                if eps_f == 0.0 {
                    let (num, den) = lossless_pair(sign, mantissa, exp);
                    return self.make_rational_bigint(num, den);
                }
                let a_f = f - eps_f;
                let b_f = f + eps_f;
                if !a_f.is_finite() {
                    return Err(self.trap(RubyError::FloatDomainError {
                        msg: crate::vm::numeric::float_domain_label(a_f).to_string(),
                    }));
                }
                if !b_f.is_finite() {
                    return Err(self.trap(RubyError::FloatDomainError {
                        msg: crate::vm::numeric::float_domain_label(b_f).to_string(),
                    }));
                }
                // Collapsed-interval guard: when eps is smaller than
                // the local ULP, `f ± eps` rounds back to `f` and the
                // interval is the single point `f`. Stern-Brocot
                // assumes `a < b` strictly and would loop forever on
                // `a == b` (the `c < b` exit test never fires). Bail
                // to the lossless representation, which is the only
                // Rational in a single-point interval.
                if a_f == b_f {
                    let (num, den) = lossless_pair(sign, mantissa, exp);
                    return self.make_rational_bigint(num, den);
                }
                // Decompose a_f and b_f to a common denominator.
                let (a_n, a_d) = float_to_rational_pair_signed(a_f);
                let (b_n, b_d) = float_to_rational_pair_signed(b_f);
                // Bring to common denominator (b_d * a_d, but both
                // are powers of 2 so the common one is the larger).
                if a_d == b_d {
                    (a_d, a_n, b_n)
                } else if a_d > b_d {
                    let factor = &a_d / &b_d;
                    (a_d, a_n, b_n * factor)
                } else {
                    let factor = &b_d / &a_d;
                    (b_d, a_n * factor, b_n)
                }
            } else {
                let (mut eps_num, eps_den) = self.coerce_to_rational_parts(eps_v)?;
                if eps_num.sign() == num_bigint::Sign::Minus {
                    eps_num = -eps_num;
                }
                if eps_num.is_zero() {
                    let (num, den) = lossless_pair(sign, mantissa, exp);
                    return self.make_rational_bigint(num, den);
                }
                let (num, den) = lossless_pair(sign, mantissa, exp);
                let common_den = &den * &eps_den;
                let term = &eps_num * &den;
                let a_num = &num * &eps_den - &term;
                let b_num = &num * &eps_den + &term;
                (common_den, a_num, b_num)
            };
            // Sign handling: Stern-Brocot below assumes 0 < a <= b.
            // For negative target, flip and negate the result.
            let negate_result = if a_num.is_zero() {
                false
            } else if a_num.sign() == num_bigint::Sign::Minus
                && b_num.sign() != num_bigint::Sign::Minus
            {
                // Interval straddles zero — return 0 (the simplest).
                return self.make_rational(0, 1);
            } else {
                a_num.sign() == num_bigint::Sign::Minus
            };
            let (a_num, b_num) = if negate_result {
                (-b_num, -a_num)
            } else {
                (a_num, b_num)
            };
            let (p, q) = stern_brocot_simplest(
                a_num, common_den.clone(), b_num, common_den,
            );
            let p = if negate_result { -p } else { p };
            self.make_rational_bigint(p, q)
        }
        #[cfg(not(feature = "bignum"))]
        {
            // No-bignum: try to fit (num, den) in i64. Any Float
            // whose IEEE-754 decomposition exceeds i64 magnitude
            // (typical for subnormals or floats with den > 2^62)
            // raises RangeError — matches the no-bignum tier's
            // policy for Rational components.
            let mantissa_i: i64 = mantissa as i64; // ≤ 53 bits, fits
            let signed = if sign < 0 { -mantissa_i } else { mantissa_i };
            let (num, den): (i64, i64) = if exp >= 0 {
                let den = 1i64;
                let shift = exp as u32;
                if shift >= 63 {
                    return Err(self.trap(RubyError::RangeError {
                        msg: "Float#to_r exceeds i64 magnitude (rebuild with --features bignum)".to_string(),
                    }));
                }
                let num = signed.checked_shl(shift).ok_or_else(|| {
                    self.trap(RubyError::RangeError {
                        msg: "Float#to_r exceeds i64 magnitude (rebuild with --features bignum)".to_string(),
                    })
                })?;
                (num, den)
            } else {
                let shift = (-exp) as u32;
                if shift >= 63 {
                    return Err(self.trap(RubyError::RangeError {
                        msg: "Float#to_r denominator exceeds i64 (rebuild with --features bignum)".to_string(),
                    }));
                }
                (signed, 1i64 << shift)
            };
            // No-bignum: Stern-Brocot needs arbitrary-precision
            // arithmetic, so both `EpsArg` and `DefaultUlp` modes
            // fall back to the lossless representation here. The
            // result is still a valid simpler-or-equal Rational
            // representation of the value (the contract doesn't
            // require the simplest, only one within ±eps). Caller-
            // visible divergence: `0.1.rationalize == (1/10)` under
            // bignum becomes the lossless huge fraction under no-
            // bignum. Documented in spec headers.
            let _ = mode;
            self.make_rational(num, den)
        }
    }

    /// Coerce a validated Numeric arg (Int / BigInt / Float / Rational)
    /// to a `(BigInt, BigInt)` num/den pair under bignum. Only used
    /// by `float_to_rational_value` for the rationalize-eps path so
    /// the eps tolerance can be compared against the lossless target.
    #[cfg(feature = "bignum")]
    fn coerce_to_rational_parts(
        &mut self,
        v: &Value,
    ) -> Result<(num_bigint::BigInt, num_bigint::BigInt), Trap> {
        use num_bigint::BigInt;
        use num_traits::One;
        match v {
            Value::Int(n) => Ok((BigInt::from(*n), BigInt::one())),
            Value::BigInt(id) => Ok((self.heap.bigint(*id).clone(), BigInt::one())),
            Value::Float(g) => {
                debug_assert!(g.is_finite(), "non-finite eps must be filtered upstream");
                let (sign, mantissa, exp) = crate::vm::numeric::float_decompose(*g)
                    .expect("finite per debug_assert");
                if mantissa == 0 { return Ok((BigInt::from(0), BigInt::one())); }
                let mant_big = BigInt::from(mantissa);
                let signed = if sign < 0 { -mant_big } else { mant_big };
                if exp >= 0 {
                    Ok((signed << exp as usize, BigInt::one()))
                } else {
                    Ok((signed, BigInt::one() << (-exp) as usize))
                }
            }
            Value::Rational(id) => {
                let r = self.heap.rational(*id);
                Ok((r.num.clone(), r.den.clone()))
            }
            _ => unreachable!("eps validated by caller as Numeric (Int / BigInt / Float / Rational); nil rejected at TypeError gate upstream"),
        }
    }

    /// `Rational#**(exp)` — phase C.4.4 power dispatch.
    ///
    /// Integer exp (Int / BigInt) keeps the result exact:
    ///   `Rational(num, den) ** k` → `Rational(num^k, den^k)` for k>0,
    ///   the reciprocal for k<0, and `Rational(1, 1)` for k==0.
    ///   `Rational(0, 1) ** k` with k<0 raises ZeroDivisionError.
    /// Float / Rational exp demotes the receiver to f64 and uses
    /// `f64::powf`, matching CRuby's `Rational#**` Float fallback.
    /// Non-Numeric exp → TypeError.
    pub(crate) fn rational_pow(&mut self, recv: &Value, exp: &Value) -> Result<Value, Trap> {
        let r_id = match recv {
            Value::Rational(id) => *id,
            _ => unreachable!("rational_pow called on non-Rational receiver"),
        };
        // Integer exponent (fast / exact path). BigInt exponents
        // STAY integer-typed even when they don't fit i64 — they
        // can't be silently demoted to Float because the
        // 0**negative → ZeroDivisionError invariant and the BigInt
        // cap (|k| ≤ 2^16) would be bypassed by `0.0_f64.powf(-big)`
        // returning Infinity instead of trapping.
        if let Value::Int(n) = exp {
            return self.rational_pow_int(r_id, *n);
        }
        #[cfg(feature = "bignum")]
        if let Value::BigInt(id) = exp {
            // Capture exp metadata into owned scalars / i64 result
            // up-front so the `&BigInt` borrow doesn't conflict with
            // the subsequent `&mut self` calls below.
            use num_bigint::Sign;
            use num_traits::Zero;
            let (exp_negative, exp_is_odd, exp_fits_i64) = {
                let exp_big = self.heap.bigint(*id);
                (
                    exp_big.sign() == Sign::Minus,
                    exp_big.bit(0),
                    i64::try_from(exp_big).ok(),
                )
            };
            // Zero base + negative exp is always ZeroDivisionError —
            // surface here so a huge BigInt negative exp doesn't
            // escape into the cap path.
            if exp_negative && self.heap.rational(r_id).num.is_zero() {
                return Err(self.trap(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                }));
            }
            // Unit bases (0/1, 1/1, -1/1) are exactly representable
            // for any integer exponent without touching BigInt::pow,
            // so the 2^16 cap shouldn't gate them. Short-circuit
            // BEFORE the i64-fit conversion below.
            if let Some(v) = self.try_unit_base_pow(r_id, exp_is_odd)? {
                return Ok(v);
            }
            // Fits i64 → exact path. Otherwise the magnitude is
            // necessarily above the |k| ≤ 2^16 cap inside
            // `rational_pow_int`, so surface the same RangeError
            // up-front rather than letting it slip into Float.
            if let Some(k) = exp_fits_i64 {
                return self.rational_pow_int(r_id, k);
            }
            return Err(self.trap(RubyError::RangeError {
                msg: "Rational#** exponent magnitude exceeds 2^16 cap".to_string(),
            }));
        }
        // Float / Rational exp:
        //   1. Integer-valued exp (Float.fract() == 0.0 within i64 range;
        //      Rational with den == 1) routes through the exact integer
        //      path so `Rational(2,1) ** Rational(3,1)` returns `(8/1)`
        //      and `Rational(2,1) ** 3.0` returns `(8/1)`, matching CRuby
        //      (which promotes integer-valued non-Integer exps to the
        //      integer power algorithm).
        //   2. Otherwise demote to Float. Pre-demote, guard zero-base +
        //      negative exp so `Rational(0,1) ** -0.5` raises
        //      ZeroDivisionError rather than `0.0.powf(-0.5) == Infinity`
        //      (same invariant the BigInt-exp branch above enforces).
        if let Some(k) = integer_valued_exp(exp, &self.heap) {
            return self.rational_pow_int(r_id, k);
        }
        let exp_f: Option<f64> = match exp {
            Value::Float(g) => Some(*g),
            Value::Rational(eid) => Some(crate::heap::rational_to_f64(self.heap.rational(*eid))),
            _ => None,
        };
        if let Some(g) = exp_f {
            // Zero base + negative non-integer exp → ZeroDivisionError
            // (matches CRuby; mirrors the BigInt-exp guard above).
            #[cfg(feature = "bignum")]
            let recv_num_is_zero = {
                use num_traits::Zero;
                self.heap.rational(r_id).num.is_zero()
            };
            #[cfg(not(feature = "bignum"))]
            let recv_num_is_zero = self.heap.rational(r_id).num == 0;
            if recv_num_is_zero && g < 0.0 {
                return Err(self.trap(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                }));
            }
            let base_f = crate::heap::rational_to_f64(self.heap.rational(r_id));
            return Ok(Value::Float(base_f.powf(g)));
        }
        Err(self.trap(RubyError::TypeError {
            msg: format!(
                "{} can't be coerced into Rational",
                crate::vm::numeric::type_name_for_coerce(exp),
            ),
        }))
    }

    /// Return `Some(unit_pow_result)` when the Rational at `r_id`
    /// is a unit base (0/1, 1/1, or -1/1) — these are exactly
    /// representable for any integer exponent without `BigInt::pow`
    /// or `checked_pow`. Returns `None` for non-unit bases so the
    /// caller proceeds with the regular pow path. `ak_is_odd` is
    /// the parity of `|k|` and only matters for the -1/1 case.
    ///
    /// Bignum-gated because the no-bignum tier inlines the same
    /// short-circuit at its single call site (the i64 RationalRepr
    /// makes the unit-base match trivial without a helper).
    #[cfg(feature = "bignum")]
    fn try_unit_base_pow(
        &mut self,
        r_id: ObjId,
        ak_is_odd: bool,
    ) -> Result<Option<Value>, Trap> {
        use num_bigint::BigInt;
        use num_traits::{One, Zero};
        let (is_zero, is_one, is_neg_one) = {
            let r = self.heap.rational(r_id);
            if !r.den.is_one() {
                return Ok(None);
            }
            (
                r.num.is_zero(),
                r.num.is_one(),
                r.num == BigInt::from(-1),
            )
        };
        if is_zero {
            // k must be > 0 here (caller traps 0**negative upstream).
            return Ok(Some(self.make_rational(0, 1)?));
        }
        if is_one {
            return Ok(Some(self.make_rational(1, 1)?));
        }
        if is_neg_one {
            let signed = if ak_is_odd { -1 } else { 1 };
            return Ok(Some(self.make_rational(signed, 1)?));
        }
        Ok(None)
    }

    /// Integer-exponent power for Rational. Splits bignum and
    /// no-bignum so `BigInt::pow(u32)` is only required on the
    /// bignum tier — no-bignum uses `i64::checked_pow`.
    fn rational_pow_int(&mut self, r_id: ObjId, k: i64) -> Result<Value, Trap> {
        #[cfg(feature = "bignum")]
        {
            use num_bigint::BigInt;
            use num_traits::{One, Zero};
            if k == 0 {
                return self.make_rational(1, 1);
            }
            // 0 base + negative exp is always ZeroDivisionError —
            // check BEFORE the cap so `(0/1r) ** -70000` returns
            // the right error class (the cap would otherwise mask
            // it as RangeError).
            if k < 0 && self.heap.rational(r_id).num.is_zero() {
                return Err(self.trap(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                }));
            }
            // Unit bases (0/1, 1/1, -1/1) are exactly representable
            // for any integer exponent without touching BigInt::pow,
            // so the 2^16 cap shouldn't gate them either. Same fix
            // structure as the no-bignum path's u32::try_from fence.
            let ak = k.unsigned_abs();
            if let Some(v) = self.try_unit_base_pow(r_id, ak % 2 == 1)? {
                return Ok(v);
            }
            // Cap |k| so a pathological non-unit literal can't drive
            // BigInt pow into multi-GB allocations. 2^16 is well
            // above anything a sane source uses but small enough that
            // the worst-case result (limit ≈ 2^(53*65536) bytes) is
            // still memory-bounded by the host's existing alloc
            // guard. Matches the spirit of the bignum_primitive pow
            // cap in vm/bignum.rs.
            if ak > 65536 {
                return Err(self.trap(RubyError::RangeError {
                    msg: "Rational#** exponent magnitude exceeds 2^16 cap".to_string(),
                }));
            }
            let ak_u32 = ak as u32;
            // Borrow the heap Rational just long enough to compute
            // new_num / new_den via `BigInt::pow(&self, u32)` (which
            // only needs `&BigInt`). Avoids cloning r.num / r.den
            // up-front when the existing `&BigInt` borrows suffice.
            // Drop the borrow before the subsequent `&mut self` calls.
            let (new_num, new_den) = {
                let r = self.heap.rational(r_id);
                (r.num.pow(ak_u32), r.den.pow(ak_u32))
            };
            // The caller-side canonical form was already coprime
            // and den-positive; integer pow preserves both, but
            // make_rational_bigint re-normalizes defensively (the
            // gcd ends up being 1 so the work is cheap).
            if k > 0 {
                self.make_rational_bigint(new_num, new_den)
            } else {
                // k < 0 → reciprocal. Sign of num flows to the new
                // numerator; absolute swap goes through
                // make_rational_bigint's sign-normalization so
                // `den > 0` stays canonical.
                if new_num.sign() == num_bigint::Sign::Minus {
                    // `new_num` consumed by the negation; no further
                    // use, so move rather than clone.
                    self.make_rational_bigint(-new_den, -new_num)
                } else if new_num.is_one() {
                    self.make_rational_bigint(new_den, BigInt::one())
                } else {
                    self.make_rational_bigint(new_den, new_num)
                }
            }
        }
        #[cfg(not(feature = "bignum"))]
        {
            let r = *self.heap.rational(r_id);
            if k == 0 {
                return self.make_rational(1, 1);
            }
            if r.num == 0 && k < 0 {
                return Err(self.trap(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                }));
            }
            let ak = k.unsigned_abs();
            // Unit bases (0 / ±1) are exactly representable for any
            // integer exponent without touching `checked_pow` — so
            // short-circuit BEFORE the u32 conversion fence below.
            // Otherwise `(1/1r) ** 10**18` would trip the u32::try_from
            // even though the result is just (1/1). `(0/1r) ** k` with
            // k > 0 is 0 (k < 0 was already trapped above).
            if r.den == 1 {
                match r.num {
                    0 => return self.make_rational(0, 1),
                    1 => return self.make_rational(1, 1),
                    -1 => {
                        let signed = if ak % 2 == 0 { 1 } else { -1 };
                        return self.make_rational(signed, 1);
                    }
                    _ => {}
                }
            }
            // u32 is `checked_pow`'s exponent type. Anything beyond
            // overflows for any base other than the unit bases handled
            // above; real overflow detection is delegated to
            // `checked_pow` so base-specific stability (e.g.
            // `(1/2r) ** 60`) succeeds where a naïve `ak > 62` cap
            // would have rejected it.
            let ak_u32 = u32::try_from(ak).map_err(|_| {
                self.trap(RubyError::RangeError {
                    msg: "Rational#** exponent magnitude exceeds u32 (rebuild with --features bignum)".to_string(),
                })
            })?;
            let new_num = r.num.checked_pow(ak_u32).ok_or_else(|| {
                self.trap(RubyError::RangeError {
                    msg: "Rational#** numerator overflows i64 (rebuild with --features bignum)".to_string(),
                })
            })?;
            let new_den = r.den.checked_pow(ak_u32).ok_or_else(|| {
                self.trap(RubyError::RangeError {
                    msg: "Rational#** denominator overflows i64 (rebuild with --features bignum)".to_string(),
                })
            })?;
            if k > 0 {
                self.make_rational(new_num, new_den)
            } else {
                // reciprocal — make_rational sign-normalizes.
                self.make_rational(new_den, new_num)
            }
        }
    }

    /// `try_rational_binop` — the `Op::BinOp` arm for Rational
    /// operands. Called between `try_bigint_binop` and
    /// `primitive_call`, so by the time we get here neither side
    /// can be (Int × Int) without one side being Rational.
    ///
    /// Returns `Ok(Some(v))` on a handled pair, `Ok(None)` if
    /// neither side is a Rational (caller falls through). Errors
    /// propagate (ZeroDivisionError on `r / 0`, RangeError on i64
    /// overflow).
    ///
    /// Coverage:
    ///   - Rational × Rational    → Rational
    ///   - Rational × Integer     → Rational
    ///   - Integer × Rational     → Rational
    ///   - Rational × Float       → Float (Float dominates)
    ///   - Float × Rational       → Float
    ///
    /// Under `bignum` the Integer side widens to BigInt and the
    /// internal arithmetic is infallible (no `RangeError` from
    /// overflow). Under no-bignum the legacy i64 checked path
    /// is preserved.
    pub(crate) fn try_rational_binop(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        a: &Value,
        b: &Value,
    ) -> Result<Option<Value>, Trap> {
        use crate::bytecode::BinOpKind as K;
        // Float arms first — Float dominates Numeric, and both
        // operands are demoted to f64 before the op runs.
        let (a_f, b_f, has_float) = match (a, b) {
            (Value::Float(x), Value::Rational(id)) => {
                let r = self.heap.rational(*id);
                (Some(*x), Some(crate::heap::rational_to_f64(r)), true)
            }
            (Value::Rational(id), Value::Float(y)) => {
                let r = self.heap.rational(*id);
                (Some(crate::heap::rational_to_f64(r)), Some(*y), true)
            }
            _ => (None, None, false),
        };
        if has_float {
            let x = a_f.unwrap();
            let y = b_f.unwrap();
            let result = match kind {
                K::Add => Value::Float(x + y),
                K::Sub => Value::Float(x - y),
                K::Mul => Value::Float(x * y),
                // IEEE-754 / CRuby Float div by 0.0 yields ±Infinity
                // (or NaN for 0.0 / 0.0); no exception. Matches the
                // existing Float×Float `BinOpKind::Div` arm in
                // numeric.rs which uses bare `a / b`. Pre-fix this
                // arm raised ZeroDivisionError for `Rational(1, 2)
                // / 0.0`, divergent from `1.0 / 0.0`.
                K::Div => Value::Float(x / y),
                K::Mod => Value::Float(crate::vm::numeric::floor_mod_f64(x, y)),
                K::Lt => Value::Bool(x < y),
                K::Le => Value::Bool(x <= y),
                K::Gt => Value::Bool(x > y),
                K::Ge => Value::Bool(x >= y),
                K::Eq => Value::Bool(x == y),
                K::Ne => Value::Bool(x != y),
            };
            return Ok(Some(result));
        }
        // At least one side must be Rational (the Int × Int path
        // is already covered by apply_int upstream).
        if !matches!(a, Value::Rational(_)) && !matches!(b, Value::Rational(_)) {
            return Ok(None);
        }
        #[cfg(feature = "bignum")]
        {
            // Under bignum the BigInt path handles every operand
            // shape — Int / BigInt / Rational. Non-numeric → None.
            self.rational_binop_bigint(kind, a, b)
        }
        #[cfg(not(feature = "bignum"))]
        {
            // i64 checked-arithmetic path. Integer side synthesises
            // (n, 1); Rational side reads canonical (num, den).
            // Non-Int / non-Rational operands fall through to caller.
            let to_pair = |v: &Value, heap: &crate::heap::Heap| -> Option<(i64, i64)> {
                match v {
                    Value::Int(n) => Some((*n, 1)),
                    Value::Rational(id) => {
                        let r = heap.rational(*id);
                        Some((r.num, r.den))
                    }
                    _ => None,
                }
            };
            let (an, ad) = match to_pair(a, &self.heap) { Some(p) => p, None => return Ok(None) };
            let (bn, bd) = match to_pair(b, &self.heap) { Some(p) => p, None => return Ok(None) };
            // `fn` (not a closure) so it satisfies `FnOnce` by-value
            // on every call site without forcing the surrounding
            // closures to be `Copy`/`Clone`.
            fn overflow() -> Trap {
                Trap::new(RubyError::RangeError {
                    msg: "Rational result overflows i64 (rebuild with --features bignum)".to_string(),
                })
            }
            match kind {
                K::Add => {
                    let p1 = an.checked_mul(bd).ok_or_else(overflow)?;
                    let p2 = bn.checked_mul(ad).ok_or_else(overflow)?;
                    let num = p1.checked_add(p2).ok_or_else(overflow)?;
                    let den = ad.checked_mul(bd).ok_or_else(overflow)?;
                    Ok(Some(self.make_rational(num, den)?))
                }
                K::Sub => {
                    let p1 = an.checked_mul(bd).ok_or_else(overflow)?;
                    let p2 = bn.checked_mul(ad).ok_or_else(overflow)?;
                    let num = p1.checked_sub(p2).ok_or_else(overflow)?;
                    let den = ad.checked_mul(bd).ok_or_else(overflow)?;
                    Ok(Some(self.make_rational(num, den)?))
                }
                K::Mul => {
                    let num = an.checked_mul(bn).ok_or_else(overflow)?;
                    let den = ad.checked_mul(bd).ok_or_else(overflow)?;
                    Ok(Some(self.make_rational(num, den)?))
                }
                K::Div => {
                    if bn == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let num = an.checked_mul(bd).ok_or_else(overflow)?;
                    let den = ad.checked_mul(bn).ok_or_else(overflow)?;
                    Ok(Some(self.make_rational(num, den)?))
                }
                K::Mod => {
                    // Phase C.2 defers Rational#% — fall through to
                    // NoMethodError.
                    let _ = (an, ad, bn, bd);
                    Ok(None)
                }
                K::Lt | K::Le | K::Gt | K::Ge | K::Eq | K::Ne => {
                    let lhs = (an as i128) * (bd as i128);
                    let rhs = (bn as i128) * (ad as i128);
                    Ok(Some(Value::Bool(match kind {
                        K::Lt => lhs < rhs,
                        K::Le => lhs <= rhs,
                        K::Gt => lhs > rhs,
                        K::Ge => lhs >= rhs,
                        K::Eq => lhs == rhs,
                        K::Ne => lhs != rhs,
                        _ => unreachable!(),
                    })))
                }
            }
        }
    }

    /// BigInt-precision arithmetic for `try_rational_binop`. Replaces
    /// the i64 checked-arithmetic path under `bignum`. Reads each
    /// side as `(BigInt num, BigInt den)` — Int / BigInt operands
    /// synthesise `(n, 1)`, Rational operands clone the heap repr.
    #[cfg(feature = "bignum")]
    fn rational_binop_bigint(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        a: &Value,
        b: &Value,
    ) -> Result<Option<Value>, Trap> {
        use crate::bytecode::BinOpKind as K;
        use num_bigint::BigInt;
        use num_traits::One;
        let to_pair = |v: &Value, heap: &crate::heap::Heap| -> Option<(BigInt, BigInt)> {
            match v {
                Value::Int(n) => Some((BigInt::from(*n), BigInt::one())),
                Value::BigInt(id) => Some((heap.bigint(*id).clone(), BigInt::one())),
                Value::Rational(id) => {
                    let r = heap.rational(*id);
                    Some((r.num.clone(), r.den.clone()))
                }
                _ => None,
            }
        };
        let (an, ad) = match to_pair(a, &self.heap) { Some(p) => p, None => return Ok(None) };
        let (bn, bd) = match to_pair(b, &self.heap) { Some(p) => p, None => return Ok(None) };
        match kind {
            K::Add => {
                let num = &an * &bd + &bn * &ad;
                let den = &ad * &bd;
                Ok(Some(self.make_rational_bigint(num, den)?))
            }
            K::Sub => {
                let num = &an * &bd - &bn * &ad;
                let den = &ad * &bd;
                Ok(Some(self.make_rational_bigint(num, den)?))
            }
            K::Mul => {
                let num = &an * &bn;
                let den = &ad * &bd;
                Ok(Some(self.make_rational_bigint(num, den)?))
            }
            K::Div => {
                use num_traits::Zero;
                if bn.is_zero() {
                    return Err(self.trap(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    }));
                }
                let num = &an * &bd;
                let den = &ad * &bn;
                Ok(Some(self.make_rational_bigint(num, den)?))
            }
            K::Mod => Ok(None),
            K::Lt | K::Le | K::Gt | K::Ge | K::Eq | K::Ne => {
                // canonical-form `den > 0` on both sides, so the
                // sign of `an*bd - bn*ad` follows `r1 - r2` directly.
                let lhs = &an * &bd;
                let rhs = &bn * &ad;
                Ok(Some(Value::Bool(match kind {
                    K::Lt => lhs < rhs,
                    K::Le => lhs <= rhs,
                    K::Gt => lhs > rhs,
                    K::Ge => lhs >= rhs,
                    K::Eq => lhs == rhs,
                    K::Ne => lhs != rhs,
                    _ => unreachable!(),
                })))
            }
        }
    }

    /// `String#encoding` intercept — pushes the preamble's
    /// `Encoding::UTF_8` instance and returns true if the call
    /// matches the shape; returns false otherwise so the caller
    /// falls through to its usual primitive dispatch.
    ///
    /// Used by BOTH `do_call` and `do_call_block`. The Encoding
    /// object lives in the joined-name constants table seeded by
    /// the preamble; materialising it requires `&mut self`, which
    /// the stateless `primitive::string_call` free function can't
    /// supply.
    ///
    /// ICE if the constant is missing — only reachable when the
    /// preamble didn't load (e.g. a misconfigured test harness),
    /// and silently returning Nil leaves downstream callers
    /// (`enc.dummy?` etc.) with a NoMethodError far from the root
    /// cause. Panic surfaces the actual bootstrap failure.
    pub(crate) fn try_push_string_encoding(
        &mut self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> bool {
        let Value::Str(rs) = recv else { return false };
        if name != "encoding" || !args.is_empty() {
            return false;
        }
        // E1: the tag picks which preamble singleton comes back.
        // `Other(_)` is unconstructible until the Tier 2 registry
        // exists; report UTF-8 rather than panicking so a future
        // partial wiring degrades visibly (wrong name) instead of
        // fatally.
        let const_name: std::borrow::Cow<'static, str> = match rs.encoding.get() {
            crate::value::EncodingTag::Utf8 => "Encoding::UTF_8".into(),
            crate::value::EncodingTag::UsAscii => "Encoding::US_ASCII".into(),
            crate::value::EncodingTag::Binary => "Encoding::ASCII_8BIT".into(),
            #[cfg(feature = "_encoding_full")]
            crate::value::EncodingTag::Other(idx) => {
                // Registry constant: "ISO-8859-1" → Encoding::ISO_8859_1
                // (the preamble's encoding_full segment defines them).
                match crate::encoding_full::name(idx) {
                    Some(n) => format!("Encoding::{}", n.replace('-', "_")).into(),
                    None => "Encoding::UTF_8".into(),
                }
            }
            #[cfg(not(feature = "_encoding_full"))]
            crate::value::EncodingTag::Other(_) => "Encoding::UTF_8".into(),
        };
        let key = self.interner.intern(&const_name);
        let v = self.constants.get(&key).cloned()
            .expect("ICE: Encoding constant not in table — preamble didn't load");
        self.stack.push(v);
        true
    }

    /// `String#force_encoding(enc)` + `String#encode(...)` — the
    /// E1 subset (ADR 0020). force_encoding flips the TAG without
    /// touching bytes (CRuby contract), returns self; frozen
    /// receivers raise FrozenError. encode returns a NEW string:
    /// same-encoding → plain copy; cross-encoding with ASCII-only
    /// bytes → copy with the new tag (the conversion is the
    /// identity there); anything else needs real transcoding →
    /// Encoding::UndefinedConversionError, mirroring CRuby's
    /// error class for the cases E1 declines (Tier 2's
    /// `_encoding_full` will convert instead). Returns true when
    /// the call matched and a value was pushed.
    pub(crate) fn try_string_encoding_ops(
        &mut self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> Result<bool, Trap> {
        use crate::value::EncodingTag;
        let Value::Str(rs) = recv else { return Ok(false) };
        match (name, args) {
            ("force_encoding", [arg]) => {
                if rs.frozen.get() {
                    return Err(self.trap(RubyError::FrozenError {
                        msg: "can't modify frozen String".to_string(),
                    }));
                }
                let Some(tag) = self.resolve_encoding_arg(arg) else {
                    let shown = match arg {
                        Value::Str(s) => s.to_string_lossy(),
                        other => other.type_name().to_string(),
                    };
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("unknown encoding name - {shown}"),
                    }));
                };
                rs.encoding.set(tag);
                self.stack.push(recv.clone());
                Ok(true)
            }
            ("encode", []) | ("encode", [_]) | ("encode", [_, Value::Hash(_)]) => {
                // Trailing kwargs Hash: `undef: :replace` (+ optional
                // `replace: "str"`) opts into replacement instead of
                // raising on unmappable characters. Other keys are
                // accepted and ignored (CRuby has more options; the
                // E2 subset implements the replacement pair).
                let mut replace: Option<Vec<u8>> = None;
                if let Some(Value::Hash(hid)) = args.get(1) {
                    let undef_key = Value::Sym(self.interner.intern("undef"));
                    let undef_on = matches!(
                        self.heap.hash_index_lookup(*hid, &undef_key)
                            .map(|pos| &self.heap.hash(*hid)[pos].1),
                        Some(Value::Sym(s)) if &**self.interner.resolve(*s) == "replace"
                    );
                    if undef_on {
                        let rep_key = Value::Sym(self.interner.intern("replace"));
                        replace = Some(match self.heap.hash_index_lookup(*hid, &rep_key)
                            .map(|pos| self.heap.hash(*hid)[pos].1.clone())
                        {
                            Some(Value::Str(r)) => r.content.borrow().clone(),
                            _ => b"?".to_vec(),
                        });
                    }
                }
                let target = match args.first() {
                    None => rs.encoding.get(),
                    Some(arg) => match self.resolve_encoding_arg(arg) {
                        Some(t) => t,
                        None => {
                            let shown = match &args[0] {
                                Value::Str(s) => s.to_string_lossy(),
                                other => other.type_name().to_string(),
                            };
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: format!("unknown encoding name - {shown}"),
                            }));
                        }
                    },
                };
                // Real transcoding pairs (Utf8 ↔ registry) — handled
                // before the ascii-only shortcut so multi-byte text
                // actually converts.
                #[cfg(feature = "_encoding_full")]
                {
                    use crate::value::EncodingTag;
                    let src = rs.encoding.get();
                    if let (EncodingTag::Utf8, EncodingTag::Other(idx)) = (src, target) {
                        let text = rs.to_string_lossy();
                        match crate::encoding_full::encode_from_utf8(idx, &text, replace.as_deref()) {
                            Ok(bytes) => {
                                let v = Value::new_str_bytes(bytes);
                                if let Value::Str(ref ns) = v {
                                    ns.encoding.set(target);
                                }
                                self.stack.push(v);
                                return Ok(true);
                            }
                            Err((cp, to)) => {
                                return Err(self.trap(RubyError::HostException {
                                    class_name: "Encoding::UndefinedConversionError".to_string(),
                                    message: format!("U+{cp:04X} from UTF-8 to {to}"),
                                }));
                            }
                        }
                    }
                    if let (EncodingTag::Other(idx), EncodingTag::Utf8) = (src, target) {
                        let bytes = rs.content.borrow().clone();
                        if let Some(text) = crate::encoding_full::decode_to_utf8(idx, &bytes) {
                            self.stack.push(Value::new_str(text));
                            return Ok(true);
                        }
                    }
                }
                let _ = &replace;
                let src = rs.encoding.get();
                let bytes = rs.content.borrow().clone();
                let ascii_only = bytes.iter().all(|&b| b < 0x80);
                if src != target && !ascii_only {
                    // Real transcoding territory — E1 declines with
                    // CRuby's error class AND message shape (Tier 2
                    // converts instead): the first offending unit is
                    // shown as `"\xNN"` when the source is
                    // byte-oriented, or `U+XXXX` when the source is
                    // UTF-8 (CRuby names the codepoint).
                    let disp = |t: EncodingTag| match t {
                        EncodingTag::Utf8 => "UTF-8",
                        EncodingTag::UsAscii => "US-ASCII",
                        EncodingTag::Binary => "ASCII-8BIT",
                        EncodingTag::Other(_) => "OTHER",
                    };
                    let (from, to) = (disp(src), disp(target));
                    let offender = if src == EncodingTag::Utf8 {
                        std::str::from_utf8(&bytes)
                            .ok()
                            .and_then(|t| t.chars().find(|c| !c.is_ascii()))
                            .map(|c| format!("U+{:04X}", c as u32))
                    } else {
                        None
                    };
                    let offender = offender.unwrap_or_else(|| {
                        let b = bytes.iter().copied().find(|&b| b >= 0x80).unwrap_or(0);
                        format!("\"\\x{b:02X}\"")
                    });
                    return Err(self.trap(RubyError::HostException {
                        class_name: "Encoding::UndefinedConversionError".to_string(),
                        message: format!("{offender} from {from} to {to}"),
                    }));
                }
                let v = Value::new_str_bytes(bytes);
                if let Value::Str(ref ns) = v {
                    ns.encoding.set(target);
                }
                self.stack.push(v);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Resolve a `force_encoding` / `encode`-style encoding
    /// argument — a String name (case-insensitive, CRuby's
    /// fold set) or a preamble `Encoding` instance (read its
    /// `@name`) — to an E1 tag. `None` = unknown name (caller
    /// raises CRuby's ArgumentError shape).
    pub(crate) fn resolve_encoding_arg(&mut self, arg: &Value) -> Option<crate::value::EncodingTag> {
        use crate::value::EncodingTag;
        let name: String = match arg {
            Value::Str(s) => s.to_string_lossy(),
            Value::Object(id) => {
                let inst = self.heap.instance(*id);
                if inst.class.name != "Encoding" {
                    return None;
                }
                let name_id = self.interner.intern("@name");
                match inst.ivars.get(&name_id) {
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    _ => return None,
                }
            }
            _ => return None,
        };
        match name.to_ascii_uppercase().as_str() {
            "UTF-8" => Some(EncodingTag::Utf8),
            "US-ASCII" | "ASCII" => Some(EncodingTag::UsAscii),
            "ASCII-8BIT" | "BINARY" => Some(EncodingTag::Binary),
            #[cfg(feature = "_encoding_full")]
            other => crate::encoding_full::find(other),
            #[cfg(not(feature = "_encoding_full"))]
            _ => None,
        }
    }

    /// `Integer#chr(encoding)` — CRuby widens the accepted range and
    /// returns the codepoint encoded in the requested encoding. We
    /// intercept when the sole argument is an `Encoding` instance and
    /// branch on its name, handling the three encodings the preamble
    /// exposes exactly as CRuby does:
    ///
    /// - **UTF-8** — 0..=U+10FFFF (minus surrogates) → multi-byte UTF-8.
    /// - **US-ASCII** — 0..=0x7F → the byte; 0x80..=0xFF → RangeError
    ///   "invalid codepoint 0xN in US-ASCII"; otherwise out-of-range.
    /// - **ASCII-8BIT** (BINARY) — 0..=0xFF → that single raw byte
    ///   (binary-safe: `RStr` is byte-backed, so 0x80..=0xFF — which is
    ///   not valid UTF-8 — round-trips through `#bytes`); else out-of-range.
    ///
    /// Returns `Ok(true)` when handled (result pushed), `Ok(false)` to
    /// fall through to the stateless `numeric_call` (which raises the
    /// CRuby-shaped "no implicit conversion of X into Encoding" TypeError
    /// for a non-Encoding argument), and `Err(RangeError)` for an
    /// out-of-range / invalid codepoint.
    ///
    /// Needs `&mut self` because recognising the `Encoding` object and
    /// reading its `@name` require the heap that the stateless
    /// `numeric_call` free function can't see — same rationale as
    /// `try_push_string_encoding`.
    pub(crate) fn try_push_int_chr_encoding(
        &mut self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> Result<bool, Trap> {
        let cp = match recv {
            Value::Int(n) if name == "chr" && args.len() == 1 => *n,
            _ => return Ok(false),
        };
        // The arg must be an Encoding instance; otherwise fall through so
        // the stateless path raises the TypeError. Use `real_class_of`,
        // not `class_of`: the latter returns the singleton class when one
        // is installed, so an Encoding with a singleton method (e.g.
        // `def Encoding::UTF_8.foo; end`) would otherwise miss this check.
        let enc_id = match &args[0] {
            Value::Object(id) if self.heap.real_class_of(*id).name == "Encoding" => *id,
            _ => return Ok(false),
        };
        let name_sym = self.interner.intern("@name");
        let enc_name = match self.heap.instance(enc_id).ivars.get(&name_sym) {
            Some(Value::Str(s)) => s.to_string_lossy(),
            // Not a recognisable Encoding instance — fall through.
            _ => return Ok(false),
        };

        // `out_of_range` is built lazily (only when a bound is actually
        // exceeded) — the success path is hot (e.g. JSON `\u` decoding)
        // and shouldn't allocate an error string it never uses.
        let out_of_range = || format!("{cp} out of char range");
        match enc_name.as_str() {
            "UTF-8" => {
                if !(0..=0x10_FFFF).contains(&cp) {
                    return Err(self.trap(RubyError::RangeError { msg: out_of_range() }));
                }
                match char::from_u32(cp as u32) {
                    Some(c) => {
                        let mut s = String::with_capacity(c.len_utf8());
                        s.push(c);
                        self.stack.push(Value::new_str(s));
                        Ok(true)
                    }
                    // In range but not a Unicode scalar value (a surrogate).
                    None => Err(self.trap(RubyError::RangeError {
                        msg: format!("invalid codepoint 0x{cp:X} in UTF-8"),
                    })),
                }
            }
            "US-ASCII" => {
                if !(0..=0xFF).contains(&cp) {
                    return Err(self.trap(RubyError::RangeError { msg: out_of_range() }));
                }
                if cp > 0x7F {
                    return Err(self.trap(RubyError::RangeError {
                        msg: format!("invalid codepoint 0x{cp:X} in US-ASCII"),
                    }));
                }
                self.stack.push(Value::new_str((cp as u8 as char).to_string()));
                Ok(true)
            }
            "ASCII-8BIT" => {
                if !(0..=0xFF).contains(&cp) {
                    return Err(self.trap(RubyError::RangeError { msg: out_of_range() }));
                }
                // A single raw byte (binary-safe via the byte-backed RStr).
                self.stack.push(Value::new_str_bytes(vec![cp as u8]));
                Ok(true)
            }
            // Some other (unmodelled) encoding — fall through.
            _ => Ok(false),
        }
    }

    /// Re-entrant dispatch entry for C extensions calling back into
    /// Ruby via `rb_funcall*`. Invokes `recv.method(args)` through
    /// the normal `do_call` path, leaving the result on the stack
    /// where the caller can pop it.
    ///
    /// Setup mirrors what the compiler emits for a Ruby-side
    /// `recv.method(args)`: push the receiver, then each argument,
    /// then call `do_call` with `no_recv = false`. After `do_call`
    /// the result sits on top of the operand stack — pop and return.
    ///
    /// `cache_id = u16::MAX` is a sentinel that
    /// `lookup_method_cached` treats as "no cache slot": the
    /// `idx < call_caches.len()` guard naturally fails (the table
    /// is bounded by the number of compiled `Op::Call` instructions
    /// — nowhere near 65535 in any realistic program), so both the
    /// read and writeback paths short-circuit. Without this sentinel
    /// a hard-coded `cache_id = 0` would poison whichever compiled
    /// call site got slot 0 — that site would silently dispatch
    /// whatever class/method the C ext last invoked. Future work:
    /// allocate a per-`(recv-class, method)` cache for cext calls
    /// if profiling shows the uncached path matters.
    #[cfg(all(feature = "cext", not(target_os = "wasi")))]
    pub(crate) fn cext_invoke_method(
        &mut self,
        recv: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, Trap> {
        let name_id = self.interner.intern(method);
        let argc = args.len();
        self.stack.push(recv);
        for a in args {
            self.stack.push(a);
        }
        self.do_call(
            name_id,
            argc,
            /* no_recv = */ false,
            /* cache_id = */ u16::MAX,
        )?;
        Ok(self
            .stack
            .pop()
            .expect("ICE: cext_invoke_method: do_call produced no result"))
    }



    /// Look up `method_missing` on `recv`'s class chain. If found,
    /// prepend the missed `name_id` as a Symbol arg and invoke it
    /// (pushing a frame); returns `Ok(true)` so the caller can
    /// `return Ok(())` instead of raising. Returns `Ok(false)` when
    /// the receiver doesn't carry a `method_missing` (or isn't a
    /// `Value::Object`) — caller proceeds to raise NoMethodError.
    ///
    /// Scope of this PoC: only Object receivers (user instances).
    /// Primitive receivers (Int, Str, …) skip the lookup — adding
    /// per-primitive class chains is a follow-up.
    pub(crate) fn try_method_missing(
        &mut self,
        recv: &Value,
        name_id: SymId,
        args: Vec<Value>,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        let mm_id = self.interner.intern("method_missing");
        // Class / Module receivers consult the singleton-method
        // chain — same lookup `Klass.foo` itself uses — so a
        // `method_missing` defined in a module extended into the
        // class (`extend M` / `Module.new { extend M }`, which
        // sinatra-contrib/Extension does to record DSL calls)
        // is reachable. Pre-fix this returned `Ok(false)` for
        // every non-Object receiver, swallowing valid Class/
        // Module method_missing handlers — surfaced as
        // "undefined method `X' for Class" instead of the
        // user's recorder firing.
        let m = match recv {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, mm_id)
            }
            Value::Class(cls) => {
                self.lookup_class_singleton_method(cls, mm_id)
            }
            _ => None,
        };
        let m = match m {
            Some(m) => m,
            None => return Ok(false),
        };
        let mut new_args = Vec::with_capacity(args.len() + 1);
        new_args.push(Value::Sym(name_id));
        new_args.extend(args);
        self.invoke_method_with_block(m, recv.clone(), new_args, block)?;
        Ok(true)
    }

    /// `respond_to?`'s fallback: when normal resolution misses, CRuby
    /// consults a user-defined `respond_to_missing?(name, include_priv)`
    /// — the companion to `method_missing` for proxy / DSL objects. If
    /// the receiver's class defines it, invoke it (its boolean result
    /// becomes the `respond_to?` result, same as how `try_method_missing`
    /// lets `method_missing`'s value flow through) and return `Ok(true)`;
    /// otherwise `Ok(false)` so the caller pushes the default `false`.
    pub(crate) fn try_respond_to_missing(
        &mut self,
        recv: &Value,
        name_sym: SymId,
        include_private: bool,
    ) -> Result<bool, Trap> {
        let rtm_id = self.interner.intern("respond_to_missing?");
        let m = match recv {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, rtm_id)
            }
            Value::Class(cls) => self.lookup_class_singleton_method(cls, rtm_id),
            // Primitives: a `respond_to_missing?` reopened onto the
            // value's core class (rare, but mirror the reopened-method
            // path `responds_to` itself now honours).
            _ => match self.class_of(recv) {
                Value::Class(cls) => self.lookup_method_uncached(&cls, rtm_id),
                _ => None,
            },
        };
        match m {
            Some(m) => {
                let args = vec![Value::Sym(name_sym), Value::Bool(include_private)];
                self.invoke_method_with_block(m, recv.clone(), args, None)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Stringify `v` for `p` / `puts` / `print`: if the receiver's class
    /// defines a user `inspect` (when `inspect`) / `to_s` (otherwise)
    /// method, invoke it via dispatch and use its String result; else
    /// fall back to the native `to_inspect` / `to_display`. CRuby's
    /// p/puts/print call inspect/to_s, so a user override must win — but
    /// rubyrs's native conversions don't dispatch. Callers MUST pin the
    /// args first: this runs arbitrary user code (the override) which can
    /// trigger GC, and the `p`/`puts` arg buffer isn't in the root set.
    /// (Only the TOP-LEVEL value dispatches; a custom-inspect object
    /// nested inside an Array/Hash still renders via the native
    /// collection inspect — a documented follow-up.)
    /// CRuby's default `Exception#inspect`: `#<ClassName: message>`, or
    /// bare `#<ClassName>` when the message is empty. Returns `None` when
    /// `recv` is not an Exception instance. Shared by the universal
    /// `inspect` dispatch arm and `stringify_for_output`, so `p exc`
    /// renders the same string as `exc.inspect` (plain `to_inspect`
    /// drops the message — `inspect` is a native arm, not a table method,
    /// so `stringify_for_output`'s method lookup misses it).
    pub(crate) fn exception_inspect_string(&mut self, recv: &Value) -> Option<String> {
        let Value::Object(id) = recv else { return None };
        let cls = match self.class_of(recv) {
            Value::Class(c) => c,
            _ => return None,
        };
        let exc_id = self.interner.intern("Exception");
        let exc_cls = self.classes.get(&exc_id).cloned()?;
        if !super::class_is_a(&cls, &exc_cls) {
            return None;
        }
        let msg_sym = self.interner.intern("@message");
        let msg = self
            .heap
            .instance(*id)
            .ivars
            .get(&msg_sym)
            .cloned()
            .map(|v| v.to_display(&self.heap, &self.interner))
            .unwrap_or_default();
        let cls_name = cls.name.clone();
        // CRuby's `exc_inspect`: an empty message (e.g.
        // `RuntimeError.new("")`) renders as the BARE class name —
        // `"RuntimeError"`, not `"#<RuntimeError>"`. A non-empty message
        // (including the default `.new`-with-no-args message, which
        // equals the class name) uses the `#<ClassName: message>` form.
        Some(if msg.is_empty() {
            cls_name.to_string()
        } else {
            format!("#<{cls_name}: {msg}>")
        })
    }

    /// Cycle-safe, dispatch-aware `inspect` renderer for Array / Hash.
    /// `Value::to_inspect` is a non-dispatching LEAF renderer: it recurses
    /// in Rust (overflowing the native stack on a self-referential
    /// collection — `a = []; a << a`) and renders each element via
    /// `to_inspect` again, so a custom `inspect` override or an
    /// Exception's message is lost inside a collection. This walks the
    /// container element-by-element, dispatches each element's real
    /// `inspect` through `stringify_for_output`, and emits `[...]` /
    /// `{...}` when it re-enters a container already on `inspect_stack`
    /// — matching CRuby's recursive-inspect behaviour. Scalars (and
    /// plain Objects) delegate straight to `stringify_for_output`.
    pub(crate) fn inspect_value(&mut self, v: &Value) -> Result<String, Trap> {
        match v {
            Value::Array(id) => {
                if self.inspect_stack.contains(id) {
                    return Ok("[...]".to_string());
                }
                // Pin the array so its heap-ref elements stay rooted
                // across element-inspect dispatch (which may alloc + GC);
                // marking the array transitively marks its elements.
                let mut g = PinGuard::new(self);
                g.pin(v.clone());
                let elems = g.vm.heap.array(*id).clone();
                g.vm.inspect_stack.push(*id);
                let mut parts = Vec::with_capacity(elems.len());
                for e in &elems {
                    match g.vm.inspect_value(e) {
                        Ok(s) => parts.push(s),
                        Err(t) => { g.vm.inspect_stack.pop(); return Err(t); }
                    }
                }
                g.vm.inspect_stack.pop();
                Ok(format!("[{}]", parts.join(", ")))
            }
            Value::Hash(id) => {
                if self.inspect_stack.contains(id) {
                    return Ok("{...}".to_string());
                }
                let mut g = PinGuard::new(self);
                g.pin(v.clone());
                let entries: Vec<(Value, Value)> = g.vm.heap.hash(*id)
                    .iter().map(|(k, val)| (k.clone(), val.clone())).collect();
                g.vm.inspect_stack.push(*id);
                let mut parts = Vec::with_capacity(entries.len());
                for (k, val) in &entries {
                    let vs = match g.vm.inspect_value(val) {
                        Ok(s) => s,
                        Err(t) => { g.vm.inspect_stack.pop(); return Err(t); }
                    };
                    // CRuby 3.4+: Symbol keys use `name: value` shorthand
                    // (quoted when not bareword-safe); other keys use the
                    // `key => value` rocket form.
                    let part = if let Value::Sym(sid) = k {
                        let name = g.vm.interner.resolve(*sid).to_string();
                        if crate::heap::sym_needs_quotes(&name) {
                            format!("\"{name}\": {vs}")
                        } else {
                            format!("{name}: {vs}")
                        }
                    } else {
                        let ks = match g.vm.inspect_value(k) {
                            Ok(s) => s,
                            Err(t) => { g.vm.inspect_stack.pop(); return Err(t); }
                        };
                        format!("{ks} => {vs}")
                    };
                    parts.push(part);
                }
                g.vm.inspect_stack.pop();
                Ok(format!("{{{}}}", parts.join(", ")))
            }
            _ => self.stringify_for_output(v, true),
        }
    }

    pub(crate) fn stringify_for_output(&mut self, v: &Value, inspect: bool) -> Result<String, Trap> {
        // Collections route through the cycle-safe, per-element
        // dispatching renderer so `p [exc]` / `p [custom]` keep each
        // element's real `inspect` and self-referential containers don't
        // overflow the stack. (`to_s` of a collection — inspect=false —
        // still uses the leaf renderer below; CRuby's Array#to_s aliases
        // inspect, but cyclic `to_s` is far rarer and kept as-is.)
        if inspect && matches!(v, Value::Array(_) | Value::Hash(_)) {
            return self.inspect_value(v);
        }
        // CRuby `puts` / `print` write String args directly
        // (rb_io_puts: T_STRING short-circuits before any to_s
        // dispatch) — a user `String#to_s` override is NOT consulted.
        // `p` (inspect=true) still dispatches a user String#inspect.
        if !inspect && let Value::Str(_) = v {
            return Ok(v.to_display(&self.heap, &self.interner));
        }
        let meth_id = self.interner.intern(if inspect { "inspect" } else { "to_s" });
        let m = match v {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_uncached(&cls, meth_id)
            }
            Value::Class(cls) => self.lookup_class_singleton_method(cls, meth_id),
            _ => match self.class_of(v) {
                Value::Class(cls) => self.lookup_method_uncached(&cls, meth_id),
                _ => None,
            },
        };
        let native = |vm: &Self| if inspect {
            v.to_inspect(&vm.heap, &vm.interner)
        } else {
            v.to_display(&vm.heap, &vm.interner)
        };
        let Some(m) = m else {
            // No table/override method. `inspect` on an Exception has no
            // table entry (it's a native dispatch arm), so route it
            // through the shared renderer to keep the `@message` —
            // otherwise `p exc` would drop it. Everything else keeps the
            // native to_inspect / to_display.
            if inspect && let Some(s) = self.exception_inspect_string(v) {
                return Ok(s);
            }
            return Ok(native(self));
        };
        let pre = self.frames.len();
        self.invoke_method(m, v.clone(), vec![])?;
        self.dispatch_until(pre)?;
        let r = self.stack.pop().unwrap_or(Value::Nil);
        Ok(match &r {
            Value::Str(s) => s.to_string_lossy(),
            // A non-String result (a misbehaving override) — render it
            // natively rather than erroring, matching the lenient spirit.
            _ => if inspect {
                r.to_inspect(&self.heap, &self.interner)
            } else {
                r.to_display(&self.heap, &self.interner)
            },
        })
    }

    /// Last-resort fallback for collection receivers: route a method NOT
    /// handled by any native arm to the Enumerable MODULE's `each`-based
    /// implementation, run with `recv` as self. Array / Hash / Range
    /// DON'T `include Enumerable` in rubyrs's registry on purpose — their
    /// iterators are native primitives that aren't in the method table,
    /// so an `include Enumerable` would let `Enumerable#sort` (in the
    /// table) SHADOW the native sort, and `Enumerable#sort`'s `to_a.sort`
    /// would recurse forever. Routing here AFTER the native arms keeps
    /// native precedence while still exposing the Enumerable methods
    /// CRuby inherits but rubyrs has no primitive for (minmax / minmax_by
    /// / each_entry / `min(n)` / `max(n)` / `sum`-with-block). Returns
    /// `Ok(true)` when it dispatched (result on the stack).
    pub(crate) fn try_enumerable_module_fallback(
        &mut self,
        recv: &Value,
        name_id: SymId,
        args: Vec<Value>,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        if !matches!(recv, Value::Array(_) | Value::Hash(_) | Value::Range(_)) {
            return Ok(false);
        }
        let enum_sym = self.interner.intern("Enumerable");
        let Some(enum_mod) = self.classes.get(&enum_sym).cloned() else {
            return Ok(false);
        };
        let Some(m) = self.lookup_method_uncached(&enum_mod, name_id) else {
            return Ok(false);
        };
        self.invoke_method_with_block(m, recv.clone(), args, block)?;
        Ok(true)
    }



    /// Invoke a registered host fn (either v1 or v2 slot).
    ///
    /// V1 stashes `*mut Vm` via `with_vm_ptr_set` so a cext-style
    /// re-entrant `rb_funcall` can find the running VM (ADR 0013) —
    /// but only on builds where that re-entry channel actually
    /// exists, i.e. `all(feature = "cext", not(target_os = "wasi"))`.
    /// With `--no-default-features` (or on wasi) `with_vm_ptr_set`
    /// itself lives inside the cfg'd-off `mod cext`, so the V1 arm
    /// just calls `host(args)` directly; see the in-fn comment for
    /// the migration site if a non-cext V1 host ever needs TLS-Vm
    /// access. V1 closures hold no Rust borrow of `self` during the
    /// call, so the raw-ptr reborrow inside cext is the only access
    /// path and aliasing is well-defined.
    ///
    /// V2 deliberately does NOT call `with_vm_ptr_set`. The V2
    /// closure holds a `HostCtx` that borrows `&self.heap` for the
    /// duration of the call; if we *also* re-aimed CURRENT_VM_PTR at
    /// `self` and the closure reborrowed it as `&mut Vm`, that
    /// reborrow would alias the live `&self.heap` borrow — any heap
    /// mutation during the inner call could realloc the backing
    /// `Vec<HeapObj>` and dangle slices returned by
    /// `ctx.resolve_array` / `resolve_hash`.
    ///
    /// Note that `CURRENT_VM_PTR` may already be non-null on entry
    /// (an outer v1/cext frame set it), so the V2 arm is NOT
    /// asserting "TLS is null." The actual boundary is: the TLS is
    /// `pub(crate)`, so an external v2 closure has no language-level
    /// path to read it — the unsafe re-entry channel is unreachable
    /// to user code in the V2 slot. Skipping the overwrite here is
    /// the closing brick: even an internal future v2 helper would
    /// have to explicitly opt into touching the TLS, which is the
    /// point at which the soundness review is expected.
    ///
    /// cext bridges register as V1, so nothing legitimate needs the
    /// ptr from the V2 arm.
    fn invoke_host_fn(&mut self, slot: HostFnSlot, args: &[Value]) -> Result<Value, Trap> {
        match slot {
            HostFnSlot::V1(host) => {
                // V1 contract under the `cext` feature gate:
                // `with_vm_ptr_set` parks the Vm pointer in TLS so a
                // cext-bridge V1 host can re-enter the VM through
                // rb_funcallv. With cext off there is no rb_funcall
                // path to need it and `with_vm_ptr_set` itself lives
                // inside `mod cext`, so we just call the host body
                // directly. Today every legitimate V1 caller IS a
                // cext bridge (see the V1/V2 doc above), so the
                // contract change is invisible at runtime; if a
                // future non-cext V1 host needs TLS-Vm access, this
                // is the site to move `with_vm_ptr_set` out of
                // `mod cext` and lift the cfg gate.
                // Set the TLS Vm pointer for re-entrant V1
                // host fns:
                //   - cext bridge: rb_funcallv callback
                //     dispatches through CURRENT_VM_PTR
                //   - _http_server battery: per-request Ruby
                //     block invocation reads CURRENT_VM_PTR
                //     to access &mut Vm for step_block
                // Either feature enables the machinery; both
                // share the same TLS slot defined in
                // super::vm_ptr.
                #[cfg(any(
                    all(feature = "cext", not(target_os = "wasi")),
                    feature = "_http_server",
                    feature = "_fiber",
                    feature = "_json_native",
                    feature = "_yaml_native",
                    feature = "_liquid_native",
                    feature = "_sqlite",
                ))]
                {
                    let vm_ptr: *mut Vm = self;
                    with_vm_ptr_set(vm_ptr, || host(args))
                }
                #[cfg(not(any(
                    all(feature = "cext", not(target_os = "wasi")),
                    feature = "_http_server",
                    feature = "_fiber",
                    feature = "_json_native",
                    feature = "_yaml_native",
                    feature = "_liquid_native",
                    feature = "_sqlite",
                )))]
                { host(args) }
            }
            HostFnSlot::V2(host) => {
                let ctx = HostCtx::new(&self.heap, &self.interner);
                host(&ctx, args)
            }
        }
    }

    /// Resolve a `Symbol` / `String` arg into a SymId for the ivar
    /// name, validating it against an **ASCII-only subset** of
    /// CRuby's ivar-name grammar: `@[A-Za-z_][A-Za-z0-9_]*`.
    /// CRuby accepts some non-ASCII identifier characters too;
    /// rubyrs takes the conservative ASCII subset because no
    /// caller in the surfaced surface needs Unicode ivar names —
    /// see `is_valid_ivar_name` for the precise grammar. Rejects:
    ///   - bare `@` (no body)
    ///   - `@@x` (class var — two `@`)
    ///   - `@1` (digit start after `@`)
    ///   - `@foo?` / `@foo=` / `@foo!` (suffixes that work for
    ///     methods but not for ivars)
    ///
    /// String path enforces `Config::max_symbols` so untrusted code
    /// can't grow the interner unbounded via
    /// `instance_variable_{get,set}("@x#{i}", ...)` in a loop.
    /// Non-Symbol-non-String args raise TypeError matching the
    /// shape `parse_send_target` uses for `send` / `__send__`.
    fn resolve_ivar_name_arg(&mut self, arg: &Value) -> Result<SymId, Trap> {
        match arg {
            Value::Sym(id) => {
                let resolved = self.interner.resolve(*id);
                if is_valid_ivar_name(resolved) {
                    return Ok(*id);
                }
                // Happy path returns above with no allocation. Only
                // the error path materialises the message; build the
                // String here so the borrow of `resolved` is dropped
                // before the `&mut self` call to `trap`.
                let msg = format!(
                    "'{}' is not allowed as an instance variable name",
                    resolved,
                );
                Err(self.trap(RubyError::NameError { msg }))
            }
            Value::Str(s) => {
                let raw = s.to_string_lossy();
                if !is_valid_ivar_name(&raw) {
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("'{}' is not allowed as an instance variable name", raw),
                    }));
                }
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&raw) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Ok(self.interner.intern(&raw))
            }
            other => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
        }
    }

    /// Parse the first arg of a `send` / `__send__` call as the
    /// target method name. Symbol passes through; String is
    /// interned (CRuby's transparent `to_sym` on the name arg).
    /// Anything else returns the CRuby-shape TypeError
    /// (`<inspect> is not a symbol nor a string`); zero args
    /// returns the CRuby-shape ArgumentError. Shared by all four
    /// send-recogniser sites (`do_call` / `do_call_block`, each
    /// with their no_recv and recv arms) so the validation +
    /// error formatting can't drift between paths.
    fn parse_send_target(&mut self, args: &[Value]) -> Result<SymId, Trap> {
        if args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1+)".into(),
            }));
        }
        match &args[0] {
            Value::Sym(s) => Ok(*s),
            Value::Str(s) => {
                // Same `Config::max_symbols` cap as `String#to_sym`
                // (vm/string.rs:971) — without this, untrusted code
                // could grow the interner unbounded by calling
                // `send("dyn_#{i}")` in a loop. Existing symbols
                // always re-resolve; only fresh names count.
                let name = s.to_string_lossy();
                if let Some(max) = self.max_symbols
                    && !self.interner.contains(&name) && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                Ok(self.interner.intern(&name))
            }
            other => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
        }
    }

    /// Primitive-receiver fast-path for the handful of zero-arg
    /// methods (`String#length` / `#size` / `#to_s`, `Integer#to_s`
    /// / `#inspect`) that profiling showed dominate fizzbuzz-shape
    /// loops. Returns true after pushing the result; false if the
    /// receiver / name / arity don't match and `do_call` should
    /// continue through normal dispatch.
    ///
    /// Currently safe to call after `take_bypass_visibility()`
    /// because every arm matches a primitive Value (no visibility
    /// model). Adding an arm for a receiver with a user-Class
    /// method table requires threading the bypass flag through —
    /// see the comment at the call site in `do_call`.
    ///
    /// Reopen soundness: each arm is gated on the
    /// `fast_prim_str_safe` / `fast_prim_int_safe` flags (same
    /// `method_gen`-revalidated pass as `try_fast_index`), so a
    /// user `String#length` / `Integer#to_s` reopen wins through
    /// the slow path's primitive-receiver user-method gate. Before
    /// the flags existed these arms silently shadowed reopens.
    fn try_fast_primitive(&mut self, name_id: SymId, argc: usize, no_recv: bool) -> bool {
        if no_recv || argc != 0 {
            return false;
        }
        if self.fast_index_checked_gen != self.method_gen {
            self.fast_index_revalidate();
        }
        let v = {
            let recv = self
                .stack
                .last()
                .expect("ICE: stack underflow before do_call receiver");
            match recv {
                // `frozen?` on shapes whose answer is a constant of
                // the shape (or the RStr flag). Gated on the GLOBAL
                // prim_reopen_mask — any user reopen anywhere on the
                // primitive classes turns these off and the
                // reopen-precedence gate in do_call takes over.
                // Jekyll's Utils.duplicate_frozen_values probes
                // frozen? per data-hash value per merge (~60k probes
                // per 1k-site build; measured ~100ns through full
                // dispatch vs 4ns in CRuby). Array/Hash deliberately
                // NOT here: their (no-op freeze) answer stays with
                // the canonical collection arms.
                Value::Str(a)
                    if name_id == self.sym_frozen_q && self.prim_reopen_mask == 0 =>
                {
                    Value::Bool(a.frozen.get())
                }
                Value::Int(_) | Value::Sym(_) | Value::Float(_) | Value::Bool(_) | Value::Nil
                    if name_id == self.sym_frozen_q && self.prim_reopen_mask == 0 =>
                {
                    Value::Bool(true)
                }
                // `nil?` — same universal-constant shape as `frozen?`,
                // same mask gate ("nil?" is in the universal arm-name
                // list, so any primitive-class reopen flips the mask).
                Value::Str(_) | Value::Int(_) | Value::Sym(_) | Value::Float(_) | Value::Bool(_)
                    if name_id == self.sym_nil_q && self.prim_reopen_mask == 0 =>
                {
                    Value::Bool(false)
                }
                Value::Nil if name_id == self.sym_nil_q && self.prim_reopen_mask == 0 => {
                    Value::Bool(true)
                }
                _ if matches!(recv, Value::Str(_)) && !self.fast_prim_str_safe => return false,
                _ if matches!(recv, Value::Int(_)) && !self.fast_prim_int_safe => return false,
                Value::Str(a) if name_id == self.sym_length || name_id == self.sym_size => {
                    // Registry-tagged strings count under their own
                    // encoding — fall to the slow path (the
                    // string_call arm consults the registry).
                    #[cfg(feature = "_encoding_full")]
                    if matches!(a.encoding.get(), crate::value::EncodingTag::Other(_)) {
                        return false;
                    }
                    Value::Int(a.char_count() as i64)
                }
                Value::Str(a) if name_id == self.sym_to_s => Value::Str(a.clone()),
                // Mirrors string.rs's canonical arm byte-for-byte:
                // `(Value::Str(a), "empty?", []) => a.borrow().is_empty()`
                // (byte-emptiness is encoding-independent). Reopen-gated
                // via fast_prim_str_safe — `empty?` is in the
                // revalidate name list.
                Value::Str(a) if name_id == self.sym_empty_q => {
                    Value::Bool(a.borrow().is_empty())
                }
                Value::Int(n) if name_id == self.sym_to_s || name_id == self.sym_inspect => {
                    crate::vm::numeric::integer_to_s_value(*n)
                }
                _ => return false,
            }
        };
        self.stack.pop();
        self.stack.push(v);
        true
    }

    /// Collection-index fast path: `h[key]` / `a[int]` (and the
    /// `[]=` write twins) on a PLAIN (untagged) Hash / Array
    /// short-circuit the full dispatch preamble (name resolve + arm
    /// probing — ~150ns/call, 8× CRuby's `opt_aref`, measured hot in
    /// both Jekyll's read phase — data-hash probes in
    /// `populate_categories` / `merge_data!` — and Liquid's render
    /// scopes). Soundness gates, in order:
    ///   - refined `[]`/`[]=` detours before the call site
    ///     (`maybe_refined`)
    ///   - a user `[]`/`[]=` anywhere on the Hash/Array ancestor
    ///     chain turns the matching path off via the
    ///     `method_gen`-revalidated flags (`fast_index_revalidate`
    ///     below; per-name flags so e.g. a `[]=`-only reopen leaves
    ///     the read path fast)
    ///   - subclass instances (class_tag set) fall through to the
    ///     subclass-override gate
    ///   - Hash misses on a defaulted hash (scalar default or
    ///     default-block) fall through so the canonical hash.rs arm
    ///     owns those semantics; Array non-Int args (Range / two-arg
    ///     slice arrive as other shapes) fall through likewise
    ///   - writes: capped Vms (`max_value_bytes`, embed-only) and
    ///     Array growth / too-negative wrap (nil-padding, byte cap,
    ///     IndexError shapes) fall through — the write fast path is
    ///     Hash insert/overwrite + Array IN-BOUNDS overwrite only
    ///
    /// Hit semantics mirror the canonical arms byte-for-byte: Hash
    /// get → `hash_index_lookup` + pair clone / Nil; Array get →
    /// negative-wrap index, out-of-range Nil; Hash set → the same
    /// `hash_insert` the canonical arm calls; both sets evaluate to
    /// the assigned value. No GC-heap allocation on any of these
    /// paths, so no `maybe_gc` (same as the arms they mirror).
    fn try_fast_index(&mut self, name_id: SymId, argc: usize, no_recv: bool) -> bool {
        if no_recv {
            return false;
        }
        let is_get = argc == 1 && name_id == self.sym_index_op;
        let is_set = argc == 2 && name_id == self.sym_index_set_op;
        if !is_get && !is_set {
            return false;
        }
        let n = self.stack.len();
        if n < argc + 1 {
            return false;
        }
        if self.fast_index_checked_gen != self.method_gen {
            self.fast_index_revalidate();
        }
        let recv_idx = n - argc - 1;
        match &self.stack[recv_idx] {
            Value::Hash(id) => {
                let id = *id;
                if self.heap.hash_class_tag(id).is_some() {
                    return false;
                }
                if is_get {
                    if !self.fast_index_hash_safe {
                        return false;
                    }
                    let v = if let Some(pos) = self.heap.hash_index_lookup(id, &self.stack[n - 1])
                    {
                        self.heap.hash(id)[pos].1.clone()
                    } else {
                        if self.heap.hash_default_value(id).is_some()
                            || self.heap.hash_default_block(id).is_some()
                        {
                            return false;
                        }
                        Value::Nil
                    };
                    self.stack.truncate(recv_idx);
                    self.stack.push(v);
                } else {
                    if !self.fast_index_hash_set_safe {
                        return false;
                    }
                    // The canonical arm's byte cap only fires when
                    // `max_value_bytes` is set (embed-only); rather
                    // than duplicate the cap logic, capped Vms take
                    // the slow path.
                    if self.max_value_bytes.is_some() {
                        return false;
                    }
                    let v = self.stack[n - 1].clone();
                    let k = self.stack[n - 2].clone();
                    self.heap.hash_insert(id, k, v.clone());
                    self.stack.truncate(recv_idx);
                    self.stack.push(v);
                }
                true
            }
            Value::Array(id) => {
                let id = *id;
                if self.heap.array_class_tag(id).is_some() {
                    return false;
                }
                let Value::Int(i) = self.stack[recv_idx + 1] else {
                    return false;
                };
                if is_get {
                    if !self.fast_index_array_safe {
                        return false;
                    }
                    let a = self.heap.array(id);
                    let idx = if i < 0 { a.len() as i64 + i } else { i };
                    let v = a.get(idx as usize).cloned().unwrap_or(Value::Nil);
                    self.stack.truncate(recv_idx);
                    self.stack.push(v);
                } else {
                    if !self.fast_index_array_set_safe {
                        return false;
                    }
                    // In-bounds overwrites only: growth (idx >= len,
                    // nil-padding + byte cap) and too-negative wrap
                    // (IndexError-class shapes) keep their semantics
                    // in the canonical array.rs arm.
                    let a_len = self.heap.array(id).len() as i64;
                    let idx = if i < 0 { a_len + i } else { i };
                    if idx < 0 || idx >= a_len {
                        return false;
                    }
                    if self.max_value_bytes.is_some() {
                        return false;
                    }
                    let v = self.stack[n - 1].clone();
                    self.heap.array_mut(id)[idx as usize] = v.clone();
                    self.stack.truncate(recv_idx);
                    self.stack.push(v);
                }
                true
            }
            _ => false,
        }
    }

    /// Recompute the `try_fast_index` override flags at the current
    /// `method_gen`. The verdict intentionally uses the same
    /// `lookup_method_uncached` walk (includes / prepends /
    /// superclass chain) that the slow path's primitive-receiver
    /// user-method gate resolves through, on the same class objects
    /// (`classes["Hash"]` / `classes["Array"]` — the Rcs `class_of`
    /// caches), so the two paths can't disagree. Missing class (raw
    /// pre-preamble Vm) → flag stays off → slow path, correct.
    fn fast_index_revalidate(&mut self) {
        self.fast_index_checked_gen = self.method_gen;
        let idx_sym = self.sym_index_op;
        let set_sym = self.sym_index_set_op;
        let hash_sym = self.interner.intern("Hash");
        (self.fast_index_hash_safe, self.fast_index_hash_set_safe) =
            match self.classes.get(&hash_sym).cloned() {
                Some(c) => (
                    self.lookup_method_uncached(&c, idx_sym).is_none(),
                    self.lookup_method_uncached(&c, set_sym).is_none(),
                ),
                None => (false, false),
            };
        let array_sym = self.interner.intern("Array");
        (self.fast_index_array_safe, self.fast_index_array_set_safe) =
            match self.classes.get(&array_sym).cloned() {
                Some(c) => (
                    self.lookup_method_uncached(&c, idx_sym).is_none(),
                    self.lookup_method_uncached(&c, set_sym).is_none(),
                ),
                None => (false, false),
            };
        // `try_fast_primitive` twins — same gen, same walk.
        let str_sym = self.interner.intern("String");
        self.fast_prim_str_safe = match self.classes.get(&str_sym).cloned() {
            Some(c) => {
                self.lookup_method_uncached(&c, self.sym_length).is_none()
                    && self.lookup_method_uncached(&c, self.sym_size).is_none()
                    && self.lookup_method_uncached(&c, self.sym_to_s).is_none()
                    && self.lookup_method_uncached(&c, self.sym_empty_q).is_none()
            }
            None => false,
        };
        let int_sym = self.interner.intern("Integer");
        self.fast_prim_int_safe = match self.classes.get(&int_sym).cloned() {
            Some(c) => {
                self.lookup_method_uncached(&c, self.sym_to_s).is_none()
                    && self.lookup_method_uncached(&c, self.sym_inspect).is_none()
            }
            None => false,
        };
        // Reopen-precedence mask: per primitive class, does the OWN
        // method table hold any name a primitive arm claims? The
        // preamble is audited collision-free
        // (preamble_defines_no_primitive_arm_collisions), so the
        // mask is 0 until a USER reopen lands and the per-call gate
        // in do_call stays a single u8 compare.
        const PRIM_CLASSES: [(u8, &str); 8] = [
            (0, "Integer"), (1, "Float"), (2, "String"), (3, "Symbol"),
            (4, "NilClass"), (5, "TrueClass"), (5, "FalseClass"), (6, "Rational"),
        ];
        let mut mask = 0u8;
        for (bit, cname) in PRIM_CLASSES {
            let sym = self.interner.intern(cname);
            if let Some(c) = self.classes.get(&sym) {
                let methods = c.methods.borrow();
                if methods.keys().any(|nid| {
                    Self::primitive_arm_name_for_class(cname, self.interner.resolve(*nid))
                }) {
                    mask |= 1 << bit;
                }
            }
        }
        self.prim_reopen_mask = mask;
    }

    /// `no_recv` builtin-or-host fast path. Tries the host-side
    /// builtin table first (`builtin_call` covers `puts` / `p` /
    /// `sprintf` / `require` / ...), then the
    /// `register_fn`-installed host fns. Returns `Ok(true)` if
    /// one of those handled the call (caller should `return
    /// Ok(())` immediately), or `Ok(false)` if neither matched
    /// and `do_call` should fall through to the next arm.
    ///
    /// Extracted from `do_call`'s no_recv preamble per #192
    /// commit 1/5 (the #152 research's first recommendation,
    /// scoped narrower than the research's initial estimate
    /// because the broader 362-431 range turned out to be
    /// interleaved with `try_fast_primitive` and the stack drain;
    /// see #192's commit message for why).
    ///
    /// `suppress_call_result_push` handling stays inside the
    /// helper: `require_relative` (and any future builtin that
    /// could see its caller unwound to an outer `rescue` mid-call)
    /// sets the flag to signal "don't push my return value — the
    /// stack is now the rescue handler's, not yours". Helper
    /// checks + clears the flag (one-shot) just like the inline
    /// code did.
    fn try_dispatch_no_recv_builtin_or_host(
        &mut self,
        name: &str,
        name_id: SymId,
        args: &[Value],
    ) -> Result<bool, Trap> {
        if let Some(res) = self.builtin_call(name, args) {
            let v = res?;
            if self.suppress_call_result_push {
                self.suppress_call_result_push = false;
            } else {
                self.stack.push(v);
            }
            return Ok(true);
        }
        if let Some(host) = self.host_fns.get(&name_id).cloned() {
            let v = self.invoke_host_fn(host, args)?;
            self.stack.push(v);
            return Ok(true);
        }
        Ok(false)
    }

    /// Result of consulting the `send` / `__send__` bypass
    /// recogniser. `Handled` means the helper has already done
    /// all the work (parsed target sym, set
    /// `bypass_visibility_once`, pushed args/recv, recursed
    /// into `do_call`) and the caller should `return` the
    /// contained `Result` immediately. `NotHandled` means the
    /// call isn't a `send` form, or it's a `send` with a user-
    /// defined override on the surrounding self/recv (reserved-
    /// name rule applies only to `__send__`); the helper has
    /// moved `args` and `recv_opt` *back out* so the caller can
    /// continue dispatch.
    ///
    /// See `try_dispatch_send_bypass` for the full doc; #192
    /// commit 2/5.
    fn try_dispatch_send_bypass(
        &mut self,
        name: &str,
        name_id: SymId,
        cache_id: u16,
        args: ArgsBuf,
        recv_opt: Option<Value>,
    ) -> SendBypass {
        // Early out for non-send names — the common case.
        // `public_send` routes through the same machinery; rubyrs
        // doesn't model send-visibility, so it behaves like `send`.
        if !matches!(name, "send" | "__send__" | "public_send") {
            return SendBypass::NotHandled { args, recv_opt };
        }
        // Subject for the user-override check:
        //   - With-recv form: the explicit receiver.
        //   - No-recv form: the surrounding frame's `self_val`
        //     (because `bare_send(:x)` is implicit-self).
        let frame_self_storage;
        let subject: &Value = match &recv_opt {
            Some(r) => r,
            None => {
                frame_self_storage = self.frames.last()
                    .expect("ICE: do_call(no_recv) with empty frames")
                    .self_val
                    .clone();
                &frame_self_storage
            }
        };
        // User override only blocks `send` (the reserved-name
        // rule applies only to `__send__`). Same lookup shape
        // as the originals at the two inlined sites.
        let user_override = name == "send" && match subject {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_cached(&cls, name_id, cache_id).is_some()
            }
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
            _ => false,
        };
        if user_override {
            return SendBypass::NotHandled { args, recv_opt };
        }
        // Bypass path. Parse target sym from args[0]; on failure
        // surface the trap through Handled so the caller's `?`
        // sees it.
        let target_sym = match self.parse_send_target(&args) {
            Ok(t) => t,
            Err(e) => return SendBypass::Handled(Err(e)),
        };
        let new_argc = args.len() - 1;
        // Set bypass_visibility BEFORE recursing so the inner
        // do_call's `take_bypass_visibility()` sees it. Note:
        // recursing through the same `do_call` entry preserves
        // the existing setter-then-recurse pattern; the helper
        // does NOT call do_call while still holding any borrow.
        self.bypass_visibility_once = true;
        let no_recv_for_recursion = recv_opt.is_none();
        if let Some(recv) = recv_opt {
            self.stack.push(recv);
        }
        for a in args.into_iter().skip(1) {
            self.stack.push(a);
        }
        SendBypass::Handled(self.do_call(target_sym, new_argc, no_recv_for_recursion, u16::MAX))
    }

    /// Callable intrinsics — dispatch to the `Method` / `Block` /
    /// `BoundMethod` / `UnboundMethod` / `CurriedProc` family.
    ///
    /// Returns [`CallableOutcome::Handled`] if one of the arms
    /// fired (the helper has already pushed any result to the
    /// stack, or has recursed into `do_call` and bubbled its
    /// result via `?`); the caller `do_call` should `return Ok(())`
    /// immediately. Returns [`CallableOutcome::NotHandled { args,
    /// recv }`] if no arm matched; the caller continues with the
    /// rest of dispatch using the returned `args` + `recv`.
    ///
    /// Extracted from `do_call` per the #152 research deliverable;
    /// see #192 commit 3/5 for the migration rationale.
    fn try_dispatch_callable_intrinsics(
        &mut self,
        name: &str,
        _name_id: SymId,
        args: ArgsBuf,
        recv: Value,
    ) -> Result<CallableOutcome, Trap> {
        if let Value::Block(bid) = &recv
            && matches!(name, "call" | "[]" | "()" | "yield") {
                // CRuby exposes block invocation under four names:
                // `.call(args)`, `.()` (already lowered to `call`
                // by parsers but kept here defensively), `[args]`
                // bracket form, and `.yield(args)` (mostly a
                // documentation alias). All four route the same
                // way: invoke the block, drive until its frame
                // returns, leave the result on the stack.
                let pre_frames = self.frames.len();
                self.invoke_block(*bid, args.into_vec())?;
                self.dispatch_until(pre_frames)?;
                // ADR 0024 Phase A.6 round 2: stored Proc tried
                // to `break` after returning to its caller. There
                // was no Op::Yield wrapper above the block (it
                // was invoked via `.call`, not `yield`), so
                // `break_signaled` has no observer above. CRuby
                // raises `LocalJumpError: break from proc-closure`.
                if self.break_signaled {
                    self.break_signaled = false;
                    self.sync_control_signals();
                    // Discard the break value the block left on
                    // the stack — it won't be the call's result.
                    self.stack.pop();
                    return Err(self.trap(crate::error::RubyError::LocalJumpError {
                        msg: "break from proc-closure".to_string(),
                    }));
                }
                return Ok(CallableOutcome::Handled);
            }
        // `Proc#arity` — CRuby-shape arity for the block. Block
        // params in rubyrs Tier-1 are only required + rest (no
        // optionals, no keyword params — `compile_block` accepts
        // only `BlockParam::{Single, Destructure, Rest}`), so
        // the formula is:
        //   has_rest → -(n_required + 1)
        //   else     →  n_required
        // The Proto's `rest_param` field is NOT populated for
        // blocks (rest_slot lives on the BlockHandle directly);
        // can't share the `proto_arity` helper used by
        // `Method#arity` / `UnboundMethod#arity` without
        // walking the BlockHandle here. Sinatra's `compile!`
        // (sinatra/base.rb:1810) reads `block.arity` to size
        // the route block's positional bindings. (TRY_RUNS
        // layer #24.)
        if matches!(&recv, Value::Block(_) | Value::CurriedProc(_))
            && name == "arity" && !args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
            }));
        }
        if let Value::Block(bid) = &recv
            && name == "arity" && args.is_empty() {
            let (n_required, has_rest) = {
                let bh = self.heap.block(*bid);
                (bh.n_params as i64, bh.rest_slot.is_some())
            };
            let arity = if has_rest { -(n_required + 1) } else { n_required };
            self.stack.push(Value::Int(arity));
            return Ok(CallableOutcome::Handled);
        }
        // `Proc#source_location` — `[file, line]` of the block's
        // body (CRuby points at the `proc {` line; we report the
        // first op's line, which lands on or just after it — the
        // callers that matter locate the enclosing block by "line
        // within block range", e.g. the rouge-native IR compiler).
        // nil for blocks whose source isn't tracked.
        if let Value::Block(bid) = &recv
            && name == "source_location" && args.is_empty() {
            let proto_idx = self.heap.block(*bid).proto_idx;
            let proto = &self.protos[proto_idx];
            let filename = proto.filename.clone();
            let span = proto.op_spans.first().copied();
            let line = match (span, self.sources.get(filename.as_ref())) {
                (Some(sp), Some(src)) => {
                    crate::error::line_col(src, sp.byte_offset).0 as i64
                }
                _ => 0,
            };
            if line == 0 {
                self.stack.push(Value::Nil);
            } else {
                let arr = vec![
                    Value::new_str(filename.to_string()),
                    Value::Int(line),
                ];
                let id = self.heap.alloc(crate::heap::HeapObj::Array(arr.into()));
                self.stack.push(Value::Array(id));
            }
            return Ok(CallableOutcome::Handled);
        }
        // `CurriedProc#arity` — CRuby returns -1 for any curried
        // proc/lambda regardless of remaining required slots
        // (the curried wrapper accepts a variable number of args
        // per `.call` site as the partial application grows).
        // Without this arm, `proc { |a| }.curry.arity` falls
        // through to NoMethodError even though `Proc#arity`
        // works — inconsistent now that the Block arm exists.
        // (Copilot review #263 round 3.)
        if let Value::CurriedProc(_) = &recv
            && name == "arity" && args.is_empty() {
            self.stack.push(Value::Int(-1));
            return Ok(CallableOutcome::Handled);
        }
        // `Object#method(:name)` — capture (recv, name_id) into a
        // BoundMethod heap object. Returned Value can be `.call`'d
        // (handled in the next arm) or stored. Args must be a
        // single Symbol; CRuby also accepts String but we keep
        // the subset narrow for now.
        //
        // GC rooting: `recv` here came from the operand-stack pop
        // at the top of `do_call` and lives only in this Rust
        // local. The `maybe_gc` below would otherwise sweep its
        // heap slot (e.g. a fresh `Squared.new.method(:call)`
        // where the Squared instance has no other root), then the
        // alloc'd BoundMethod would store a stale ObjId. Repro:
        // `proc_curry_compose.rb` under STRESS_GC=1 — the
        // BoundMethod survives but its `recv` points at a Dead
        // slot, panicking later in `class_of`.
        if matches!(name, "method" | "singleton_method" | "public_method")
            && args.len() == 1
            && let Value::Sym(bound_name_id) = &args[0] {
                // Snapshot the resolved Method at capture time so
                // `bm.call` survives a subsequent `remove_method`
                // (CRuby parity, matches the `instance_method` arm).
                //
                // Use the DISPATCH class (`heap.class_of`) for
                // Object receivers — that's the class chain that
                // a regular `recv.foo` would walk, and it
                // honours singleton methods (`def obj.foo; ...`).
                // `Vm::class_of` reports the *real* class for
                // script-visible `obj.class`, which skips the
                // eigenclass; using that here would snapshot the
                // real-class body and silently invoke it instead
                // of the singleton override.
                let snapshot = match &recv {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_uncached(&cls, *bound_name_id)
                    }
                    // Class receivers store their class-method
                    // entries in `cls.singleton_methods`, not in
                    // the per-instance method table. Use the
                    // same helper as explicit `cls.foo`
                    // dispatch so `K.public_method(:cls_m)`
                    // finds class methods correctly. The old
                    // `Vm::class_of(K)` would return the `Class`
                    // class and miss every class-method
                    // (PR #314 cycle-2).
                    Value::Class(cls) => self.lookup_class_singleton_method(cls, *bound_name_id),
                    _ => match self.class_of(&recv) {
                        Value::Class(cls) => self.lookup_method_uncached(&cls, *bound_name_id),
                        _ => None,
                    },
                };
                // `singleton_method` / `public_method` narrow the
                // snapshot match relative to plain `method`:
                //
                //   * `singleton_method(:name)` — installed
                //     DIRECTLY on the eigenclass (Value::Object)
                //     or in `cls.singleton_methods` (Value::Class).
                //     Inherited methods from the receiver's real
                //     class don't count; raise NameError if the
                //     method is reachable via dispatch but isn't
                //     a singleton entry.
                //
                //   * `public_method(:name)` — same chain as
                //     `method`, but raises NameError if the
                //     captured Method's visibility is Private
                //     OR Protected. Only Public passes. Also
                //     raises NameError when the method is
                //     entirely missing (snapshot is None) so the
                //     getter fails at capture time rather than
                //     at the later `.call`.
                if name == "singleton_method" {
                    // Walk the eigenclass's own table PLUS its
                    // transitive includes / prepends so methods
                    // brought in by `obj.extend(M)` or
                    // `class << self; prepend M; end` are
                    // reachable — matches `Object#singleton_methods`
                    // (vm/dispatch.rs:4550 walk_chain). Without
                    // this widening, `c.singleton_methods` would
                    // list `:m` while `c.singleton_method(:m)`
                    // raised NameError, contradicting itself.
                    // PR #314 cycle-4.
                    fn chain_has(
                        c: &std::rc::Rc<crate::value::Class>,
                        target: crate::intern::SymId,
                        visited: &mut Vec<*const crate::value::Class>,
                    ) -> bool {
                        let ptr = std::rc::Rc::as_ptr(c);
                        if visited.contains(&ptr) { return false; }
                        visited.push(ptr);
                        if c.methods.borrow().contains_key(&target) { return true; }
                        for inc in c.includes.borrow().iter() {
                            if chain_has(inc, target, visited) { return true; }
                        }
                        for pre in c.prepends.borrow().iter() {
                            if chain_has(pre, target, visited) { return true; }
                        }
                        false
                    }
                    let is_singleton = match &recv {
                        Value::Object(id) => {
                            if let crate::heap::HeapObj::Instance(inst) = self.heap.get(*id) {
                                inst.singleton_class.as_ref().is_some_and(|sc| {
                                    let mut visited = Vec::new();
                                    chain_has(sc, *bound_name_id, &mut visited)
                                })
                            } else {
                                false
                            }
                        }
                        Value::Class(c) => {
                            // Class-level singleton table; also
                            // honour `singleton_prepends` walked
                            // the same way `singleton_methods`
                            // does for Class receivers.
                            if c.singleton_methods.borrow().contains_key(bound_name_id) {
                                true
                            } else {
                                let mut visited = Vec::new();
                                c.singleton_prepends.borrow().iter().any(|p| {
                                    chain_has(p, *bound_name_id, &mut visited)
                                })
                            }
                        }
                        _ => false,
                    };
                    if !is_singleton {
                        let name_str = self.interner.resolve(*bound_name_id).to_string();
                        let recv_str = recv.to_inspect(&self.heap, &self.interner);
                        return Err(self.trap(RubyError::NameError {
                            msg: format!(
                                "undefined singleton method '{}' for '{}'",
                                name_str, recv_str,
                            ),
                        }));
                    }
                } else if name == "public_method" {
                    // CRuby rejects both Private and Protected
                    // here (only Public passes). Treat the
                    // captured snapshot's visibility as the
                    // primary signal; if no snapshot exists
                    // (primitive arms, built-ins like
                    // Class#new, universal arms like `to_s` /
                    // `inspect`), consult `responds_to` to tell
                    // truly-missing-method from
                    // missing-Method-entry-but-dispatchable.
                    let vis = snapshot.as_ref().map(|m| m.visibility.get());
                    let label = match vis {
                        Some(crate::value::Visibility::Private) => Some("private"),
                        Some(crate::value::Visibility::Protected) => Some("protected"),
                        Some(crate::value::Visibility::Public) => None,
                        // No Method entry — defer to
                        // `responds_to` (PR #314 cycle-2). If
                        // the receiver actually dispatches this
                        // name, we shouldn't lie via NameError.
                        None => {
                            if self.responds_to(&recv, *bound_name_id, true) {
                                None
                            } else {
                                // Sentinel — same shape as
                                // CRuby's "undefined method"
                                // branch below.
                                Some("__missing__")
                            }
                        }
                    };
                    if let Some(tag) = label {
                        let name_str = self.interner.resolve(*bound_name_id).to_string();
                        // For Class receivers, use the eigenclass-
                        // shell form `#<Class:K>` (matches CRuby).
                        // For Object receivers, use the class of
                        // the instance. Falling back to
                        // `self.class_of(&recv)` would return
                        // "Class" / "Module" for Class receivers
                        // — the cycle-3 review caught this giving
                        // `for class 'Class'` instead of
                        // `for class 'K'` / `'#<Class:K>'`.
                        let cls_name = match &recv {
                            Value::Class(c) => format!("#<Class:{}>", c.name),
                            _ => match self.class_of(&recv) {
                                Value::Class(c) => c.name.clone(),
                                _ => "Object".to_string(),
                            },
                        };
                        let msg = if tag == "__missing__" {
                            format!(
                                "undefined method '{}' for class '{}'",
                                name_str, cls_name,
                            )
                        } else {
                            format!(
                                "method '{}' for class '{}' is {}",
                                name_str, cls_name, tag,
                            )
                        };
                        return Err(self.trap(RubyError::NameError { msg }));
                    }
                }
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(recv.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::BoundMethod {
                    recv: recv.clone(),
                    name_id: *bound_name_id,
                    method: snapshot,
                });
                g.vm.stack.push(Value::BoundMethod(id));
                return Ok(CallableOutcome::Handled);
            }
        // `bm.call(args)` / `bm.()` / `bm[args]` — dispatch the
        // captured method on the captured receiver. We re-enter
        // `do_call` recursively with the bound recv pushed below
        // the args, the captured name interned, and the original
        // argc.
        // `bm.unbind` — strip the receiver, keep (class_of(recv),
        // name_id). The captured class is the receiver's class at
        // unbind time; CRuby technically captures the *owner* (the
        // class that defined the method), but for our subset
        // `class_of` is the closest approximation and roundtrips
        // through `bind` correctly for the common shapes.
        if let Value::BoundMethod(bid) = &recv && name == "unbind" && args.is_empty() {
            // Inherit the snapshot the BoundMethod was carrying;
            // if it has none (legacy values constructed before
            // the snapshot field, or `method` capture sites that
            // synthesise a transient BM), look up live from the
            // receiver's class. The resulting UnboundMethod
            // survives a subsequent `remove_method` on either
            // side of the round-trip.
            let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id, method } => (recv.clone(), *name_id, method.clone()),
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            // Use the DISPATCH class (heap.class_of) for Object
            // receivers so the captured class reflects any
            // singleton class on `recv`. Otherwise the
            // UnboundMethod would carry the REAL class plus a
            // singleton-method snapshot — `um.bind(other)` would
            // pass the is_a fence (other is_a real_class) and
            // silently invoke the singleton body on an unrelated
            // instance. With the dispatch class, the captured
            // class IS the singleton class, and is_a on a
            // different instance correctly fails (singleton
            // classes only contain the original instance via
            // class_is_a).
            let cls = match &bm_recv {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&bm_recv) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: "cannot unbind method on a value without a class".into(),
                    })),
                },
            };
            let snapshot = bm_method.or_else(|| self.lookup_method_uncached(&cls, bm_name_id));
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::UnboundMethod {
                class: cls,
                name_id: bm_name_id,
                method: snapshot,
            });
            self.stack.push(Value::UnboundMethod(id));
            return Ok(CallableOutcome::Handled);
        }
        // `ubm.bind(obj)` — reconstitute a BoundMethod, checking
        // that `obj` is_a? the captured class. Raises TypeError on
        // mismatch, matching CRuby.
        if let Value::UnboundMethod(uid) = &recv && name == "bind" && args.len() == 1 {
            let (cap_class, cap_name_id, cap_method) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id, method } => {
                    (class.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args.into_vec();
            let target = args.swap_remove(0);
            // Use dispatch class (heap.class_of) for Object
            // targets — matches the eigenclass-aware capture in
            // unbind. Otherwise binding a singleton-method
            // UnboundMethod back to its ORIGINAL instance would
            // fail the is_a fence (target's real class doesn't
            // walk through the singleton class).
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            // Kernel is the universally-bindable sentinel — CRuby
            // models it as a Module included in Object, so every
            // value is_a Kernel. Modules in general also accept
            // any receiver: CRuby's
            // `Module#instance_method(:foo).bind(obj)` succeeds
            // regardless of whether obj's class includes the
            // module (verified against 3.4). Class.instance_method
            // stays strict — `obj.is_a?(cls)` required. Same
            // fence as the `bind_call` arm below.
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            // GC rooting: `target` came from `args.swap_remove(0)`,
            // which itself was drained from the operand stack at the
            // top of `do_call`. It now lives only in this Rust local
            // — not in `self.stack`, not in any frame's locals. The
            // `maybe_gc` below would otherwise sweep its heap slot
            // (Greeter.new in `kernel_instance_method.rb` under
            // STRESS_GC=1), and the BoundMethod's `recv` would point
            // at a Dead slot. Same fix shape as `Object#method` and
            // `invoke_block` rest-slot in commit 86db73d.
            let mut g = crate::vm::PinGuard::new(self);
            g.pin(target.clone());
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            // Propagate the snapshot from the UnboundMethod —
            // a later `bm.call` after a `remove_method` on the
            // captured class still invokes the original body.
            let id = g.vm.heap.alloc(HeapObj::BoundMethod {
                recv: target,
                name_id: cap_name_id,
                method: cap_method,
            });
            g.vm.stack.push(Value::BoundMethod(id));
            return Ok(CallableOutcome::Handled);
        }
        // `ubm.bind_call(recv, *args)` — CRuby 2.7+ fused
        // bind-then-call: identical to `ubm.bind(recv).call(*args)`
        // but without allocating a transient BoundMethod heap
        // object. Re-uses the same is_a check (with the Kernel
        // sentinel) and dispatches the captured method with
        // `recv` pushed below the args.
        //
        // Motivating consumer: tilt-2.7.0
        // `lib/tilt/template.rb:496` calls
        // `method.bind_call(scope, **locals, &block)` per render —
        // the fast path that replaces older `bind(scope).call(...)`
        // shapes. Without this arm tilt falls through to
        // NoMethodError on every render.
        //
        // Arity: at least 1 arg (the receiver); extra args + block
        // are forwarded to the captured method.
        // `Method#bind_call(other, *args)` — mirror of
        // `UnboundMethod#bind_call`, but starts from a bound
        // Method (which carries a receiver). Equivalent to
        // `m.unbind.bind(other).call(*args)` but doesn't
        // allocate intermediate UnboundMethod / Method
        // wrappers. The is-a fence + snapshot-preferred
        // dispatch are identical to the UnboundMethod arm
        // below — see the longer comment block there for the
        // singleton-class / Module-mixin / Kernel edge cases.
        if let Value::BoundMethod(bid) = &recv && name == "bind_call" && !args.is_empty() {
            let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id, method } => (recv.clone(), *name_id, method.clone()),
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            // Capture class from the original receiver — same
            // dispatch-class shape `unbind` uses, so singleton
            // methods round-trip correctly.
            let cap_class = match &bm_recv {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&bm_recv) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: "cannot bind_call on a Method whose receiver has no class".into(),
                    })),
                },
            };
            let mut args = args.into_vec();
            let target = args.remove(0);
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind_call argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind_call argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            let m = match bm_method.or_else(|| self.lookup_method_uncached(&cap_class, bm_name_id)) {
                Some(m) => m,
                None => {
                    let mname = self.interner.resolve(bm_name_id).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cap_class.name),
                    }));
                }
            };
            self.invoke_method(m, target, args)?;
            return Ok(CallableOutcome::Handled);
        }
        if let Value::BoundMethod(_) = &recv && name == "bind_call" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..)".into(),
            }));
        }
        if let Value::UnboundMethod(uid) = &recv && name == "bind_call" && !args.is_empty() {
            let (cap_class, cap_name_id, cap_method) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id, method } => {
                    (class.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args.into_vec();
            let target = args.remove(0);
            // Dispatch class for Object targets — mirrors the
            // eigenclass-aware capture in unbind so a
            // singleton-method UnboundMethod can bind_call back
            // to its original receiver.
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind_call argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            // Skip the is-a fence when:
            // (a) captured class is Kernel — every value is_a
            //     Kernel in CRuby; we don't model the Kernel
            //     Module-mixin and use this sentinel to match.
            // (b) captured class is any Module — CRuby's
            //     `Module#instance_method(:foo).bind_call(obj)`
            //     accepts ANY obj, not just instances of classes
            //     that include the module. Verified against 3.4:
            //     `module M; def foo; end; end;
            //      M.instance_method(:foo).bind_call(Object.new)`
            //     succeeds and runs `foo`. Note the captured
            //     method is invoked directly via `invoke_method`
            //     on the resolved Method (snapshot-preferred,
            //     `cap_class`-chain fallback) — no name-based
            //     lookup on the receiver's class chain happens,
            //     so the receiver doesn't need to have `foo`
            //     defined on its class.
            //     `Class.instance_method(:foo).bind_call(obj)`
            //     stays strict — `obj.is_a?(cls)` required.
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind_call argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            // Prefer the snapshot taken at capture time — tilt's
            // pattern of capture→remove→bind_call would otherwise
            // miss the now-removed entry. Fall back to live chain
            // lookup when no snapshot exists (e.g. UnboundMethod
            // values created from `unbind` paths that pre-date
            // the snapshot field).
            let m = match cap_method.or_else(|| self.lookup_method_uncached(&cap_class, cap_name_id)) {
                Some(m) => m,
                None => {
                    let mname = self.interner.resolve(cap_name_id).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cap_class.name),
                    }));
                }
            };
            self.invoke_method(m, target, args)?;
            return Ok(CallableOutcome::Handled);
        }
        if let Value::UnboundMethod(_) = &recv && name == "bind_call" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..)".into(),
            }));
        }
        // `m.to_proc` — explicit conversion to a Proc. Equivalent
        // to the implicit `&m` coercion: routes through the same
        // `coerce_callable_to_block` forwarder so calling the
        // resulting Proc splats its args back into `bm.call(...)`.
        if let Value::BoundMethod(bid) = &recv
            && name == "to_proc" && args.is_empty() {
                let bm_id = *bid;
                let id = self.coerce_callable_to_block(Value::BoundMethod(bm_id))?;
                self.stack.push(Value::Block(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m.curry` / `m.curry(n)` — host-side partial application.
        // Returns a CurriedProc that gathers args across successive
        // `.call` invocations until `target_arity` is reached, then
        // invokes the underlying with the full arg list. `class_of`
        // reports CurriedProc as `Proc`, matching CRuby.
        if matches!(&recv, Value::BoundMethod(_) | Value::Block(_))
            && name == "curry" && args.len() <= 1 {
                let target_arity: u16 = if let Some(Value::Int(n)) = args.first() {
                    if *n < 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("negative arity for curry ({})", n),
                        }));
                    }
                    if *n > u16::MAX as i64 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("curry arity out of range ({})", n),
                        }));
                    }
                    *n as u16
                } else if let Value::BoundMethod(bid) = &recv {
                    let (bm_recv, m_name_id) = {
                        let (r, n) = self.heap.bound_method(*bid);
                        (r.clone(), n)
                    };
                    let class = match self.class_of(&bm_recv) {
                        Value::Class(c) => c,
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: "Method receiver has no resolvable class".into(),
                        })),
                    };
                    match self.lookup_method_uncached(&class, m_name_id) {
                        Some(m) => self.protos[m.proto_idx].n_required_positional,
                        None => return Err(self.trap(RubyError::ArgumentError {
                            msg: "cannot curry a method with unknown arity (builtin)".into(),
                        })),
                    }
                } else if let Value::Block(bid) = &recv {
                    // Proc#curry — derive arity from the underlying
                    // proto's required-positional count. Rest / kw
                    // are not supported as auto-arity for curry; user
                    // can still pass an explicit arity hint above.
                    let bh = self.heap.block(*bid);
                    let proto = &self.protos[bh.proto_idx];
                    if bh.rest_slot.is_some() && proto.n_required_positional == 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "cannot curry a proc with only rest params (pass explicit arity)".into(),
                        }));
                    }
                    proto.n_required_positional
                } else {
                    unreachable!()
                };
                // Pin `recv` (the underlying BoundMethod / Proc):
                // it was popped from the operand stack by do_call, so
                // it has no GC root by the time maybe_gc fires. Same
                // root-hole shape as the BoundMethod-coerce-to-Block
                // fix in PR #45 (5874798 / 50867c5).
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(recv.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::CurriedProc {
                    underlying: recv.clone(),
                    gathered: Vec::new(),
                    target_arity,
                });
                g.vm.stack.push(Value::CurriedProc(id));
                return Ok(CallableOutcome::Handled);
            }
        // `cp.call(args)` — append to gathered; invoke if arity hit,
        // else return a new CurriedProc carrying the appended state.
        if let Value::CurriedProc(cid) = &recv
            && matches!(name, "call" | "[]" | "()") {
                let (underlying, gathered, arity) = {
                    let (u, g, a) = self.heap.curried_proc(*cid);
                    (u.clone(), g.clone(), a)
                };
                let mut combined = gathered;
                combined.extend(args);
                if combined.len() >= arity as usize {
                    let argc = combined.len();
                    self.stack.push(underlying);
                    for a in combined { self.stack.push(a); }
                    let call_sym = self.interner.intern("call");
                    self.do_call(call_sym, argc, false, u16::MAX)?;
                    return Ok(CallableOutcome::Handled);
                }
                // Same pin-the-underlying pattern as the curry-on-Method
                // branch above. `combined` may also contain heap-typed
                // arg values that are only held in this Rust-local Vec;
                // pinning the underlying alone is enough because the
                // mark phase walks CurriedProc's contents only after
                // alloc — but the new alloc's reading the SAME Vec, so
                // we need both pinned across the maybe_gc call.
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(underlying.clone());
                for v in &combined { g.pin(v.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::CurriedProc {
                    underlying,
                    gathered: combined,
                    target_arity: arity,
                });
                g.vm.stack.push(Value::CurriedProc(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m >> other` / `m << other` — function composition.
        // `(m >> g).(x) == g.(m.(x))`; `(m << g).(x) == m.(g.(x))`.
        // Both sides must be callable — BoundMethod or Block. The
        // result is a Block (Proc) that splats `*args` through the
        // chain in the right order.
        if matches!(&recv, Value::BoundMethod(_) | Value::Block(_))
            && matches!(name, ">>" | "<<") && args.len() == 1 {
                let mut args = args.into_vec();
                let other = args.swap_remove(0);
                if !matches!(&other, Value::BoundMethod(_) | Value::Block(_)) {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "compose argument must be a Method or Proc (got {})",
                            other.type_name(),
                        ),
                    }));
                }
                let (outer, inner) = if name == ">>" {
                    (other, recv)
                } else {
                    (recv, other)
                };
                let id = self.coerce_compose_to_block(outer, inner)?;
                self.stack.push(Value::Block(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m.hash` — Integer hash derived from receiver identity
        // (ObjId / value / Rc-ptr address) + name_id. Two
        // BoundMethods compared equal under `Method#==` must
        // collide; that's the only invariant CRuby promises. The
        // mix below is wrapping_add + wrapping_mul to be cheap
        // and avoid raising.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "hash" && args.is_empty() {
                let h: i64 = match &recv {
                    Value::BoundMethod(bid) => {
                        // Mirror the BoundMethod ==/eql? resolution
                        // chain so hash agrees with equality:
                        // recv_identity + (snapshot Rc-ptr, falling
                        // back to live lookup, then to `name`).
                        // Without this, post-redefine BoundMethods
                        // that compare unequal under the new == arm
                        // would still collide on hash — violating
                        // `a.eql?(b) ⇒ a.hash == b.hash` in the
                        // opposite direction.
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let recv_h = method_recv_hash(&r);
                        let key = snap.clone().or_else(|| match self.class_of(&r) {
                            Value::Class(c) => self.lookup_method_uncached(&c, n),
                            _ => None,
                        });
                        let method_h = match key {
                            Some(m) => std::rc::Rc::as_ptr(&m) as i64,
                            None => n.0 as i64,
                        };
                        recv_h.wrapping_mul(0x9E3779B1).wrapping_add(method_h)
                    }
                    Value::UnboundMethod(uid) => {
                        // Mirror `eql?`'s identity: hash the
                        // underlying Method's Rc pointer. Prefer
                        // the capture-time snapshot so hash agrees
                        // with the other capture-preserving arms
                        // (bind_call, source_location) — UnboundMethod
                        // semantics pin to the resolution at capture
                        // time, not the live class table. Two
                        // UnboundMethods sharing the same definition
                        // (e.g. `C.instance_method(:foo)` and
                        // `D.instance_method(:foo)` for `D < C`'s
                        // inherited foo) satisfy
                        // `a.eql?(b) ⇒ a.hash == b.hash`. Falls back
                        // to a live `lookup_method_uncached`, then to
                        // the captured-class pointer — eql? takes
                        // the same fallback chain, so hash stays
                        // consistent in every branch.
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        let key = match snap.or_else(|| self.lookup_method_uncached(&cls, n)) {
                            Some(m) => std::rc::Rc::as_ptr(&m) as i64,
                            None => std::rc::Rc::as_ptr(&cls) as i64,
                        };
                        key.wrapping_mul(0x9E3779B1).wrapping_add(n.0 as i64)
                    }
                    _ => unreachable!(),
                };
                self.stack.push(Value::Int(h));
                return Ok(CallableOutcome::Handled);
            }
        // `m.source_location` — three shapes:
        //   - User-defined methods: `[filename, lineno]` derived
        //     from the proto's first op_span via the Vm-side
        //     `sources` mirror; falls back to lineno 0 if the
        //     source text isn't available (rare — synthesised
        //     protos for forwarders / preamble eval).
        //   - Synth builtins with `source_label = Some(label)`
        //     (Kernel reflection records): `[label, line]` where
        //     label is the static "<internal:kernel>" string and
        //     line is the meta's placeholder.
        //   - Synth builtins with `source_label = None`
        //     (BasicObject reflection records): `nil`. CRuby
        //     reports nil for these C-defined methods even though
        //     the Kernel set returns a label — we mirror.
        //   - Methods with no snapshot (none-of-the-above
        //     fallback): `nil`.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "source_location" && args.is_empty() {
                // Prefer the snapshot Method so introspection
                // survives a subsequent `remove_method` between
                // capture and the source_location query.
                let (class, m_name_id, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let snap = snap.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => { self.stack.push(Value::Nil); return Ok(CallableOutcome::Handled); }
                        };
                        (cls, n, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                let m = match snapshot.or_else(|| self.lookup_method_uncached(&class, m_name_id)) {
                    Some(m) => m,
                    None => { self.stack.push(Value::Nil); return Ok(CallableOutcome::Handled); }
                };
                // Builtin Methods carry their own source_location
                // label (e.g. `"<internal:kernel>"`) rather than a
                // real proto's filename. The proto_idx on a builtin
                // is a placeholder; reading `self.protos[0].filename`
                // would surface an unrelated file.
                if let Some(meta) = &m.builtin {
                    // `None` source_label → nil. CRuby's behavior
                    // for some C-defined methods (e.g.
                    // BasicObject's __id__).
                    let Some(label) = meta.source_label else {
                        self.stack.push(Value::Nil);
                        return Ok(CallableOutcome::Handled);
                    };
                    let filename_str = Value::new_str(label.to_string());
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::Array(vec![filename_str, Value::Int(meta.source_line)].into()));
                    self.stack.push(Value::Array(id));
                    return Ok(CallableOutcome::Handled);
                }
                let proto = &self.protos[m.proto_idx];
                let filename = proto.filename.clone();
                let first_offset = proto.op_spans.first().map(|s| s.byte_offset).unwrap_or(0);
                let line: u32 = self.sources.get(&*filename)
                    .map(|src| crate::error::line_col(src, first_offset).0)
                    .unwrap_or(0);
                let filename_str = Value::new_str(filename.to_string());
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(vec![filename_str, Value::Int(line as i64)].into()));
                self.stack.push(Value::Array(id));
                return Ok(CallableOutcome::Handled);
            }
        // `m.owner` — the class that defined the resolved Method
        // (CRuby's `Method#owner` / `UnboundMethod#owner`). Walks
        // the ancestor chain to find where the method actually
        // lives; falls back to the captured class for builtins
        // (whose primitive_call backing has no Method record).
        //
        // `m.receiver` — the captured recv on a BoundMethod.
        // UnboundMethod#receiver raises NoMethodError, matching
        // CRuby (it has no receiver to give).
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && matches!(name, "owner" | "receiver") && args.is_empty() {
                if name == "receiver" {
                    return match &recv {
                        Value::BoundMethod(bid) => {
                            let (r, _) = self.heap.bound_method(*bid);
                            let r = r.clone();
                            self.stack.push(r);
                            Ok(CallableOutcome::Handled)
                        }
                        Value::UnboundMethod(_) => Err(self.trap(RubyError::NoMethodError {
                            kind: crate::error::NoMethodErrorKind::Missing,
                            method: "receiver".into(),
                            recv_type: std::borrow::Cow::Borrowed("UnboundMethod"),
                        })),
                        _ => unreachable!(),
                    };
                }
                // owner: resolve Method through snapshot (or live
                // lookup as fallback) and prefer its
                // `defining_class.upgrade()` over the captured
                // class.
                let (cap_class, m_name_id, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let snap = snap.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, n, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                let owner = match snapshot.or_else(|| self.lookup_method_uncached(&cap_class, m_name_id)) {
                    Some(m) => m.defining_class.as_ref()
                        .and_then(|w| w.upgrade())
                        .unwrap_or_else(|| cap_class.clone()),
                    None => cap_class.clone(),
                };
                self.stack.push(Value::Class(owner));
                return Ok(CallableOutcome::Handled);
            }
        // `m.name` — returns the captured method-name Symbol.
        // Same shape for BoundMethod and UnboundMethod; aliased
        // methods report the alias name (CRuby parity — the
        // captured name is what `.method(:x)` was called with).
        // Arity-check inside the arm rather than via the guard so
        // excess-arg calls raise ArgumentError (CRuby parity)
        // instead of falling through to NoMethodError.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "name" {
                if !args.is_empty() {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0)",
                            args.len()
                        ),
                    }));
                }
                let nid = match &recv {
                    Value::BoundMethod(bid) => self.heap.bound_method(*bid).1,
                    Value::UnboundMethod(uid) => self.heap.unbound_method(*uid).1,
                    _ => unreachable!(),
                };
                self.stack.push(Value::Sym(nid));
                return Ok(CallableOutcome::Handled);
            }
        // `m.original_name` — returns the Method's pre-alias name.
        // For a method defined as `def foo` and captured as
        // `.method(:foo)`, equal to `name`. For an alias
        // (`alias_method :bar, :foo`), the captured BoundMethod
        // reports `name == :bar` but `original_name == :foo` (CRuby
        // parity — the original-def Symbol is preserved through the
        // shared Rc<Method>). Falls back to the captured name when
        // the underlying Method has no recorded original name
        // (rare — only for synthesised Methods that predate this
        // wiring).
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "original_name" {
                if !args.is_empty() {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0)",
                            args.len()
                        ),
                    }));
                }
                let (cap_class, captured_name, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let snap = snap.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, n, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                let resolved = snapshot
                    .or_else(|| self.lookup_method_uncached(&cap_class, captured_name));
                let orig = resolved
                    .as_ref()
                    .and_then(|m| m.original_name)
                    .unwrap_or(captured_name);
                self.stack.push(Value::Sym(orig));
                return Ok(CallableOutcome::Handled);
            }
        // `m.super_method` — returns the Method/UnboundMethod that
        // `super` would dispatch to, or nil if no super definition
        // exists. CRuby parity: walks past the captured Method's
        // defining class and resolves the name against that class's
        // ancestor chain. For BoundMethod the result is bound to
        // the same receiver; for UnboundMethod it's anchored on the
        // super-defining class.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && name == "super_method" {
                if !args.is_empty() {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0)",
                            args.len()
                        ),
                    }));
                }
                let (cap_class, m_name_id, snapshot, recv_opt) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (r, n, snap) = self.heap.bound_method_full(*bid);
                        let r = r.clone();
                        let snap = snap.clone();
                        let cls = match self.class_of(&r) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, n, snap, Some(r))
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap, None)
                    }
                    _ => unreachable!(),
                };
                // Resolve the current Method's defining class —
                // snapshot first (capture-time anchor), then live
                // lookup as fallback. Builtin methods have no
                // resolvable defining class → super_method is nil.
                let cur_method = snapshot
                    .or_else(|| self.lookup_method_uncached(&cap_class, m_name_id));
                let defining_class = cur_method.as_ref()
                    .and_then(|m| m.defining_class.as_ref())
                    .and_then(|w| w.upgrade());
                // Walk the receiver's (or captured class's) full
                // ancestor chain — prepend → own → include → super
                // — past the defining class, returning the next
                // (class, method) that defines `m_name_id`.
                // Required for include/prepend cases:
                //   class A; def foo; end; prepend M_overrides_foo; end
                //   A.new.method(:foo).super_method → A#foo
                // and `class B < P; include M_overrides_foo; end`
                // → P#foo, neither of which would be reachable via
                // a plain `defining_class.superclass` walk (Modules
                // have no superclass).
                let super_resolved = defining_class.and_then(|dc| {
                    self.lookup_super_method_uncached(&cap_class, m_name_id, &dc)
                });
                match super_resolved {
                    Some((super_cls, super_method)) => {
                        let mut g = crate::vm::PinGuard::new(self);
                        if let Some(r) = recv_opt.as_ref() { g.pin(r.clone()); }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let id = match recv_opt {
                            Some(r) => g.vm.heap.alloc(HeapObj::BoundMethod {
                                recv: r,
                                name_id: m_name_id,
                                method: Some(super_method),
                            }),
                            None => g.vm.heap.alloc(HeapObj::UnboundMethod {
                                class: super_cls,
                                name_id: m_name_id,
                                method: Some(super_method),
                            }),
                        };
                        let v = match &recv {
                            Value::BoundMethod(_) => Value::BoundMethod(id),
                            Value::UnboundMethod(_) => Value::UnboundMethod(id),
                            _ => unreachable!(),
                        };
                        g.vm.stack.push(v);
                    }
                    None => self.stack.push(Value::Nil),
                }
                return Ok(CallableOutcome::Handled);
            }
        // `m.arity` / `m.parameters` — Method introspection. Walks
        // the captured class chain to find the user-defined Method;
        // if absent (builtin / primitive_call backed), returns
        // CRuby's "fully varadic" signature: arity = -1,
        // parameters = `[[:rest]]`. Same shape for BoundMethod and
        // UnboundMethod.
        if matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_))
            && matches!(name, "arity" | "parameters") && args.is_empty() {
                let (class, m_name_id, snapshot) = match &recv {
                    Value::BoundMethod(bid) => {
                        let (bm_recv, nid, snap) = {
                            let (r, n, snap) = self.heap.bound_method_full(*bid);
                            (r.clone(), n, snap.clone())
                        };
                        let cls = match self.class_of(&bm_recv) {
                            Value::Class(c) => c,
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: "Method receiver has no resolvable class".into(),
                            })),
                        };
                        (cls, nid, snap)
                    }
                    Value::UnboundMethod(uid) => {
                        let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                        (cls, n, snap)
                    }
                    _ => unreachable!(),
                };
                // Prefer the snapshot Method — survives a later
                // remove_method that strips the live entry.
                let m_opt = snapshot.or_else(|| self.lookup_method_uncached(&class, m_name_id));
                let (arity, params_info) = match m_opt {
                    // Builtin Methods (synthesised on Kernel etc.)
                    // carry their introspection metadata directly —
                    // their `proto_idx` is a placeholder. Read from
                    // `builtin` before falling back to the
                    // proto-derived path.
                    Some(ref m) if m.builtin.is_some() => {
                        let meta = m.builtin.as_ref().unwrap();
                        (meta.arity, meta.parameters.clone())
                    }
                    Some(m) => {
                        let proto = &self.protos[m.proto_idx];
                        // Shared `proto_arity` helper carries the
                        // CRuby formula (required-kw bumping,
                        // block-param exclusion, etc.). NOTE:
                        // `Proc#arity` does NOT share this helper
                        // — blocks store rest info on
                        // `BlockHandle`, not on the Proto, so the
                        // block intrinsic arm above computes
                        // arity from the handle directly.
                        let arity = self.proto_arity(m.proto_idx);
                        // Other counts still needed for the
                        // `parameters` build below.
                        let n_req_pos = proto.n_required_positional as usize;
                        let rest_count = proto.rest_param.is_some() as usize;
                        let kw_count = proto.kw_param_defaults.len();
                        let kw_rest_count = proto.kw_rest_param.is_some() as usize;
                        let block_count = proto.block_param.is_some() as usize;
                        let positional_total = proto.params.len()
                            .saturating_sub(rest_count + kw_count + kw_rest_count + block_count);
                        let mut params: Vec<(&'static str, Option<String>)> = Vec::new();
                        for i in 0..n_req_pos {
                            params.push(("req", Some(proto.params[i].clone())));
                        }
                        for i in n_req_pos..positional_total {
                            params.push(("opt", Some(proto.params[i].clone())));
                        }
                        if let Some(rname) = &proto.rest_param {
                            let n = if rname.is_empty() { None } else { Some(rname.clone()) };
                            params.push(("rest", n));
                        }
                        let kw_name_start = positional_total + rest_count;
                        for (i, default) in proto.kw_param_defaults.iter().enumerate() {
                            let kind = if default.is_none() { "keyreq" } else { "key" };
                            params.push((kind, Some(proto.params[kw_name_start + i].clone())));
                        }
                        if let Some(krname) = &proto.kw_rest_param {
                            let n = if krname == "__kw_rest_anon" { None } else { Some(krname.clone()) };
                            params.push(("keyrest", n));
                        }
                        if let Some(bname) = &proto.block_param {
                            // For anonymous `def foo(&)` the sentinel
                            // `"&"` round-trips here as the Symbol
                            // `:&` — matches CRuby exactly, which
                            // also surfaces the anonymous block as
                            // `[[:block, :&]]` (the literal `&` is a
                            // legal Symbol payload, just an unusual
                            // one). No anonymization needed: passing
                            // the sentinel through gives byte-for-
                            // byte parity. NOT analogous to the
                            // `__kw_rest_anon` case above, which
                            // CRuby DOES report as nameless.
                            params.push(("block", Some(bname.clone())));
                        }
                        (arity, params)
                    }
                    // Primitive-backed method with no table entry. The
                    // generic answer is CRuby's fully-variadic -1 /
                    // [[:rest]], but the canonical BINARY OPERATORS are
                    // unambiguously arity 1 on every builtin class
                    // (`5.method(:+).arity == 1`) — report those
                    // correctly; everything else keeps the -1 fallback.
                    None => {
                        let m_name = self.interner.resolve(m_name_id).clone();
                        if matches!(
                            &*m_name,
                            "+" | "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^"
                                | "<<" | ">>" | "<=>" | "==" | "===" | "!="
                                | "<" | "<=" | ">" | ">=" | "eql?"
                        ) {
                            (1i64, vec![("req", None)])
                        } else {
                            (-1i64, vec![("rest", None)])
                        }
                    }
                };
                if name == "arity" {
                    self.stack.push(Value::Int(arity));
                    return Ok(CallableOutcome::Handled);
                }
                // Build [[kind_sym, name_sym?], ...] array. Anonymous
                // rest / kw_rest yields a single-element pair, matching
                // CRuby's `[[:rest]]` / `[[:keyrest]]`.
                //
                // PinGuard across the whole loop so the inner-pair
                // ObjIds in `outer` survive every maybe_gc — without
                // this, under STRESS_GC each iteration's pair slot
                // gets swept (no GC root: `outer` is a Rust-local
                // Vec), the next alloc reuses it, and the final
                // `heap.alloc(HeapObj::Array(outer))` can land on the
                // same recycled slot — yielding a self-referencing
                // Array whose `.inspect` recurses to stack overflow.
                let mut g = crate::vm::PinGuard::new(self);
                let mut outer: Vec<Value> = Vec::with_capacity(params_info.len());
                for (kind, name_opt) in params_info {
                    let kind_sym = g.vm.interner.intern(kind);
                    let mut pair = vec![Value::Sym(kind_sym)];
                    if let Some(n) = name_opt {
                        let nsym = g.vm.interner.intern(&n);
                        pair.push(Value::Sym(nsym));
                    }
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let pid = g.vm.heap.alloc(HeapObj::Array(pair.into()));
                    g.pin(Value::Array(pid));
                    outer.push(Value::Array(pid));
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let aid = g.vm.heap.alloc(HeapObj::Array(outer.into()));
                g.vm.stack.push(Value::Array(aid));
                return Ok(CallableOutcome::Handled);
            }
        if let Value::BoundMethod(bid) = &recv
            && matches!(name, "call" | "[]" | "()") {
                let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                    HeapObj::BoundMethod { recv, name_id, method } => {
                        (recv.clone(), *name_id, method.clone())
                    }
                    _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
                };
                // Snapshot fast path: invoke the captured Method
                // directly so a `remove_method` on the captured
                // class between capture and call doesn't break
                // `bm.call` (CRuby parity, matches the bind_call
                // path).
                if let Some(m) = bm_method {
                    self.invoke_method(m, bm_recv, args.into_vec())?;
                    return Ok(CallableOutcome::Handled);
                }
                let argc = args.len();
                self.stack.push(bm_recv);
                for a in args {
                    self.stack.push(a);
                }
                self.do_call(
                    bm_name_id, argc,
                    /* no_recv = */ false,
                    /* cache_id = */ u16::MAX,
                )?;
                return Ok(CallableOutcome::Handled);
            }
        // No arm matched; return args + recv intact for the caller
        // to continue dispatch.
        Ok(CallableOutcome::NotHandled { args, recv })
    }
    
    /// Class-receiver intrinsics — `cls.[]` (Hash[]) / `cls.new` /
    /// `cls.allocate` / `cls.include` / `cls.prepend` / `cls.extend`
    /// / `cls.private` / `cls.public` / `cls.protected` /
    /// `cls.name` / `cls.superclass` / `cls.method_defined?`.
    ///
    /// Returns [`ClassOutcome::Handled`] if one of the arms
    /// fired; caller `return`s `Ok(())`. Returns
    /// [`ClassOutcome::NotHandled { args, recv }`] if no arm
    /// matched; caller continues with the rest of dispatch.
    ///
    /// Extracted from `do_call` per the #152 research's
    /// Candidate E recommendation, #192 commit 4/5. The
    /// `Class.new` arm integrates with `cext_alloc_func` +
    /// `with_vm_ptr_set` (R1 from the research). Existing
    /// code pre-clones `cls.name` to a String before entering
    /// the cext closure, so no `cls`-borrow conflict surfaces
    /// from the extraction; kept as-is.
    ///
    /// `_name_id` / `_cache_id` are unused today (arms match
    /// on `name: &str`); kept in the signature for forward
    /// compat with future arms that may need them.
    /// Apply `private_class_method` / `public_class_method`: flip the
    /// visibility of the named singleton methods on `target`. Own-
    /// table entries flip their Cell in place; chain-inherited
    /// (superclass-singleton / extended-module) methods get an own-
    /// table COPY carrying the new visibility — flipping the shared
    /// record would leak the change to the parent. A name with no
    /// singleton method anywhere raises NameError (CRuby shape).
    /// Bumps method_gen: the class-singleton inline cache stores the
    /// `Rc<Method>` whose visibility gate is checked at call time,
    /// but the own-table copy path INSERTS a new record that cached
    /// entries would otherwise never see.
    fn apply_class_method_visibility(
        &mut self,
        target: &Rc<crate::value::Class>,
        args: &[Value],
        vis: Visibility,
    ) -> Result<(), Trap> {
        for a in args {
            let mid: SymId = match a {
                Value::Sym(s) => *s,
                Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                _ => continue,
            };
            let own = target.singleton_methods.borrow().get(&mid).cloned();
            if let Some(m) = own {
                m.visibility.set(vis);
            } else if let Some(m) = self.lookup_class_singleton_method(target, mid) {
                let copy = std::rc::Rc::new(crate::value::Method {
                    params: m.params.clone(),
                    proto_idx: m.proto_idx,
                    fixed_arity: m.fixed_arity,
                    defining_class: Some(std::rc::Rc::downgrade(target)),
                    visibility: std::cell::Cell::new(vis),
                    closure: m.closure.clone(),
                    original_name: m.original_name,
                    builtin: m.builtin.clone(),
                });
                target.singleton_methods.borrow_mut().insert(mid, copy);
            }
            // No singleton-method record (e.g. the builtin `new` /
            // `allocate` constructor arms — kramdown's Parser::Base
            // does `private_class_method(:new, :allocate)`): keep
            // the historical no-op for that name. Privatising a
            // BUILTIN class method stays a documented gap; raising
            // NameError here would break real gems whose builtin
            // privatisation we can't yet honour.
        }
        self.method_gen = self.method_gen.wrapping_add(1);
        Ok(())
    }

    /// Enforce private/protected access rules for an Object
    /// receiver dispatch (explicit-receiver path).
    ///
    /// Private: cannot be invoked with an explicit receiver,
    /// except the modern (CRuby 3.x) `self.foo` form where
    /// `self == recv` by ObjId.
    ///
    /// Protected: caller's `self` class must be an instance of
    /// (or descendant of) the method's *defining* class — CRuby's
    /// rule, not the receiver's class.
    ///
    /// `bypass_visibility` is the `send` / `__send__` one-shot
    /// override consumed by `do_call` before this call.
    fn check_method_visibility(
        &self,
        m: &Method,
        recv: &Value,
        name: &str,
        bypass_visibility: bool,
    ) -> Result<(), Trap> {
        let vis = m.visibility.get();
        // Literal-`self` receiver exemption (private methods are
        // callable as `self.foo`). Object identity for instance
        // receivers; Rc identity for Class receivers (`self.helper`
        // inside a class body / `class << self` context, where the
        // method is a private class method via private_class_method).
        let self_recv = matches!(
            (recv, self.frames.last().map(|f| &f.self_val)),
            (Value::Object(rid), Some(Value::Object(sid))) if rid == sid
        ) || matches!(
            (recv, self.frames.last().map(|f| &f.self_val)),
            (Value::Class(rc), Some(Value::Class(sc))) if Rc::ptr_eq(rc, sc)
        );
        if vis == Visibility::Private && !bypass_visibility && !self_recv {
            return Err(self.trap(RubyError::NoMethodError {
                kind: crate::error::NoMethodErrorKind::Private,
                method: name.to_string(),
                recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(recv)),
            }));
        }
        if vis == Visibility::Protected && !bypass_visibility {
            let caller_self = self
                .frames
                .last()
                .map(|f| f.self_val.clone())
                .unwrap_or(Value::Nil);
            let caller_cls = match &caller_self {
                Value::Object(id) => Some(self.heap.class_of(*id)),
                _ => None,
            };
            let defining = m.defining_class.as_ref().and_then(|w| w.upgrade());
            let allowed = match (&caller_cls, &defining) {
                (Some(c), Some(d)) => super::class_is_a(c, d),
                _ => false,
            };
            if !allowed {
                return Err(self.trap(RubyError::NoMethodError {
                    kind: crate::error::NoMethodErrorKind::Protected,
                    method: name.to_string(),
                    recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(recv)),
                }));
            }
        }
        Ok(())
    }

    /// CRuby-shape receiver description for NoMethodError-style
    /// messages. Object instances render as
    /// `"an instance of <ClassName>"` (matches CRuby 3.3+); all
    /// other Value variants fall back to `Value::type_name()`.
    /// Used by the private/protected visibility error sites so
    /// scripts asserting on the message text see the same words
    /// as CRuby. (TRY_RUNS pass-10 layer #5.)
    pub(crate) fn recv_desc_for_error(&self, recv: &Value) -> String {
        match recv {
            Value::Object(id) => {
                // `real_class_of` skips the eigenclass shell.
                // `class_of` would return the singleton class
                // when one has been installed (e.g. via
                // `def obj.foo`), rendering the error as
                // "an instance of #<Class:#<Inner>>" — never
                // what a script wants to see. (Copilot review
                // #291 round 1.)
                //
                // Known gap: CRuby switches *format* when a
                // singleton is installed — it inspects the
                // receiver with its memory address
                // ("for #<Inner:0x000…>") instead of using
                // "an instance of …". That would require us to
                // mirror `Object#inspect` here, including the
                // memory-address suffix. Tier-1 ships the
                // simpler "an instance of <real class>" form;
                // a script that asserts on the inspect-form
                // wording for singleton-bearing receivers
                // sees a known divergence we accept until a
                // real consumer needs it.
                // `try_real_class_of` is the fallible variant
                // so a corrupt `Value::Object(id)` reaching
                // here doesn't panic the host on the failure
                // path — falls back to the generic type tag.
                // (Code-review #291 round 2.)
                match self.heap.try_real_class_of(*id) {
                    Some(cls) => format!("an instance of {}", cls.name),
                    None => recv.type_name().to_string(),
                }
            }
            other => other.type_name().to_string(),
        }
    }

    /// Class-receiver introspection arms — the second Class
    /// cluster deferred from #192 commit 4. Matches when
    /// `recv` is `Value::Class` AND `name` is one of the
    /// `ancestors` / `include?` / `superclass` /
    /// `singleton_class` / `instance_methods` family /
    /// `constants` / `method_defined?` / `undef_method` /
    /// `instance_method` arms. Returns `Ok(true)` when
    /// handled, `Ok(false)` when the receiver isn't a Class
    /// or no arm matched (caller falls through to the
    /// remaining do_call dispatch).
    ///
    /// No cext integration (unlike commit 4's first Class
    /// cluster) — pure runtime introspection. Free of the
    /// R1 borrow-conflict risk that motivated that helper's
    /// pre-cloning discipline.
    fn try_dispatch_class_introspection(
        &mut self,
        name: &str,
        args: &[Value],
        recv: &Value,
    ) -> Result<bool, Trap> {
        let Value::Class(cls_ref) = recv else { return Ok(false); };
        let cls = cls_ref.clone();
        match (name, args) {
            ("ancestors", []) => {
                let chain: Vec<Value> = super::flatten_ancestors(&cls)
                    .into_iter()
                    .map(Value::Class)
                    .collect();
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(chain.into()));
                self.stack.push(Value::Array(id));
                Ok(true)
            }
            ("include?", [Value::Class(m)]) => {
                if !m.is_module {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "wrong argument type Class (expected Module)".to_string(),
                    }));
                }
                let included = super::class_is_a(&cls, m);
                self.stack.push(Value::Bool(included));
                Ok(true)
            }
            ("include?", [other]) => {
                Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "wrong argument type {} (expected Module)",
                        other.type_name(),
                    ),
                }))
            }
            ("superclass", []) => {
                // CRuby: `Module#superclass` raises NoMethodError
                // because modules don't have a superclass chain
                // (Class < Module but Module has no parent slot).
                // BasicObject has no parent and returns nil. User
                // classes return their parent.
                if cls.is_module {
                    // Probe for a user-defined singleton override
                    // first — `def M.superclass; ...; end` (or
                    // `M.singleton_class.prepend(...)`) lets user
                    // code shadow the default raise. Falling through
                    // here lets the normal dispatch chain in
                    // try_dispatch_callable_intrinsics' caller
                    // resolve and invoke the override.
                    let sup_id = self.interner.intern("superclass");
                    if self.lookup_class_singleton_method(&cls, sup_id).is_some() {
                        return Ok(false);
                    }
                    // No override: raise NoMethodError. CRuby
                    // formats this as
                    // "undefined method 'superclass' for module M",
                    // i.e. lowercase "module" + the actual name.
                    // Carry the dynamic name through `recv_type`'s
                    // owned-Cow form so we match CRuby exactly.
                    // Anonymous modules (`Module.new`) have an
                    // empty `cls.name`; CRuby renders these as
                    // `#<Module:0x...>` in the error. We don't
                    // model the object-id placeholder, so use a
                    // stable `"#<Module>"` instead of letting the
                    // message end with a trailing space.
                    let label = if cls.name.is_empty() {
                        "#<Module>".to_string()
                    } else {
                        cls.name.clone()
                    };
                    return Err(self.trap(RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::Missing,
                        method: "superclass".to_string(),
                        recv_type: std::borrow::Cow::Owned(format!("module {}", label)),
                    }));
                }
                let v = match cls.superclass.borrow().clone() {
                    Some(p) => Value::Class(p),
                    None => Value::Nil,
                };
                self.stack.push(v);
                Ok(true)
            }
            // `Class#<` / `<=` / `>` / `>=` — subclass relation. CRuby:
            //   A <  B → true if A is a STRICT descendant of B
            //                 (B appears in A's ancestor chain, A != B)
            //   A <= B → A == B OR A < B
            //   A >  B → B <  A
            //   A >= B → B <= A
            // Unrelated classes return nil (not false!). Wrong-type
            // arg (not a Class/Module) → TypeError. Used by Class#<
            // family in user code; also reachable through tilt
            // fixtures that assert `Subclass < Parent`.
            // Wrong-arity guard — without it, `A.send(:<)` or
            // `A.send(:<, B, C)` would fall through this exact-
            // one-arg arm and surface as NoMethodError. CRuby
            // raises ArgumentError instead.
            ("<" | "<=" | ">" | ">=", args_) if args_.len() != 1 => {
                Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1)", args_.len()),
                }))
            }
            ("<" | "<=" | ">" | ">=", [arg]) => {
                let Value::Class(other) = arg else {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "compared with non class/module".to_string(),
                    }));
                };
                let same = std::rc::Rc::ptr_eq(&cls, other);
                let self_is_desc = !same && super::class_is_a(&cls, other);
                let other_is_desc = !same && super::class_is_a(other, &cls);
                let result = match name {
                    "<"  => if self_is_desc { Value::Bool(true) }
                            else if same || other_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    "<=" => if same || self_is_desc { Value::Bool(true) }
                            else if other_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    ">"  => if other_is_desc { Value::Bool(true) }
                            else if same || self_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    ">=" => if same || other_is_desc { Value::Bool(true) }
                            else if self_is_desc { Value::Bool(false) }
                            else { Value::Nil },
                    _ => unreachable!(),
                };
                self.stack.push(result);
                Ok(true)
            }
            // Lazy eigenclass-shell. The shell carries
            // `singleton_target = Some(Weak(cls))`, which the 3
            // method-install paths consult to redirect installs
            // into `cls.singleton_methods` instead of the shell's
            // own `methods` table. Subsequent calls reuse the
            // cached shell so `A.singleton_class.equal?(A.singleton_class)`
            // holds. Layer #23 of TRY_RUNS pass series.
            //
            // KNOWN GAP — introspection on the shell (e.g.
            // `A.singleton_class.instance_methods(false)`,
            // `A.singleton_class.include?(Mod)`,
            // `A.singleton_class.include(Mod)`) operates on the
            // shell's OWN empty tables; redirected installs are
            // visible only via the real class's
            // singleton-method dispatch chain. Sinatra and the
            // mainstream `singleton_class.class_eval` idiom
            // don't probe the shell reflectively, so this is
            // documented as a Tier-1 divergence rather than
            // fixed by mirroring writes into the shell's
            // tables. (Code-review #253 round 1 #4 / #7 —
            // partial decline.)
            ("singleton_class", []) => {
                let view = {
                    let mut slot = cls.singleton_view.borrow_mut();
                    if let Some(existing) = slot.as_ref() {
                        existing.clone()
                    } else {
                        // Point the shell's superclass at the real
                        // class's own superclass so
                        // `A.singleton_class.ancestors.include?(Object)`
                        // and `A.singleton_class.superclass`
                        // both behave reasonably for code that
                        // walks the metaclass chain — matches the
                        // pre-PR Tier-1 stub's effective behavior
                        // (the stub returned the receiver itself,
                        // so `.superclass` was the real class's
                        // superclass). NOT CRuby's exact metaclass
                        // tower (`#<Class:A> < #<Class:Object> <
                        // … < Class`), but a close-enough Tier-1
                        // approximation that doesn't regress the
                        // common idiom. (Code-review #253 round 9
                        // #2.)
                        let shell_superclass = cls.superclass.borrow().clone();
                        let v = std::rc::Rc::new(crate::value::Class {
                            name: format!("#<Class:{}>", cls.name),
                            is_module: false,
                            ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                            methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                            singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                            superclass: std::cell::RefCell::new(shell_superclass),
                            includes: std::cell::RefCell::new(Vec::new()),
                            prepends: std::cell::RefCell::new(Vec::new()),
                            singleton_prepends: std::cell::RefCell::new(Vec::new()),
                            singleton_includes: std::cell::RefCell::new(Vec::new()),
                            singleton_view: std::cell::RefCell::new(None),
                            singleton_target: std::cell::RefCell::new(Some(std::rc::Rc::downgrade(&cls))),
                            class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: std::cell::RefCell::new(None),
                            #[cfg(feature = "cext")]
                            cext_alloc_func: std::cell::Cell::new(None),
                        });
                        *slot = Some(v.clone());
                        v
                    }
                };
                self.stack.push(Value::Class(view));
                Ok(true)
            }
            ("instance_methods", args_)
            | ("public_instance_methods", args_)
            | ("private_instance_methods", args_)
            | ("protected_instance_methods", args_)
                if args_.is_empty()
                    || matches!(args_, [Value::Bool(_)]) =>
            {
                use crate::value::Visibility;
                let inherited = !matches!(args_, [Value::Bool(false)]);
                let allow: fn(Visibility) -> bool = match name {
                    "instance_methods" => |v| matches!(v, Visibility::Public | Visibility::Protected),
                    "public_instance_methods" => |v| v == Visibility::Public,
                    "private_instance_methods" => |v| v == Visibility::Private,
                    "protected_instance_methods" => |v| v == Visibility::Protected,
                    _ => unreachable!(),
                };
                let mut sids: Vec<crate::intern::SymId> = Vec::new();
                if inherited {
                    let mut visited: Vec<*const crate::value::Class> = Vec::new();
                    fn walk(
                        c: &std::rc::Rc<crate::value::Class>,
                        allow: fn(Visibility) -> bool,
                        out: &mut Vec<crate::intern::SymId>,
                        visited: &mut Vec<*const crate::value::Class>,
                    ) {
                        let ptr = std::rc::Rc::as_ptr(c);
                        if visited.contains(&ptr) { return; }
                        visited.push(ptr);
                        for (k, m) in c.methods.borrow().iter() {
                            if allow(m.visibility.get()) && !out.contains(k) {
                                out.push(*k);
                            }
                        }
                        for inc in c.includes.borrow().iter() {
                            walk(inc, allow, out, visited);
                        }
                        if let Some(sup) = c.superclass.borrow().clone() {
                            walk(&sup, allow, out, visited);
                        }
                    }
                    walk(&cls, allow, &mut sids, &mut visited);
                } else {
                    for (k, m) in cls.methods.borrow().iter() {
                        if allow(m.visibility.get()) {
                            sids.push(*k);
                        }
                    }
                }
                sids.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
                let elems: Vec<Value> = sids.into_iter().map(Value::Sym).collect();
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(elems.into()));
                self.stack.push(Value::Array(id));
                Ok(true)
            }
            ("constants", args_) if args_.is_empty()
                || matches!(args_, [Value::Bool(_)]) =>
            {
                // `Module#constants(inherit=true)` lists the
                // module's own constants PLUS those of its ancestors
                // (included/prepended modules and superclasses up to
                // — but not including — Object). `constants(false)`
                // lists own constants only. CRuby returns own-first
                // then inherited (deduped); we approximate definition
                // order within each ancestor by the HashMap iteration
                // order (rubyrs has no insertion-ordered const table),
                // so the relative order WITHIN a scope can differ from
                // CRuby — but own-before-inherited and the
                // inherit-vs-own membership match.
                let inherit = !matches!(args_, [Value::Bool(false)]);
                let mut names: Vec<String> = Vec::new();
                let collect = |prefix: &str, names: &mut Vec<String>| {
                    for k in self.constants.keys() {
                        let s = self.interner.resolve(*k).to_string();
                        if let Some(short) = s.strip_prefix(prefix)
                            && !short.contains("::")
                            && !names.contains(&short.to_string()) {
                            names.push(short.to_string());
                        }
                    }
                };
                let own_prefix = format!("{}::", cls.name);
                collect(&own_prefix, &mut names);
                if inherit {
                    // Walk the full ancestry (prepends, includes,
                    // superclasses) for inherited constants. Skip the
                    // class itself (already collected) and Object —
                    // its toplevel constants are NOT reported by
                    // `Foo.constants` in CRuby.
                    for anc in super::flatten_ancestors(&cls) {
                        if anc.name.is_empty()
                            || anc.name == cls.name
                            || anc.name == "Object"
                        {
                            continue;
                        }
                        let anc_prefix = format!("{}::", anc.name);
                        collect(&anc_prefix, &mut names);
                    }
                }
                let elems: Vec<Value> = names.into_iter()
                    .map(|n| Value::Sym(self.interner.intern(&n)))
                    .collect();
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(elems.into()));
                self.stack.push(Value::Array(id));
                Ok(true)
            }
            ("method_defined?", [Value::Sym(sid)])
            | ("method_defined?", [Value::Sym(sid), _]) => {
                let answer = class_method_defined(self, &cls, *sid);
                self.stack.push(Value::Bool(answer));
                Ok(true)
            }
            ("method_defined?", [Value::Str(s)])
            | ("method_defined?", [Value::Str(s), _]) => {
                let sid = self.interner.intern(&s.to_string_lossy());
                let answer = class_method_defined(self, &cls, sid);
                self.stack.push(Value::Bool(answer));
                Ok(true)
            }
            ("undef_method", args) => {
                // Removal itself is a Tier 1 no-op (see docs/SUBSET.md),
                // but the `method_undefined(name)` hook still fires
                // for every Symbol/String arg — Rails-style code
                // observes the call regardless of whether the
                // method dispatch table actually changes.
                //
                // Per-arg validation mirrors `remove_method`:
                //   - Symbol: use sid directly.
                //   - String: route through with_str_lossy + the
                //     `Config::max_symbols` cap (untrusted code
                //     calling `undef_method("dyn_#{i}")` in a loop
                //     must not grow the interner past the cap).
                //   - Anything else: raise TypeError (CRuby parity
                //     and consistency with remove_method).
                for arg in args {
                    let sid: SymId = match arg {
                        Value::Sym(sid) => *sid,
                        Value::Str(s) => s.with_str_lossy(|raw| -> Result<SymId, Trap> {
                            if let Some(max) = self.max_symbols
                                && !self.interner.contains(raw)
                                && self.interner.len() >= max
                            {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                            Ok(self.interner.intern(raw))
                        })?,
                        other => {
                            let inspected = other.to_inspect(&self.heap, &self.interner);
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!("{} is not a symbol nor a string", inspected),
                            }));
                        }
                    };
                    self.fire_method_lifecycle_hook(&cls, "method_undefined", sid)?;
                }
                self.stack.push(Value::Class(cls));
                Ok(true)
            }
            // `Module#remove_method(name, ...)` — removes the
            // method(s) from THIS class's own methods table. Does
            // NOT walk the superclass chain (that's `undef_method`'s
            // job in CRuby; we route undef as a no-op pending real
            // semantics).
            //
            // Motivating consumer: tilt-2.7.0
            // `lib/tilt/template.rb:490` calls
            // `TOPOBJECT.class_eval { remove_method(method_name) }`
            // after each `evaluate` to wipe the synthesised
            // `__tilt_<id>` entry. With this arm tilt's cleanup
            // path runs to completion.
            //
            // Variadic: CRuby accepts any number of args
            // (`remove_method(:a, :b, :c)`); 0 args is a no-op
            // returning self.
            //
            // CRuby raises NameError on a method not defined on
            // this class, INCLUDING for primitives — verified
            // against CRuby 3.4 that `String.remove_method(:foo)`
            // raises. This diverges from the permissive stance
            // at `instance_method` / `method_defined?` (which DO
            // skip the user-class fence for primitives because
            // probing is benign). `remove_method` is a mutation,
            // not a probe; matching CRuby's strict shape here
            // avoids quiet divergence on a surface that's
            // unlikely to be exercised as a feature-detect.
            ("remove_method", args) if !args.is_empty() => {
                // Iterative: process args left-to-right, removing
                // each in turn. If a later arg is missing (or a
                // TypeError fires), earlier removals stay — CRuby
                // is partial-mutation on this surface (verified
                // against 3.4: `A.remove_method(:x, :nope)`
                // removes `:x` BEFORE raising NameError on
                // `:nope`). Track whether anything was removed
                // so we can bump `method_gen` on the error path
                // too — without that, inline caches would keep
                // returning the stale lookup for the removed
                // method.
                //
                // Per-arg arg-to-SymId resolution: Symbol uses sid
                // directly (no resolve/intern roundtrip + no
                // `max_symbols` check — Symbols are already
                // interned). String goes through `with_str_lossy`
                // so the cap check + intern run on a borrowed
                // &str (zero-alloc on the valid-UTF-8 hot path).
                // Mirrors the established pattern at the
                // `instance_method` String arm.
                //
                // Strict-on-primitive parity: primitives are NOT
                // exempt from the missing-method NameError
                // (unlike `instance_method` / `method_defined?`,
                // which keep their permissive stance because
                // probes are benign feature-detects;
                // `remove_method` is a mutation).
                //
                // No \`any_removed\` tracking needed: each
                // successful removal bumps `method_gen` before
                // firing its hook (see the pre-fire bump below),
                // so a half-completed variadic call has already
                // invalidated inline caches for everything it
                // removed by the time any later arg's
                // type/missing-method error path runs.
                for arg in args {
                    let sid: SymId = match arg {
                        Value::Sym(sid) => *sid,
                        Value::Str(s) => s.with_str_lossy(|raw| -> Result<SymId, Trap> {
                            if let Some(max) = self.max_symbols
                                && !self.interner.contains(raw)
                                && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                            Ok(self.interner.intern(raw))
                        })?,
                        other => {
                            let inspected = other.to_inspect(&self.heap, &self.interner);
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!("{} is not a symbol nor a string", inspected),
                            }));
                        }
                    };
                    // Single `remove()` call: HashMap::remove
                    // returns Option so we get presence-check +
                    // mutation in one hash lookup + one
                    // `borrow_mut()`.
                    if cls.methods.borrow_mut().remove(&sid).is_none() {
                        // Resolve name only on the rare missing
                        // path. Free for the common case.
                        let name_for_msg = self.interner.resolve(sid).to_string();
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("method '{}' not defined in {}", name_for_msg, cls.name),
                        }));
                    }
                    // Bump `method_gen` BEFORE firing the hook so
                    // any inline-cache-backed dispatch inside the
                    // user-defined `method_removed` body sees the
                    // mutation. Without the pre-fire bump, the hook
                    // could still observe (and re-invoke) the just-
                    // removed method through a stale cached entry.
                    // The bump also covers the hook-raise path: an
                    // exception propagates with caches already
                    // invalidated, so any rescue downstream is
                    // safe.
                    //
                    // `method_removed(name)` fires per successful
                    // removal — CRuby invokes it once for each
                    // Symbol the user passed, in arg order.
                    self.method_gen = self.method_gen.wrapping_add(1);
                    self.fire_method_lifecycle_hook(&cls, "method_removed", sid)?;
                }
                self.stack.push(Value::Class(cls));
                Ok(true)
            }
            ("remove_method", _) => {
                // 0-arg form: no-op, return receiver (CRuby parity).
                self.stack.push(Value::Class(cls));
                Ok(true)
            }
            // Arity guard FIRST so wrong-count calls surface as
            // ArgumentError (CRuby check order: arity → type).
            // 0 args / 2+ args both raise here.
            ("instance_method", args) if args.len() != 1 => {
                Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len()
                    ),
                }))
            }
            // 1 arg of a type other than Symbol or String: CRuby
            // raises TypeError "<inspect> is not a symbol nor a
            // string" (the literal wording from
            // rb_mod_instance_method).
            ("instance_method", [other]) if !matches!(other, Value::Sym(_) | Value::Str(_)) => {
                let inspected = other.to_inspect(&self.heap, &self.interner);
                Err(self.trap(RubyError::TypeError {
                    msg: format!("{} is not a symbol nor a string", inspected),
                }))
            }
            ("instance_method", [Value::Sym(sid)]) => {
                // Snapshot the Method here so the UnboundMethod
                // survives a subsequent `remove_method` between
                // capture and bind/bind_call. Tilt's
                // `compile_template_method` does exactly that —
                // captures, then removes from the class table,
                // then bind_call's the captured handle.
                //
                // Kernel builtin synth check: when the receiver is
                // Kernel and the name matches a registered
                // builtin (`:class`, `:nil?`, `:is_a?`, ...),
                // synthesise a Method carrying reflection metadata
                // (arity/parameters/source_location). Kept off
                // Kernel.methods deliberately so regular dispatch
                // doesn't re-find it; the registry lives only for
                // this introspection surface.
                // User-defined methods on the class table win —
                // reopening Kernel/BasicObject to shadow `class` /
                // `equal?` / etc. should surface that method
                // through reflection, not the synth metadata.
                // Registry is the fallback when the live table
                // misses, and the ancestor-chain walk lets
                // inherited reflection (`User.instance_method(:class)`
                // → Kernel synth via Object→Kernel include chain)
                // work the same as the direct case.
                let snapshot = self.lookup_method_uncached(&cls, *sid)
                    .or_else(|| self.builtin_method_via_ancestor_chain(&cls, *sid));
                if snapshot.is_none() && !is_primitive_class_name(&cls.name) {
                    let mname = self.interner.resolve(*sid).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cls.name),
                    }));
                }
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::UnboundMethod {
                    class: cls.clone(),
                    name_id: *sid,
                    method: snapshot,
                });
                self.stack.push(Value::UnboundMethod(id));
                Ok(true)
            }
            // `instance_method` accepts a String too (CRuby
            // `to_sym`'s it). Tilt-2.7.0 lib/tilt/template.rb:489
            // calls `TOPOBJECT.instance_method(method_name)`
            // where `method_name` is a String synthesised via
            // `"__tilt_#{...}"` interpolation. `with_str_lossy` is
            // Cow-backed: zero-alloc on the valid-UTF-8 hot path.
            // Cap check + intern + lookup all happen inside the
            // closure; the NameError path `format!`s the borrowed
            // `raw` directly. Same parity stance as the
            // `method_defined?` arm above.
            ("instance_method", [Value::Str(s)]) => {
                s.with_str_lossy(|raw| {
                    if let Some(max) = self.max_symbols
                        && !self.interner.contains(raw)
                        && self.interner.len() >= max {
                        return Err(self.trap(RubyError::ResourceExhausted {
                            msg: format!("interner exhausted: {} symbols", max),
                        }));
                    }
                    let sid = self.interner.intern(raw);
                    // Same registry consultation as the Symbol-form
                    // arm above — live table first, then ancestor-
                    // chain walk so inherited reflection works.
                    let snapshot = self.lookup_method_uncached(&cls, sid)
                        .or_else(|| self.builtin_method_via_ancestor_chain(&cls, sid));
                    if snapshot.is_none() && !is_primitive_class_name(&cls.name) {
                        return Err(self.trap(RubyError::NameError {
                            msg: format!("undefined method '{}' for class '{}'", raw, cls.name),
                        }));
                    }
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::UnboundMethod {
                        class: cls.clone(),
                        name_id: sid,
                        method: snapshot,
                    });
                    self.stack.push(Value::UnboundMethod(id));
                    Ok(true)
                })
            }
            _ => Ok(false),
        }
    }

    fn try_dispatch_class_intrinsics(
        &mut self,
        name: &str,
        name_id: SymId,
        _cache_id: u16,
        args: ArgsBuf,
        recv: Value,
    ) -> Result<ClassOutcome, Trap> {
        // Local SymId for "new" — used by the `cls.new`
        // override arm. Originally derived in the surrounding
        // `do_call` body above the extracted cluster; computed
        // inside the helper now so the cluster is self-
        // contained.
        let new_id = self.interner.intern("new");
    // Singleton-class-shell fence: `A.singleton_class.new` raises
    // TypeError in CRuby ("can't create instance of singleton
    // class"). Without this fence the shell falls into the
    // default `Class.new` allocator at line 2294 and silently
    // allocates a `Value::Object` whose class is the shell —
    // producing an orphan instance whose every method call
    // raises NoMethodError because the shell's method table is
    // empty. Defensive code that `rescue TypeError`s to detect
    // singleton-class misuse would skip; the orphan only
    // surfaces as the confusing downstream NoMethodError.
    // (Code-review #253 round 9 #1.)
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.singleton_target.borrow().is_some()
    {
        return Err(self.trap(RubyError::TypeError {
            msg: "can't create instance of singleton class".into(),
        }));
    }
    // `Hash[...]` class-method constructor. CRuby has three
    // call shapes:
    //   - `Hash[]`               → empty Hash
    //   - `Hash[k1, v1, k2, v2]` → flat-pair form (even arity)
    //   - `Hash[[[k, v], ...]]`  → 1 Array of 2-element pairs
    //   - `Hash[{k => v, ...}]`  → 1 Hash (copy semantics)
    // The flat-pair form is the most common; older gems prefer
    // it over `pairs.to_h`. Without this intercept, `Hash[]`
    // would NoMethodError on Class (no `[]` defined on
    // Value::Class).
    //
    // Odd-arity (k without matching v) is ArgumentError in
    // CRuby; mirror that.
    if name == "[]"
        && let Value::Class(cls) = &recv
        && (cls.name.as_str() == "Hash" || class_inherits_named(cls, "Hash"))
    {
        // A Hash subclass (`class Conf < Hash`) constructs a tagged
        // instance of itself — `Jekyll::Configuration[override]` is
        // `Configuration.[]` inherited from Hash. The literal Hash
        // class tags None (a plain Hash).
        let class_tag = if cls.name.as_str() == "Hash" {
            None
        } else {
            Some(cls.clone())
        };
        // GC rooting: `args` came from `self.stack.drain(...)`
        // and is a Rust-local Vec with no GC root, so any heap-
        // shaped element (Array / Hash for the `Hash[[[k,v],...]]`
        // and `Hash[{…}]` shapes) gets swept if `maybe_gc` runs
        // before we finish reading their pairs. Pin every arg
        // across the entire alloc + pair-extract window. Repro
        // pre-fix: `Hash[[[:x, 10], [:y, 20]]]` under STRESS_GC=1
        // tripped `ICE: use-after-free` on the inner-pair walk.
        let mut g = PinGuard::new(self);
        for a in &args { g.pin(a.clone()); }
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let pairs: Vec<(Value, Value)> = if args.len() == 1 {
            match &args[0] {
                Value::Array(aid) => {
                    // `Hash[[[k, v], ...]]`. Each element must be
                    // a 2-element Array; anything else is
                    // ArgumentError in CRuby (`invalid number of
                    // elements (X for 2)`), but we follow the
                    // common shape — non-pair elements are dropped
                    // with TypeError. Stay strict only on the
                    // outer Array shape.
                    let outer = g.vm.heap.array(*aid).clone();
                    let mut out = Vec::with_capacity(outer.len());
                    for elem in outer {
                        if let Value::Array(pair_id) = elem {
                            let pair = g.vm.heap.array(pair_id);
                            if pair.len() == 2 {
                                out.push((pair[0].clone(), pair[1].clone()));
                            } else {
                                return Err(g.vm.trap(RubyError::ArgumentError {
                                    msg: format!("invalid number of elements ({} for 2)", pair.len()),
                                }));
                            }
                        } else {
                            return Err(g.vm.trap(RubyError::TypeError {
                                msg: format!("wrong element type {} (expected array)", elem.type_name()),
                            }));
                        }
                    }
                    out
                }
                Value::Hash(hid) => g.vm.heap.hash(*hid).clone(),
                _ => return Err(g.vm.trap(RubyError::ArgumentError {
                    msg: "odd number of arguments for Hash".into(),
                })),
            }
        } else if args.len().is_multiple_of(2) {
            args.chunks(2).map(|c| (c[0].clone(), c[1].clone())).collect()
        } else {
            return Err(g.vm.trap(RubyError::ArgumentError {
                msg: "odd number of arguments for Hash".into(),
            }));
        };
        let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj {
            pairs,
            default_block: None,
            default_value: None,
            class_tag,
            ivars: crate::intern::FxHashMap::default(),
            index: None,
        }));
        g.vm.stack.push(Value::Hash(hid));
        return Ok(ClassOutcome::Handled);
    }
    // User-defined `def self.new` takes precedence over the
    // built-in allocator AND over the Hash.new / String.new /
    // other built-in class-level intercepts below. CRuby's
    // `Class#new` is a normal Ruby method (allocate +
    // initialize), and reopening any class — built-in or
    // user — to override `self.new` should win. Without this
    // check ahead of the Hash / String special-cases, e.g.
    // `class Hash; def self.new; ...; end; end; Hash.new`
    // silently bypassed the override and returned an empty
    // `{}` from the hardcoded Hash path.
    //
    // The block-form path (`do_call_block`) generally routes
    // user `self.new` overrides through its general
    // Value::Class singleton-method dispatch arm, so most
    // classes don't need a mirrored check there. The one
    // exception is `do_call_block`'s `Hash.new { block }`
    // intercept, which fires before that generic arm — it
    // carries the same singleton pre-check pattern as this
    // one for parity.
    //
    // Documented gap: `def self.new ... super ... end` still
    // hits the allocator via super only if Class's builtin
    // `new` is reachable through super_lookup — which it
    // isn't today. Override-without-super covers the tilt
    // entry-point (and the common DSL builder pattern); the
    // super-into-allocator case is a separable follow-up.
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && let Some(m) = self.lookup_class_singleton_method(cls, new_id) {
        self.invoke_method(m, recv.clone(), args.into_vec())?;
        return Ok(ClassOutcome::Handled);
    }
    // `String.new` / `String.new(s)` — Tier 1 primitive
    // constructor. Without this intercept the generic
    // `Class.new` allocator below would build a
    // `Value::Object` (Instance with `class = String`), and
    // every String primitive method (`length`, `<<`,
    // `bytesize`, …) would `NoMethodError` because they
    // pattern-match on `Value::Str`, not `Value::Object`.
    //
    // CRuby supports `String.new(s, encoding: …, capacity: …)`;
    // the encoding model is Tier 3 (ADR 0017), so we cover
    // only the positional `s` argument here. Anything else
    // raises ArgumentError.
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "String"
    {
        match &args[..] {
            [] => {
                self.stack.push(Value::new_str(""));
                return Ok(ClassOutcome::Handled);
            }
            [Value::Str(s)] => {
                // Fresh, mutable copy — CRuby's `String.new(s)`
                // returns an unfrozen clone even if `s` was
                // frozen.
                let copy = s.to_string_lossy();
                self.stack.push(Value::new_str(copy));
                return Ok(ClassOutcome::Handled);
            }
            [other] => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        other.type_name(),
                    ),
                }));
            }
            _ => {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0..1)",
                        args.len(),
                    ),
                }));
            }
        }
    }
    // `Module.new` (no block) — returns a fresh anonymous
    // Module. Empty name is the sentinel for "anonymous"
    // that `Module#name` consults to return `nil`; `to_s` /
    // `inspect` render `"#<Module>"` instead. The block-form
    // `Module.new { |m| ... }` evaluates the block as the
    // module body and lives in `do_call_block` — same shape
    // as the existing `Hash.new` / `class_eval` intercepts.
    //
    // Documented divergence (NOT addressed here): CRuby
    // assigns the module's name on first constant write
    // (`M = Module.new` → `M.name == "M"`). rubyrs leaves
    // the name empty until a future StoreConst hook lands;
    // most real-world uses (`include` an anonymous helper)
    // don't depend on the name-promote behaviour.
    // Tier-1 2b: `Proc.new` without an explicit block raises
    // ArgumentError, matching CRuby 3.x (which removed implicit
    // block capture from caller). Without this check the
    // default Object#new path returns a Proc-class instance
    // that has no `.call` arm — `.call` on it raises a
    // confusing NoMethodError instead of the canonical
    // "tried to create Proc object without a block".
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Proc"
    {
        return Err(self.trap(RubyError::ArgumentError {
            msg: "tried to create Proc object without a block".to_string(),
        }));
    }
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Module"
    {
        if !args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len(),
                ),
            }));
        }
        let m = std::rc::Rc::new(Class {
            name: String::new(),
            is_module: true,
            ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            superclass: std::cell::RefCell::new(None),
            includes: std::cell::RefCell::new(Vec::new()),
            prepends: std::cell::RefCell::new(Vec::new()),
            singleton_prepends: std::cell::RefCell::new(Vec::new()),
            singleton_includes: std::cell::RefCell::new(Vec::new()),
            singleton_view: std::cell::RefCell::new(None),
            singleton_target: std::cell::RefCell::new(None),
            class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: std::cell::RefCell::new(None),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        });
        self.stack.push(Value::Class(m));
        return Ok(ClassOutcome::Handled);
    }
    // `Module#define_method` no-block path. The block-form
    // intrinsic lives in `do_call_block`; this arm handles the
    // no-block shapes that CRuby validates here, ordered to
    // match CRuby's actual validation sequence (arity first,
    // then missing-block). The 2-arg form
    // (`define_method(:foo, proc { … })` / Method / UnboundMethod)
    // is implemented at the 2-arg case below via
    // `install_method_from_value` — see PR #321.
    // (PR #245 Copilot round 2 #2 + round 4 #1 + round 5 #1.)
    if name == "define_method"
        && let Value::Class(cls) = &recv
    {
        // Same precedence rule as the block-form arm — user
        // override wins regardless of arity (let the override
        // own its own validation).
        if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
            let recv_val = Value::Class(cls.clone());
            self.invoke_method(m, recv_val, args.into_vec())?;
            return Ok(ClassOutcome::Handled);
        }
        // CRuby validates arity before the missing-block check:
        //   0 args      → ArgumentError "wrong number of arguments
        //                 (given 0, expected 1..2)"
        //   1 arg, none → ArgumentError "tried to create Proc
        //                 object without a block"
        //   2 args      → Proc / Method / UnboundMethod install
        //                 form (PR #321) — args[1] is the body
        //                 source, name is args[0]. Built-in
        //                 method bodies (snapshot=None, e.g.
        //                 `m = obj.method(:object_id)`) raise
        //                 TypeError because rubyrs needs a real
        //                 Proto to install; a name-forwarding
        //                 fallback is a Tier-2 follow-up.
        //   3+ args     → ArgumentError "wrong number of arguments
        //                 (given N, expected 1..2)"
        match args.len() {
            0 => return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..2)".into(),
            })),
            1 => return Err(self.trap(RubyError::ArgumentError {
                msg: "tried to create Proc object without a block".into(),
            })),
            2 => {
                // 2-arg Proc / Method / UnboundMethod install.
                // Name is args[0], source is args[1]. Visibility
                // defaults to Public on the explicit-receiver
                // path — the bare-in-class-body shape (where
                // class_visibility_stack matters) is handled
                // before the bridge re-enters here (PR #321
                // cycle-1), so reaching this arm means the
                // caller explicitly wrote `cls.define_method(...)`
                // and CRuby treats those installs as Public
                // regardless of the surrounding class body's
                // visibility mode.
                let name_sym = match &args[0] {
                    Value::Sym(s) => *s,
                    Value::Str(s) => {
                        let raw = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&raw) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        self.interner.intern(&raw)
                    }
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Symbol or String)",
                            other.type_name(),
                        ),
                    })),
                };
                let src = args[1].clone();
                let installed = self
                    .install_method_from_value(
                        cls,
                        name_sym,
                        &src,
                        crate::value::Visibility::Public,
                    )
                    .map_err(|e| self.trap(e))?;
                self.stack.push(Value::Sym(installed));
                return Ok(ClassOutcome::Handled);
            }
            n => return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1..2)", n),
            })),
        }
    }
    // `Class.new` / `Class.new(superclass)` — no-block path.
    // Mirrors the block-form arm in `do_call_block` (which
    // ALSO runs the body as a class_eval); this one returns
    // the freshly-built anonymous Class without invoking any
    // body. Pre-fix this fell through to the generic Class
    // allocator below which produced a `Value::Object` whose
    // class was `Class` — NOT a real `Value::Class` —
    // breaking downstream `Class.new(anon) { ... }` block-form
    // calls and any introspection (`#superclass`, `#name`,
    // `#new` on the result) that requires the Class value
    // variant. Mustermann's
    // `mustermann/ast/translator.rb:75`
    //   `Class.new(const_get(:NodeTranslator)) do ... end`
    // tripped this because `NodeTranslator` (built via an
    // earlier `Class.new(Delegator)`) was the Value::Object
    // form, and the block-form arm's `[Value::Class(sc)]`
    // pattern failed to match, raising "superclass must be
    // a Class (Object given)".
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Class"
    {
        let explicit_super: Option<Rc<Class>> = match &args[..] {
            [] => None,
            [Value::Class(sc)] if !sc.is_module => Some(sc.clone()),
            [Value::Class(_)] => {
                return Err(self.trap(RubyError::TypeError {
                    msg: "superclass must be an instance of Class (given an instance of Module)".to_string(),
                }));
            }
            [other] => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!("superclass must be an instance of Class (given an instance of {})", other.type_name()),
                }));
            }
            _ => {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0..1)",
                        args.len(),
                    ),
                }));
            }
        };
        let object_sym = self.interner.intern("Object");
        let parent = explicit_super.or_else(|| self.classes.get(&object_sym).cloned());
        let new_cls = Rc::new(Class {
            name: String::new(),
            is_module: false,
            ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            superclass: std::cell::RefCell::new(parent),
            includes: std::cell::RefCell::new(Vec::new()),
            prepends: std::cell::RefCell::new(Vec::new()),
            singleton_prepends: std::cell::RefCell::new(Vec::new()),
            singleton_includes: std::cell::RefCell::new(Vec::new()),
            singleton_view: std::cell::RefCell::new(None),
            singleton_target: std::cell::RefCell::new(None),
            class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: std::cell::RefCell::new(None),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        });
        // Fire the parent's `inherited(subclass)` hook, matching
        // CRuby's `Class.new(P)` → `P.inherited(<anon>)` contract.
        // Source-form `class C < P` fires this via `Op::DefClass`;
        // the dynamic `Class.new(P)` path didn't, breaking gems
        // (Mustermann's AST::Translator at `mustermann/ast/
        // translator.rb:62`) that rely on per-subclass setup
        // happening in the hook. Look up the parent's `inherited`
        // and invoke it with the new class as the single arg.
        // Missing-hook is silently accepted (matches CRuby's
        // default Object#inherited no-op).
        self.invoke_inherited_hook(&new_cls)?;
        self.stack.push(Value::Class(new_cls));
        return Ok(ClassOutcome::Handled);
    }
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Hash"
    {
        // `Hash.new` without a block. CRuby shapes:
        //   - 0 args: empty Hash, no default
        //   - 1 arg:  empty Hash with scalar default; missing-
        //             key lookup returns this value as-is (not
        //             cached into the Hash).
        //   - 2+ args: ArgumentError
        // The block-form (`Hash.new { |h, k| ... }`) routes
        // through `do_call_block` and has its own intercept
        // (which raises ArgumentError when a scalar default is
        // also given — CRuby refuses both at once).
        if args.len() > 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0..1)", args.len()),
            }));
        }
        let default = args.first().cloned();
        // Pin the default across maybe_gc — if it's a heap
        // value (Array / Hash / String), it could be a
        // temporary on its way to becoming the default and
        // would otherwise be unrooted between args.first() and
        // hash_set_default_value below.
        let mut g = PinGuard::new(self);
        if let Some(v) = &default { g.pin(v.clone()); }
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
        if default.is_some() {
            g.vm.heap.hash_set_default_value(hid, default);
        }
        g.vm.stack.push(Value::Hash(hid));
        return Ok(ClassOutcome::Handled);
    }
    // `Array[1, 2]` / `Subclass[1, 2]` — the literal-ish class
    // constructor (mirrors the Hash[] intercept above). A subclass
    // constructs a TAGGED instance of itself; the literal Array
    // class tags None.
    if name == "[]"
        && let Value::Class(cls) = &recv
        && (cls.name.as_str() == "Array" || class_inherits_named(cls, "Array"))
    {
        let class_tag = if cls.name.as_str() == "Array" {
            None
        } else {
            Some(cls.clone())
        };
        let mut g = PinGuard::new(self);
        for a in &args {
            if a.is_gc_heap_ref() { g.pin(a.clone()); }
        }
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let aid = g.vm.heap.alloc(HeapObj::Array(crate::heap::ArrayObj {
            elems: args.to_vec(),
            class_tag,
            ivars: crate::intern::FxHashMap::default(),
        }));
        g.vm.stack.push(Value::Array(aid));
        return Ok(ClassOutcome::Handled);
    }
    if name_id == new_id
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Array"
    {
        // `Array.new` WITHOUT a block (the block form lives in
        // do_call_block):
        //   - 0 args      → []
        //   - Int n       → [nil] * n
        //   - Int n, val  → [val] * n   (val is SHARED, not copied)
        //   - Array a     → a shallow copy of a
        // n < 0 → ArgumentError; a lone non-Int/non-Array → TypeError.
        // Without this, no-block `Array.new(...)` fell to the generic
        // Class#new and produced a bare `#<Array>` instance.
        let elems: Vec<Value> = match &args[..] {
            [] => Vec::new(),
            [Value::Int(n)] | [Value::Int(n), _] => {
                if *n < 0 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: "negative array size".to_string(),
                    }));
                }
                let fill = args.get(1).cloned().unwrap_or(Value::Nil);
                vec![fill; *n as usize]
            }
            [Value::Array(aid)] => self.heap.array(*aid).clone(),
            [other] => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        other.type_name()
                    ),
                }));
            }
            _ => {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0..2)",
                        args.len()
                    ),
                }));
            }
        };
        let mut g = PinGuard::new(self);
        for e in &elems {
            if e.is_gc_heap_ref() { g.pin(e.clone()); }
        }
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let aid = g.vm.heap.alloc(HeapObj::Array(elems.into()));
        g.vm.stack.push(Value::Array(aid));
        return Ok(ClassOutcome::Handled);
    }
    // `Regexp.compile(pat)` / `Regexp.new(pat)` — compile a
    // String pattern into a Regexp. Same code path the regex
    // literal `/.../` takes (Op::LoadRegex / Op::CompileRegex),
    // including `preprocess_regex_pattern` so Onigmo-specific
    // anchors like `\G` translate identically. Needed by gems
    // that build patterns from runtime data (rack-cors uses
    // `Regexp.compile("^[a-z]+://#{Regexp.quote(host)}$")`
    // when turning `origins 'example.com'` into a matcher).
    #[cfg(feature = "regex")]
    if (name == "compile" || name_id == new_id)
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Regexp"
    {
        if args.is_empty() || args.len() > 2 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
            }));
        }
        let pat = match &args[0] {
            Value::Str(s) => s.to_string_lossy(),
            other => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", other.type_name()),
                }));
            }
        };
        // `Regexp.new(str, options)`: an Integer is a flag bitmask
        // (IGNORECASE=1|EXTENDED=2|MULTILINE=4, plus
        // encoding bits rubyrs ignores for matching but preserves
        // in #options); any other truthy value is the legacy
        // boolean form meaning IGNORECASE; nil/false/absent → 0.
        let flags: u8 = match args.get(1) {
            None | Some(Value::Nil) | Some(Value::Bool(false)) => 0,
            Some(Value::Int(n)) => *n as u8,
            Some(_) => crate::regex_engine::RB_IGNORECASE,
        };
        let translated = crate::vm::step::preprocess_regex_pattern(&pat);
        let prefixed = crate::vm::step::apply_ruby_flags(&translated, flags);
        let compiled = crate::regex_engine::compile_with_flags(&prefixed, flags, &translated).map_err(|e| {
            self.trap(RubyError::SyntaxError {
                msg: format!("invalid regex /{}/: {}", pat, e),
            })
        })?;
        self.stack.push(Value::Regex(Rc::new(compiled)));
        return Ok(ClassOutcome::Handled);
    }

    // `Regexp.last_match` / `Regexp.last_match(n)` — the `$~` of the
    // current scope. No arg returns the whole MatchData (or nil); an
    // Integer returns that capture group (0 = whole match), or nil.
    // Discovery: P3 Jekyll spike — `convertible.rb#read_yaml` splits
    // front matter with `Regexp.last_match.post_match` /
    // `Regexp.last_match(1)`.
    #[cfg(feature = "regex")]
    if name == "last_match"
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Regexp"
    {
        let v = match args.first() {
            // No arg → the whole MatchData (with pre/post-match), or nil.
            None => self.materialize_last_match()?,
            // `Regexp.last_match(n)`: same as `MatchData#[]`. Index 0
            // is the whole match; n>=1 the n-th capture. A negative
            // index counts from the end of the *captures* (CRuby's
            // `rb_reg_nth_match`) — it can reach any capture but never
            // wraps to the whole match. `LastMatch.caps` holds ONLY
            // the captures (index 0 == group 1).
            Some(Value::Int(n)) => match self.scoped_last_match() {
                None => Value::Nil,
                Some(lm) => {
                    let cl = lm.caps.len() as i64;
                    // Resolve to a captures index, -1 for the whole
                    // match, or None for out-of-range.
                    let pick: Option<i64> = if *n < 0 {
                        let j = *n + cl;
                        if j >= 0 { Some(j) } else { None }
                    } else if *n == 0 {
                        Some(-1)
                    } else if *n - 1 < cl {
                        Some(*n - 1)
                    } else {
                        None
                    };
                    match pick {
                        None => Value::Nil,
                        Some(-1) => Value::new_str(lm.whole.clone()),
                        Some(j) => lm.caps[j as usize]
                            .as_ref()
                            .map(|s| Value::new_str(s.clone()))
                            .unwrap_or(Value::Nil),
                    }
                }
            },
            // `Regexp.last_match(:name)` / `("name")` — named capture.
            // An existing-but-non-participating group is nil; an
            // unknown name raises IndexError (CRuby, via MatchData#[]).
            Some(Value::Sym(_)) | Some(Value::Str(_)) => {
                let key = match args.first() {
                    Some(Value::Sym(id)) => self.interner.resolve(*id).to_string(),
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    _ => unreachable!(),
                };
                let resolved: Option<Value> = match self.scoped_last_match() {
                    None => Some(Value::Nil),
                    Some(lm) => match lm.named.iter().find(|(n, _)| *n == key) {
                        Some((_, Some(s))) => Some(Value::new_str(s.clone())),
                        Some((_, None)) => Some(Value::Nil),
                        None => None,
                    },
                };
                match resolved {
                    Some(v) => v,
                    None => {
                        return Err(self.trap(RubyError::IndexError {
                            msg: format!("undefined group name reference: {}", key),
                        }));
                    }
                }
            }
            Some(_) => Value::Nil,
        };
        self.stack.push(v);
        return Ok(ClassOutcome::Handled);
    }

    // `Regexp.escape(s)` / `Regexp.quote(s)` — escape regex
    // metacharacters in `s` so it can be safely interpolated
    // into a pattern. The `regex` crate's `escape` covers the
    // same metacharacter set Ruby's Regexp.escape does for
    // ASCII; rack-cors uses this to quote user-supplied
    // origin hostnames before compiling a Regexp.
    //
    // Gated on the `regex` feature alongside the sibling
    // `Regexp.compile` / `Regexp.new` arm above — same
    // metacharacter handling lives in the `regex` crate and is
    // unavailable in no-default-features builds (wasm32-wasip1).
    #[cfg(feature = "regex")]
    if (name == "escape" || name == "quote")
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Regexp"
    {
        if args.len() != 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
            }));
        }
        let s = match &args[0] {
            Value::Str(s) => s.to_string_lossy(),
            other => {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", other.type_name()),
                }));
            }
        };
        let escaped = regex::escape(&s);
        self.stack.push(Value::new_str_bytes(escaped.into_bytes()));
        return Ok(ClassOutcome::Handled);
    }

    // `Regexp.union(*patterns)` — combine String / Regexp args
    // into one alternation Regexp. Strings are escaped via
    // `regex::escape` so metacharacters become literals; Regexp
    // args contribute their existing source. A single Array
    // argument is splatted (CRuby parity). No args -> `(?!)`
    // (never-matching pattern). Required by Rack 3
    // `rack/utils.rb:607`
    //   `Regexp.union(*[::File::SEPARATOR, ::File::ALT_SEPARATOR].compact)`
    // which evaluates at class-body load time during the P3
    // Sinatra spike.
    #[cfg(feature = "regex")]
    if name == "union"
        && let Value::Class(cls) = &recv
        && cls.name.as_str() == "Regexp"
    {
        // Splat a sole Array arg per CRuby (Regexp.union([a, b])
        // behaves like Regexp.union(a, b)).
        let parts_iter: Vec<Value> = if args.len() == 1 {
            if let Value::Array(oid) = &args[0] {
                self.heap.array(*oid).clone()
            } else {
                args.to_vec()
            }
        } else {
            args.to_vec()
        };
        let mut chunks: Vec<String> = Vec::with_capacity(parts_iter.len());
        for v in &parts_iter {
            match v {
                Value::Str(s) => chunks.push(regex::escape(&s.to_string_lossy())),
                Value::Regex(r) => chunks.push(r.as_str().to_string()),
                other => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into String", other.type_name()),
                    }));
                }
            }
        }
        let combined = if chunks.is_empty() {
            // CRuby's empty `Regexp.union` returns `/(?!)/` —
            // a zero-width negative lookahead that always
            // fails. The native `regex` crate doesn't support
            // lookaround; `[^\s\S]` (intersection-complement
            // char class — never matches anything) is the
            // linear-engine equivalent. Behavioural difference
            // is only in `.source` (which Sinatra/Rack don't
            // inspect for the union result).
            "[^\\s\\S]".to_string()
        } else {
            chunks.join("|")
        };
        let translated = crate::vm::step::preprocess_regex_pattern(&combined);
        let compiled = crate::regex_engine::compile(&translated).map_err(|e| {
            self.trap(RubyError::SyntaxError {
                msg: format!("invalid regex /{}/: {}", combined, e),
            })
        })?;
        self.stack.push(Value::Regex(Rc::new(compiled)));
        return Ok(ClassOutcome::Handled);
    }

    // `Class#allocate` user-singleton override — CRuby allows
    // `def self.allocate` to replace the built-in allocator (used
    // by Marshal / dup / ORM hydration hooks). Mirrors the
    // `def self.new` pre-check at line 1053. Must fire BEFORE the
    // builtin allocate arm below or the user override is silently
    // shadowed; do_call_block has the same precedence (its
    // generic singleton check at ~4601 runs before its allocate
    // arm). PR #181 follow-up: code-review caught the asymmetry.
    if name == "allocate"
        && let Value::Class(cls) = &recv
        && let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
        self.invoke_method(m, recv.clone(), args.into_vec())?;
        return Ok(ClassOutcome::Handled);
    }
    // `Class#allocate` — bare-instance allocator without calling
    // `initialize`. Used by frameworks for unmarshalling / dup /
    // clone / ORM hydration, and by the TRY_RUNS pass-7 probe's
    // `ERB.new` stub (layer #4). Sits before the `new` arm so the
    // class-receiver path is uniform.
    //
    // Semantics:
    //   - User classes (`Value::Class` not in the primitive
    //     whitelist): allocate a fresh `HeapObj::Instance` with
    //     the class pointer set, empty ivars, no singleton class.
    //     No `initialize` call.
    //   - Primitive class shells fall into two groups:
    //       * "Truly disallowed" in CRuby — Integer / Float /
    //         Symbol / Regexp / Proc / Method / UnboundMethod /
    //         TrueClass / FalseClass / NilClass / Kernel. CRuby
    //         raises TypeError; rubyrs matches byte-for-byte.
    //       * "Allowed in CRuby" — String / Array / Hash / Range.
    //         CRuby produces a bare instance of the builtin
    //         (empty string / array / hash / Range struct); rubyrs
    //         currently raises TypeError because the heap model
    //         unboxes those values and we don't yet route through
    //         a TypedData allocator. Documented as a KNOWN GAP
    //         below; the comment used to claim CRuby parity here
    //         which was wrong (PR #181 review round 4 Copilot
    //         comment #2).
    //     Either way: zero Instance slot to populate, so the
    //     bare-allocator path can't run for any primitive shell.
    //   - Zero args; any positional arg raises ArgumentError
    //     with the standard "wrong number of arguments" shape.
    //
    // KNOWN GAP: `cext_alloc_func` (set by
    // `rb_define_alloc_func`) is currently NOT routed through
    // this arm. The `new` arm below DOES route through it (so a
    // cext `Foo.new` produces a TypedData-wrapped Object), but
    // `Foo.allocate` here falls back to the default bare
    // Instance. For a cext whose initialize-after-allocate
    // relies on the alloc_func having wrapped its C struct, the
    // separation of allocate-vs-new becomes visible. No caller
    // surfaced today (pass-7 probe layer #4 only needs the
    // bare Instance path). Routed via a follow-up if a cext
    // surfaces the need; tracked as a comment so a future
    // reader doesn't think the bare-allocate is an oversight.
    // String-compare on the already-resolved `name` instead of
    // interning "allocate" each call (PR #181 review round 3
    // Copilot comment #1). Avoids both the per-call hash lookup
    // on a hot dispatch path and the latent edge case where
    // unconditional `intern()` could grow the symbol table
    // outside the existing `Config::max_symbols` accounting
    // points.
    if name == "allocate"
        && let Value::Class(cls) = &recv {
        if !args.is_empty() {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
            }));
        }
        // Eigenclass-shell fence — CRuby:
        // `A.singleton_class.allocate` raises TypeError ("can't
        // create instance of singleton class"). Without this the
        // shell falls into the bare-instance allocator below and
        // produces an orphan. (Code-review #253 round 9 #1.)
        if cls.singleton_target.borrow().is_some() {
            return Err(self.trap(RubyError::TypeError {
                msg: "can't create instance of singleton class".into(),
            }));
        }
        // Module / Class shells are NOT user classes — a real
        // CRuby raises NoMethodError ("undefined method
        // 'allocate' for ...Module/Class...") on Module-flavored
        // receivers; we approximate with the same TypeError
        // surface as the primitive shells so the call site sees
        // a clean failure instead of a bogus bare-Instance whose
        // `class` says Module but which can't behave like one
        // (PR #181 review #1 — Copilot flagged this gap).
        // KNOWN GAP: `Class.allocate` itself in CRuby DOES
        // succeed (returns a new anonymous Class). We block it
        // here for safety until a proper Class/Module allocator
        // lands; the only caller surfaced today (ERB stub) wants
        // an Instance, not a Class.
        if cls.is_module
            || cls.name == "Module"
            || cls.name == "Class"
            || is_primitive_class_name(&cls.name)
        {
            // Anonymous Module / Class shells have an empty
            // `cls.name`; without a fallback the message would
            // read "allocator undefined for " (trailing space,
            // no class hint). Pick "Module" vs "Class" by the
            // `is_module` flag so the surface is actionable
            // (PR #181 review round 3 Copilot comment #2).
            let display = if cls.name.is_empty() {
                if cls.is_module { "Module" } else { "Class" }
            } else {
                &cls.name
            };
            return Err(self.trap(RubyError::TypeError {
                msg: format!("allocator undefined for {}", display),
            }));
        }
        let obj = self.alloc_default_instance(cls)?;
        self.stack.push(obj);
        return Ok(ClassOutcome::Handled);
    }
    if name_id == new_id
        && let Value::Class(cls) = &recv {
            // L3-F: cext-registered allocator path. When the class
            // came from rb_define_class_under AND the cext called
            // rb_define_alloc_func on it, route the allocation
            // through that callback (typically wraps a malloc'd C
            // struct in TypedData) instead of producing a bare
            // Instance. Without this, every TypedData_Get_Struct in
            // the cext's instance methods fails because `self` is a
            // plain Instance, not a TypedData slot.
            // Outer PinGuard covers BOTH the allocator call and
            // the subsequent initialize. cext_dispatch can trigger
            // maybe_gc (TypedData wrap, result translation,
            // nested rb_funcall); args + obj live only as Rust
            // locals here and would be swept otherwise (PR #50
            // review #1 + #3 — same shape as the Integer#times
            // PinGuard fix in L3-D).
            let mut g = PinGuard::new(self);
            for a in &args { g.pin(a.clone()); }
            // Default Instance allocator — used by every branch of
            // the cext-selection cascade below that doesn't go
            // through `rb_define_alloc_func`. Delegates to
            // `Vm::alloc_default_instance` so this path and the
            // `Class#allocate` arm above can't drift on
            // GC/rooting/allocation behavior (PR #181 review #2).
            let alloc_instance = |g: &mut PinGuard, cls: &Rc<Class>| -> Result<Value, Trap> {
                g.vm.alloc_default_instance(cls)
            };
            // Allocator selection. With `cext`, the class may carry
            // an `rb_define_alloc_func`-registered allocator that
            // must run instead of the default Instance allocation.
            // Without `cext`, there is no path that could set such
            // a function, so we collapse to the default allocator
            // unconditionally. Splitting the whole expression by
            // cfg (instead of the previous `Option<()>` sentinel
            // trick) keeps both arms well-typed and removes a
            // brittle `unreachable!()` site that any future
            // refactor inside the cfg arm could turn into a real
            // panic.
            #[cfg(feature = "cext")]
            let obj = if let Some(alloc_func) = cls.cext_alloc_func.get() {
                #[cfg(not(target_os = "wasi"))]
                {
                    // arity=0 (self-only) is the alloc_func ABI:
                    // VALUE allocate(VALUE klass). CURRENT_VM_PTR
                    // must be set so the cext can rb_funcall back
                    // and rb_data_typed_object_wrap can locate
                    // the Vm to allocate on its heap.
                    let class_name = cls.name.clone();
                    let qualified = format!("{}::allocate", class_name);
                    let vm_ptr: *mut Vm = g.vm;
                    let raw = super::cext::with_vm_ptr_set(vm_ptr, || {
                        super::cext::cext_dispatch(
                            &qualified,
                            alloc_func,
                            0,
                            &[],
                            super::cext::CextSelfHandle::Class(&class_name),
                        )
                    })?;
                    // PR #50 review #2: validate that the cext
                    // honored the rb_define_alloc_func contract.
                    // CRuby's allocator must return an Object
                    // (typically TypedData_Wrap_Struct'd); if a
                    // buggy cext returns Nil / a Class / an Int
                    // and we silently proceed, `initialize` is
                    // called on something that's not an instance,
                    // and instance-method dispatch later fails
                    // in a way that's hard to trace back to the
                    // allocator. Trap immediately with TypeError.
                    match &raw {
                        Value::Object(_) => raw,
                        other => {
                            let msg = format!(
                                "allocator function for {} must return an Object, got {}",
                                class_name,
                                other.type_name()
                            );
                            return Err(g.vm.trap(RubyError::TypeError { msg }));
                        }
                    }
                }
                #[cfg(target_os = "wasi")]
                {
                    // wasi: cext path is stubbed; fall back to
                    // plain Instance allocation. The `alloc_func`
                    // from the if-let binding is unused on this
                    // target (no cext_dispatch to forward it to);
                    // marker reference keeps -D warnings happy.
                    let _ = alloc_func;
                    alloc_instance(&mut g, cls)?
                }
            } else {
                alloc_instance(&mut g, cls)?
            };
            #[cfg(not(feature = "cext"))]
            let obj = {
                // No cext_alloc_func field exists in this build;
                // the class always allocates a plain Instance.
                alloc_instance(&mut g, cls)?
            };
            // Pin the freshly-allocated obj across initialize so
            // a maybe_gc inside the (cext-defined or Ruby-defined)
            // initialize doesn't sweep it.
            g.pin(obj.clone());
            let init_id = g.vm.interner.intern("initialize");
            let ruby_init = g.vm.lookup_method_uncached(cls, init_id);
            if let Some(m) = ruby_init {
                // Ruby-defined initialize takes precedence.
                // Drop the guard before invoke_method (which
                // needs &mut self uncontested); the pinned
                // entries survive only the alloc step — by this
                // point obj/args are already on Rust locals that
                // invoke_method propagates.
                drop(g);
                self.invoke_method(m, obj.clone(), args.into_vec())?;
                self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
            } else if let Value::Array(aid) = &obj
                && !args.is_empty()
            {
                // Array subclass with NO user initialize:
                // `Subclass.new(n, fill)` must still honour
                // Array#initialize semantics (CRuby: SR.new(3, :x)
                // → [:x, :x, :x] tagged SR). The collection_call
                // "initialize" arm fills the tagged instance in
                // place. Zero-arg new skips this — the allocator's
                // empty elems already ARE Array#initialize().
                let aid = *aid;
                drop(g);
                let recv = Value::Array(aid);
                self.collection_call(&recv, "initialize", &args)?;
                self.stack.push(recv);
                return Ok(ClassOutcome::Handled);
            } else {
                // L3-F + L3-H: cext-defined initialize (registered
                // via rb_define_method) lives in
                // cext_instance_methods. Dispatch through the
                // existing instance-method path if present — this
                // picks up arity validation and rb_raise handling
                // for free. Both fixed arity 0..=5 AND variadic
                // arity -1 are now dispatchable (L3-H setjmp shim
                // supports case -1); the filter below mirrors
                // cext_dispatch's accepted-arities rule.
                #[cfg(all(feature = "cext", not(target_os = "wasi")))]
                {
                    // PR #60 review #10: don't silently skip
                    // initialize on arity mismatch — that
                    // diverges from Ruby semantics
                    // (`Klass.new` must raise ArgumentError if
                    // the args don't fit initialize). Only
                    // filter on whether the arity is
                    // dispatchable by the setjmp shim at all
                    // ({-1} ∪ 0..=5); cext_dispatch then
                    // validates argc against arity for fixed
                    // cases and raises ArgumentError on a
                    // mismatch.
                    let cext_init_reg = g.vm.cext_instance_methods
                        .get(cls.name.as_str())
                        .and_then(|t| t.get(&init_id).cloned())
                        .filter(|reg| reg.arity == -1 || (0..=5).contains(&reg.arity));
                    if let Some(reg) = cext_init_reg {
                        let qualified = reg.qualified_name.clone();
                        let func = reg.func;
                        let arity = reg.arity;
                        let obj_clone = obj.clone();
                        let args_ref = args.to_vec();
                        let vm_ptr: *mut Vm = g.vm;
                        super::cext::with_vm_ptr_set(vm_ptr, || {
                            super::cext::cext_dispatch(
                                &qualified, func, arity, &args_ref,
                                super::cext::CextSelfHandle::Object(obj_clone),
                            )
                        })?;
                    }
                }
                drop(g);
                self.stack.push(obj);
            }
            return Ok(ClassOutcome::Handled);
        }
        // No class-arm matched; return args + recv intact.
        Ok(ClassOutcome::NotHandled { args, recv })
    }

        /// `Op::CallKw*` entry — the compiler emits this for call
        /// sites whose trailing arg came from `KeywordHashNode`
        /// (`foo(k: v)` sugar). Peek at the trailing Hash on the
        /// stack; if the call targets a primitive that consumes
        /// the kwarg (currently only `Integer#round(half:)` /
        /// `Float#round(half:)`), dispatch the kwarg-aware path
        /// directly. Otherwise fall through to `do_call`, which
        /// continues to treat the trailing Hash as a positional
        /// arg — preserves today's behaviour for user-defined
        /// methods (whose `invoke_method` already pops the Hash
        /// when the proto declares kw_params) and for primitives
        /// that genuinely take a positional Hash.
        pub(crate) fn do_call_kw(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
            // Empty / nil keyword-splat contributes ZERO arguments,
            // matching CRuby: `f(**{})` and `f(**nil)` pass nothing
            // (and `f(1, **{})` passes just `1`). The kwargs travel
            // as the trailing stack arg under CallKw; an EMPTY Hash
            // (from `**{}` or an empty `**h`) or `nil` (from `**nil`)
            // must be dropped so a `*rest` callee doesn't collect a
            // phantom positional — `pos(**{})` is `[]`, not `[{}]`.
            // Non-empty kwargs hashes are left intact (they're real
            // kwargs / the trailing positional hash a no-kwarg callee
            // receives). Runs before the `round` arm so
            // `5.round(**{})` degrades to `5.round`.
            if argc > 0 {
                let drop_trailing = match self.stack.last() {
                    Some(Value::Hash(hid)) => self.heap.hash(*hid).is_empty(),
                    Some(Value::Nil) => true,
                    _ => false,
                };
                if drop_trailing {
                    self.stack.pop();
                    return self.do_call(name_id, argc - 1, no_recv, cache_id);
                }
            }
            // Only `round` is kwarg-aware today, AND only for
            // Int/Float receivers with a supported arg shape.
            // Every other shape — user-defined `C#round(half:)`,
            // 2+ positional args, non-Integer precision, BigInt
            // receiver — must fall back to `do_call` so the
            // existing primitive arms (arity ArgumentError, TypeError
            // for non-Integer precision) AND user-method dispatch
            // still fire. The trailing Hash travels as positional in
            // that path, identical to pre-CallKw behaviour.
            // SymId compare instead of resolving + cloning the
            // name on every CallKw dispatch — the `interner.intern`
            // is amortised across the run (same id returned for the
            // canonical "round" string), so a single == lookup
            // beats a per-call heap allocation. Same pattern below
            // for the `:half` key probe.
            let round_id = self.interner.intern("round");
            if name_id != round_id {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            // Peek receiver + trailing arg WITHOUT disturbing the
            // stack — the fallback `do_call` needs the stack intact.
            if argc == 0 {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            let stack_len = self.stack.len();
            let trailing = self.stack[stack_len - 1].clone();
            let Value::Hash(hash_id) = trailing else {
                return self.do_call(name_id, argc, no_recv, cache_id);
            };
            // Receiver position: if `no_recv` it's the frame self;
            // else it's stack[stack_len - argc - 1].
            let recv_peek = if no_recv {
                self.frames.last().expect("ICE: do_call_kw no frames").self_val.clone()
            } else {
                if stack_len < argc + 1 {
                    return self.do_call(name_id, argc, no_recv, cache_id);
                }
                self.stack[stack_len - argc - 1].clone()
            };
            if !matches!(recv_peek, Value::Int(_) | Value::Float(_)) {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            // Positional arg shape — only `[]` (no precision) and
            // `[Int]` (single Integer precision) are supported by
            // the kwarg helpers. Anything else (arity > 1,
            // non-Integer precision, BigInt precision) is left to
            // the regular round arm in numeric.rs which has the
            // existing ArgumentError / TypeError / BigInt guards.
            let positional_argc = argc - 1; // exclude the kwargs Hash
            if positional_argc > 1 {
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            if positional_argc == 1 {
                let precision = &self.stack[stack_len - 2];
                if !matches!(precision, Value::Int(_)) {
                    return self.do_call(name_id, argc, no_recv, cache_id);
                }
            }
            // Resolve the :half kwarg. CRuby raises
            // `ArgumentError: unknown keyword: :foo` for unknown
            // keys, `ArgumentError: invalid rounding mode: foo`
            // for unknown values.
            let half_sym = self.interner.intern("half");
            let pairs: Vec<(Value, Value)> = self.heap.hash(hash_id).clone();
            let mut mode = crate::vm::numeric::HalfMode::Up;
            for (k, v) in &pairs {
                match k {
                    Value::Sym(s) if *s == half_sym => {
                        // Mode resolution without per-dispatch allocation:
                        // Symbol values match against the canonical SymId
                        // (pre-interned once before the loop); String
                        // values use `with_str_lossy` so the comparison
                        // runs against borrowed `&str` instead of an
                        // owned `String`. Non-Sym/Str values surface a
                        // CRuby-shape "invalid rounding mode: <inspect>"
                        // — using `to_inspect` instead of the class name
                        // mirrors `Float#round` / `Numeric#round`'s
                        // shape (e.g. `0` / `nil` / `1.5` instead of
                        // `Integer` / `nil` / `Float`).
                        let up_id = self.interner.intern("up");
                        let down_id = self.interner.intern("down");
                        let even_id = self.interner.intern("even");
                        let resolved: Option<crate::vm::numeric::HalfMode> = match v {
                            Value::Sym(vsym) => {
                                if *vsym == up_id { Some(crate::vm::numeric::HalfMode::Up) }
                                else if *vsym == down_id { Some(crate::vm::numeric::HalfMode::Down) }
                                else if *vsym == even_id { Some(crate::vm::numeric::HalfMode::Even) }
                                else { None }
                            }
                            Value::Str(s) => s.with_str_lossy(|t| match t {
                                "up" => Some(crate::vm::numeric::HalfMode::Up),
                                "down" => Some(crate::vm::numeric::HalfMode::Down),
                                "even" => Some(crate::vm::numeric::HalfMode::Even),
                                _ => None,
                            }),
                            _ => {
                                let inspected = v.to_inspect(&self.heap, &self.interner);
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: format!("invalid rounding mode: {}", inspected),
                                }));
                            }
                        };
                        mode = match resolved {
                            Some(m) => m,
                            None => {
                                // For unknown Symbol/String values
                                // CRuby reports the bare name
                                // (`invalid rounding mode: weird`);
                                // for non-Sym/Str values the inspect
                                // shape carries more information
                                // (handled in the outer match arm).
                                let label: String = match v {
                                    Value::Sym(vsym) => self.interner.resolve(*vsym).to_string(),
                                    Value::Str(s) => s.to_string_lossy(),
                                    _ => unreachable!("non-Sym/Str handled by outer arm"),
                                };
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: format!("invalid rounding mode: {}", label),
                                }));
                            }
                        };
                    }
                    Value::Sym(s) => {
                        let key = self.interner.resolve(*s).to_string();
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!("unknown keyword: :{}", key),
                        }));
                    }
                    _ => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "non-symbol key in keyword arguments".to_string(),
                        }));
                    }
                }
            }
            // Stack consume — receiver + positional + kwargs Hash.
            // Guards above guarantee shape is one of:
            //   - (Int|Float, [])
            //   - (Int|Float, [Int])
            let _kwargs_hash = self.stack.pop().expect("ICE: kwargs hash");
            let pos_args: Vec<Value> = {
                let split = self.stack.len() - positional_argc;
                self.stack.drain(split..).collect()
            };
            let recv = if no_recv {
                self.frames.last().expect("ICE: do_call_kw no frames").self_val.clone()
            } else {
                self.stack.pop().expect("ICE: do_call_kw recv")
            };
            // i128 overflow → BigInt promotion under bignum, or a
            // RangeError without it (matches CRuby's behaviour for
            // overflow into a number that doesn't fit native int).
            let promote_overflow = |this: &mut Vm, overflow: i128| -> Result<Value, Trap> {
                #[cfg(feature = "bignum")]
                {
                    let b = num_bigint::BigInt::from(overflow);
                    this.bigint_to_value(b)
                }
                #[cfg(not(feature = "bignum"))]
                {
                    let _ = overflow;
                    Err(this.trap(RubyError::RangeError {
                        msg: "rounded result out of i64 range".to_string(),
                    }))
                }
            };
            let result = match (&recv, pos_args.as_slice()) {
                (Value::Int(a), []) => {
                    match crate::vm::numeric::int_round_with_half(*a, 0, mode) {
                        Ok(v) => v,
                        Err(over) => promote_overflow(self, over)?,
                    }
                }
                (Value::Int(a), [Value::Int(n)]) => {
                    match crate::vm::numeric::int_round_with_half(*a, *n, mode) {
                        Ok(v) => v,
                        Err(over) => promote_overflow(self, over)?,
                    }
                }
                (Value::Float(a), []) => {
                    crate::vm::numeric::float_round_with_half(*a, 0, mode)
                        .map_err(|e| self.trap(e))?
                }
                (Value::Float(a), [Value::Int(n)]) => {
                    crate::vm::numeric::float_round_with_half(*a, *n, mode)
                        .map_err(|e| self.trap(e))?
                }
                _ => unreachable!("guards above limit recv+args to Int/Float × [] | [Int]"),
            };
            self.stack.push(result);
            Ok(())
        }
        pub(crate) fn do_call(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        // Consume `bypass_visibility_once` at the dispatch
        // boundary, before any arm runs. A naive consume-at-the-
        // vis-check would leak the flag whenever the dispatch
        // bottoms out without entering the Value::Object arm
        // (e.g. `send(:nonexistent)` on a primitive receiver
        // raises NoMethodError before the Object arm is reached
        // — the flag would survive and silently bypass the next
        // call's vis check).
        let bypass_visibility = self.take_bypass_visibility();
        // One-shot primitive-dispatch request from a primitive-alias
        // forwarder (`Op::ApplyCallPrimitive`). Taken at the boundary
        // (like `bypass_visibility`) so it applies to THIS dispatch
        // only and can't leak. When set, a subclassed-primitive's user
        // override is skipped so the primitive itself runs.
        let force_primitive = std::mem::take(&mut self.force_primitive_dispatch);
        if no_recv {
            let self_val = self
                .frames
                .last()
                .expect("ICE: do_call(no_recv) with empty frames")
                .self_val
                .clone();
            if matches!(self_val, Value::Nil)
                && !self.host_fns.contains_key(&name_id)
                && let Some(m) = self.lookup_toplevel_method_cache_hit(cache_id)
                && self.try_invoke_fixed_method_from_stack(m, self_val, argc, None)?
            {
                return Ok(());
            }
        }
        // A name with an active refinement must NOT take the fast paths
        // (they'd return the original primitive / cached method before
        // the refinement check below). The gate is the cheap empty-set
        // test, so no-refinement programs are unaffected; even with
        // refinements active, only the few refined names detour.
        let maybe_refined = !self.refined_method_names.is_empty()
            && self.refined_method_names.contains(&name_id);
        // Primitive-receiver fast-path. Runs after
        // `take_bypass_visibility()` above; the helper's doc
        // comment spells out why that's currently safe and what
        // changes if a non-primitive arm is ever added.
        if !maybe_refined && self.try_fast_primitive(name_id, argc, no_recv) {
            return Ok(());
        }
        // Collection-index fast path (`h[k]` / `a[i]` on plain
        // Hash/Array) — same gating contract as `try_fast_primitive`
        // above; the helper's doc comment spells out the soundness
        // gates (override flags, subclass tags, default fall-through).
        if !maybe_refined && self.try_fast_index(name_id, argc, no_recv) {
            return Ok(());
        }
        // Explicit-receiver monomorphic fast path: an `obj.method(args)`
        // call on a user Object whose cached method is public, fixed-arity
        // and non-closure invokes stack-direct (no args Vec, pooled
        // locals), short-circuiting the full dispatch preamble. Everything
        // else (private/protected, method_missing, the send-forms,
        // primitives, non-fixed arity) falls through to the path below,
        // which resolves identically (same class_of + lookup_method_cached).
        if !maybe_refined && !no_recv && self.try_invoke_explicit_recv_cached(name_id, argc, cache_id)? {
            return Ok(());
        }
        // Class/Module-receiver sibling (`X.class_method`): resolves
        // via the same singleton walk the canonical Class-recv arm
        // below uses, gated by `class_singleton_deny` so no earlier
        // name-keyed arm is bypassed. See the helper's doc comment.
        if !maybe_refined
            && !no_recv
            && !force_primitive
            && self.try_invoke_class_singleton_cached(name_id, argc, cache_id)?
        {
            return Ok(());
        }
        let name = self.interner.resolve(name_id).clone();
        // Universal-Object bare-call routing. Several universal
        // `Object` methods are implemented only in the explicit-recv
        // dispatch arms (they gate on `&recv` being
        // `Some(Value::Object(...))`), but a bare/implicit-self call
        // keeps `recv` None and never reaches them — so e.g.
        // `is_a?(Foo)` inside an instance method raised
        // `NoMethodError: undefined method 'is_a?'` even though
        // `obj.is_a?(Foo)` works. Close the gap by treating these
        // bare-form calls as `self.<method>(args)` when self is a
        // `Value::Object`: push self below the args and re-enter
        // with no_recv=false so the explicit path (incl. any user
        // override) handles them. Discovery: P3 Jekyll spike —
        // `convertible.rb#type` does `:pages if is_a?(Page)`.
        if no_recv
            && matches!(
                &*name,
                "instance_variable_get"
                    | "instance_variable_set"
                    | "instance_variable_defined?"
                    | "instance_variables"
                    | "is_a?"
                    | "kind_of?"
                    | "instance_of?"
            )
        {
            let self_val = self.frames.last()
                .expect("ICE: do_call(no_recv) with empty frames for ivar bare-call routing")
                .self_val.clone();
            if matches!(&self_val, Value::Object(_)) {
                // Insert receiver BELOW the args so the explicit-
                // recv path's stack layout (`[..., recv, arg1,
                // ..., argN]`) is satisfied — `do_call` drains
                // `argc` from the top, then pops the receiver
                // beneath it.
                let insertion = self.stack.len() - argc;
                self.stack.insert(insertion, self_val);
                return self.do_call(name_id, argc, /*no_recv=*/ false, cache_id);
            }
        }
        if no_recv {
            let self_val = self.frames.last()
                .expect("ICE: do_call(no_recv) with empty frames")
                .self_val.clone();
            let can_try_toplevel_fast_path = matches!(self_val, Value::Nil)
                && !self.host_fns.contains_key(&name_id)
                && !Self::is_builtin_name(&name)
                && !matches!(&*name, "send" | "__send__" | "method" | "__dir__");
            if can_try_toplevel_fast_path
                && let Some(m) = self.lookup_toplevel_method_cached(name_id, cache_id)
                && self.try_invoke_fixed_method_from_stack(m, self_val, argc, None)?
            {
                return Ok(());
            }
        }
        // Stack-buffer the args for small argc (the hot primitive-
        // receiver case — `arr.push(i)`, `h[k] = v`, `s << t`): no
        // heap Vec alloc on the path through `primitive_call` /
        // `collection_call`, which only ever borrow `&args`. argc >
        // ARGS_INLINE falls back to the prior `drain(..).collect()`.
        // The dispatch decision sequence below is byte-identical to
        // the all-`Vec` shape; only the args *container* changes.
        let args = ArgsBuf::drain_from(&mut self.stack, argc);
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before do_call receiver"))
        };

        if no_recv && self.try_dispatch_no_recv_builtin_or_host(&name, name_id, &args)? {
            return Ok(());
        }
        // Refinements: an active `using`'d refinement on the receiver's
        // class wins over the original method. Gated on the cheap
        // `refined_method_names` set, so a program that never calls
        // `using` pays nothing (the set is empty) and every non-refined
        // call short-circuits before the class_of lookup.
        if !self.refined_method_names.is_empty()
            && self.refined_method_names.contains(&name_id)
            && let Some(r) = &recv
        {
            let cls = self.class_of(r);
            if let Value::Class(c) = &cls {
                let tname = self.interner.intern(&c.name);
                if let Some(m) = self.active_refinements.get(&(tname, name_id)).cloned() {
                    let r = r.clone();
                    return self.invoke_method(m, r, args.into_vec());
                }
            }
        }
        // Reopen-precedence early gate: a USER `def` directly on a
        // primitive's class (`class String; def upcase; …`) must win
        // over the builtin arm (CRuby semantics), but the
        // primitive_call family runs BEFORE the primitive-receiver
        // user-method gate further down — so without this, the
        // builtin silently shadowed every such reopen. Gated to a
        // single u8 compare in the no-reopen universe (the
        // method_gen-revalidated `prim_reopen_mask`; the preamble is
        // audited collision-free). OWN table only — `include`d
        // modules must not beat builtin arms (String includes
        // Comparable). Operator SYNTAX on numerics compiles to
        // Op::BinOp and never reaches do_call — that layer stays
        // native (documented boundary); method-call syntax
        // (`5.+(2)`, `5.send(:+, 2)`) honors the reopen.
        if !force_primitive && let Some(r) = &recv {
            // Revalidate-on-gen-change here too: the other
            // revalidation triggers (try_fast_index /
            // try_fast_primitive) skip argc >= 1 non-index calls,
            // and a stale mask would silently miss a just-defined
            // reopen.
            if self.fast_index_checked_gen != self.method_gen {
                self.fast_index_revalidate();
            }
            if self.prim_reopen_mask != 0 {
                let bit: u8 = match r {
                    Value::Int(_) => 0,
                    #[cfg(feature = "bignum")]
                    Value::BigInt(_) => 0,
                    Value::Float(_) => 1,
                    Value::Str(_) => 2,
                    Value::Sym(_) => 3,
                    Value::Nil => 4,
                    Value::Bool(_) => 5,
                    Value::Rational(_) => 6,
                    _ => 7,
                };
                if bit < 7
                    && self.prim_reopen_mask & (1 << bit) != 0
                    && let Value::Class(cls) = self.class_of(r)
                {
                    let m = cls.methods.borrow().get(&name_id).cloned();
                    if let Some(m) = m {
                        let r = r.clone();
                        return self.invoke_method(m, r, args.into_vec());
                    }
                }
            }
        }
        // `__dir__` — Kernel private instance method. CRuby
        // allows two call shapes:
        //   - bare `__dir__` (implicit self / no_recv)
        //   - `self.__dir__` (explicit `self` receiver — the
        //     one private-method exception)
        // Any other receiver (`obj.__dir__`, `42.__dir__`) is
        // a "private method called" NoMethodError. Pre-fix the
        // arm only fired in the `no_recv` branch below; even
        // `self.__dir__` (the canonical idiom for forwarding
        // through `module_function`-style helpers) raised
        // NoMethodError.
        if &*name == "__dir__" && args.is_empty() {
            let is_implicit = recv.is_none();
            // Identity-compare the receiver with the current
            // frame's `self_val`. Matches the discriminator
            // pattern used by `equal?` (line 5236+): same-shape
            // arms for the heap variants, value match for the
            // primitives. Lets `self.__dir__` work from inside
            // any method body (`self` is the singleton receiver
            // expected to call its own private methods).
            let frame_self = self.frames.last().map(|f| &f.self_val);
            let is_self = matches!((&recv, frame_self), (Some(r), Some(s)) if {
                use std::rc::Rc;
                match (r, s) {
                    (Value::Nil, Value::Nil) => true,
                    (Value::Bool(a), Value::Bool(b)) => a == b,
                    (Value::Int(a), Value::Int(b)) => a == b,
                    (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
                    (Value::Sym(a), Value::Sym(b)) => a == b,
                    (Value::Object(a), Value::Object(b)) => a == b,
                    (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
                    (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b),
                    _ => false,
                }
            });
            if !is_implicit && !is_self {
                // Fall through to the normal method-lookup path
                // so the resulting NoMethodError carries the
                // correct receiver class name in its message.
            } else {
            use std::path::Path;
            let fname = self.frames.last()
                .map(|f| self.protos[f.proto_idx].filename.to_string())
                .unwrap_or_default();
            let lexical_parent = |fname: &str| -> String {
                Path::new(fname).parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| ".".to_string())
            };
            let wide_open = self.allow_filesystem_io && self.allowed_paths.is_none();
            let dir = if wide_open {
                match std::fs::canonicalize(&fname) {
                    Ok(real) => real.parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| ".".to_string()),
                    Err(_) => lexical_parent(&fname),
                }
            } else {
                lexical_parent(&fname)
            };
            self.stack.push(Value::new_str(dir));
            return Ok(());
            }
        }
        if no_recv {
            // Bare `send(:foo)` / `__send__(:foo)` — CRuby treats
            // these as `self.send(:foo)`. Resolve target and re-aim
            // through `do_call` with `no_recv = true` so the call
            // routes through the same implicit-self lookup path the
            // bare-call arm uses below. User `def send` on the
            // surrounding self wins for `send` (reserved-name rule
            // applies only to `__send__`); when the lookup finds a
            // user override, skip the recogniser so the normal
            // implicit-self arm below invokes it.
            //
            // The visibility-bypass flag is irrelevant here — the
            // no_recv arm doesn't enforce private/protected (calls
            // with implicit-self are always allowed) — but we still
            // set it for parity with the receiver-form arm, so any
            // helper that later inspects the flag sees a consistent
            // shape.
            // send/__send__ bypass recogniser — unified helper
            // (#192 commit 2/5). NotHandled returns args back so
            // the dispatcher can continue below.
            let args = match self.try_dispatch_send_bypass(&name, name_id, cache_id, args, None) {
                SendBypass::Handled(r) => return r,
                SendBypass::NotHandled { args, .. } => args,
            };
            // Bare `method(:foo)` — implicit-self capture. Same
            // shape as `obj.method(:foo)` (the receiver-form arm
            // below) but the receiver is the surrounding frame's
            // `self_val`. Lets `arr.map(&method(:foo))` work from
            // inside an instance method body without writing
            // `&self.method(:foo)`.
            let self_val = self.frames.last().expect("ICE: do_call with empty frames").self_val.clone();
            if &*name == "method" && args.len() == 1
                && let Value::Sym(bound_name_id) = &args[0] {
                    // Snapshot the Method at capture time so the
                    // BoundMethod survives a later remove_method.
                    // Use `heap.class_of` for Object self so a
                    // singleton method on `self` is captured
                    // (matches dispatch); `Vm::class_of` would
                    // skip the eigenclass and snapshot the real
                    // class's body instead.
                    let snapshot = match &self_val {
                        Value::Object(id) => {
                            let cls = self.heap.class_of(*id);
                            self.lookup_method_uncached(&cls, *bound_name_id)
                        }
                        _ => match self.class_of(&self_val) {
                            Value::Class(cls) => self.lookup_method_uncached(&cls, *bound_name_id),
                            _ => None,
                        },
                    };
                    self.maybe_gc(); // allow: gc-rooting — BoundMethod holds `recv: self_val.clone()` (cloned from `frames.last().self_val`, which stays rooted via `self.frames` for the whole alloc window) and a primitive `SymId`; no unrooted slot at risk.
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::BoundMethod {
                        recv: self_val.clone(),
                        name_id: *bound_name_id,
                        method: snapshot,
                    });
                    self.stack.push(Value::BoundMethod(id));
                    return Ok(());
                }
            if let Value::Object(id) = &self_val {
                let cls = self.heap.class_of(*id);
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                    self.invoke_method(m, self_val.clone(), args.into_vec())?;
                    return Ok(());
                }
            }
            // Toplevel `self` is `Value::Nil` in rubyrs; a bare call not
            // in the toplevel-`def` table (checked above) must still walk
            // self's ancestry — NilClass → Object → Kernel — so a method
            // added by reopening `module Kernel` (CRuby's main includes
            // Kernel) resolves implicitly. Without this, `kmeth("x")`
            // raised "undefined method for NilClass" even though
            // `self.kmeth("x")` (explicit Nil receiver, same ancestry)
            // worked. This is what installs the `Kernel#BigDecimal()`
            // conversion function on require.
            if matches!(&self_val, Value::Nil)
                && let Value::Class(cls) = self.class_of(&self_val)
                && let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                self.invoke_method(m, self_val.clone(), args.into_vec())?;
                return Ok(());
            }
            // `self` is a Class — inside a class singleton method
            // body (`def self.foo; bar; end` or `class << self; def
            // foo; bar; end; end`), a bare call to `bar` should
            // resolve against THIS class's own `singleton_methods`
            // table AND its superclass chain (so `Sub.foo` defined
            // via `def self.foo` is reachable from inside `Sub`'s
            // class methods even when foo lives on `Super`).
            // Without this arm the lookup fell through to
            // toplevel_methods only and produced
            // "undefined method ... for Class" — even though
            // `bar` was sitting right there on `self`.
            //
            // Uses the same `lookup_class_singleton_method` helper
            // as the explicit `cls.foo` dispatch (vm/dispatch.rs
            // ~660), so `self.bar` and bare `bar` resolve
            // identically.
            if let Value::Class(c) = &self_val
                && let Some(m) = self.lookup_class_singleton_method(c, name_id) {
                self.invoke_method(m, self_val.clone(), args.into_vec())?;
                return Ok(());
            }
            // Kernel private methods are implicit-self callable from ANY
            // self — every object's ancestry includes Kernel (via
            // Object). The arms above resolve self's own class /
            // singleton methods first; this fallback covers a Kernel
            // method called bare from a context the specific arms miss —
            // notably a class-method body / module function (self = a
            // Class), e.g. liquid's `def self.to_number; BigDecimal(...);
            // end`, and user `module Kernel; def Foo; end` conversion
            // functions.
            if let Some(ksym) = self.kernel_class_sym
                && let Some(kernel) = self.classes.get(&ksym).cloned()
                && let Some(m) = self.lookup_method_cached(&kernel, name_id, cache_id) {
                self.invoke_method(m, self_val.clone(), args.into_vec())?;
                return Ok(());
            }
            // Bare calls inside reopened-primitive method bodies —
            // `class Integer; def to_json; to_s; end; end` shape.
            // The Object arm above only fires for `Value::Object`
            // self; primitive selves (Int / Str / Sym / Float /
            // Array / Hash / TrueClass / FalseClass / NilClass /
            // ...) previously fell through to method_missing /
            // NoMethodError, even though `self.<name>` works fine.
            // The narrower `respond_to?`-only fix (~line 3924)
            // documented this gap explicitly; user code in the
            // wild — every `to_json` / `as_json` mixin shape
            // installed via reopening basic types — needs bare
            // call lookup on the primitive's class.
            //
            // Two-tier resolution:
            //   1. `lookup_method_uncached` on the primitive's
            //      class — catches user-defined sibling methods
            //      (`def helper; ...; end` plus `def caller;
            //      helper; end`).
            //   2. If step 1 doesn't find a Ruby-level method,
            //      bridge to the receiver-form dispatch by
            //      pushing `self_val + args` back on the stack
            //      and re-entering `do_call` with `no_recv=false`.
            //      Same pattern as the Class-bridge whitelist
            //      below. Catches primitive-only methods like
            //      `to_s` / `inspect` that live in the
            //      `try_fast_primitive` / primitive-arm path,
            //      not the class method table.
            //
            // Gated on `!matches!(Object | Class | Nil)` so the
            // Object and Class arms above stay authoritative for
            // their shapes, AND Nil-self stays on the toplevel
            // method path (rubyrs uses Value::Nil as the toplevel
            // `main` self; bridging from Nil would clobber the
            // `def foo; ...; end at the top level` slow path
            // below at ~line 3708, surfacing as NoMethodError
            // for NilClass instead of CRuby's correct
            // ArgumentError on arity mismatch). A real reopened
            // `class NilClass; def helper; ...; def caller;
            // helper; end; end` with bare-call sibling can still
            // be expressed via explicit `self.helper`; the
            // limitation is documented in SUBSET.md as the
            // mirror-image of this fix.
            // Bare call with a real `nil` receiver. rubyrs overloads
            // `Value::Nil` as BOTH the toplevel `<main>` self and
            // actual nil values, so the primitive arm below excludes
            // Nil to keep `<main>`'s bare calls on the toplevel-method
            // path (and preserve `def foo(a); foo; end` arity errors).
            // But a *user-defined* `NilClass` method must still resolve
            // when self is genuinely nil inside a method body — e.g.
            // ActiveSupport's `NilClass#blank?`, reached when the
            // inherited `Object#present?` calls bare `blank?` on a nil
            // receiver. `defining_class.is_some()` is true only for
            // real method frames (None for `<main>`, blocks, class
            // bodies), so this never fires for the toplevel main self.
            // Look up NilClass's own chain first; if it has no such
            // method, fall through to the toplevel path unchanged
            // (toplevel `def foo` lives in `toplevel_methods`, not on
            // NilClass, so it is untouched). Self-as-nil inside a block
            // body keeps the documented limitation (SUBSET.md).
            if matches!(&self_val, Value::Nil)
                && self.frames.last().is_some_and(|f| f.defining_class.is_some())
                && let Value::Class(cls) = self.class_of(&self_val)
                && let Some(m) = self.lookup_method_uncached(&cls, name_id)
            {
                self.invoke_method(m, self_val.clone(), args.into_vec())?;
                return Ok(());
            }
            if !matches!(&self_val, Value::Object(_) | Value::Class(_) | Value::Nil) {
                if let Value::Class(cls) = self.class_of(&self_val)
                    && let Some(m) = self.lookup_method_uncached(&cls, name_id)
                {
                    self.invoke_method(m, self_val.clone(), args.into_vec())?;
                    return Ok(());
                }
                // No Ruby-level method — bridge to receiver form
                // so primitive dispatch (Int#to_s, Str#length,
                // Sym#to_s, …) fires the same arm the explicit
                // `self.foo` lowering would have hit.
                let argc = args.len();
                self.stack.push(self_val.clone());
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
            }
            // Bare calls on Class instances inside `class Foo
            // ... end` bodies and `def self.X` singleton methods.
            // Each whitelisted name has a receiver-form arm
            // further down `do_call` (Class.new allocator,
            // Class#name, Class#method_defined?, Class#
            // instance_method, ...). Without this bridge the
            // bare-call branch would fall through to
            // `toplevel_methods` and raise NoMethodError, even
            // though `self.foo` works fine. Vendored msgpack-
            // ruby surfaced two of these:
            //   - `def self.from_msgpack_ext(...); new(...); end`
            //     in timestamp.rb (bare `new`)
            //   - `class Symbol; if method_defined?(:name); ...`
            //     in symbol.rb (bare `method_defined?` inside an
            //     `if`/`else` at class-body top level)
            // Sinatra surfaced more (TRY_RUNS pass 8 layer #8):
            //   - `class Bar < Foo; superclass.class_eval { ... }`
            //     (bare `superclass` inside class body)
            // Push self_val + the original args back onto the
            // stack and re-enter `do_call` with `no_recv=false`
            // so the receiver-form dispatch takes over. Re-entry
            // walks all the explicit-receiver arms in order —
            // for `allocate` this means the dedicated arm with
            // its Module/primitive fences and user-singleton
            // override fires WITH all fences intact (PR #196
            // Copilot review #1 caught that a previous version
            // of this comment claimed `allocate` was omitted
            // "to preserve fences", but the bridge re-entry
            // routes through the dedicated arm, so including
            // it both fixes bare `allocate` AND keeps the
            // fences).
            //
            // Whitelist contract: this set is exactly lookup.rs's
            // `Value::Class(_)` primitive-method respond_to set
            // (see the `Value::Class(cls) =>` arm of
            // `Vm::responds_to`, around the `"allocate"` gate).
            // Keep both in lockstep — `respond_to?(:foo)` true
            // should mean a bare call to `foo` from inside a
            // class body resolves identically to `self.foo`.
            // `allocate` has the same Module fence as respond_to
            // (applied below); the rest of the names apply to
            // all `Value::Class` receivers.
            //
            // `class_eval` / `module_eval` added so a bare call
            // inside a class body (`class C; class_eval(...); end`)
            // reaches the receiver-form dispatch instead of falling
            // through to NoMethodError, mirroring how
            // `self.class_eval(...)` and `respond_to?(:class_eval)`
            // already work.
            //
            // (A future refactor could lift this list to a
            // shared `pub(crate) const &[&str]` consumed by
            // both sites — out of scope for this PR but tracked
            // as a follow-up by Copilot review #1.)
            //
            // `define_singleton_method` is also valid with an
            // Object instance as self (top-level `main` or
            // inside any instance method body). The
            // Value::Class branch below handles the
            // bare-in-class-body case; this small bridge
            // covers the Object-self case by re-entering as
            // explicit-receiver. Without it, bare-call no_recv
            // misses both the Class bridge and the receiver-
            // form arm, surfacing as NoMethodError instead of
            // CRuby's ArgumentError / TypeError. Block-form is
            // already handled by `do_call_block`'s own no_recv
            // path. PR #309 cycle-5.
            if matches!(&self_val, Value::Object(_))
                && &*name == "define_singleton_method"
            {
                let argc = args.len();
                self.stack.push(self_val.clone());
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
            }
            // 2-arg `define_method` / `define_singleton_method`
            // in a class body — intercept BEFORE the bridge
            // re-enters as explicit-recv. For
            // `define_method` this matters because the install
            // inherits the surrounding class-body visibility
            // (which the bridge re-entry would have lost); the
            // recv-form arm in `try_dispatch_class_intrinsics`
            // defaults to Public for that 2-arg form precisely
            // because this intercept takes the no_recv path
            // first. For `define_singleton_method` the install
            // is always Public regardless of context (matching
            // the block-form arm and CRuby's class-method
            // semantics), but it's intercepted here too so the
            // bridge whitelist doesn't need to special-case
            // arity. PR #321 cycle-1.
            if matches!(&*name, "define_method" | "define_singleton_method")
                && args.len() == 2
                && let Value::Class(cls) = &self_val
            {
                let name_sym = match &args[0] {
                    Value::Sym(s) => *s,
                    Value::Str(s) => {
                        let raw = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&raw) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        self.interner.intern(&raw)
                    }
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Symbol or String)",
                            other.type_name(),
                        ),
                    })),
                };
                let src = args[1].clone();
                let vis = self.class_visibility_stack.last().copied()
                    .unwrap_or(crate::value::Visibility::Public);
                let installed = if &*name == "define_method" {
                    self.install_method_from_value(cls, name_sym, &src, vis)
                } else {
                    self.install_singleton_method_on_class_from_value(
                        cls, name_sym, &src,
                    )
                }
                .map_err(|e| self.trap(e))?;
                self.stack.push(Value::Sym(installed));
                return Ok(());
            }
            if let Value::Class(cls) = &self_val {
                let in_set = matches!(&*name,
                    "new" | "name" | "to_s" | "inspect"
                    | "method_defined?" | "instance_method" | "undef_method" | "remove_method"
                    | "superclass" | "ancestors" | "include?"
                    | "instance_methods" | "public_instance_methods"
                    | "private_instance_methods" | "protected_instance_methods"
                    | "constants"
                    | "autoload" | "autoload?" | "const_defined?" | "const_get" | "const_set" | "private_constant" | "public_constant"
                    | "deprecate_constant"
                    | "private_class_method" | "public_class_method"
                    | "singleton_class"
                    // Reflection over the class-method tier: bare
                    // `public_methods` / `methods` / … inside a
                    // `module M; ... end` body (no_recv, self = the
                    // Class) bridges to the receiver-form arm so it
                    // returns the singleton-method list instead of
                    // raising NoMethodError. colorator's
                    // `CORE_METHODS = (public_methods - Object.methods)`
                    // at module-body top level is the motivating case.
                    | "methods" | "public_methods" | "private_methods"
                    | "protected_methods" | "singleton_methods"
                    | "class_eval" | "module_eval"
                    // `define_method` joins the bridge so bare
                    // `define_method(:foo)` inside a class body
                    // (no_recv, NO block) is forwarded to the
                    // Value::Class(cls) recv form, where
                    // `try_dispatch_class_intrinsics` raises the
                    // CRuby-shape `ArgumentError ("tried to create
                    // Proc object without a block")`. The block
                    // form (`define_method(:foo) { … }`) has its
                    // own no_recv handling in `do_call_block` and
                    // does NOT need this bridge. Keeps the
                    // do_call bridge whitelist in lockstep with
                    // lookup.rs's respond_to whitelist (PR #245
                    // Copilot round 2 #1).
                    | "define_method"
                    // `define_singleton_method` joins the bridge
                    // for the same reason: bare bare form inside
                    // a class body (no_recv, no block) needs to
                    // reach the receiver-form arm at line ~5427
                    // so the user sees ArgumentError / TypeError
                    // instead of NoMethodError. Block-form has
                    // its own no_recv handling in
                    // `do_call_block` (line ~6964).
                    // PR #309 cycle-4.
                    | "define_singleton_method"
                );
                // `allocate` gets the same Module fence as
                // lookup.rs's respond_to gate so bare `allocate`
                // inside a `module Foo; ... end` body falls
                // through to NoMethodError instead of bridging
                // into the dedicated arm and raising TypeError.
                // True lockstep with respond_to: if
                // `m.respond_to?(:allocate)` is false (Modules,
                // the global `Module` shell), bare `allocate`
                // shouldn't dispatch. PR #196 Copilot round 2 #1.
                let allocate_allowed =
                    &*name == "allocate"
                        && !cls.is_module
                        && cls.name != "Module";
                if in_set || allocate_allowed {
                    let argc = args.len();
                    self.stack.push(self_val.clone());
                    for a in args { self.stack.push(a); }
                    // `cache_id = u16::MAX` (sentinel: skip cache
                    // write) — re-entry from a bare-call site
                    // into a receiver-form lookup; the cache
                    // slot was minted for the bare shape and
                    // mustn't be populated with a receiver-form
                    // entry that a future bare retry could
                    // consult. Same pattern as send / send_with_
                    // block re-entries (lines ~464 / ~924, plus
                    // the lib.rs sentinel comment at ~77).
                    return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
                }
            }
            // (`__dir__` is now handled by the hoisted arm
            // above the `if no_recv` block — Kernel mixin
            // parity. Don't add a duplicate here.)
            // Mirror the fast-path guard above (`can_try_toplevel_fast_path`
            // around line 345): the toplevel cache slot key
            // (`TOPLEVEL_METHOD_CACHE_KEY`) doesn't carry the
            // name, so the cache-hit fast path
            // (`lookup_toplevel_method_cache_hit`) can't tell a
            // user `def sprintf` from the builtin. Skipping the
            // populator here for builtin names keeps the cache
            // slot empty for those call sites, so the fast path
            // can't return a shadowing user method on a future
            // hit. This is the load-bearing version of the
            // `debug_assert!` inside `lookup_toplevel_method_cached`
            // (which only fires in debug builds).
            if !Self::is_builtin_name(&name)
                && let Some(m) = self.lookup_toplevel_method_cached(name_id, cache_id)
            {
                self.invoke_method(m, self_val, args.into_vec())?;
                return Ok(());
            }
            // `include Mod` / `extend Mod` / `prepend Mod` inside
            // a class body — `self` is the class, name resolves
            // with no receiver. Pushes the source module onto the
            // target's `includes` or `prepends` chain (split by
            // method name; see the dispatch order comment on
            // `lookup_method_uncached`). Methods aren't copied —
            // `lookup_method_uncached` walks the chain at dispatch
            // time. Bumps `method_gen` so any monomorphic inline
            // cache entry that thought the class lacked the
            // included/prepended methods invalidates.
            // `private_constant :Foo, :Bar` / `public_constant ...` —
            // visibility hints for module constants. CRuby uses them
            // to prevent external `Tilt::EMPTY_HASH` access; rubyrs
            // doesn't enforce constant visibility yet (separate gap),
            // so the call is a no-op that returns the class. Returning
            // self matches CRuby's chainable form. Required for tilt
            // load (tilt.rb:11/14, tilt/mapping.rb:77/411 all use this).
            //
            // `Mod.autoload :Const, "path"` (or a bare
            // `autoload :Const, "path"` inside a `module Mod` body,
            // where self is the Class) — CRuby's lazy-load hook: the
            // constant materialises when first referenced. Phase 2 of
            // issue #224 records the entry in `autoloads_scoped` keyed
            // by the QUALIFIED name (`Mod::Const`); the first
            // reference that would otherwise miss in
            // `resolve_const_path` pops the entry, `require`s the
            // path, and re-resolves. Rack 3 / Sinatra register 40+ of
            // these at module-load time, so without this every
            // `Rack::Response` / `Rack::Builder` reference NameErrors.
            //
            // Arity matches CRuby: exactly 2 args. Wrong arity still
            // raises ArgumentError so caller bugs don't get hidden by
            // the stub fast-path. Returns nil (CRuby's actual return).
            if &*name == "autoload"
                && let Value::Class(owner) = &self_val {
                // `owner` drives the scoped-registry key, which only
                // exists on non-wasi (no `require` on wasm32-wasi).
                #[cfg(target_os = "wasi")]
                let _ = owner;
                if args.len() != 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
                    }));
                }
                // wasm32-wasi has no `require` (no file I/O), so the
                // trigger can never fire there — keep the historical
                // no-op rather than registering an entry that would
                // dangle. Named build registers into `autoloads_scoped`.
                #[cfg(not(target_os = "wasi"))]
                {
                    let const_name = match &args[0] {
                        Value::Sym(id) => self.interner.resolve(*id).to_string(),
                        Value::Str(s) => s.to_string_lossy(),
                        other => {
                            // CRuby reports the INSPECTED value
                            // (`123 is not a symbol nor a string`),
                            // not the type name.
                            let inspected = other.to_inspect(&self.heap, &self.interner);
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!("{} is not a symbol nor a string", inspected),
                            }));
                        }
                    };
                    let path = match &args[1] {
                        Value::Str(s) => s.to_string_lossy(),
                        other => {
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "no implicit conversion of {} into String",
                                    other.type_name()
                                ),
                            }));
                        }
                    };
                    // Qualified key parallel to `self.constants`:
                    // `Mod::Const`. A toplevel / anonymous owner
                    // (empty or "Object" name) keys by the bare
                    // const name — it can't form a useful `::`
                    // prefix, and the scoped trigger only consults
                    // qualified `Owner::Const` lookups anyway.
                    let key = if owner.name.is_empty() || owner.name == "Object" {
                        const_name
                    } else {
                        format!("{}::{}", owner.name, const_name)
                    };
                    let key_id = self.interner.intern(&key);
                    self.autoloads_scoped.insert(key_id, path);
                }
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // `autoload?(:Const [, inherit])` — CRuby returns the
            // file path string if `:Const` is still pending autoload
            // on this module, else nil. Phase 2 (issue #224) reads
            // the `autoloads_scoped` registry by qualified key
            // (`Mod::Const`) — returns the path while pending, nil
            // once the trigger has fired (the entry is removed) or
            // when never registered. tilt's `mapping.rb:362` calls
            // `scope.autoload?(n)` inside `constant_defined?`.
            if &*name == "autoload?"
                && let Value::Class(owner) = &self_val {
                #[cfg(target_os = "wasi")]
                let _ = owner;
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                #[cfg(not(target_os = "wasi"))]
                {
                    let const_name = match &args[0] {
                        Value::Sym(id) => Some(self.interner.resolve(*id).to_string()),
                        Value::Str(s) => Some(s.to_string_lossy()),
                        _ => None,
                    };
                    if let Some(cn) = const_name {
                        let key = if owner.name.is_empty() || owner.name == "Object" {
                            cn
                        } else {
                            format!("{}::{}", owner.name, cn)
                        };
                        if self.interner.contains(&key) {
                            let key_id = self.interner.intern(&key);
                            if let Some(path) = self.autoloads_scoped.get(&key_id) {
                                let v = Value::new_str(path.clone());
                                self.stack.push(v);
                                return Ok(());
                            }
                        }
                    }
                }
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // `Mod.const_defined?(:Const [, inherit])` — looks up
            // the qualified name in `self.classes` (Class/Module
            // table) AND `self.constants` (other Value constants).
            // tilt's `mapping.rb:361-365` walks `Tilt::Backend` etc.
            // via `scope.const_defined?(n)`. The `inherit` arg is
            // accepted for arity parity but Tier-1 doesn't model
            // ancestor const lookup — `Foo::Bar` only resolves on
            // Foo itself, not its includes/superclass chain.
            // (TRY_RUNS pass-10 layer #2.)
            if &*name == "const_defined?"
                && let Value::Class(cls) = &self_val {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                // CRuby splits the path on `::` for String args
                // but treats Symbol args as bare names
                // (`:"Foo::Bar"` raises wrong-name).
                // `resolve_const_path` centralises validation,
                // intern-cap gating, and per-segment walk.
                // (Copilot review #277 round 4 #3.)
                let (const_name, split) = match &args[0] {
                    Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                    Value::Str(s) => (s.to_string_lossy(), true),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                    })),
                };
                let cls_clone = cls.clone();
                let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
                match outcome {
                    ConstPathOutcome::Found(_) => self.stack.push(Value::Bool(true)),
                    ConstPathOutcome::Missing { .. } => self.stack.push(Value::Bool(false)),
                    ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                        msg: format!("wrong constant name {}", name),
                    })),
                    ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                        msg: format!("{} does not refer to class/module", full_path),
                    })),
                    // A scoped-autoload `require` trapped — re-raise.
                    #[cfg(not(target_os = "wasi"))]
                    ConstPathOutcome::Trap(t) => return Err(t),
                }
                return Ok(());
            }
            // `Mod.const_get(:Const [, inherit])` — paired with
            // const_defined?. Returns the actual Class/Value
            // constant if defined; raises NameError otherwise.
            // tilt's `constant_defined?` walk calls `scope.const_get(n)`
            // after the `const_defined?` check passes.
            if &*name == "const_get"
                && let Value::Class(cls) = &self_val {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                let (const_name, split) = match &args[0] {
                    Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                    Value::Str(s) => (s.to_string_lossy(), true),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                    })),
                };
                let cls_clone = cls.clone();
                let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
                match outcome {
                    ConstPathOutcome::Found(v) => { self.stack.push(v); return Ok(()); }
                    ConstPathOutcome::Missing { missing_qualified } => return Err(self.trap(RubyError::NameError {
                        msg: format!("uninitialized constant {}", missing_qualified),
                    })),
                    ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                        msg: format!("wrong constant name {}", name),
                    })),
                    ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                        msg: format!("{} does not refer to class/module", full_path),
                    })),
                    // A scoped-autoload `require` trapped — re-raise.
                    #[cfg(not(target_os = "wasi"))]
                    ConstPathOutcome::Trap(t) => return Err(t),
                }
            }
            // `private_constant` / `public_constant` /
            // `deprecate_constant` accept any number of symbol args
            // (CRuby; including zero, which is a no-op). We don't
            // enforce that args are Symbols since the stub ignores
            // them anyway; the documented gap is that wrong arg
            // types silently no-op here instead of TypeError.
            // `deprecate_constant` would emit a deprecation warning
            // in CRuby when the constant is read; rubyrs doesn't
            // model deprecation warnings, so the read path returns
            // the value silently (visibility unaffected).
            // Motivating use: MRI `lib/erb.rb:264`
            // (`deprecate_constant :Revision`).
            if matches!(&*name, "private_constant" | "public_constant" | "deprecate_constant")
                && let Value::Class(_) = &self_val {
                self.stack.push(self_val);
                return Ok(());
            }
            // `private_class_method` / `public_class_method` accept
            // any number of method-name args (Symbol or String) and
            // return the receiver. Flips the named singleton
            // methods' visibility (rubygems, fileutils and
            // forwardable-extended all call this during require;
            // the explicit-receiver twin lives in the recv arm).
            if matches!(&*name, "private_class_method" | "public_class_method")
                && let Value::Class(target) = &self_val {
                let vis = if &*name == "private_class_method" {
                    Visibility::Private
                } else {
                    Visibility::Public
                };
                let target = target.clone();
                self.apply_class_method_visibility(&target, &args, vis)?;
                self.stack.push(self_val);
                return Ok(());
            }
            // Bareword `alias_method(new, old)` inside a Class
            // singleton method (self is the Class). Sibling of the
            // explicit-receiver runtime arm — pre-fix this fell
            // through to the no_recv NoMethodError because
            // alias_method isn't a builtin Kernel method. rack-
            // protection's base.rb hits this via
            // `def self.default_reaction(reaction); alias_method(
            // :default_reaction, reaction); end`.
            if &*name == "alias_method" && args.len() == 2
                && let Value::Class(target) = &self_val {
                let new_id_opt = match &args[0] {
                    Value::Sym(id) => Some(*id),
                    Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                    _ => None,
                };
                let old_id_opt = match &args[1] {
                    Value::Sym(id) => Some(*id),
                    Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                    _ => None,
                };
                if let (Some(new_id), Some(old_id)) = (new_id_opt, old_id_opt) {
                    let m = self.lookup_method_uncached(target, old_id);
                    match m {
                        Some(method) => {
                            target.methods.borrow_mut().insert(new_id, method);
                            self.method_gen = self.method_gen.wrapping_add(1);
                            self.stack.push(Value::Class(target.clone()));
                            return Ok(());
                        }
                        None => {
                            let old_name = self.interner.resolve(old_id).to_string();
                            return Err(self.trap(RubyError::NameError {
                                msg: format!(
                                    "undefined method '{}' for class '{}'",
                                    old_name, target.name,
                                ),
                            }));
                        }
                    }
                }
            }
            if matches!(&*name, "include" | "extend" | "prepend") && !args.is_empty()
                && let Value::Class(target) = &self_val {
                    let is_prepend = &*name == "prepend";
                    let is_include = &*name == "include";
                    let target_cls = target.clone();
                    let mut fire_hooks: Vec<std::rc::Rc<crate::value::Class>> = Vec::new();
                    // CRuby processes `include M1, M2, ...` args
                    // RIGHT-to-LEFT — M2 inserts first, then M1.
                    // Each insert goes to the head of the chain, so
                    // M1 (last inserted) ends up at the head and
                    // M1.included fires LAST. Hook fire order also
                    // mirrors this iteration. Single-arg cases are
                    // unaffected. PR #347 documented follow-up.
                    //
                    // All three keywords iterate args right-to-left
                    // to match CRuby: `extend M1, M2` (and the same
                    // for include / prepend) processes M2 first so
                    // M1 ends up at the chain head and its hook
                    // fires LAST. Branch on the index inside the
                    // loop instead of allocating a boxed iterator —
                    // include/prepend/extend is hot enough that a
                    // heap alloc per call is wasteful.
                    let reverse_args = is_prepend || is_include || (&*name == "extend");
                    let n_args = args.len();
                    for idx in 0..n_args {
                        let a = if reverse_args { &args[n_args - 1 - idx] } else { &args[idx] };
                        let src = match a {
                            Value::Class(c) => c.clone(),
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "wrong argument type {} (expected Module)",
                                    a.type_name(),
                                ),
                            })),
                        };
                        // CRuby last-{included,prepended}-wins:
                        // push to the front so it's checked first
                        // by the lookup walk (which goes head-to-
                        // tail). `prepend` and `include` route into
                        // separate chains — `lookup_method_uncached`
                        // walks prepends BEFORE the class's own
                        // methods, and includes AFTER.
                        //
                        // Idempotency is PER-CHAIN for include /
                        // prepend (distinct insertion slots), not
                        // full ancestor-chain: `include M; prepend M`
                        // on the same target must succeed at both
                        // steps. The check still walks transitively
                        // within the chain — so `include
                        // ContainsM; include M` (where ContainsM
                        // includes M) skips the second include
                        // because M is reachable via the include
                        // chain. PR #347 documented follow-up.
                        //
                        // `Klass.extend(M)` (and bareword `extend M`
                        // inside a class body, which this no-recv
                        // arm handles) — see the explicit-receiver
                        // arm below for the singleton_includes
                        // rationale. Same fix; just a second site.
                        let is_extend = !is_include && !is_prepend;
                        let already_reachable = if is_extend {
                            target_cls.singleton_includes.borrow().iter().any(|m| std::rc::Rc::ptr_eq(m, &src))
                        } else {
                            super::class_reaches_via_chain(&target_cls, &src, is_prepend)
                        };
                        if !already_reachable {
                            let mut chain = if is_prepend {
                                target_cls.prepends.borrow_mut()
                            } else if is_extend {
                                target_cls.singleton_includes.borrow_mut()
                            } else {
                                target_cls.includes.borrow_mut()
                            };
                            chain.insert(0, src.clone());
                            drop(chain);
                            // include/prepend changes the cref-ancestor
                            // constant walk (`const_via_ancestors`) —
                            // invalidate the const ICs.
                            self.bump_const_gen();
                        }
                        // CRuby fires the `included` / `prepended`
                        // hook on EVERY include/prepend call — even
                        // when the chain mutation is a no-op
                        // (idempotent re-include). The hook isn't
                        // gated on chain change; it's documented as
                        // "called whenever a module is included in
                        // another module". For `extend` the hook is
                        // `Module.extended(target)`.
                        fire_hooks.push(src);
                    }
                    self.method_gen = self.method_gen.wrapping_add(1);
                    let hook_name = if is_prepend {
                        "prepended"
                    } else if is_include {
                        "included"
                    } else {
                        "extended"
                    };
                    self.fire_inclusion_hooks(&fire_hooks, &Value::Class(target_cls), hook_name)?;
                    self.stack.push(self_val.clone());
                    return Ok(());
                }
            // `private` / `protected` / `public` inside a class
            // body. With no args, switch the current visibility
            // mode for any subsequent `def`s. With Symbol or
            // String args, retroactively flip the visibility of
            // the listed methods on the current class. Outside a
            // class body these are no-ops returning nil — same
            // shape as CRuby's Module#private at the toplevel.
            // `module_function` — bare form switches the current
            // class-body to "module function" visibility mode (new
            // `def`s become private instance methods AND get
            // copied to the module's singleton class as public
            // module-level functions). Symbol-arg form retroactively
            // converts the listed already-defined instance methods.
            // Only intercepted for `Value::Class` receivers — other
            // receivers fall through so CRuby-style NoMethodError /
            // NameError surfaces naturally. (TRY_RUNS pass-10 layer
            // #12 — rack-3.1.10/lib/rack/utils.rb:37 + 161 use both
            // forms during sinatra-4's load chain.)
            //
            // Tier-1 divergences (documented):
            //   - Bare `module_function` from a non-Class receiver
            //     was previously a silent no-op; now falls through
            //     so the runtime can raise the right error.
            //
            // (The previously-documented "bare form doesn't auto-
            // mirror subsequent defs to singleton class" gap is
            // now closed — `module_function_active_stack` + the
            // dual-install arm in `Op::DefMethod` mirror new defs
            // onto `cls.singleton_methods` so `M.bare_def_method
            // (...)` resolves at runtime. `tests/diff/
            // module_function_bare.rb` pins the contract.)
            if &*name == "module_function"
                && let Value::Class(cls) = &self_val
            {
                if args.is_empty() {
                    if let Some(top) = self.class_visibility_stack.last_mut() {
                        *top = crate::value::Visibility::Private;
                    }
                    // Flip the parallel "auto-mirror to singleton"
                    // flag so subsequent `Op::DefMethod` inside
                    // this body installs a public clone on
                    // `cls.singleton_methods` alongside the
                    // private instance entry. Without this, bare
                    // `module_function` only flipped the visibility
                    // and `M.foo` (after a subsequent `def foo`)
                    // still raised NoMethodError — the documented
                    // Tier-1 gap that sinatra_jsonp_smoke's
                    // vendored multi_json shim and modular Sinatra
                    // plugins both stumbled into.
                    if let Some(active) = self.module_function_active_stack.last_mut() {
                        *active = true;
                    }
                } else {
                    // Symbol/String args: install a FRESH Method
                    // on the singleton with Public visibility, and
                    // flip the original instance method to Private.
                    // (Sharing the Rc — as the round-1 version did
                    // — would propagate `Private` to the singleton
                    // copy too because Method.visibility is a Cell
                    // shared through the Rc. CRuby's contract is
                    // "instance private, singleton public". Code-
                    // review #324 round 1.)
                    use crate::value::{Method, Visibility};
                    use std::cell::Cell;
                    let snapshot: Vec<(crate::intern::SymId, std::rc::Rc<Method>)> = {
                        let methods = cls.methods.borrow();
                        let mut out = Vec::with_capacity(args.len());
                        for a in &args {
                            let sid = match a {
                                Value::Sym(s) => *s,
                                Value::Str(s) => {
                                    let lossy = s.to_string_lossy();
                                    if let Some(max) = self.max_symbols
                                        && !self.interner.contains(&lossy)
                                        && self.interner.len() >= max
                                    {
                                        return Err(self.trap(RubyError::ResourceExhausted {
                                            msg: format!("interner exhausted: {} symbols", max),
                                        }));
                                    }
                                    self.interner.intern(&lossy)
                                }
                                other => {
                                    let inspected = other.to_inspect(&self.heap, &self.interner);
                                    return Err(self.trap(RubyError::TypeError {
                                        msg: format!(
                                            "{} is not a symbol nor a string",
                                            inspected,
                                        ),
                                    }));
                                }
                            };
                            match methods.get(&sid) {
                                Some(m) => out.push((sid, m.clone())),
                                None => {
                                    let nm = self.interner.resolve(sid).to_string();
                                    let kind = if cls.is_module { "module" } else { "class" };
                                    // Anonymous Module/Class shells have
                                    // an empty `cls.name`; fall back to
                                    // the kind label ("Module"/"Class")
                                    // so the error stays actionable —
                                    // mirrors the allocate TypeError
                                    // path at ~line 3032. (Code-review
                                    // #324 round 6.)
                                    let display = if cls.name.is_empty() {
                                        if cls.is_module { "Module" } else { "Class" }
                                    } else {
                                        &cls.name
                                    };
                                    return Err(self.trap(RubyError::NameError {
                                        msg: format!(
                                            "undefined method `{}' for {} `{}'",
                                            nm, kind, display,
                                        ),
                                    }));
                                }
                            }
                        }
                        out
                    };
                    for (sid, m) in snapshot {
                        let singleton_copy = std::rc::Rc::new(Method {
                            params: m.params.clone(),
                            proto_idx: m.proto_idx,
                            fixed_arity: m.fixed_arity,
                            // Singleton copy anchors at the class
                            // that physically holds it (matches
                            // other singleton-install paths and
                            // keeps `super` / `Method#owner`
                            // anchored consistently). (Code-review
                            // #324 round 4.)
                            defining_class: Some(std::rc::Rc::downgrade(cls)),
                            visibility: Cell::new(Visibility::Public),
                            closure: m.closure.clone(),
                            original_name: m.original_name,
                            builtin: m.builtin.clone(),
                        });
                        cls.singleton_methods.borrow_mut().insert(sid, singleton_copy);
                        m.visibility.set(Visibility::Private);
                    }
                    self.method_gen = self.method_gen.wrapping_add(1);
                }
                // CRuby's Module#module_function return value
                // (verified against MRI 3.x via `ruby -e`):
                //   - bare form          → nil
                //   - single Sym/Str arg → the symbol
                //   - multi args         → array of symbols
                // Earlier this arm always pushed Nil, which
                // matches the bare form but silently diverged
                // for the argument forms — callers using the
                // result as an expression got Nil instead of
                // the symbol/array. (Code-review #324
                // post-empty pass.)
                // CRuby preserves the original arg types in the
                // return value (strings stay strings — coercion
                // to Symbol happens internally for the lookup
                // only, not for the result). Verified via
                // `ruby -e 'module M; def w; end; r =
                // module_function("w", :x); p r; end'` →
                // `["w", :x]`.
                let ret = if args.is_empty() {
                    Value::Nil
                } else if args.len() == 1 {
                    args[0].clone()
                } else {
                    let id = self
                        .heap
                        .alloc(crate::heap::HeapObj::Array(args.to_vec().into()));
                    Value::Array(id)
                };
                self.stack.push(ret);
                return Ok(());
            }
            if let Some(vis) = visibility_from_name(&name) {
                if let Value::Class(cls) = &self_val {
                    if args.is_empty() {
                        if let Some(top) = self.class_visibility_stack.last_mut() {
                            *top = vis;
                        }
                    } else {
                        let methods = cls.methods.borrow();
                        for a in &args {
                            let key: Option<SymId> = match a {
                                Value::Sym(s) => Some(*s),
                                Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                                _ => None,
                            };
                            if let Some(mid) = key
                                && let Some(m) = methods.get(&mid) {
                                    m.visibility.set(vis);
                                }
                        }
                    }
                    self.stack.push(Value::Nil);
                    return Ok(());
                }
                // Toplevel `private` / `protected` / `public` —
                // CRuby treats these as visibility modifiers on
                // Object's singleton class. We don't model
                // toplevel methods as Object instance methods, so
                // the call has no observable effect; accept it as
                // a no-op rather than NoMethodError to keep
                // common preamble patterns (`private; def helper;`
                // at the toplevel) parseable.
                self.stack.push(Value::Nil);
                return Ok(());
            }
            // `respond_to?(:foo)` / `respond_to?(:foo, true)` with no
            // explicit receiver — implicit-self dispatch against the
            // current frame's `self_val`. Mirrors the recv-bearing
            // arm below (~line 2239); included here because the
            // no-recv path runs FIRST and would NoMethodError before
            // reaching that arm. Required by tilt.rb:143's
            // `respond_to?(:deprecate_constant, true)` feature
            // detection inside `class Tilt` body where self is a
            // Class.
            if &*name == "respond_to?" {
                // Arity: CRuby raises ArgumentError on 0 args or 3+,
                // before reaching method_missing / NoMethodError. The
                // no-recv path runs FIRST so this guard is what users
                // see for bare `respond_to?` calls inside a method
                // body or class body.
                if args.is_empty() || args.len() > 2 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                    }));
                }
                // Reopened-primitive user override: `class String;
                // def respond_to?; ...; end; end` installs a method
                // on the primitive's preamble class. Value::Object
                // self routes through `lookup_method_cached` at
                // line ~320; Value::Class through
                // `lookup_class_singleton_method` at ~343. Primitives
                // (Str / Int / Sym / Array / Hash / ...) had no
                // equivalent user-method lookup before the stub
                // fired, so a user override on the primitive was
                // silently shadowed. Resolve the primitive's class
                // via `class_of` and check its method table; if a
                // user `respond_to?` exists, invoke it instead of
                // the stub.
                //
                // Documented narrower gap: this only fixes
                // `respond_to?` specifically. Other bare calls in
                // reopened-primitive method bodies (e.g.
                // `class String; def trigger; custom_helper; end;
                // end`) still surface NoMethodError because the
                // no-recv path doesn't generally consult the
                // primitive's class. Tracked as a separate broader
                // gap in SUBSET.md.
                if !matches!(&self_val, Value::Object(_) | Value::Class(_))
                    && let Value::Class(cls) = self.class_of(&self_val)
                    && let Some(m) = self.lookup_method_uncached(&cls, name_id)
                {
                    self.invoke_method(m, self_val.clone(), args.into_vec())?;
                    return Ok(());
                }
                // Type: CRuby raises `TypeError: X is not a symbol nor
                // a string` when arg[0] isn't a Symbol or String.
                // Without this guard the call would silently fall
                // through to method_missing / NoMethodError, which
                // misreports the failure as "method missing" instead
                // of "wrong arg type" and confuses debugging.
                let lookup_name: SymId = match &args[0] {
                    Value::Sym(id) => *id,
                    Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} is not a symbol nor a string",
                            other.to_inspect(&self.heap, &self.interner),
                        ),
                    })),
                };
                let include_private = matches!(args.get(1), Some(Value::Bool(true)));
                if self.responds_to(&self_val, lookup_name, include_private) {
                    self.stack.push(Value::Bool(true));
                    return Ok(());
                }
                if self.try_respond_to_missing(&self_val, lookup_name, include_private)? {
                    return Ok(());
                }
                self.stack.push(Value::Bool(false));
                return Ok(());
            }
            // method_missing fallback (PoC #2). For Object self, look
            // up the class chain — if found, hand it the missed name
            // as a Symbol arg. Primitives skip this and raise directly.
            if self.try_method_missing(&self_val, name_id, args.into_vec(), None)? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                kind: crate::error::NoMethodErrorKind::Missing,
                method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
            }));
        }

        let recv = recv.expect("ICE: receiver missing");

        // `cls.class_eval(source_string [, file, line])` — runtime
        // parse + compile + run of a Ruby source string. Tier 1
        // divergence (documented in docs/SUBSET.md): does NOT
        // switch to the receiver class's class-body context, so
        // `Foo.class_eval("def bar; end")` lands `bar` at top
        // level. Tilt's tilt-2.7.0 `eval_compiled_method` self-
        // wraps its source in a nested block-form
        // `Tilt::TOPOBJECT.class_eval do def ... end end`, so
        // the inner block-form (intercepted in `do_call_block`)
        // does the actual class context switching.
        // No-arg, no-block `C.class_eval` / `C.module_eval` would
        // otherwise fall through to NoMethodError, but
        // respond_to?(:class_eval) reports true. CRuby raises
        // ArgumentError "wrong number of arguments (given 0,
        // expected 1..3)" for the no-arg string-form call;
        // (block-only form is handled in do_call_block).
        if (&*name == "class_eval" || &*name == "module_eval")
            && let Value::Class(cls) = &recv
            && args.is_empty()
            && self.lookup_class_singleton_method(cls, name_id).is_none()
        {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "wrong number of arguments (given 0, expected 1..3)".into(),
            }));
        }
        if (&*name == "class_eval" || &*name == "module_eval")
            && let Value::Class(cls) = &recv
            && !args.is_empty()
            // Defer to user-defined `def self.class_eval(s)` /
            // `def self.module_eval(s)` if present — same
            // ordering as the singleton-method lookup at
            // dispatch.rs:1597. Without this check, a class
            // overriding its own `class_eval` would have the
            // override silently bypassed.
            && self.lookup_class_singleton_method(cls, name_id).is_none()
        {
            // Arity guard FIRST so too-many-arg calls surface as
            // ArgumentError, matching CRuby's check order (arity
            // → type). Without this, `C.class_eval(123, "f", 1,
            // :extra)` would report a misleading TypeError on
            // args[0] even though the call is out of the 1..3
            // signature.
            if args.len() > 3 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1..3)",
                        args.len()
                    ),
                }));
            }
            // Validate args[0] (source) type after arity. Non-
            // String falls through here (no user override + no
            // block path matched) and should surface as TypeError,
            // NOT NoMethodError. `respond_to?(:class_eval)`
            // returns true, so the dispatch reaching this point
            // means the method exists — bad arg type is a
            // TypeError.
            if !matches!(args[0], Value::Str(_)) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        args[0].type_name()
                    ),
                }));
            }
            // Validate args[1] (filename) type when present:
            // CRuby raises TypeError for non-String. Falling back
            // to the default label would silently ignore the
            // caller's mistake.
            if let Some(a1) = args.get(1)
                && !matches!(a1, Value::Str(_)) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into String",
                        a1.type_name()
                    ),
                }));
            }
            // Validate args[2] (line) when present: CRuby raises
            // TypeError for non-Integer-coercible values. Accept
            // Int and Float (Float has `to_int`); reject other
            // types even though we ultimately ignore the line
            // offset — silent acceptance would mask caller bugs.
            if let Some(a2) = args.get(2)
                && !matches!(a2, Value::Int(_) | Value::Float(_)) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        a2.type_name()
                    ),
                }));
            }
            let src = if let Value::Str(s) = &args[0] { s.to_string_lossy() } else { unreachable!() };
            // Track whether the filename is our synthetic default
            // or caller-supplied. Only the synthetic case opts
            // into the source-table collision-suffix dedupe; an
            // explicit user filename should stay verbatim across
            // repeated calls so `__FILE__` is stable.
            let (filename, synthetic) = match args.get(1) {
                Some(Value::Str(f)) => (f.to_string_lossy(), false),
                _ => ("(class_eval)".to_string(), true),
            };
            let v = self.eval_string(&src, &filename, synthetic)?;
            if self.suppress_call_result_push {
                self.suppress_call_result_push = false;
            } else {
                self.stack.push(v);
            }
            return Ok(());
        }

        // `Object#send(:name, args...)` / `__send__(:name, args...)`
        // — dynamic dispatch. Resolve the first arg as the target
        // method name and re-enter `do_call` with `recv` pushed
        // back, the remaining args on the stack, and the resolved
        // SymId in name_id. The whole normal lookup path then
        // handles the rest (primitives, singleton methods, host
        // fns, method_missing, etc.) — `send` is just a name
        // re-aim, not a separate dispatch table.
        //
        // The method-name arg accepts both Symbol and String
        // (CRuby's transparent `to_sym`). Same precedent as
        // `Object#method` but broader because shared specs and
        // tilt-style libraries commonly pass `send("foo")`.
        // Block-form (`send(:name) { ... }`) lives in
        // `do_call_block`; this arm covers the block-less call.
        //
        // cache_id passed as `u16::MAX` because the re-entered call
        // resolves a runtime-dynamic name — caching it at the
        // original `send` call site's slot would poison whatever
        // method the bytecode actually compiled for that slot.
        //
        // **CRuby parity — user-defined `def send`**: only
        // `__send__` is reserved. A user `def send` on the
        // receiver's class wins over the built-in re-aim when the
        // call is named `send`. We check that first and fall
        // through to the regular `Value::Object` arm if found.
        //
        // **CRuby parity — visibility bypass**: `send` and
        // `__send__` may invoke private/protected methods. Set
        // `bypass_visibility_once` to suppress the visibility
        // check during the re-entered call. The flag is consumed
        // (single-shot) at the top of the next `do_call` /
        // `do_call_block` into a local — *not* at the visibility
        // check site — so a dispatch that bottoms out before the
        // Object arm (e.g. `send(:nonexistent)` raising
        // NoMethodError on a primitive) can't leak the bypass
        // into the next unrelated call.
        // send/__send__ bypass recogniser — unified helper
        // (#192 commit 2/5). NotHandled returns recv + args
        // back so the dispatcher can continue below.
        let (recv, args) = match self.try_dispatch_send_bypass(&name, name_id, cache_id, args, Some(recv)) {
            SendBypass::Handled(r) => return r,
            SendBypass::NotHandled { args, recv_opt } => (recv_opt.expect("with-recv path"), args),
        };

        // Int#+/-/* operator method-call BigInt-aware intercept.
        // Op::BinOp's hot path inlines `apply_int.unwrap_or →
        // bigint_arith`, but the method-call form (`a.+(b)` /
        // `a.send(:+, b)`) goes through primitive_call which uses
        // plain i64 ops that wrap on overflow. Route Int×Int
        // operator names through `apply_int_promote` here so
        // `a.send(:+, big_literal)` matches Op::BinOp's
        // overflow-promotion behaviour exactly. With bignum off
        // apply_int_promote falls back to wrapping so the
        // pre-PR behaviour is preserved.
        #[cfg(feature = "bignum")]
        if args.len() == 1
            && matches!(&recv, Value::Int(_))
            && matches!(&args[0], Value::Int(_))
            && let Some(kind) = crate::bytecode::BinOpKind::from_op_name(&name)
            && matches!(kind,
                crate::bytecode::BinOpKind::Add
                | crate::bytecode::BinOpKind::Sub
                | crate::bytecode::BinOpKind::Mul
            )
        {
            let (Value::Int(x), Value::Int(y)) = (&recv, &args[0]) else { unreachable!() };
            let v = self.apply_int_promote(kind, *x, *y)?;
            self.stack.push(v);
            return Ok(());
        }

        if self.try_push_int_chr_encoding(&recv, &name, &args)? {
            return Ok(());
        }
        if self.try_string_encoding_ops(&recv, &name, &args)? {
            return Ok(());
        }
        if self.try_push_string_encoding(&recv, &name, &args) {
            return Ok(());
        }
        // A user-defined singleton method overrides the built-in
        // Module/Class `name` / `to_s` / `inspect` primitives (CRuby
        // parity — `def self.name`, or an inherited
        // `class << self; attr_reader :name; end`, must win over the
        // structural class name). Without this the primitive below
        // shadows the override; rouge's Token DSL relies on
        // `Token.name` reading its `@name` ivar, not the class name.
        // Mirrors the `superclass` arm's override probe in
        // try_dispatch_class_introspection.
        let class_intrinsic_overridden = if let Value::Class(c) = &recv {
            matches!(&*name, "name" | "to_s" | "inspect") && {
                let c = c.clone();
                self.lookup_class_singleton_method(&c, name_id).is_some()
            }
        } else {
            false
        };
        if !class_intrinsic_overridden
            && let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes)
            .map_err(|e| self.trap(e))? {
            self.stack.push(v);
            return Ok(());
        }
        if let Some(v) = self.sym_primitive(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // BigInt method dispatch — `primitive_call` and friends
        // are stateless and can't read the heap, so the BigInt
        // surface is hooked here where `&mut self` is available.
        // Covers `to_s` / `inspect` (heap read) AND the operator
        // method-call shape (`big.+(1)`, `big.send(:==, x)`),
        // routed through `try_bigint_binop` so method-call form
        // matches the `Op::BinOp` semantics exactly. Without this
        // route, `big.send(:==, other)` would fall through to
        // `ruby_eq`'s Object-identity arm and miss canonical-value
        // equality.
        #[cfg(feature = "bignum")]
        if let Some(v) = self.bigint_primitive(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }

        // `Hash.new` interception. The preamble defines a stub
        // `class Hash; end` (lib.rs) that has no connection to the
        // primitive `Value::Hash` storage — without this short-
        // circuit, `Hash.new` falls through to the generic
        // `Class.new` allocator below and returns a bare
        // `Value::Object`, which then NoMethodErrors on every
        // collection-style call (`.[]`, `.keys`, `.each`, ...).
        //
        // Three call shapes (CRuby semantics):
        //   - `Hash.new`           → empty Hash, no default
        //   - `Hash.new(default)`  → empty Hash, scalar default
        //     (NOT yet modelled — falls through to no-default; the
        //     scalar arg is silently ignored as a documented gap)
        //   - `Hash.new { |h, k| block }` → empty Hash with default-
        //     block stored alongside; `Hash#[]` invokes it on
        //     missing keys with `(self, key)`.
        //
        // Tilt's `@lazy_map = Hash.new { |h, k| h[k] = [] }` (the
        // motivating case) is the block form. Without default-
        // block support the whole tilt-load chain stalls on the
        // first `@lazy_map[ext]` access.
        // Class-receiver intrinsics — Hash[] / new / allocate /
        // include / prepend / extend / private / public / protected
        // / name / superclass / method_defined?. Extracted into
        // try_dispatch_class_intrinsics (#192 commit 4/5).
        let (args, recv) = match self.try_dispatch_class_intrinsics(&name, name_id, cache_id, args, recv)? {
            ClassOutcome::Handled => return Ok(()),
            ClassOutcome::NotHandled { args, recv } => (args, recv),
        };

        // Primitive-receiver fallback to the user-Class method
        // table. CRuby's dispatch walks every value's class chain
        // uniformly; rubyrs's primitive arms above handle the
        // built-in methods, but `class Symbol; alias_method
        // :to_msgpack_ext, :name; end` installs a forwarder in
        // `self.classes[Symbol].methods` that's only reachable
        // through the user-Class table. Look up the primitive's
        // class name via `class_of` and try `lookup_method_cached`
        // on it. Skip Object (its own arm below handles that) and
        // Class (Class.new etc. handled by the earlier arm).
        //
        // No synth-bypass flag is needed: the Kernel reflection
        // builtins live in a separate `Vm.kernel_builtin_metas`
        // registry, NOT on `Kernel.methods`, so chain-walking
        // here doesn't re-find them. See `install_kernel_builtins`
        // (vm/lookup.rs) for the rationale.
        if !force_primitive
            && !matches!(&recv, Value::Object(_) | Value::Class(_))
            && let Value::Class(cls) = self.class_of(&recv)
            && let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
            self.invoke_method(m, recv.clone(), args.into_vec())?;
            return Ok(());
        }
        // `try_class_of`: a class-less Object slot (HeapObj::Fiber)
        // skips the user-method lookup and falls through to the
        // universal primitive arms below (nil? / == / to_s) instead
        // of ICE-ing in class_of.
        if let Value::Object(id) = &recv
            && let Some(cls) = self.heap.try_class_of(*id)
        {
            if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                self.check_method_visibility(&m, &recv, &name, bypass_visibility)?;
                self.invoke_method(m, recv.clone(), args.into_vec())?;
                return Ok(());
            }
            // L3-C: cext-registered instance method
            // (`rb_define_method`). Looked up AFTER script-defined
            // methods so a Ruby-side override wins for
            // concrete-class methods.
            //
            // **Known limitation** (review #1 on PR #27): the
            // current shape walks the script-method ancestor chain
            // via lookup_method_cached, THEN checks cext methods
            // only on the receiver's own class. So a Ruby method
            // on a superclass shadows a cext method on the
            // subclass, and a cext method on a superclass is
            // invisible to subclass instances. A complete fix
            // would interleave cext lookup INSIDE the per-class
            // walk in lookup_method_cached — out of L3-C wedge
            // scope. Real-world impact is small: the common pattern
            // is `class Foo; end` + `rb_define_method(Foo, ...)`
            // on the same class, which works correctly.
            #[cfg(all(feature = "cext", not(target_os = "wasi")))]
            {
                if let Some(table) = self.cext_instance_methods.get(cls.name.as_str())
                    && let Some(reg) = table.get(&name_id).cloned() {
                        // Pin recv + args across the cext call
                        // (review #4 on PR #27). cext_dispatch may
                        // run maybe_gc during arg translation /
                        // TypedData wrapping / result translation;
                        // recv was popped from vm.stack before we
                        // got here, so without pinning a STRESS_GC
                        // sweep can reclaim it mid-call →
                        // use-after-free in the cext body. Same
                        // shape as the L1.5 P0-A pattern.
                        //
                        // RAII guard holding only a `*mut Vec<Value>`
                        // (not `&mut Vm`) so it doesn't conflict with
                        // the `vm_ptr: *mut Vm` we hand to
                        // `with_vm_ptr_set` — PinGuard's `&mut Vm`
                        // would alias under Stacked Borrows when
                        // cext_dispatch's rb_funcall reentrance
                        // re-derefs the raw pointer (same gotcha L3-A
                        // review #15 / PR #6 hit). The narrower
                        // pointer is sound because it borrows only
                        // the field, not the whole Vm.
                        //
                        // Truncate runs on Drop, so a panic from
                        // `with_vm_ptr_set` / `cext_dispatch` (or
                        // the trailing `?`) doesn't leak pinned
                        // entries — fixes review #11 on PR #27,
                        // where the prior manual push/truncate
                        // skipped truncate on the unwind path.
                        struct PinTruncateGuard {
                            pinned: *mut Vec<Value>,
                            saved_depth: usize,
                        }
                        impl Drop for PinTruncateGuard {
                            fn drop(&mut self) {
                                // SAFETY: `pinned` was taken from
                                // `&mut self.pinned` in the
                                // enclosing scope; the guard is
                                // dropped before that borrow could
                                // be used elsewhere, and no other
                                // Rust code mutates `pinned` while
                                // the cext call is on the stack.
                                unsafe { (*self.pinned).truncate(self.saved_depth); }
                            }
                        }
                        let saved_pin_depth = self.pinned.len();
                        self.pinned.push(recv.clone());
                        for a in &args { self.pinned.push(a.clone()); }
                        let _pin_guard = PinTruncateGuard {
                            pinned: &raw mut self.pinned,
                            saved_depth: saved_pin_depth,
                        };
                        let vm_ptr: *mut Vm = self;
                        let recv_clone = recv.clone();
                        let v = with_vm_ptr_set(vm_ptr, || {
                            crate::vm::cext::cext_dispatch(
                                &reg.qualified_name,
                                reg.func,
                                reg.arity,
                                &args,
                                crate::vm::cext::CextSelfHandle::Object(recv_clone),
                            )
                        })?;
                        // Explicit drop here is documentation, not
                        // necessity — `_pin_guard` drops at scope
                        // end either way.
                        drop(_pin_guard);
                        self.stack.push(v);
                        return Ok(());
                    }
            }
        }
        // C-ext singleton dispatch: `BCrypt::Engine.__bc_crypt(args)`
        // arrives here with recv = Value::Class(c). Look up the
        // method in the per-class cext table populated by
        // `Vm::cext_require` (rb_define_singleton_method).
        // File class-method shims. CRuby exposes File.read / .write
        // / .exist? / .open / .basename as class methods; we don't
        // have a `def self.foo` syntax yet, so the dispatch is a
        // hand-rolled intercept on the File class. I/O paths
        // surface OS errors as a generic RuntimeError so scripts
        // can `rescue` them.
        if let Value::Class(cls) = &recv {
            // User-Ruby `def self.foo` singletons: walk the
            // per-class `singleton_methods` table chain via the
            // shared helper. CRuby's metaclass model has the
            // singleton class of `Dog < Animal` inherit from the
            // singleton class of `Animal`, so `Dog.kingdom` finds
            // `Animal`'s `def self.kingdom`. The same helper is
            // used by the bare-call path (no_recv when self is a
            // Class) so `self.bar` and bare `bar` stay in sync.
            let user_singleton = self.lookup_class_singleton_method(cls, name_id);
            if let Some(m) = user_singleton {
                // `private_class_method` visibility — same check (and
                // same literal-`self` / `send` exemptions) the
                // instance explicit-recv path applies. PRIVATE only:
                // protected class methods (`class << self; protected`
                // — rouge's `Lexer.register` cross-subclass pattern)
                // stay permissive, because honouring them needs the
                // metaclass `is_a?` walk rubyrs doesn't model. The
                // class-singleton fast path rejects non-Public and
                // falls through to here, so the error shape stays in
                // one place.
                if m.visibility.get() == Visibility::Private {
                    self.check_method_visibility(&m, &recv, &name, bypass_visibility)?;
                }
                let target_self = recv.clone();
                return self.invoke_method(m, target_self, args.into_vec());
            }
            // `Kernel.foo` / `Kernel::foo` — explicit-receiver
            // dispatch of a Kernel module-function. CRuby's Kernel
            // methods (`load`, `require`, `puts`, `p`, `format`,
            // `Integer`, `rand`, `exit`, `raise`, `eval`, ...) are
            // `module_function`s: callable bare (private instance
            // method, implicit self) AND as a public method on the
            // Kernel module object itself. rubyrs implements the
            // bare form via `builtin_call`, but the explicit-recv
            // path lands here with `recv = Value::Class(Kernel)` and,
            // finding no singleton method, would raise NoMethodError
            // ("undefined method 'load' for Class"). Route Kernel-
            // module receivers through `builtin_call` so the two call
            // shapes share one implementation. A user `def self.foo`
            // on Kernel still wins (checked above). The `is_module`
            // gate keeps this from intercepting a same-named method
            // on an unrelated class; matching `cls.name == "Kernel"`
            // (same convention as the File / Dir arms below) targets
            // the Kernel module specifically.
            if cls.is_module
                && cls.name.as_str() == "Kernel"
                && Self::is_kernel_module_function(&name)
                && let Some(res) = self.builtin_call(&name, &args)
            {
                self.stack.push(res?);
                return Ok(());
            }
            if cls.name.as_str() == "File"
                && let Some(v) = self.file_class_dispatch(&name, &args)? {
                    self.stack.push(v);
                    return Ok(());
                }
            if cls.name.as_str() == "Dir"
                && let Some(v) = self.dir_class_dispatch(&name, &args)? {
                    self.stack.push(v);
                    return Ok(());
                }
            if cls.name.as_str() == "FileUtils"
                && let Some(v) = self.fileutils_class_dispatch(&name, &args)? {
                    self.stack.push(v);
                    return Ok(());
                }
            // `RubyrsSass.compile(scss)` — host primitive backing the
            // jekyll-sass-converter shim. Compiles SCSS/Sass to CSS via
            // the active `SassBackend` (grass under `--features sass`);
            // a backend error or the feature-absent case raises so the
            // caller's `rescue` surfaces it.
            if cls.name.as_str() == "RubyrsSass"
                && &*name == "compile"
                && let [Value::Str(src)] = &args[..] {
                let scss = src.to_string_lossy();
                match crate::sass::compile(&scss) {
                    Ok(css) => {
                        self.stack.push(Value::new_str(css));
                        return Ok(());
                    }
                    Err(msg) => {
                        return Err(self.trap(RubyError::RuntimeError { msg }));
                    }
                }
            }
            // `RubyrsDigest.hexdigest(algo, data)` / `.digest(algo, data)`
            // — host primitive backing the `Digest::SHA2 / SHA1 / MD5`
            // veneer (`stdlib_vendor/digest.rb`). `algo` is the lowercase
            // tag ("sha256"/"sha1"/"md5"); `data` is hashed by its raw
            // bytes (binary-safe). `hexdigest` returns the lowercase hex
            // String; `digest` returns the raw bytes as a binary String.
            if cls.name.as_str() == "RubyrsDigest"
                && matches!(&*name, "hexdigest" | "digest")
                && let [Value::Str(algo), Value::Str(data)] = &args[..] {
                let algo_s = algo.to_string_lossy();
                let bytes = data.borrow().clone();
                match crate::digest::raw(&algo_s, &bytes) {
                    Some(raw) => {
                        let v = if &*name == "hexdigest" {
                            Value::new_str(crate::digest::to_hex(&raw))
                        } else {
                            Value::new_str_bytes(raw)
                        };
                        self.stack.push(v);
                        return Ok(());
                    }
                    None => {
                        return Err(self.trap(RubyError::RuntimeError {
                            msg: format!("rubyrs: unsupported digest algorithm {algo_s:?}"),
                        }));
                    }
                }
            }
            // `Module.nesting` — CRuby reflection returning the
            // lexical scope chain at the call site, innermost-first.
            // Resolves through the current frame's proto's
            // `lexical_scope` (built at compile time from
            // `b.class_path`). Each SymId is looked up in
            // `self.classes`; missing entries are skipped (a top-
            // level `module` whose body hasn't run yet at the call
            // site can't appear here in practice — class_path is
            // set ONLY when we're already inside the body, so the
            // class table already has the entry by the time
            // `Module.nesting` runs).
            if cls.name.as_str() == "Module" && &*name == "nesting" && args.is_empty() {
                let frame = self.frames.last().expect("ICE: Module.nesting no frame");
                let lex = self.protos[frame.proto_idx].lexical_scope.clone();
                let mut items: Vec<Value> = Vec::with_capacity(lex.len());
                for sym in lex {
                    if let Some(c) = self.classes.get(&sym).cloned() {
                        items.push(Value::Class(c));
                    }
                }
                self.maybe_gc();
                self.check_alloc()?;
                let id = self.heap.alloc(HeapObj::Array(items.into()));
                self.stack.push(Value::Array(id));
                return Ok(());
            }
            #[cfg(feature = "cext")]
            if let Some(table) = self.cext_class_methods.get(cls.name.as_str())
                && let Some(host) = table.get(&name_id).cloned() {
                    // Stash Vm pointer for the singleton-method's
                    // C body — same rationale as the top-level
                    // host_fns dispatch above.
                    #[cfg(not(target_os = "wasi"))]
                    let v = {
                        let vm_ptr: *mut Vm = self;
                        with_vm_ptr_set(vm_ptr, || host(&args))?
                    };
                    #[cfg(target_os = "wasi")]
                    let v = host(&args)?;
                    self.stack.push(v);
                    return Ok(());
                }
        }
        // `include Mod` — without real Modules in the subset, we
        // approximate by copying the source class's method table
        // into the target class. Only fills methods the target
        // doesn't already define, so user overrides win (matching
        // CRuby's ancestor-chain semantics where own methods
        // shadow included ones). Defines `include` ad-hoc on
        // Class receivers; the call is a no-op for any other
        // receiver and falls through to NoMethodError.
        // `proc.call(args)` / `lambda.call(args)` — invoke the
        // block synchronously and push its result. Sub-frame
        // runs until it returns; same dispatch shape as iterator
        // drivers' invoke_block + dispatch_until pattern, but
        // accessible from script code (rather than only from
        // builtin iterators).
        // Callable intrinsics — Block.call / method capture /
        // BoundMethod / UnboundMethod / CurriedProc family.
        // Extracted into try_dispatch_callable_intrinsics
        // (#192 commit 3/5). NotHandled returns args + recv back.
        let (args, recv) = match self.try_dispatch_callable_intrinsics(&name, name_id, args, recv)? {
            CallableOutcome::Handled => return Ok(()),
            CallableOutcome::NotHandled { args, recv } => (args, recv),
        };
        // Explicit-receiver no-op stubs — `Foo.private_constant :X`,
        // `Foo.public_constant :X`, `Foo.deprecate_constant :X`,
        // `Foo.autoload :X, "path"`. Explicit-receiver parallel of
        // the no_recv arm in `do_call`; both register into
        // `autoloads_scoped` (Phase 2 of issue #224) so the first
        // `Foo::X` reference triggers a `require`. Tilt's
        // `Tilt.autoload class_name, file` inside `register_lazy`
        // and Rack's 40+ `autoload :Response, 'rack/response'` are
        // the canonical callers.
        if &*name == "autoload"
            && let Value::Class(owner) = &recv {
            #[cfg(target_os = "wasi")]
            let _ = owner;
            if args.len() != 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
                }));
            }
            #[cfg(not(target_os = "wasi"))]
            {
                let const_name = match &args[0] {
                    Value::Sym(id) => self.interner.resolve(*id).to_string(),
                    Value::Str(s) => s.to_string_lossy(),
                    other => {
                        // CRuby reports the inspected value, not the
                        // type name (`123 is not a symbol nor a string`).
                        let inspected = other.to_inspect(&self.heap, &self.interner);
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!("{} is not a symbol nor a string", inspected),
                        }));
                    }
                };
                let path = match &args[1] {
                    Value::Str(s) => s.to_string_lossy(),
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "no implicit conversion of {} into String",
                                other.type_name()
                            ),
                        }));
                    }
                };
                let key = if owner.name.is_empty() || owner.name == "Object" {
                    const_name
                } else {
                    format!("{}::{}", owner.name, const_name)
                };
                let key_id = self.interner.intern(&key);
                self.autoloads_scoped.insert(key_id, path);
            }
            self.stack.push(Value::Nil);
            return Ok(());
        }
        // `Foo.autoload?(:Bar)` — explicit-receiver parallel of the
        // no_recv arm above. Returns the pending path String while
        // the scoped autoload is registered, nil once it has fired
        // (or was never set).
        if &*name == "autoload?"
            && let Value::Class(owner) = &recv {
            #[cfg(target_os = "wasi")]
            let _ = owner;
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            #[cfg(not(target_os = "wasi"))]
            {
                let const_name = match &args[0] {
                    Value::Sym(id) => Some(self.interner.resolve(*id).to_string()),
                    Value::Str(s) => Some(s.to_string_lossy()),
                    _ => None,
                };
                if let Some(cn) = const_name {
                    let key = if owner.name.is_empty() || owner.name == "Object" {
                        cn
                    } else {
                        format!("{}::{}", owner.name, cn)
                    };
                    if self.interner.contains(&key) {
                        let key_id = self.interner.intern(&key);
                        if let Some(path) = self.autoloads_scoped.get(&key_id) {
                            let v = Value::new_str(path.clone());
                            self.stack.push(v);
                            return Ok(());
                        }
                    }
                }
            }
            self.stack.push(Value::Nil);
            return Ok(());
        }
        // `Foo.const_defined?(:Bar)` — explicit-receiver parallel.
        // tilt's actual call site is
        // `scope.const_defined?(n)` where scope is reached via the
        // `inject(Object)` walk in `constant_defined?`.
        if &*name == "const_defined?"
            && let Value::Class(cls) = &recv {
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            // String args walked via `::`; Symbol args treated as
            // bare names — see `resolve_const_path` doc.
            // (Copilot review #277 round 4 #3.)
            let (const_name, split) = match &args[0] {
                Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                Value::Str(s) => (s.to_string_lossy(), true),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                })),
            };
            let cls_clone = cls.clone();
            let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
            match outcome {
                ConstPathOutcome::Found(_) => self.stack.push(Value::Bool(true)),
                ConstPathOutcome::Missing { .. } => self.stack.push(Value::Bool(false)),
                ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                    msg: format!("wrong constant name {}", name),
                })),
                ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                    msg: format!("{} does not refer to class/module", full_path),
                })),
                // A scoped-autoload `require` trapped — re-raise.
                #[cfg(not(target_os = "wasi"))]
                ConstPathOutcome::Trap(t) => return Err(t),
            }
            return Ok(());
        }
        if &*name == "const_get"
            && let Value::Class(cls) = &recv {
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            let (const_name, split) = match &args[0] {
                Value::Sym(s) => (self.interner.resolve(*s).to_string(), false),
                Value::Str(s) => (s.to_string_lossy(), true),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                })),
            };
            let cls_clone = cls.clone();
            let outcome = self.resolve_const_path(&cls_clone, &const_name, split);
            match outcome {
                ConstPathOutcome::Found(v) => { self.stack.push(v); return Ok(()); }
                ConstPathOutcome::Missing { missing_qualified } => return Err(self.trap(RubyError::NameError {
                    msg: format!("uninitialized constant {}", missing_qualified),
                })),
                ConstPathOutcome::WrongName { name } => return Err(self.trap(RubyError::NameError {
                    msg: format!("wrong constant name {}", name),
                })),
                ConstPathOutcome::NotClass { full_path } => return Err(self.trap(RubyError::TypeError {
                    msg: format!("{} does not refer to class/module", full_path),
                })),
                // A scoped-autoload `require` trapped — re-raise.
                #[cfg(not(target_os = "wasi"))]
                ConstPathOutcome::Trap(t) => return Err(t),
            }
        }
        if matches!(&*name, "private_constant" | "public_constant" | "deprecate_constant")
            && let Value::Class(_) = &recv {
            self.stack.push(recv);
            return Ok(());
        }
        // `Foo.private_class_method :m` / `Foo.public_class_method :m`
        // — explicit-receiver parallel of the no_recv arm in
        // `do_call`. Flips the named singleton methods' visibility
        // (enforced by `check_method_visibility` at the singleton
        // dispatch arms). forwardable-extended's
        // `klass.private_class_method(method)` and rubygems both
        // hit this during their require.
        if matches!(&*name, "private_class_method" | "public_class_method")
            && let Value::Class(target) = &recv {
            let vis = if &*name == "private_class_method" {
                Visibility::Private
            } else {
                Visibility::Public
            };
            let target = target.clone();
            self.apply_class_method_visibility(&target, &args, vis)?;
            self.stack.push(recv);
            return Ok(());
        }
        // `Foo.attr_accessor(:x)` / `Foo.singleton_class.send(
        // :attr_accessor, :x)` — the explicit-receiver runtime form
        // of the attr_* family (the bareword class-body form is
        // compile-time, compiler.rs). Installs ivar accessors on the
        // receiver class's instance-method table; for a singleton
        // class that means class-level accessors on the original.
        // CRuby 3.0+ returns the created method names. Liquid does
        // `singleton_class.send(:attr_accessor, :cache_classes)`.
        if let Some((do_reader, do_writer)) = crate::ast::attr_reader_writer_flags(&name)
            && let Value::Class(cls) = &recv
        {
            // All args must be Symbol/String method names. A non-name
            // arg → TypeError (CRuby parity), matching the strictness
            // of the compile-time path's SymbolLit-only guard.
            let mut names: Vec<String> = Vec::with_capacity(args.len());
            for a in &args {
                match a {
                    Value::Sym(sid) => names.push(self.interner.resolve(*sid).to_string()),
                    Value::Str(s) => names.push(s.to_string_lossy()),
                    other => {
                        let inspected = other.to_inspect(&self.heap, &self.interner);
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!("{} is not a symbol nor a string", inspected),
                        }));
                    }
                }
            }
            let cls = cls.clone();
            let mut created: Vec<Value> = Vec::new();
            for n in &names {
                for sid in self.install_attr_accessor(&cls, n, do_reader, do_writer) {
                    created.push(Value::Sym(sid));
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(created.into()));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `cls.const_set(name, value)` — install a constant on the
        // class. CRuby returns the assigned value. The qualified
        // key (`Foo::Bar::Baz`) mirrors the path the existing
        // `Op::StoreConst` emits for `class Foo; class Bar; BAZ
        // = ...; end; end`, so subsequent `Foo::Bar::BAZ` reads
        // resolve through the same `self.constants` lookup path.
        //
        // Hit by Mustermann's inherited-hook pattern at
        // `mustermann/ast/translator.rb:62`:
        //   subclass.const_set(:NodeTranslator, node_translator)
        // — sets a constant on the subclass at class-build time.
        if &*name == "const_set" && args.len() == 2
            && let Value::Class(cls) = &recv {
            let const_name = match &args[0] {
                Value::Sym(s) => self.interner.resolve(*s).to_string(),
                Value::Str(s) => s.to_string_lossy(),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into Symbol", other.type_name()),
                })),
            };
            let value = args[1].clone();
            // CRuby raises NameError on lowercase-leading names; the
            // simple `Class.const_set(:foo, ...)` form is what gem
            // code actually hits, but mirror the check so we don't
            // silently install a name that `const_get` can't read.
            if !const_name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                return Err(self.trap(RubyError::NameError {
                    msg: format!("wrong constant name {}", const_name),
                }));
            }
            let const_id = self.interner.intern(&const_name);
            // Effective owner name: structural `name`, or the
            // `assigned_name` an anon owner picked up on its own
            // first const-assignment. An owner that is STILL
            // anonymous in both senses keeps its constants in the
            // per-class `consts` table (the qualified-name scheme
            // would collapse `("" + "BAZ")` into a toplevel-aliasing
            // key); a named/assigned owner mirrors into the global
            // qualified maps so external `Owner::BAZ` reads resolve.
            match cls.effective_name() {
                None => {
                    // Anonymous receiver — route through the
                    // per-class `consts` table. resolve_const_path /
                    // const_via_ancestors check this table when the
                    // start scope is anon. If THIS anon owner is
                    // later const-assigned, `name_anon_class` will
                    // promote these entries into the global maps.
                    cls.consts.borrow_mut().insert(const_id, value.clone());
                }
                Some(owner_name) => {
                    let qualified = format!("{}::{}", owner_name, const_name);
                    let key = self.interner.intern(&qualified);
                    // If the assigned value IS an anonymous Class
                    // (e.g. minted by `Class.new(...)`), name it
                    // `Owner::Const` and recursively promote its own
                    // nested `const_set` tree into the global maps —
                    // so `Owner::Const.new` AND a deep
                    // `Owner::Const::Leaf` reference both resolve,
                    // mirroring how `class Owner; class Const; end;
                    // end` installs the whole nested namespace.
                    // (rouge's token tree is built exactly this way.)
                    if let Value::Class(installed) = &value {
                        self.name_anon_class(installed, &qualified);
                        self.classes.insert(key, installed.clone());
                    }
                    self.constants.insert(key, value.clone());
                }
            }
            // const_set (anon-table OR global-map write) invalidates
            // the constant ICs.
            self.bump_const_gen();
            self.stack.push(value);
            return Ok(());
        }
        // `obj.extend(Mod, ...)` for plain Value::Object — install
        // each Module into the receiver's eigenclass so M's
        // instance methods become callable on `obj` directly.
        // Materializes the singleton class via `ensure_singleton_class`
        // (idempotent — subsequent calls reuse the same Rc) and
        // pushes Mod onto its `includes` table, matching how the
        // Class-receiver `extend` arm below (and the class-body
        // arm above) treat their targets. Module-lookup precedence
        // comes from `class_of(obj_id)` returning the eigenclass:
        // the existing `methods_for_obj` walk hits eigenclass.includes
        // before the real class. CRuby last-extended-wins is
        // honoured by inserting at the head of the chain.
        // Zero-arg `obj.extend` raises ArgumentError in CRuby
        // ("wrong number of arguments (given 0, expected 1+)"),
        // not NoMethodError. Surface that explicitly before the
        // arity-checked main arm below so a missing arg can't
        // fall through to the dispatch-not-found path.
        if let Value::Object(_) = &recv
            && &*name == "extend" && args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "wrong number of arguments (given 0, expected 1+)".to_string(),
                }));
            }
        if let Value::Object(id) = &recv
            && &*name == "extend" && !args.is_empty() {
                // CRuby walks `obj.extend(M1, M2)` args RIGHT-to-LEFT
                // — M2 inserts into the eigenclass first, M1 last and
                // ends up at the head; hook fire order mirrors the
                // insertion order (M2.extended then M1.extended).
                // Single-arg cases are unaffected.
                let mut modules: Vec<std::rc::Rc<crate::value::Class>> = Vec::with_capacity(args.len());
                for a in args.iter().rev() {
                    match a {
                        Value::Class(c) if c.is_module => modules.push(c.clone()),
                        Value::Class(_) => return Err(self.trap(RubyError::TypeError {
                            msg: "wrong argument type Class (expected Module)".to_string(),
                        })),
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Module)",
                                a.type_name(),
                            ),
                        })),
                    }
                }
                let sc = self.heap.ensure_singleton_class(*id);
                let mut fire_hooks: Vec<std::rc::Rc<crate::value::Class>> = Vec::new();
                for src in modules {
                    if !super::class_is_a(&sc, &src) {
                        sc.includes.borrow_mut().insert(0, src.clone());
                        self.bump_const_gen();
                    }
                    // `Module.extended(obj)` fires on every extend
                    // call (CRuby parity — same shape as included /
                    // prepended). Hook arg is the receiver Object,
                    // not a Class, since `obj.extend(M)` extends an
                    // instance.
                    fire_hooks.push(src);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                let target_v = recv.clone();
                self.fire_inclusion_hooks(&fire_hooks, &target_v, "extended")?;
                self.stack.push(recv.clone());
                return Ok(());
            }
        // `Klass.alias_method(:new_name, :old_name)` — runtime
        // dispatch path (compile-time intercept at compiler.rs:225
        // only catches the literal-Symbol shape inside a class
        // body). Surfaced by rack-protection's
        // `def self.default_reaction(reaction); alias_method(:default_reaction,
        // reaction); end`, where the second arg is a parameter
        // (not a literal Symbol). The lookup walks the receiver
        // class's ancestor chain via `lookup_method_uncached`,
        // installs the same Rc<Method> under the new name on the
        // receiver class itself, and bumps `method_gen` so cached
        // call sites re-resolve. CRuby's `alias_method` returns
        // the receiver class; mirror that.
        if let Value::Class(target) = &recv
            && &*name == "alias_method" && args.len() == 2 {
            let new_id_opt = match &args[0] {
                Value::Sym(id) => Some(*id),
                Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                _ => None,
            };
            let old_id_opt = match &args[1] {
                Value::Sym(id) => Some(*id),
                Value::Str(s) => Some(self.interner.intern(&s.to_string_lossy())),
                _ => None,
            };
            if let (Some(new_id), Some(old_id)) = (new_id_opt, old_id_opt) {
                let m = self.lookup_method_uncached(target, old_id);
                match m {
                    Some(method) => {
                        target.methods.borrow_mut().insert(new_id, method);
                        self.method_gen = self.method_gen.wrapping_add(1);
                        self.stack.push(Value::Sym(new_id));
                        return Ok(());
                    }
                    None => {
                        let old_name = self.interner.resolve(old_id).to_string();
                        return Err(self.trap(RubyError::NameError {
                            msg: format!(
                                "undefined method '{}' for class '{}'",
                                old_name, target.name,
                            ),
                        }));
                    }
                }
            }
        }
        if let Value::Class(target) = &recv
            && matches!(&*name, "include" | "extend" | "prepend") && !args.is_empty() {
                // Explicit-receiver form: `MyClass.include(Mod)` /
                // `.prepend(Mod)`. Same chain-push semantics as the
                // no-receiver form above — see that comment for the
                // rationale and the prepend-vs-include split. The
                // `Module.included` / `Module.prepended` hooks fire
                // here too, mirroring the no-receiver path.
                let is_prepend = &*name == "prepend";
                let is_include = &*name == "include";
                let target_cls = target.clone();
                let mut fire_hooks: Vec<std::rc::Rc<crate::value::Class>> = Vec::new();
                // Same right-to-left iteration as the no-receiver
                // arm — see that comment for rationale. All three
                // keywords (include / prepend / extend) reverse.
                let reverse_args = is_prepend || is_include || (&*name == "extend");
                let n_args = args.len();
                for idx in 0..n_args {
                    let a = if reverse_args { &args[n_args - 1 - idx] } else { &args[idx] };
                    let src = match a {
                        Value::Class(c) => c.clone(),
                        _ => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Module)",
                                a.type_name(),
                            ),
                        })),
                    };
                    // Per-chain transitive idempotency, same as the
                    // no-receiver arm — see that comment for the
                    // include-vs-prepend coexistence rationale and
                    // the extend-keeps-class_is_a gate.
                    //
                    // `Klass.extend(M)` is CRuby-equivalent to
                    // `class << Klass; include M; end`: M's instance
                    // methods become class-level methods, NOT
                    // instance methods of Klass. So extend writes to
                    // `singleton_includes` (a separate chain walked
                    // by `lookup_class_singleton_method`), while
                    // include/prepend still write to the class's own
                    // includes/prepends chain (which lookup uses for
                    // instance method dispatch). Pre-fix, extend was
                    // pushing into `includes` here — instances of
                    // Klass picked up M's methods, but `Klass.foo`
                    // did NOT (the singleton-lookup walk doesn't
                    // consult the instance-method chain). Surfaced
                    // by sinatra-contrib/MultiRoute's `register
                    // MultiRoute` shape, where the gem expects
                    // `Klass.get` to resolve to MultiRoute's override.
                    let is_extend = !is_include && !is_prepend;
                    let already_reachable = if is_extend {
                        target_cls.singleton_includes.borrow().iter().any(|m| Rc::ptr_eq(m, &src))
                    } else {
                        super::class_reaches_via_chain(&target_cls, &src, is_prepend)
                    };
                    if !already_reachable {
                        let mut chain = if is_prepend {
                            target_cls.prepends.borrow_mut()
                        } else if is_extend {
                            target_cls.singleton_includes.borrow_mut()
                        } else {
                            target_cls.includes.borrow_mut()
                        };
                        chain.insert(0, src.clone());
                        drop(chain);
                        // include/prepend changes the cref-ancestor
                        // constant walk — invalidate the const ICs.
                        self.bump_const_gen();
                    }
                    if is_extend {
                        // Force a method-cache generation bump so any
                        // call site that previously NoMethodError'd
                        // on this class re-resolves and finds the
                        // newly-extended module's methods.
                        self.method_gen = self.method_gen.wrapping_add(1);
                    }
                    // Hook fires on every call — see no-recv arm
                    // for rationale. Extend's hook is `extended`.
                    fire_hooks.push(src);
                }
                self.method_gen = self.method_gen.wrapping_add(1);
                let hook_name = if is_prepend {
                    "prepended"
                } else if is_include {
                    "included"
                } else {
                    "extended"
                };
                self.fire_inclusion_hooks(&fire_hooks, &Value::Class(target_cls), hook_name)?;
                self.stack.push(recv.clone());
                return Ok(());
            }
        // Universal class predicates: `is_a?` / `kind_of?` walk
        // the ancestor chain (own class + includes + superclass);
        // `instance_of?` is exact-class only. CRuby exposes both
        // on `Object`, so they apply to every receiver — for
        // primitives (Int / Str / Sym / ...) we resolve their
        // class via `class_of`.
        if matches!(&*name, "is_a?" | "kind_of?" | "instance_of?") && args.len() == 1
            && let Value::Class(target) = &args[0] {
                let recv_class_v = self.class_of(&recv);
                let recv_class = if let Value::Class(c) = recv_class_v { c } else {
                    self.stack.push(Value::Bool(false));
                    return Ok(());
                };
                let result = if &*name == "instance_of?" {
                    Rc::ptr_eq(&recv_class, target)
                } else {
                    super::class_is_a(&recv_class, target)
                };
                self.stack.push(Value::Bool(result));
                return Ok(());
            }
        // Class-receiver introspection cluster (the second
        // Class cluster from #192 commit 4 — deferred to its
        // own helper). Returns true when an arm matched and
        // pushed a result; otherwise falls through to the
        // remaining dispatch.
        if self.try_dispatch_class_introspection(&name, &args, &recv)? {
            return Ok(());
        }
        // Hash-subclass user override: a tagged Hash consults its
        // class's method chain BEFORE the Hash primitives, so
        // `class M < Hash; def [](k); …; end; end` wins over Hash#[]
        // (CRuby override semantics). Plain Hashes (tag None) skip
        // this and go straight to the primitives below. The no-block
        // path only — block-form overrides flow through
        // `do_call_block`'s own collection bridge.
        if !force_primitive
            && let Value::Hash(id) = &recv
            && let Some(tag) = self.heap.hash_class_tag(*id)
            && let Some(m) = self.lookup_method_uncached(&tag, name_id)
        {
            return self.invoke_method(m, recv.clone(), args.into_vec());
        }
        // Array twin of the Hash-subclass override gate above.
        if !force_primitive
            && let Value::Array(id) = &recv
            && let Some(tag) = self.heap.array_class_tag(*id)
            && let Some(m) = self.lookup_method_uncached(&tag, name_id)
        {
            return self.invoke_method(m, recv.clone(), args.into_vec());
        }
        if let Some(v) = self.collection_call(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // Collection receivers → Enumerable module for methods with no
        // native arm (minmax / minmax_by / each_entry / `min(n)` /
        // `max(n)`). AFTER primitive_call + collection_call so a native
        // iterator (sort / map / min / max / sum) always wins — routing
        // those to Enumerable would recurse (`Enumerable#sort` →
        // `to_a.sort`).
        if self.try_enumerable_module_fallback(&recv, name_id, args.to_vec(), None)? {
            return Ok(());
        }
        // `obj.methods` / `#public_methods` / `#private_methods` /
        // `#protected_methods` — receiver-side method introspection.
        // All four walk the same ancestor chain on Value::Object
        // (own class via `class_of` so a singleton class is included
        // if installed, then includes, then superclass) collecting
        // (SymId, Visibility) pairs and keep the first occurrence
        // of each name (matching method-lookup precedence). The
        // dispatch arm then filters by visibility:
        //   methods                   → Public | Protected
        //   public_methods            → Public
        //   private_methods           → Private
        //   protected_methods         → Protected
        // CRuby excludes private from the default `methods` list,
        // so pre-cycle behaviour (returning private too) was a
        // divergence — fixed here. Class receivers walk the
        // class-method chain (singleton_prepends recursing through
        // each module's prepends/includes, plus singleton_methods)
        // up the superclass chain. Other shapes return an empty
        // Array (the subset doesn't expose Kernel-level methods
        // individually). De-dups by SymId, sorted by interner
        // string order for determinism.
        if matches!(&*name, "methods" | "public_methods" | "private_methods" | "protected_methods")
            && args.is_empty()
        {
            use crate::value::Visibility;
            let pred: fn(Visibility) -> bool = match &*name {
                "methods" => |v| matches!(v, Visibility::Public | Visibility::Protected),
                "public_methods" => |v| v == Visibility::Public,
                "private_methods" => |v| v == Visibility::Private,
                "protected_methods" => |v| v == Visibility::Protected,
                _ => unreachable!(),
            };
            let mut names: Vec<crate::intern::SymId> = Vec::new();
            if let Value::Object(id) = &recv {
                let cls = self.heap.class_of(*id);
                let mut visited: Vec<*const crate::value::Class> = Vec::new();
                let mut pairs: Vec<(crate::intern::SymId, Visibility)> = Vec::new();
                fn walk(
                    c: &std::rc::Rc<crate::value::Class>,
                    out: &mut Vec<(crate::intern::SymId, Visibility)>,
                    visited: &mut Vec<*const crate::value::Class>,
                ) {
                    let ptr = std::rc::Rc::as_ptr(c);
                    if visited.contains(&ptr) { return; }
                    visited.push(ptr);
                    for (k, m) in c.methods.borrow().iter() {
                        if !out.iter().any(|(s, _)| s == k) {
                            out.push((*k, m.visibility.get()));
                        }
                    }
                    for inc in c.includes.borrow().iter() {
                        walk(inc, out, visited);
                    }
                    if let Some(sup) = c.superclass.borrow().clone() {
                        walk(&sup, out, visited);
                    }
                }
                walk(&cls, &mut pairs, &mut visited);
                for (sid, vis) in pairs {
                    if pred(vis) { names.push(sid); }
                }
                names.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
            } else if matches!(&*name, "methods" | "public_methods") {
                // Class receiver — class-method chain. Singleton
                // methods don't carry per-entry visibility in
                // rubyrs's Class shape and are all public by default,
                // so `public_methods` reports the same set as
                // `methods` here; `private_methods` /
                // `protected_methods` have no surface and fall
                // through to an empty Array.
                if let Value::Class(cls) = &recv {
                // Walk a prepended module's transitive includes /
                // prepends — same shape as `walk_module` in
                // lookup.rs, but collects method names rather
                // than searching for one.
                fn walk_mod(
                    m: &std::rc::Rc<crate::value::Class>,
                    out: &mut Vec<crate::intern::SymId>,
                    visited: &mut Vec<*const crate::value::Class>,
                ) {
                    let ptr = std::rc::Rc::as_ptr(m);
                    if visited.contains(&ptr) { return; }
                    visited.push(ptr);
                    for pre in m.prepends.borrow().iter() {
                        walk_mod(pre, out, visited);
                    }
                    for k in m.methods.borrow().keys() {
                        if !out.contains(k) { out.push(*k); }
                    }
                    for inc in m.includes.borrow().iter() {
                        walk_mod(inc, out, visited);
                    }
                }
                let mut sc_visited: Vec<*const crate::value::Class> = Vec::new();
                let mut mod_visited: Vec<*const crate::value::Class> = Vec::new();
                let mut current = cls.clone();
                loop {
                    let ptr = std::rc::Rc::as_ptr(&current);
                    if sc_visited.contains(&ptr) { break; }
                    sc_visited.push(ptr);
                    for pre in current.singleton_prepends.borrow().iter() {
                        walk_mod(pre, &mut names, &mut mod_visited);
                    }
                    for k in current.singleton_methods.borrow().keys() {
                        if !names.contains(k) { names.push(*k); }
                    }
                    let parent = current.superclass.borrow().clone();
                    match parent {
                        Some(p) => current = p,
                        None => break,
                    }
                }
                names.sort_by(|a, b| {
                    self.interner.resolve(*a).cmp(self.interner.resolve(*b))
                });
                }
            }
            let elems: Vec<Value> = names.into_iter().map(Value::Sym).collect();
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems.into()));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `obj.singleton_methods` — Array of Symbols of methods
        // installed directly on this object's eigenclass via
        // `def obj.foo` / `define_singleton_method`. Distinct
        // from `methods` which walks the whole ancestor chain.
        // Receivers without an eigenclass installed return an
        // empty Array. Class receivers report their own
        // singleton-method table (class methods).
        if &*name == "singleton_methods" && args.is_empty() {
            let mut names: Vec<crate::intern::SymId> = Vec::new();
            match &recv {
                Value::Object(id) => {
                    if let crate::heap::HeapObj::Instance(inst) = self.heap.get(*id)
                        && let Some(sc) = &inst.singleton_class
                    {
                        // Methods installed via `def obj.foo` /
                        // define_singleton_method land here directly.
                        for k in sc.methods.borrow().keys() {
                            if !names.contains(k) { names.push(*k); }
                        }
                        // Modules brought in via `obj.extend(M)` live
                        // on the eigenclass's `includes` chain; CRuby
                        // reports each module's instance methods as
                        // singleton methods of `obj`. Walk transitive
                        // includes AND prepends so chains like
                        // `module Q; prepend P; end; obj.extend(Q)`
                        // surface P's methods too (CRuby's
                        // `Module#ancestors` includes both).
                        fn walk_chain(
                            c: &std::rc::Rc<crate::value::Class>,
                            out: &mut Vec<crate::intern::SymId>,
                            visited: &mut Vec<*const crate::value::Class>,
                        ) {
                            for chain in [c.includes.borrow(), c.prepends.borrow()] {
                                for m in chain.iter() {
                                    let ptr = std::rc::Rc::as_ptr(m);
                                    if visited.contains(&ptr) { continue; }
                                    visited.push(ptr);
                                    for k in m.methods.borrow().keys() {
                                        if !out.contains(k) { out.push(*k); }
                                    }
                                    walk_chain(m, out, visited);
                                }
                            }
                        }
                        let mut visited: Vec<*const crate::value::Class> = Vec::new();
                        walk_chain(sc, &mut names, &mut visited);
                    }
                }
                Value::Class(cls) => {
                    for k in cls.singleton_methods.borrow().keys() {
                        if !names.contains(k) { names.push(*k); }
                    }
                }
                _ => {}
            }
            names.sort_by(|a, b| {
                self.interner.resolve(*a).cmp(self.interner.resolve(*b))
            });
            let elems: Vec<Value> = names.into_iter().map(Value::Sym).collect();
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems.into()));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `obj.instance_variables` — Array of Symbols (with `@`
        // prefix). Reads ivars from `Value::Object` (Instance) and
        // `Value::Class` receivers (cls.ivars), staying consistent
        // with `instance_variable_get` / `_set` which also support
        // both shapes. Other receivers (primitives, Array/Hash/etc.
        // that don't carry ivars in rubyrs's heap model) get an
        // empty Array.
        if &*name == "instance_variables" && args.is_empty() {
            let mut names: Vec<Value> = Vec::new();
            let ivar_ids: Vec<crate::intern::SymId> = match &recv {
                Value::Object(id) => {
                    if let crate::heap::HeapObj::Instance(inst) = self.heap.get(*id) {
                        inst.ivars.keys().copied().collect()
                    } else {
                        Vec::new()
                    }
                }
                Value::Class(cls) => cls.ivars.borrow().keys().copied().collect(),
                _ => Vec::new(),
            };
            if !ivar_ids.is_empty() {
                let mut decorated: Vec<(String, crate::intern::SymId)> = ivar_ids.into_iter()
                    .map(|s| {
                        let raw = self.interner.resolve(s).to_string();
                        // Internal interner key includes the `@`
                        // prefix already (matches how parser interns
                        // ivar names). If not, prepend.
                        let key = if raw.starts_with('@') { raw } else { format!("@{}", raw) };
                        (key, s)
                    })
                    .collect();
                decorated.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, _) in decorated {
                    let sid = self.interner.intern(&key);
                    names.push(Value::Sym(sid));
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(names.into()));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `obj.instance_variable_get(name)` / `instance_variable_set(name, value)`
        // — pure ivar read/write by name. Surfaced as a blocker
        // for sinatra/indifferent_hash.rb's `Gem::Version#<=>` shape
        // (TRY_RUNS pass 7 layer #2) and load-bearing for any
        // introspection-heavy gem.
        //
        // Name validation: CRuby ivar names must match
        // `@[A-Za-z_][A-Za-z0-9_]*` — single `@` followed by an
        // identifier char (letter or underscore), then zero or more
        // identifier-or-digit chars. Rejects `@@x` (class var),
        // `@1` (digit start), `@foo?` (predicate suffix), bare `@`.
        // String intern path enforces `Config::max_symbols`.
        //
        // Heap shape: read/write reaches the ivar table on
        // `Value::Object` (Instance) and `Value::Class` (Class)
        // receivers — same storage that `Op::LoadIvar` /
        // `Op::StoreIvar` in vm/step.rs:552/562 read and write.
        // The set path is MORE defensive than `Op::StoreIvar`:
        // that op still calls `heap.instance_mut(*oid)` which
        // panics with the same "ICE: heap slot is not an
        // Instance" assertion this fix avoids; if `Op::StoreIvar`
        // is ever reached for a non-Instance Object slot it will
        // still ICE (a separate hardening concern, not covered
        // by this PR). The `_ =>` arm below catches every
        // non-Object/non-Class receiver — Int/Str/Float/Sym/
        // Nil/Bool/Array/Hash/Range/Proc/etc. — and raises
        // FrozenError. For mutable shapes like Array/Hash that
        // CRuby DOES allow ivars on, supporting that surface
        // would require ivar slots on those HeapObj variants;
        // explicit out-of-scope until a caller surfaces it.
        if &*name == "instance_variable_get" && args.len() == 1 {
            let ivar_id = self.resolve_ivar_name_arg(&args[0])?;
            let v = match &recv {
                Value::Object(oid) => match self.heap.get(*oid) {
                    crate::heap::HeapObj::Instance(inst) => {
                        inst.ivars.get(&ivar_id).cloned().unwrap_or(Value::Nil)
                    }
                    _ => Value::Nil,
                },
                Value::Class(cls) => {
                    cls.ivars.borrow().get(&ivar_id).cloned().unwrap_or(Value::Nil)
                }
                _ => Value::Nil,
            };
            self.stack.push(v);
            return Ok(());
        }
        if &*name == "instance_variable_set" && args.len() == 2 {
            let ivar_id = self.resolve_ivar_name_arg(&args[0])?;
            let value = args[1].clone();
            // Frozen-object guard. `Object#freeze` flips the
            // `Instance::frozen` Cell; subsequent mutation
            // attempts (CRuby contract) must raise FrozenError.
            // The frozen-read surface
            // (`frozen?`/`freeze`/`Object#frozen?`) was shipped
            // in a65e3080; this PR closes the silent-mutation
            // path that gem code relying on the post-freeze
            // invariant assumes is closed.
            if let Value::Object(oid) = &recv
                && let crate::heap::HeapObj::Instance(inst) = self.heap.get(*oid)
                && inst.frozen.get()
            {
                let cls_name = self.heap.real_class_of(*oid).name.clone();
                let inspect = recv.to_inspect(&self.heap, &self.interner);
                return Err(self.trap(RubyError::FrozenError {
                    msg: format!("can't modify frozen {}: {}", cls_name, inspect),
                }));
            }
            match &recv {
                Value::Object(oid) => match self.heap.get_mut(*oid) {
                    crate::heap::HeapObj::Instance(inst) => {
                        inst.ivars.insert(ivar_id, value.clone());
                        self.stack.push(value);
                        return Ok(());
                    }
                    // TypedData (and any future non-Instance Object
                    // heap variant) genuinely accepts ivars in CRuby
                    // — the limitation is rubyrs-specific (no ivar
                    // table on `TypedDataObj`). RubyError doesn't
                    // model `NotImplementedError` yet, so RuntimeError
                    // is the closest fit; keep the message terse and
                    // explicit about the rubyrs-side limitation so a
                    // gem hitting this knows it's not a CRuby
                    // semantic difference.
                    _ => return Err(self.trap(RubyError::RuntimeError {
                        msg: "instance_variable_set on TypedData receivers is not yet supported in rubyrs".to_string(),
                    })),
                },
                Value::Class(cls) => {
                    cls.ivars.borrow_mut().insert(ivar_id, value.clone());
                    self.stack.push(value);
                    return Ok(());
                }
                _ => {
                    let cls = crate::vm::numeric::class_name_for_error(&recv);
                    let inspected = recv.to_inspect(&self.heap, &self.interner);
                    return Err(self.trap(RubyError::FrozenError {
                        msg: format!("can't modify frozen {}: {}", cls, inspected),
                    }));
                }
            }
        }
        // `obj.instance_variable_defined?(name)` — true iff the
        // named ivar has been set (even to nil). Mirrors the
        // get/set storage shape: reads the same Instance.ivars
        // map for Value::Object and Class.ivars for Value::Class.
        // Other receivers carry no ivar table, so the answer is
        // always false. The name argument goes through the same
        // `resolve_ivar_name_arg` validator as get/set, so an
        // invalid identifier (e.g. `:foo` without `@`) raises
        // NameError before the lookup runs — matching CRuby.
        if &*name == "instance_variable_defined?" && args.len() == 1 {
            let ivar_id = self.resolve_ivar_name_arg(&args[0])?;
            let defined = match &recv {
                Value::Object(oid) => match self.heap.get(*oid) {
                    crate::heap::HeapObj::Instance(inst) => {
                        inst.ivars.contains_key(&ivar_id)
                    }
                    _ => false,
                },
                Value::Class(cls) => cls.ivars.borrow().contains_key(&ivar_id),
                _ => false,
            };
            self.stack.push(Value::Bool(defined));
            return Ok(());
        }
        // Wrong-arity arms for the ivar-introspection family —
        // match CRuby's ArgumentError surface. Without these,
        // `obj.instance_variables(1)`, `obj.instance_variable_get()`,
        // or `obj.instance_variable_set(:@x)` would fall through to
        // NoMethodError, which is wrong (CRuby reports arity, not
        // unknown method). `instance_variables` takes zero args;
        // `_get` / `_defined?` take one; `_set` takes two.
        if &*name == "instance_variables" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
            }));
        }
        if &*name == "instance_variable_get" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
            }));
        }
        if &*name == "instance_variable_set" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 2)", args.len()),
            }));
        }
        if &*name == "instance_variable_defined?" {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 1)", args.len()),
            }));
        }
        // `Integer#digits([base])` for Int receivers — LSB-first
        // digit Array, i64 fast path (no BigInt arithmetic for
        // small inputs). Default base 10; base must be >= 2.
        // Error semantics match `Vm::try_integer_digits` (the
        // BigInt-receiver path under `feature = "bignum"`) so
        // both profiles agree on the surface user code sees:
        //   - Arity > 1 → ArgumentError "wrong number of arguments
        //     (given N, expected 0..1)" matching CRuby. Under
        //     bignum the equivalent guard in `bigint_primitive`
        //     fires first; this arm catches the no-bignum profile.
        //   - Non-Integer base → TypeError matching CRuby text.
        //   - Negative base → ArgumentError "negative radix".
        //   - 0/1 base → ArgumentError "invalid radix N".
        //   - Negative receiver → ArgumentError "out of domain"
        //     (CRuby uses Math::DomainError; substituted because
        //     Math::DomainError isn't modelled in this subset —
        //     same convention as other numeric-out-of-domain
        //     arms elsewhere in `Vm::do_call`).
        // CRuby precedence: negative receiver raises
        // Math::DomainError BEFORE any arity / base check. Mirror
        // the order with the substitute ArgumentError, so user
        // code's `rescue ArgumentError` catches the negative-recv
        // path regardless of the other args' validity. Under
        // bignum the equivalent check in `bigint_primitive` fires
        // before this dispatcher runs, but keep this guard for
        // the no-bignum profile and as defense-in-depth.
        if let Value::Int(n) = &recv && &*name == "digits" && *n < 0 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: "out of domain".to_string(),
            }));
        }
        // `Integer#divmod(b)` — returns [q, r] Array where
        // q = floor(a/b), r = a - b*q (CRuby floor semantics).
        // Lives in dispatch.rs because the Array result needs
        // heap-alloc + maybe_gc + check_alloc. Sits alongside
        // `digits` for the same reason. Sibling BigInt dispatch
        // is handled in bigint_primitive (which routes its own
        // BigInt-receiver path through here for the alloc).
        let recv_is_integer = {
            #[cfg(feature = "bignum")]
            { matches!(&recv, Value::Int(_) | Value::BigInt(_)) }
            #[cfg(not(feature = "bignum"))]
            { matches!(&recv, Value::Int(_)) }
        };
        // Phase C.4.4 — `Integer ** Rational` and `Float ** Rational`.
        // Lives here (not at the Int / Float arms inside numeric.rs /
        // primitive_call) because those surfaces don't see Rational
        // args natively. Same shape as the Rational#** dispatch in
        // `rational_pow`:
        //   - Integer-valued Rational exp (`den == 1`, num fits i64)
        //     → delegate to `numeric_call` with an Int arg so the
        //     existing Int#** / Float#** paths fire (preserves type
        //     tag: `2 ** Rational(3, 1) == 8` rather than `8.0`).
        //   - Otherwise demote to Float. Pre-demote, guard
        //     `recv == 0 && exp < 0` → ZeroDivisionError so
        //     `0 ** Rational(-1, 2)` matches CRuby rather than
        //     returning `0.0_f64.powf(-0.5) == Infinity`.
        if (recv_is_integer || matches!(&recv, Value::Float(_)))
            && &*name == "**"
            && args.len() == 1
            && matches!(&args[0], Value::Rational(_))
        {
            if let Some(k) = integer_valued_exp(&args[0], &self.heap) {
                let delegated = crate::vm::numeric::numeric_call(
                    &recv, "**", &[Value::Int(k)], None,
                )
                .map_err(|e| self.trap(e))?;
                if let Some(v) = delegated {
                    self.stack.push(v);
                    return Ok(());
                }
                // numeric_call returning None for `Int/Float ** Int`
                // would be a primitive coverage gap; fall through to
                // the Float-demote path so the caller still gets a
                // sensible answer.
            }
            let base_f = match &recv {
                Value::Int(n) => *n as f64,
                #[cfg(feature = "bignum")]
                Value::BigInt(id) => {
                    crate::vm::bignum::bigint_to_f64_sign_preserving(self.heap.bigint(*id))
                }
                Value::Float(g) => *g,
                _ => unreachable!("guarded above"),
            };
            let exp_f = match &args[0] {
                Value::Rational(id) => crate::heap::rational_to_f64(self.heap.rational(*id)),
                _ => unreachable!("guarded above"),
            };
            // Zero base + negative non-integer exp → ZeroDivisionError
            // (matches CRuby; mirrors the rational_pow guard above).
            let recv_is_zero = match &recv {
                Value::Int(0) => true,
                Value::Float(g) => *g == 0.0,
                #[cfg(feature = "bignum")]
                Value::BigInt(id) => {
                    use num_traits::Zero;
                    self.heap.bigint(*id).is_zero()
                }
                _ => false,
            };
            if recv_is_zero && exp_f < 0.0 {
                return Err(self.trap(RubyError::ZeroDivisionError {
                    msg: "divided by 0".to_string(),
                }));
            }
            self.stack.push(Value::Float(base_f.powf(exp_f)));
            return Ok(());
        }
        if recv_is_integer && &*name == "divmod" {
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            // Compute q, r as Values. Float arg → both q, r Float.
            // Zero divisor (Int or Float) → ZeroDivisionError.
            // NaN divisor → FloatDomainError.
            // Non-Numeric → TypeError.
            let arg = &args[0];
            let (q, r) = match arg {
                Value::Int(b) => {
                    if *b == 0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    match &recv {
                        Value::Int(a) => {
                            // Compute via the floor helpers. Under
                            // bignum, i64::MIN/-1 needs BigInt
                            // promotion (sibling to apply_int's
                            // None-on-overflow path); route through
                            // bigint_arith for that case.
                            #[cfg(feature = "bignum")]
                            if *a == i64::MIN && *b == -1 {
                                // recv is Int(i64::MIN), no heap id
                                // to pin on the recv side — but q is
                                // a freshly-promoted BigInt whose
                                // only root is this local across the
                                // Mod call's `bigint_to_value` →
                                // `maybe_gc` window.
                                let mut g = PinGuard::new(self);
                                let q = g.vm.bigint_arith(
                                    crate::bytecode::BinOpKind::Div, &recv, arg,
                                ).expect("ICE: bigint_arith None for i64::MIN/-1")?;
                                g.pin(q.clone());
                                let r = g.vm.bigint_arith(
                                    crate::bytecode::BinOpKind::Mod, &recv, arg,
                                ).expect("ICE: bigint_arith None for i64::MIN/-1")?;
                                (q, r)
                            } else {
                                (
                                    Value::Int(crate::vm::floor_div_i64(*a, *b)),
                                    Value::Int(crate::vm::floor_mod_i64(*a, *b)),
                                )
                            }
                            #[cfg(not(feature = "bignum"))]
                            (
                                Value::Int(crate::vm::floor_div_i64(*a, *b)),
                                Value::Int(crate::vm::floor_mod_i64(*a, *b)),
                            )
                        }
                        #[cfg(feature = "bignum")]
                        Value::BigInt(_) => {
                            // BigInt × Int — promotes through bigint_arith.
                            // Pin recv AND q across BOTH calls — both
                            // route through `bigint_to_value` →
                            // `maybe_gc`, which would otherwise sweep
                            // recv (drained from the stack) before
                            // its bigint heap slot is read, AND sweep
                            // q before r lands.
                            let mut g = PinGuard::new(self);
                            g.pin(recv.clone());
                            let q = g.vm.bigint_arith(
                                crate::bytecode::BinOpKind::Div, &recv, arg,
                            ).expect("ICE: bigint_arith None for BigInt divmod")?;
                            g.pin(q.clone());
                            let r = g.vm.bigint_arith(
                                crate::bytecode::BinOpKind::Mod, &recv, arg,
                            ).expect("ICE: bigint_arith None for BigInt divmod")?;
                            (q, r)
                        }
                        _ => unreachable!("recv is Int or BigInt by outer guard"),
                    }
                }
                Value::Float(b) => {
                    if b.is_nan() {
                        // CRuby raises `FloatDomainError: NaN`.
                        // FloatDomainError < RangeError < StandardError,
                        // so `rescue FloatDomainError`, `rescue RangeError`,
                        // and a bare `rescue` all catch this (verified
                        // in tests/embed/numeric.rs's
                        // `float_domain_error_class_and_rescue_chain`).
                        return Err(self.trap(RubyError::FloatDomainError {
                            msg: "NaN".to_string(),
                        }));
                    }
                    if *b == 0.0 {
                        return Err(self.trap(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        }));
                    }
                    let a_f = match &recv {
                        Value::Int(n) => *n as f64,
                        #[cfg(feature = "bignum")]
                        Value::BigInt(id) => {
                            use num_traits::ToPrimitive;
                            self.heap.bigint(*id).to_f64().unwrap_or(f64::NAN)
                        }
                        _ => unreachable!("recv is Int or BigInt"),
                    };
                    let q_f = (a_f / *b).floor();
                    let r_f = crate::vm::numeric::floor_mod_f64(a_f, *b);
                    // CRuby: q is Integer-valued Float for Int.divmod(Float)? No —
                    // for `13.divmod(4.0)` CRuby returns `[3, 1.0]` (Int q, Float r).
                    let q_int = if q_f.is_finite() && q_f >= (i64::MIN as f64) && q_f < (i64::MAX as f64) {
                        Value::Int(q_f as i64)
                    } else {
                        // q overflows i64 → keep as Float (CRuby would
                        // promote to BigInt; approximate by Float for
                        // now matching the fdiv precision tier).
                        Value::Float(q_f)
                    };
                    (q_int, Value::Float(r_f))
                }
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => {
                    // BigInt arg arm — pin recv + arg + q across the
                    // bigint_arith calls (each routes through
                    // bigint_to_value → maybe_gc).
                    let mut g = PinGuard::new(self);
                    g.pin(recv.clone());
                    g.pin(arg.clone());
                    let q = g.vm.bigint_arith(
                        crate::bytecode::BinOpKind::Div, &recv, arg,
                    ).expect("ICE: bigint_arith None for BigInt divmod")?;
                    g.pin(q.clone());
                    let r = g.vm.bigint_arith(
                        crate::bytecode::BinOpKind::Mod, &recv, arg,
                    ).expect("ICE: bigint_arith None for BigInt divmod")?;
                    (q, r)
                }
                _ => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} can't be coerced into Integer",
                            crate::vm::numeric::type_name_for_coerce(arg),
                        ),
                    }));
                }
            };
            // GC root hole (sibling to the coerce fix in PR #289):
            // for BigInt divmod, `q` and `r` are freshly-allocated
            // BigInt ObjIds returned by `bigint_arith` — their only
            // live root at this point is the Rust local. Without the
            // PinGuard, `maybe_gc()` runs with both ObjIds
            // unreachable and sweeps them before the result Array is
            // allocated, leaving the Array with dangling slots.
            // Pin both Values across maybe_gc + heap.alloc; Drop
            // restores normal GC reachability via the freshly-pushed
            // `Value::Array(id)` on the stack.
            let arr_id = {
                let mut g = PinGuard::new(self);
                g.pin(q.clone());
                g.pin(r.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(vec![q, r].into()))
            };
            self.stack.push(Value::Array(arr_id));
            return Ok(());
        }
        // `Float#divmod(n)` — sibling to the Integer path above; lives
        // here (not numeric.rs) because it allocates the `[q, r]` Array.
        // q is the Integer-valued floor quotient, r the Float
        // floored-remainder; NaN/±Infinity recv and NaN divisor raise
        // FloatDomainError, zero divisor ZeroDivisionError (CRuby).
        if let Value::Float(a) = &recv
            && &*name == "divmod"
        {
            {
                if args.len() != 1 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1)",
                            args.len(),
                        ),
                    }));
                }
                let a = *a;
                if a.is_nan() || a.is_infinite() {
                    return Err(self.trap(RubyError::FloatDomainError {
                        msg: crate::vm::numeric::float_domain_label(a).to_string(),
                    }));
                }
                let b = match &args[0] {
                    Value::Int(b) => *b as f64,
                    Value::Float(b) => {
                        if b.is_nan() {
                            return Err(self.trap(RubyError::FloatDomainError {
                                msg: "NaN".to_string(),
                            }));
                        }
                        *b
                    }
                    #[cfg(feature = "bignum")]
                    Value::BigInt(id) => {
                        use num_traits::ToPrimitive;
                        self.heap.bigint(*id).to_f64().unwrap_or(f64::NAN)
                    }
                    _ => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "{} can't be coerced into Float",
                                crate::vm::numeric::type_name_for_coerce(&args[0]),
                            ),
                        }));
                    }
                };
                if b == 0.0 {
                    return Err(self.trap(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    }));
                }
                let q_f = (a / b).floor();
                let r_f = crate::vm::numeric::floor_mod_f64(a, b);
                let q = if q_f.is_finite()
                    && q_f >= (i64::MIN as f64)
                    && q_f < (i64::MAX as f64)
                {
                    Value::Int(q_f as i64)
                } else {
                    Value::Float(q_f)
                };
                let r = Value::Float(r_f);
                let arr_id = {
                    let mut g = PinGuard::new(self);
                    g.pin(q.clone());
                    g.pin(r.clone());
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    g.vm.heap.alloc(HeapObj::Array(vec![q, r].into()))
                };
                self.stack.push(Value::Array(arr_id));
                return Ok(());
            }
        }
        // `Integer#gcdlcm(n)` → `[gcd, lcm]` (both non-negative). Lives
        // here (not numeric.rs) because it allocates the pair Array.
        // Handles the i64 fast path; the i64::MIN / lcm-overflow edges
        // fall through (rare, matching where gcd/lcm decline alone).
        if let Value::Int(a) = &recv
            && &*name == "gcdlcm"
        {
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            let Value::Int(b) = &args[0] else {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "{} can't be coerced into Integer",
                        crate::vm::numeric::type_name_for_coerce(&args[0]),
                    ),
                }));
            };
            let (a, b) = (*a, *b);
            // [Int, Int] has no heap refs → no PinGuard needed.
            if a != i64::MIN
                && b != i64::MIN
                && let Some(l) = crate::vm::numeric::lcm_i64(a, b)
            {
                let g = crate::vm::numeric::gcd_i64(a, b);
                self.maybe_gc();
                self.check_alloc()?;
                let arr_id =
                    self.heap.alloc(HeapObj::Array(vec![Value::Int(g), Value::Int(l)].into()));
                self.stack.push(Value::Array(arr_id));
                return Ok(());
            }
        }
        // `Numeric#coerce(other)` — the Tier-2 Numeric protocol
        // entry point. Returns a 2-element Array `[other_promoted,
        // self_promoted]` so arithmetic operators on heterogeneous
        // numeric pairs can route through a uniform "promote then
        // operate on same-type" path. Implemented for Integer
        // (Int + BigInt) and Float receivers; Phase C (Rational /
        // Complex) will extend this surface.
        //
        // CRuby parity:
        //   - Int.coerce(Integer)  → [Integer, Integer]
        //   - Int.coerce(Float)    → [Float,   Float]
        //   - Float.coerce(Numeric)→ [Float,   Float]
        //   - any.coerce(non-Numeric) → TypeError
        //     "<other> can't be coerced into <recv_class>"
        let recv_is_numeric = matches!(&recv, Value::Int(_) | Value::Float(_))
            || {
                #[cfg(feature = "bignum")]
                { matches!(&recv, Value::BigInt(_)) }
                #[cfg(not(feature = "bignum"))]
                { false }
            };
        if recv_is_numeric && &*name == "coerce" {
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            let arg = &args[0];
            let recv_class: &str = match &recv {
                Value::Int(_) => "Integer",
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => "Integer",
                Value::Float(_) => "Float",
                _ => unreachable!("guarded by recv_is_numeric"),
            };
            // Pair: [coerced_other, coerced_self]. Float dominates
            // — any pair containing a Float collapses both sides
            // to Float. Otherwise both stay Integer (Int and
            // BigInt are the same Ruby class; pass through
            // unchanged).
            let (other_v, self_v) = match (&recv, arg) {
                (Value::Float(_), Value::Float(_)) => (arg.clone(), recv.clone()),
                (Value::Float(s), Value::Int(o)) => {
                    (Value::Float(*o as f64), Value::Float(*s))
                }
                (Value::Int(s), Value::Float(_)) => {
                    (arg.clone(), Value::Float(*s as f64))
                }
                #[cfg(feature = "bignum")]
                (Value::Float(s), Value::BigInt(id)) => {
                    let o_f = crate::vm::bignum::bigint_to_f64_sign_preserving(self.heap.bigint(*id));
                    (Value::Float(o_f), Value::Float(*s))
                }
                #[cfg(feature = "bignum")]
                (Value::BigInt(id), Value::Float(_)) => {
                    let s_f = crate::vm::bignum::bigint_to_f64_sign_preserving(self.heap.bigint(*id));
                    (arg.clone(), Value::Float(s_f))
                }
                (Value::Int(_), Value::Int(_)) => (arg.clone(), recv.clone()),
                #[cfg(feature = "bignum")]
                (Value::Int(_), Value::BigInt(_))
                | (Value::BigInt(_), Value::Int(_))
                | (Value::BigInt(_), Value::BigInt(_)) => (arg.clone(), recv.clone()),
                _ => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} can't be coerced into {}",
                            crate::vm::numeric::type_name_for_coerce(arg),
                            recv_class,
                        ),
                    }));
                }
            };
            // GC root hole: both `other_v` and `self_v` may carry
            // pass-through BigInt ObjIds whose only live root at this
            // point is the Rust local (recv / args were drained from
            // the stack on the way in). Without the PinGuard,
            // `maybe_gc()` runs with those ObjIds unreachable and
            // sweeps the BigInt — leaving the result Array with a
            // dangling slot. Pin both Values across the alloc; drop
            // restores normal GC reachability via the freshly-pushed
            // `Value::Array(id)` on the stack.
            let arr_id = {
                let mut g = PinGuard::new(self);
                g.pin(other_v.clone());
                g.pin(self_v.clone());
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(vec![other_v, self_v].into()))
            };
            self.stack.push(Value::Array(arr_id));
            return Ok(());
        }
        // Phase C.3 — `Integer#to_r` and `Integer#rationalize` are
        // pure constructors (no fractional part) so they trivially
        // build `Rational(self, 1)`. Lives here in dispatch.rs (not
        // primitive_call) because heap.alloc is needed.
        //
        // `Integer#rationalize(eps=nil)` accepts an optional
        // tolerance arg per CRuby but the eps value itself is
        // ignored — only meaningful for Float#rationalize
        // (Phase C.4). Type-checks the arg below: Numeric / nil
        // accepted, anything else raises TypeError. 2+ args raise
        // CRuby's ArgumentError.
        if recv_is_integer && (&*name == "to_r" || &*name == "rationalize") {
            let max_arity: usize = if &*name == "rationalize" { 1 } else { 0 };
            if args.len() > max_arity {
                // CRuby uses "expected 0" for 0-arg methods, not
                // "expected 0..0" — the range form is reserved for
                // a true range with > 0 spread (e.g. "expected 0..1"
                // for rationalize). Sibling arity guards above
                // follow the same convention.
                let expected = if max_arity == 0 {
                    "0".to_string()
                } else {
                    format!("0..{}", max_arity)
                };
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected {})",
                        args.len(), expected,
                    ),
                }));
            }
            // `rationalize(eps)` — CRuby's MRI calls `f_nonzero_p`
            // on eps internally which raises NoMethodError on
            // non-Numeric args. We surface the more standard
            // TypeError "X can't be coerced into Float" shape
            // (eps is conceptually a Float tolerance). Nil is
            // explicitly accepted as "use default tolerance".
            // The eps value itself is ignored for Integer
            // receivers (no fractional part), but the type check
            // matters for parity with what Float#rationalize
            // (Phase C.4) will enforce.
            if &*name == "rationalize" && args.len() == 1 {
                let is_numeric_or_nil = matches!(
                    &args[0],
                    Value::Int(_) | Value::Float(_) | Value::Nil | Value::Rational(_)
                ) || {
                    #[cfg(feature = "bignum")]
                    { matches!(&args[0], Value::BigInt(_)) }
                    #[cfg(not(feature = "bignum"))]
                    { false }
                };
                if !is_numeric_or_nil {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} can't be coerced into Float",
                            crate::vm::numeric::type_name_for_coerce(&args[0]),
                        ),
                    }));
                }
            }
            // Phase C.4.2: BigInt receiver routes through the
            // BigInt make_rational entry; small Int receivers
            // continue through `make_rational(i64, 1)` which
            // already widens internally under bignum.
            let v = match &recv {
                Value::Int(n) => self.make_rational(*n, 1)?,
                #[cfg(feature = "bignum")]
                Value::BigInt(id) => {
                    use num_bigint::BigInt;
                    use num_traits::One;
                    let num = self.heap.bigint(*id).clone();
                    self.make_rational_bigint(num, BigInt::one())?
                }
                _ => unreachable!("guarded by recv_is_integer"),
            };
            self.stack.push(v);
            return Ok(());
        }
        // Phase C.4.3 — `Float#to_r` and `Float#rationalize`.
        // `to_r` builds the exact-Rational representation via the
        // IEEE-754 decomposition `f = sign * mantissa * 2^exp` (no
        // rounding). `rationalize(eps)` runs the Stern-Brocot
        // mediant search for the simplest fraction within ±|eps|.
        // Bare `rationalize` (no eps) runs Stern-Brocot on the
        // half-ULP interval — returns the simplest Rational that
        // round-trips back to the same Float, matching CRuby
        // (`0.1.rationalize == (1/10)`, NOT the lossless to_r).
        // NaN / ±Inf → FloatDomainError. nil eps rejected with
        // TypeError; the eps `Value` is validated as Numeric.
        if let Value::Float(f) = &recv
            && (&*name == "to_r" || &*name == "rationalize")
        {
            let f = *f;
                let max_arity: usize = if &*name == "rationalize" { 1 } else { 0 };
                if args.len() > max_arity {
                    let expected = if max_arity == 0 {
                        "0".to_string()
                    } else {
                        format!("0..{}", max_arity)
                    };
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected {})",
                            args.len(), expected,
                        ),
                    }));
                }
                if !f.is_finite() {
                    return Err(self.trap(RubyError::FloatDomainError {
                        msg: crate::vm::numeric::float_domain_label(f).to_string(),
                    }));
                }
                // eps type-check (rationalize only). Numeric required
                // — nil is REJECTED (CRuby's `Float#rationalize(nil)`
                // raises NoMethodError 'undefined method abs for nil';
                // we surface the cleaner TypeError shape).
                let eps_value: Option<&Value> = if &*name == "rationalize" && args.len() == 1 {
                    let is_numeric = matches!(
                        &args[0],
                        Value::Int(_) | Value::Float(_) | Value::Rational(_)
                    ) || {
                        #[cfg(feature = "bignum")]
                        { matches!(&args[0], Value::BigInt(_)) }
                        #[cfg(not(feature = "bignum"))]
                        { false }
                    };
                    if !is_numeric {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "{} can't be coerced into Float",
                                crate::vm::numeric::type_name_for_coerce(&args[0]),
                            ),
                        }));
                    }
                    Some(&args[0])
                } else {
                    None
                };
                let mode = if &*name == "to_r" {
                    FloatToRationalMode::Lossless
                } else if let Some(eps_v) = eps_value {
                    FloatToRationalMode::EpsArg(eps_v.clone())
                } else {
                    FloatToRationalMode::DefaultUlp
                };
                let v = self.float_to_rational_value(f, mode)?;
                self.stack.push(v);
                return Ok(());
        }
        if let Value::Int(_) = &recv && &*name == "digits" && args.len() > 1 {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len(),
                ),
            }));
        }
        if let Value::Int(n) = &recv && &*name == "digits" && args.len() <= 1 {
            let base: i64 = match args.first() {
                None => 10,
                Some(Value::Int(b)) => *b,
                // BigInt base under bignum: `n` is i64-sized and
                // any BigInt that survived `bigint_to_value`'s
                // demote-on-fit is necessarily > i64::MAX in
                // magnitude. So `|n| < base` always holds and the
                // result is a single-element array (n or 0 after
                // the negative-recv check). Validate the base
                // sign here — negative BigInt is "negative radix"
                // matching the i64 path's text.
                #[cfg(feature = "bignum")]
                Some(Value::BigInt(id)) => {
                    if self.heap.bigint(*id).sign() == num_bigint::Sign::Minus {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "negative radix".to_string(),
                        }));
                    }
                    if *n < 0 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "out of domain".to_string(),
                        }));
                    }
                    self.maybe_gc();
                    self.check_alloc()?;
                    let id = self.heap.alloc(HeapObj::Array(vec![Value::Int(*n)].into()));
                    self.stack.push(Value::Array(id));
                    return Ok(());
                }
                Some(other) => return Err(self.trap(RubyError::TypeError {
                    // Share the same class-name helper as the
                    // BigInt-receiver path in `Vm::try_integer_digits`
                    // so cross-profile error text agrees ("nil",
                    // "true", "false" vs `Value::type_name`'s
                    // "NilClass", "Boolean").
                    msg: format!(
                        "no implicit conversion of {} into Integer",
                        crate::vm::numeric::type_name_for_coerce(other),
                    ),
                })),
            };
            if base < 0 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "negative radix".to_string(),
                }));
            }
            if base < 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("invalid radix {}", base),
                }));
            }
            if *n < 0 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "out of domain".to_string(),
                }));
            }
            let mut elems: Vec<Value> = Vec::new();
            let mut m = *n;
            if m == 0 {
                elems.push(Value::Int(0));
            } else {
                while m > 0 {
                    elems.push(Value::Int(m % base));
                    m /= base;
                }
            }
            self.maybe_gc();
            self.check_alloc()?;
            let id = self.heap.alloc(HeapObj::Array(elems.into()));
            self.stack.push(Value::Array(id));
            return Ok(());
        }
        // `Object#equal?` — identity comparison. For heap-managed
        // receivers, same `ObjId`; for inline values, same content.
        // CRuby never overrides this on subclasses, so we always
        // intercept (above class-lookup would be redundant work).
        if &*name == "equal?" && args.len() == 1 {
            let same = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) => a == b,
                (Value::Array(a), Value::Array(b)) => a == b,
                (Value::Hash(a), Value::Hash(b)) => a == b,
                (Value::Range(a), Value::Range(b)) => a == b,
                (Value::Block(a), Value::Block(b)) => a == b,
                (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
                // String is now Rc-shared and identity-bearing
                // (frozen flag, aliasing). `equal?` should reflect
                // Rc-pointer identity, not content equality.
                (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b),
                // BigInt is heap-allocated; `equal?` is ObjId
                // identity, matching CRuby (where two separately-
                // allocated Bignums with the same magnitude are
                // distinct objects). Without this arm BigInt fell
                // through to the value-equality default, so
                // `(2**64).equal?(2**64)` (two distinct allocs)
                // wrongly returned true.
                #[cfg(feature = "bignum")]
                (Value::BigInt(a), Value::BigInt(b)) => a == b,
                // Other heap-allocated variants — `equal?` is
                // ObjId / Rc-pointer identity. Pre-fix these fell
                // through to ruby_eq, which has no arms for them
                // and returned false even for self-comparison
                // (`m = obj.method(:foo); m.equal?(m)` was false).
                // Mirrors the BigInt/Array/Hash arms above.
                (Value::BoundMethod(a), Value::BoundMethod(b)) => a == b,
                (Value::UnboundMethod(a), Value::UnboundMethod(b)) => a == b,
                (Value::CurriedProc(a), Value::CurriedProc(b)) => a == b,
                #[cfg(feature = "regex")]
                (Value::Regex(a), Value::Regex(b)) => Rc::ptr_eq(a, b),
                // Immediates (Int, Float, Sym, Bool, Nil) — fall
                // back on ruby_eq (value equality).
                _ => recv.ruby_eq(&args[0], &self.heap),
            };
            self.stack.push(Value::Bool(same));
            return Ok(());
        }
        // Universal `Object#eql?` fallback. Per-type type-strict
        // numeric overrides (`Integer#eql?`, `Float#eql?`,
        // `BigInt#eql?`) live in `primitive_call` arms above and
        // would have fired before reaching here. By the time
        // control gets here no per-type arm matched, so delegate
        // to `ruby_eq`:
        //  - String / Array / Hash / Range: value equality
        //    (matches CRuby's Array#eql? / Hash#eql? overrides
        //    that compare elementwise). Minor divergence at the
        //    nested-numeric leaf where CRuby's element-wise eql?
        //    distinguishes `[5].eql?([5.0])` from `[5] == [5.0]`;
        //    we use the `==`-flavoured ruby_eq for elements, so
        //    both come out true. Acceptable for now — the common
        //    cases (same-shape containers, same-string lookups)
        //    all match CRuby.
        //  - Object / BigInt: ObjId identity via ruby_eq's
        //    per-variant arms (matches CRuby's Kernel#eql?
        //    default, which is identity for user objects).
        //  - Class: Rc::ptr_eq via ruby_eq.
        //  - BoundMethod / UnboundMethod: gated out below —
        //    handled by the dedicated Method ==/!=/eql? arm
        //    further down (ruby_eq has no Method case, so the
        //    universal path would return false even for two
        //    equivalent Methods).
        //  - CurriedProc / Block: no ruby_eq case → falls through
        //    to the catchall (returns false; CRuby's Proc#eql?
        //    is identity, which our distinct ObjIds approximate).
        //  - Sym / Bool / Nil: identity == value equality for
        //    immediates.
        // Universal `respond_to?(:eql?)` already returns true via
        // the universal whitelist.
        if &*name == "eql?"
            && !matches!(&recv, Value::Rational(_) | Value::BoundMethod(_) | Value::UnboundMethod(_)) {
            // Arity guard fires regardless of receiver — CRuby
            // raises ArgumentError before doing any per-type
            // dispatch. Primitive_call's per-type arms above only
            // match exact 1-arg shape, so we know arity must
            // mismatch if control reaches this `eql?` block with
            // != 1 arg.
            //
            // Rational recv is gated out — Phase C.2 added a
            // type-strict `eql?` arm in the Rational dispatch
            // block further below. The universal `ruby_eq` here
            // would otherwise treat `Rational(1, 1).eql?(1)` as
            // true (since ruby_eq has cross-type Rational arms),
            // breaking CRuby's numeric strictness for eql?.
            if args.len() != 1 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 1)",
                        args.len(),
                    ),
                }));
            }
            let same = recv.ruby_eq(&args[0], &self.heap);
            self.stack.push(Value::Bool(same));
            return Ok(());
        }
        // Universal `hash` arity guard — fires only after
        // per-type arms in primitive_call have rejected the
        // wrong-arity call. The per-type arms (Int/Float/BigInt
        // /String) only match the exact 0-arg shape, so arity
        // mismatch reaches here. We don't dispatch hash itself
        // universally (not every receiver supports it), but we
        // DO raise ArgumentError for receivers that do —
        // identified by `responds_to(:hash)`. Without the
        // `responds_to` check, this would also fire on
        // `obj.hash(:x)` where obj doesn't support hash at all
        // (CRuby: NoMethodError for the missing method, not
        // ArgumentError for arity). Use the existing whitelist
        // to make the distinction.
        if &*name == "hash" && !args.is_empty() {
            let name_id = self.interner.intern("hash");
            if self.responds_to(&recv, name_id, true) {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len(),
                    ),
                }));
            }
            // Falls through to NoMethodError below.
        }
        // `Method#==` / `UnboundMethod#==` — intercept before the
        // universal `==` fallback (which has no arm for these and
        // would return `false`).
        //
        // BoundMethod: same name_id AND receiver identity. Heap-
        // backed recvs compare by ObjId / Rc-pointer; primitives
        // (Int / Sym / Bool / ...) compare by value. This matches
        // CRuby, where `s1.method(:length) == s2.method(:length)`
        // is `false` for distinct String instances but `true` for
        // the same Integer literal.
        //
        // UnboundMethod: lookup both classes' Method records via
        // `lookup_method_uncached` (walks ancestor chain) and
        // compare by Rc-pointer. Two UnboundMethods that resolve
        // to the same underlying definition — e.g., a parent's
        // method inherited by a subclass — are equal, matching
        // CRuby's `C.instance_method(:foo) == D.instance_method(:foo)`.
        // Method#== / Method#!= / Method#eql? — same semantics for
        // all three (CRuby treats `eql?` as an alias of `==` for
        // Method/UnboundMethod). Without this arm, `eql?` would
        // reach the universal `ruby_eq` fallback (no Method case →
        // false), and `!=` would route through the universal `==`
        // fallback (same false result, negated to true) — both
        // wrong for two equivalent Methods.
        if (&*name == "==" || &*name == "!=" || &*name == "eql?")
            && matches!(&recv, Value::BoundMethod(_) | Value::UnboundMethod(_)) {
                if args.len() != 1 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1)",
                            args.len()
                        ),
                    }));
                }
                let other = &args[0];
                let eq = match (&recv, other) {
                    (Value::BoundMethod(a), Value::BoundMethod(b)) => {
                        // Snapshot-first identity, mirroring the
                        // UnboundMethod arm: same receiver AND the
                        // underlying Method Rc must agree. After a
                        // `def`/`remove_method` on the recv's class,
                        // a fresh `obj.method(:foo)` captures a NEW
                        // Method Rc — old and new BoundMethods then
                        // compare unequal, matching CRuby's
                        // iseq-aware Method#==. Two `.method(:foo)`
                        // captures with no intervening redefine
                        // share the same class-table Rc (clone of
                        // the HashMap entry) → Rc::ptr_eq true. For
                        // builtin / no-snapshot recvs both sides
                        // resolve to None and fall back to name —
                        // `7.method(:+) == 7.method(:+)` stays true.
                        let (ra, na, sa) = self.heap.bound_method_full(*a);
                        let ra = ra.clone();
                        let (rb, nb, sb) = self.heap.bound_method_full(*b);
                        let rb = rb.clone();
                        let sa = sa.clone();
                        let sb = sb.clone();
                        if !method_recv_identity(&ra, &rb) {
                            false
                        } else {
                            let ma = sa.or_else(|| match self.class_of(&ra) {
                                Value::Class(c) => self.lookup_method_uncached(&c, na),
                                _ => None,
                            });
                            let mb = sb.or_else(|| match self.class_of(&rb) {
                                Value::Class(c) => self.lookup_method_uncached(&c, nb),
                                _ => None,
                            });
                            match (ma, mb) {
                                (Some(x), Some(y)) => Rc::ptr_eq(&x, &y),
                                _ => na == nb,
                            }
                        }
                    }
                    (Value::UnboundMethod(a), Value::UnboundMethod(b)) => {
                        // Snapshot-first identity: prefer the
                        // capture-time Method Rc — UnboundMethod
                        // semantics pin to capture-time, matching
                        // bind_call/source_location/hash, and avoids
                        // an extra ancestor-chain walk per side.
                        // Falls through to live lookup, then to
                        // class-ptr identity, so the eql?/hash chain
                        // stays in lock-step.
                        let (ca, na, sa) = self.heap.unbound_method_full(*a);
                        let (cb, nb, sb) = self.heap.unbound_method_full(*b);
                        let ma = sa.or_else(|| self.lookup_method_uncached(&ca, na));
                        let mb = sb.or_else(|| self.lookup_method_uncached(&cb, nb));
                        match (ma, mb) {
                            (Some(x), Some(y)) => Rc::ptr_eq(&x, &y),
                            _ => na == nb && Rc::ptr_eq(&ca, &cb),
                        }
                    }
                    _ => false,
                };
                let result = if &*name == "!=" { !eq } else { eq };
                self.stack.push(Value::Bool(result));
                return Ok(());
            }
        // `Object#==` / `Object#!=` cross-type fallback. The
        // per-type primitive arms (`String == String`,
        // `Sym == Sym`, `Class == Class`, etc.) all fired earlier
        // in this dispatch. Anything that reaches here is a
        // cross-type comparison (`"x" == nil`, `nil == :foo`,
        // `[] == ""`) — those return `false` in CRuby, not
        // NoMethodError. Same-type comparisons that we don't
        // have per-type arms for (e.g. `Array == Array`) get
        // value-equality via `ruby_eq`. Universal fallback —
        // never raises — so it must go before NoMethodError.
        if args.len() == 1 && (&*name == "==" || &*name == "!=") {
            let eq = recv.ruby_eq(&args[0], &self.heap);
            let result = if &*name == "==" { eq } else { !eq };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `===` case-equality. Used by `case/when` desugaring.
        // Per-type semantics:
        //   Range#=== → include? (numeric containment)
        //   Class#=== → instance-of (walks ancestor chain)
        //   everything else → `==` value equality
        // User classes can override `===` via class-method
        // lookup, which fires above this fallback (no shadowing
        // needed since the universal check is the last resort).
        if &*name == "===" && args.len() == 1 {
            let arg = &args[0];
            let result = match &recv {
                Value::Range(rid) => {
                    // Generic numeric containment: coerce both
                    // bounds and the arg to Float so Int/Float
                    // mixes (5 in 1..10, 5.0 in 0..10, 5 in 0.0..10.0)
                    // all work. Strings / Symbols compare
                    // lexicographically — handled below.
                    let r = self.heap.range(*rid);
                    #[cfg(feature = "bignum")]
                    let to_f64 = |v: &Value| -> Option<f64> {
                        match v {
                            Value::Int(n) => Some(*n as f64),
                            Value::Float(f) => Some(*f),
                            // BigInt-to-f64 via the decimal-string
                            // round-trip — adequate for the
                            // include?/cover? containment check
                            // (Float comparison is already lossy),
                            // and avoids importing a `ToPrimitive`
                            // trait for one use. Without this arm a
                            // BigInt-bounded range fails the to_f64
                            // pass and falls into the lex fallback,
                            // which also lacked BigInt support.
                            Value::BigInt(id) => self.heap.bigint(*id).to_string().parse::<f64>().ok(),
                            _ => None,
                        }
                    };
                    #[cfg(not(feature = "bignum"))]
                    let to_f64 = |v: &Value| -> Option<f64> {
                        match v {
                            Value::Int(n) => Some(*n as f64),
                            Value::Float(f) => Some(*f),
                            _ => None,
                        }
                    };
                    let excl = r.exclusive;
                    
                    match (to_f64(&r.begin), to_f64(&r.end), to_f64(arg)) {
                        (Some(b), Some(e), Some(v)) => {
                            if excl { v >= b && v < e }
                            else { v >= b && v <= e }
                        }
                        _ => {
                            // Non-numeric: fall back to lexicographic
                            // compare using value_cmp_v if both bounds
                            // and the arg are the same comparable type.
                            let b = &r.begin; let e = &r.end;
                            let ge_lo = value_cmp_v_heap(arg, b, &self.interner, &self.heap)
                                .map(|o| o != std::cmp::Ordering::Less)
                                .unwrap_or(false);
                            let cmp_hi = value_cmp_v_heap(arg, e, &self.interner, &self.heap);
                            let le_hi = match cmp_hi {
                                Some(o) => if excl { o == std::cmp::Ordering::Less }
                                           else { o != std::cmp::Ordering::Greater },
                                None => false,
                            };
                            ge_lo && le_hi
                        }
                    }
                }
                Value::Class(target) => {
                    // Walk the argument's class chain looking for
                    // an Rc-identical match with `target`. For
                    // built-in receivers, look up the stub class
                    // by interned type name.
                    let start: Option<Rc<Class>> = match arg {
                        Value::Object(id) => Some(self.heap.class_of(*id)),
                        _ => {
                            let class_val = self.class_of(arg);
                            if let Value::Class(c) = class_val { Some(c) } else { None }
                        }
                    };
                    let mut cur = start;
                    let mut hit = false;
                    while let Some(cls) = cur {
                        if Rc::ptr_eq(&cls, target) { hit = true; break; }
                        cur = cls.superclass.borrow().clone();
                    }
                    hit
                }
                #[cfg(feature = "regex")]
                Value::Regex(re) => match arg {
                    // CRuby: `Regexp#===` (used by `case/when`) sets
                    // `$~`/`$1`.. on hit and clears them on miss,
                    // just like `=~`/`String#match`. Switch from
                    // `is_match` to `captures` so the side-channel
                    // sees the same view through every entry point.
                    // Keep `with_str_lossy` for the miss path's
                    // zero-alloc happy case (a String whose bytes
                    // are already valid UTF-8 borrows through the
                    // closure without allocating). Only materialize
                    // an owned `input` String inside the Some arm.
                    //
                    // Layer #17: capture extraction not yet
                    // dual-engine; trap on fancy patterns until
                    // the migration lands.
                    Value::Str(s) => {
                        let native = re.as_native().ok_or_else(|| self.trap(RubyError::RuntimeError {
                            msg: format!(
                                "regex op 'Regexp#===' is not yet supported on patterns requiring the fancy-regex engine (pattern: /{}/)",
                                re.as_str(),
                            ),
                        }))?;
                        s.with_str_lossy(|input| match native.captures(input) {
                        Some(caps) => {
                            let m0 = caps.get(0).unwrap();
                            let (m_start, m_end) = (m0.start(), m0.end());
                            let whole = m0.as_str().to_string();
                            let last_caps: Vec<Option<String>> = (1..caps.len())
                                .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                                .collect();
                            let named: Vec<(String, Option<String>)> = native
                                .capture_names()
                                .enumerate()
                                .filter_map(|(i, n)| {
                                    n.map(|name| (name.to_string(), caps.get(i).map(|m| m.as_str().to_string())))
                                })
                                .collect();
                            self.save_match_scope_on_write();
                            self.last_match = Some(crate::vm::LastMatch {
                                whole,
                                caps: last_caps,
                                input: input.to_string(),
                                m_start,
                                m_end,
                                named,
                            });
                            true
                        }
                        None => {
                            self.save_match_scope_on_write();
                            self.last_match = None;
                            false
                        }
                    })
                    },
                    _ => false,
                },
                _ => recv.ruby_eq(arg, &self.heap),
            };
            self.stack.push(Value::Bool(result));
            return Ok(());
        }
        // `Regexp#match(str)` — symmetric with `String#match(regex)`.
        // Returns a MatchData (setting `$~`) or nil. A nil arg is a
        // no-match (CRuby returns nil and clears `$~`). Discovery: P3
        // Jekyll spike — kramdown's header parser does
        // `HEADER_ID.match(text)`.
        #[cfg(feature = "regex")]
        if &*name == "match" && args.len() == 1
            && let Value::Regex(re) = &recv
        {
            let result = match &args[0] {
                Value::Str(s) => {
                    let re = re.clone();
                    let bound = s.to_string_lossy();
                    self.do_regexp_match(&re, bound)?
                }
                Value::Nil => {
                    self.save_match_scope_on_write();
                    self.last_match = None;
                    Value::Nil
                }
                other => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into String",
                            other.type_name()
                        ),
                    }));
                }
            };
            self.stack.push(result);
            return Ok(());
        }
        // `=~` — Regex/String matching. Returns the byte offset of
        // the first match, or nil. On a hit, populate `last_match`
        // (with captures) so `$~` and `$1`..`$N` (any positive
        // index — multi-digit forms like `$10` work too) see the
        // same match; on a miss, clear it (CRuby parity — a failed
        // `=~` wipes the prior match's globals).
        // `!~` is `!(self =~ other)` — shares the match logic below
        // (it still sets `$~` via the same path) but yields a boolean:
        // true when there's NO match. Discovery: P3 Jekyll spike —
        // kramdown's block parser uses `str !~ /pat/`.
        if (&*name == "=~" || &*name == "!~") && args.len() == 1 {
            let result = match (&recv, &args[0]) {
                #[cfg(feature = "regex")]
                (Value::Regex(re), Value::Str(s)) | (Value::Str(s), Value::Regex(re)) => {
                    let bound = s.to_string_lossy();
                    // Engine-agnostic — handles both the linear and
                    // fancy-regex backends (Mustermann's `/\A...\Z/`
                    // routes force fancy). Fancy errors only on a
                    // match-time blow-up; surface as a trap.
                    let owned = re.captures_owned(&bound).map_err(|e| {
                        self.trap(RubyError::RuntimeError {
                            msg: format!("regex match failed: {} (pattern: /{}/)", e, re.as_str()),
                        })
                    })?;
                    match owned {
                        Some(oc) => {
                            let m_start = oc.m_start;
                            // `=~` returns a CHARACTER index (CRuby),
                            // consistent with String#index — not the regex
                            // engine's byte offset, which diverges on
                            // multibyte input and corrupts StringScanner's
                            // pre_match/post_match (they add this offset to
                            // a char-based position). The byte `m_start` is
                            // still stored for internal pre/post slicing.
                            let char_idx = bound[..m_start].chars().count() as i64;
                            self.save_match_scope_on_write();
                            self.last_match = Some(crate::vm::LastMatch {
                                whole: oc.whole,
                                caps: oc.groups,
                                input: bound,
                                m_start,
                                m_end: oc.m_end,
                                named: oc.named,
                            });
                            Value::Int(char_idx)
                        }
                        None => {
                            self.save_match_scope_on_write();
                            self.last_match = None;
                            Value::Nil
                        }
                    }
                }
                _ => Value::Nil,
            };
            if &*name == "!~" {
                // No match (`result` is Nil) → true.
                self.stack.push(Value::Bool(matches!(result, Value::Nil)));
            } else {
                self.stack.push(result);
            }
            return Ok(());
        }
        // `Object#<=>` fallback for `Value::Object` receivers. The
        // per-type primitive_call arms above handle every built-in
        // lhs (Int / Float / Str / Bool / Nil — Sym lives in
        // sym_primitive). When we reach here on `<=>`, the only
        // remaining lhs shape is `Value::Object` whose class
        // didn't define `<=>`. CRuby's default `Object#<=>`
        // returns `0` if the two values are identical (in our
        // model: same `ObjId`) and `nil` otherwise. User-defined
        // `<=>` on a class already fired via class-method-lookup
        // earlier, so we don't shadow.
        if &*name == "<=>" && args.len() == 1
            && !matches!(&recv, Value::Rational(_))
        {
            // Phase C.2 — `Int <=> Rational` and `Float <=> Rational`
            // are computed DIRECTLY here (no inversion): for Int
            // recv we cross-multiply as `n*den <=> num`; for Float
            // recv we demote the Rational to f64 and use
            // `f.partial_cmp(&o_f)`. Lives in dispatch (not
            // primitive_call) because the Rational cross-multiply
            // needs heap access. The primitive_call arms are now
            // gated to fall through when rhs is Rational.
            if let Value::Rational(oid) = &args[0] {
                let o = self.heap.rational(*oid).clone();
                let result = match &recv {
                    Value::Int(n) => {
                        // n <=> o ⇔ -(o <=> n)
                        crate::heap::rational_cmp_other(&o, &Value::Int(*n), &self.heap)
                            .map(|ord| ord.reverse())
                    }
                    Value::Float(f) => {
                        let o_f = crate::heap::rational_to_f64(&o);
                        f.partial_cmp(&o_f)
                    }
                    _ => None,
                };
                let v = match result {
                    Some(std::cmp::Ordering::Less) => Value::Int(-1),
                    Some(std::cmp::Ordering::Equal) => Value::Int(0),
                    Some(std::cmp::Ordering::Greater) => Value::Int(1),
                    None => Value::Nil,
                };
                self.stack.push(v);
                return Ok(());
            }
            // Rational has its own `<=>` arm (cross-multiply against
            // Int / Float / Rational) further below; the universal
            // Object#<=> would otherwise shadow it with Nil.
            let result = match (&recv, &args[0]) {
                (Value::Object(a), Value::Object(b)) if a == b => Value::Int(0),
                _ => Value::Nil,
            };
            self.stack.push(result);
            return Ok(());
        }
        // `Object#class` — universal, no args. Returns the Class
        // associated with the receiver. For built-in types it's
        // the stub class registered by the preamble; for user
        // instances it's the instance's stored class.
        if &*name == "class" && args.is_empty() {
            let c = self.class_of(&recv);
            self.stack.push(c);
            return Ok(());
        }
        // `Object#object_id` / `BasicObject#__id__` — universal,
        // no args. Delegates to `object_id_for` (defined at the
        // bottom of this file). The encoding contract:
        //   - CRuby-exact for nil/true/false/Int (4 / 20 / 0 /
        //     `n*2+1`).
        //   - High-bit type discriminators for everything else
        //     (bit 62 = heap, 61 = Sym, 60 = Float). These bit
        //     positions are unreachable by `n*2+1` for any
        //     practical integer literal (`|n| < 2^58`), so
        //     cross-type collisions are eliminated by
        //     construction.
        //   - 4-bit type subtag at bits 58..61 distinguishes
        //     heap variants (Object vs Array vs Hash etc.),
        //     leaving a 58-bit payload that fits both u32
        //     ObjId and 48-bit virtual pointers natively.
        if (&*name == "object_id" || &*name == "__id__") && args.is_empty() {
            let id = object_id_for(&recv);
            self.stack.push(Value::Int(id));
            return Ok(());
        }
        // Arity guard for the Object-extras family. All four
        // take zero arguments; CRuby raises ArgumentError on
        // extra args regardless of whether a block is present,
        // so check before the per-method arms to keep the
        // error type consistent. Without this guard
        // `42.tap(1)` falls through to NoMethodError, hiding
        // the real mistake.
        if matches!(&*name, "itself" | "tap" | "then" | "yield_self") && !args.is_empty() {
            return Err(self.trap(crate::error::RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len()
                ),
            }));
        }
        // `Object#itself` — universal, no args. Returns the
        // receiver unchanged. Common with `group_by(&:itself)`
        // and other Symbol#to_proc idioms. CRuby ignores any
        // attached block (`obj.itself { ... }` still returns
        // obj); see the block-form fast path in
        // `collection_call_block` (vm/iter.rs) for that case.
        if &*name == "itself" && args.is_empty() {
            self.stack.push(recv);
            return Ok(());
        }
        // `obj.define_singleton_method(...)` without a block —
        // mirror the block-form arm's arity / type validation
        // so the user sees an ArgumentError / TypeError instead
        // of NoMethodError. Gated on the same receiver shapes
        // the block-form arm actually supports (Value::Object
        // and Value::Class); other receivers fall through to
        // the normal NoMethodError path (matching what CRuby
        // does at the TypeError "can't define singleton"
        // surface — close enough for primitives that don't
        // accept the install at all).
        if &*name == "define_singleton_method"
            && matches!(&recv, Value::Object(_) | Value::Class(_))
        {
            match args.len() {
                0 => return Err(self.trap(RubyError::ArgumentError {
                    msg: "wrong number of arguments (given 0, expected 1..2)".into(),
                })),
                1 => {
                    // Validate the name argument so callers get
                    // TypeError on a non-Symbol/String name even
                    // without a block. CRuby validates name
                    // before complaining about the missing block.
                    match &args[0] {
                        Value::Sym(_) | Value::Str(_) => {}
                        other => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Symbol or String)",
                                other.type_name(),
                            ),
                        })),
                    }
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: "tried to create Proc object without a block".into(),
                    }));
                }
                2 => {
                    // 2-arg form: install args[1] (Proc / Method /
                    // UnboundMethod) onto recv's eigenclass or
                    // class-singleton table.
                    let name_sym = match &args[0] {
                        Value::Sym(s) => *s,
                        Value::Str(s) => {
                            let raw = s.to_string_lossy();
                            if let Some(max) = self.max_symbols
                                && !self.interner.contains(&raw) && self.interner.len() >= max {
                                    return Err(self.trap(RubyError::ResourceExhausted {
                                        msg: format!("interner exhausted: {} symbols", max),
                                    }));
                                }
                            self.interner.intern(&raw)
                        }
                        other => return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "wrong argument type {} (expected Symbol or String)",
                                other.type_name(),
                            ),
                        })),
                    };
                    let src = args[1].clone();
                    let installed = match &recv {
                        Value::Object(id) => {
                            // Eigenclass install — methods go on
                            // the synthetic singleton class's
                            // own methods table; install_method
                            // honors that via singleton_target.
                            let sc = self.heap.ensure_singleton_class(*id);
                            self.install_method_from_value(
                                &sc,
                                name_sym,
                                &src,
                                crate::value::Visibility::Public,
                            )
                        }
                        Value::Class(c) => {
                            // Class receiver → install as a
                            // class method (cls.singleton_methods),
                            // matching the block-form arm. The
                            // generic install_method would route
                            // into cls.methods (instance methods)
                            // since the class itself has no
                            // singleton_target set.
                            self.install_singleton_method_on_class_from_value(
                                c, name_sym, &src,
                            )
                        }
                        _ => unreachable!(),
                    }
                    .map_err(|e| self.trap(e))?;
                    self.stack.push(Value::Sym(installed));
                    return Ok(());
                }
                n => return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", n),
                })),
            }
        }
        // `Object#dup` / `Object#clone` — universal shallow
        // copy. Primitive arms in vm/string.rs / vm/array.rs /
        // vm/hash.rs intercept their own receivers earlier in
        // dispatch; this arm catches everything else.
        //
        // Immediates (Int/Float/Sym/Bool/Nil) return self —
        // CRuby's `5.dup`, `nil.dup`, `:foo.dup` all return the
        // receiver unchanged since Ruby 2.4. Plain `Value::Object`
        // gets a fresh Instance with the same class and a
        // shallow-cloned ivar table; the singleton class is NOT
        // copied (CRuby's `dup` discards singleton methods, and
        // `clone` properly copies them — we don't model the
        // copy yet so both arms drop singletons. Documented
        // divergence — Tier-2 follow-up alongside the
        // `clone(freeze:)` kwarg).
        //
        // Arity: zero positional args for `dup`; `clone`
        // accepts a `freeze:` kwarg in CRuby that we don't
        // route yet — extra args fall to the wrong-arity arm
        // below.
        if matches!(&*name, "dup" | "clone") && args.is_empty() {
            let copied = match &recv {
                Value::Int(_)
                | Value::Float(_)
                | Value::Sym(_)
                | Value::Bool(_)
                | Value::Nil => recv.clone(),
                // CRuby treats Integer as immediate-like for
                // dup/clone regardless of Fixnum/Bignum
                // representation — `(10**100).dup.equal?(...)`
                // returns true. We don't have to allocate a
                // fresh heap slot for Bignum; returning the
                // same Value is identity-preserving and matches
                // user expectations.
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => recv.clone(),
                Value::Object(oid) => {
                    let (cls, ivars) = match self.heap.get(*oid) {
                        crate::heap::HeapObj::Instance(inst) => {
                            (inst.class.clone(), inst.ivars.clone())
                        }
                        // TypedData (cext-allocated) carries no
                        // ivar table on the rubyrs side; punt to
                        // the fallback below until a caller
                        // surfaces a need.
                        _ => {
                            return Err(self.trap(RubyError::NoMethodError {
                                kind: crate::error::NoMethodErrorKind::Missing,
                                method: format!("undefined method '{}' called", &*name),
                                recv_type: std::borrow::Cow::Owned(
                                    crate::vm::numeric::class_name_for_error(&recv).to_string(),
                                ),
                            }));
                        }
                    };
                    self.maybe_gc();
                    self.check_alloc()?;
                    let new_id = self.heap.alloc(HeapObj::Instance(crate::value::Instance {
                        class: cls,
                        ivars,
                        singleton_class: None,
            frozen: std::cell::Cell::new(false),
                    }));
                    Value::Object(new_id)
                }
                // Method / UnboundMethod: re-wrap the captured
                // state into a fresh heap slot. CRuby's
                // Method#dup / #clone return a distinct object
                // (`equal?` false) but compare-equal under #==
                // (same recv, same captured Method snapshot).
                Value::BoundMethod(bid) => {
                    let (r, n, snap) = self.heap.bound_method_full(*bid);
                    let r = r.clone();
                    let snap = snap.clone();
                    let mut g = crate::vm::PinGuard::new(self);
                    g.pin(r.clone());
                    g.vm.maybe_gc();
                    g.vm.check_alloc()?;
                    let new_id = g.vm.heap.alloc(HeapObj::BoundMethod {
                        recv: r,
                        name_id: n,
                        method: snap,
                    });
                    Value::BoundMethod(new_id)
                }
                Value::UnboundMethod(uid) => {
                    let (cls, n, snap) = self.heap.unbound_method_full(*uid);
                    self.maybe_gc();
                    self.check_alloc()?;
                    let new_id = self.heap.alloc(HeapObj::UnboundMethod {
                        class: cls,
                        name_id: n,
                        method: snap,
                    });
                    Value::UnboundMethod(new_id)
                }
                // Range/Block/Regex/etc.: no shallow-copy support
                // yet. Surface a clear NoMethodError rather than
                // silently returning self — a future commit can
                // add per-variant copy logic as use cases land.
                _ => {
                    return Err(self.trap(RubyError::NoMethodError {
                        kind: crate::error::NoMethodErrorKind::Missing,
                        method: format!("undefined method '{}' called", &*name),
                        recv_type: std::borrow::Cow::Owned(
                            crate::vm::numeric::class_name_for_error(&recv).to_string(),
                        ),
                    }));
                }
            };
            self.stack.push(copied);
            return Ok(());
        }
        if matches!(&*name, "dup" | "clone") {
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 0)",
                    args.len()
                ),
            }));
        }
        // `Object#tap` / `#then` / `#yield_self` without a
        // block — the block-taking forms are handled by
        // `collection_call_block` (vm/iter.rs). Reaching this
        // arm means no block was passed; CRuby raises
        // LocalJumpError for `tap`, while `then`/`yield_self`
        // would normally return an Enumerator. rubyrs has no
        // Enumerator type yet, so for now both raise
        // LocalJumpError uniformly — documented divergence,
        // less surprising than silent NoMethodError.
        if args.is_empty() && matches!(&*name, "tap" | "then" | "yield_self") {
            return Err(self.trap(crate::error::RubyError::LocalJumpError {
                msg: "no block given (yield)".to_string(),
            }));
        }
        // `Object#frozen?` — universal, no args.
        // CRuby treats all immediates (Integer, Float, Symbol,
        // true, false, nil) as always-frozen. Str/Array/Hash/Regex
        // have their own primitive arms earlier in dispatch and
        // never reach here. For Value::Object (plain user
        // instances) we consult the per-Instance `frozen` Cell
        // installed by `Object#freeze` below; everything else
        // (Class, BoundMethod, Method, Block, ...) returns false.
        if &*name == "frozen?" && args.is_empty() {
            let frozen = match &recv {
                Value::Int(_) | Value::Float(_) | Value::Sym(_)
                    | Value::Bool(_) | Value::Nil => true,
                Value::Object(id) => self.heap.instance(*id).frozen.get(),
                _ => false,
            };
            self.stack.push(Value::Bool(frozen));
            return Ok(());
        }
        // `Object#freeze` — universal, no args. CRuby's freeze is
        // a one-way flag flip: subsequent mutation attempts
        // surface FrozenError. For user-class instances we set
        // the per-Instance `frozen` Cell and return self;
        // immediates / Class / BoundMethod / ... are already
        // immutable from script's perspective, so freeze is a
        // no-op that returns self. Mutation guards on ivar set
        // / singleton install are follow-up — adding the
        // freeze read/write surface is what unblocks gems that
        // call `EmptyMapping.new.freeze` on construction.
        if &*name == "freeze" && args.is_empty() {
            if let Value::Object(id) = &recv {
                self.heap.instance(*id).frozen.set(true);
            }
            self.stack.push(recv);
            return Ok(());
        }
        // `Object#to_s` / `Object#inspect` — universal default.
        // For plain Object instances, CRuby renders as
        // `"#<ClassName:0xADDR>"`. We can't expose real addresses
        // (sandbox), so use the object_id hex form. Primitive
        // arms for Str/Int/Sym/Array/Hash run earlier in dispatch
        // and shadow this, and `Value::Class` is handled by
        // `primitive_call` (vm/primitive.rs). Any receiver type
        // without a specialized `to_s`/`inspect` handler falls
        // through here — that includes plain `Object` instances
        // but also BoundMethod / UnboundMethod / CurriedProc /
        // future heap variants we add without a custom default.
        if (&*name == "to_s" || &*name == "inspect") && args.is_empty() {
            // Range has no primitive to_s/inspect arm of its own.
            // Without this short-circuit the universal
            // `#<Range:0xHEX>` form below would silently win for
            // Range and diverge from CRuby. `to_display` /
            // `to_inspect` in heap.rs already render Range with
            // the correct endpoint-quoting and endless/beginless
            // handling — funnel through them so the Array#inspect
            // path (which also calls `to_inspect`) stays
            // consistent.
            if matches!(&recv, Value::Range(_) | Value::Rational(_)) {
                // Range and Rational both render via
                // `to_display`/`to_inspect` — Rational#to_s is
                // `"num/den"`, #inspect is `"(num/den)"`. Without
                // this short-circuit the universal `#<Class:0xHEX>`
                // fallback wins and rendering diverges from CRuby.
                let rendered = if &*name == "inspect" {
                    recv.to_inspect(&self.heap, &self.interner)
                } else {
                    recv.to_display(&self.heap, &self.interner)
                };
                self.stack.push(Value::new_str(rendered));
                return Ok(());
            }
            // BoundMethod / UnboundMethod: render
            //   `#<Method: RecvClass#name(params) filename:line>`
            //   `#<Method: RecvClass(DefiningClass)#name(params) filename:line>`
            //   `#<UnboundMethod: DefiningClass#name(params) filename:line>`
            // mirroring CRuby's form, including the trailing
            // ` filename:line` source-location suffix produced by
            // `method_source_suffix`. Without this short-circuit
            // the universal `#<Method:0xHEX>` fallback wins,
            // losing the receiver/owner class and method name
            // that defensive logging idioms rely on.
            if let Value::BoundMethod(bid) = &recv {
                let (recv_v, name_id, params, defining_rc, snap_for_src) = {
                    let (rv, nid, snap) = self.heap.bound_method_full(*bid);
                    let params = snap
                        .as_ref()
                        .map(|m| format_method_params(&self.protos[m.proto_idx]))
                        .unwrap_or_default();
                    let defining_rc = snap
                        .as_ref()
                        .and_then(|m| m.defining_class.as_ref())
                        .and_then(|w| w.upgrade());
                    let snap_clone = snap.clone();
                    (rv.clone(), nid, params, defining_rc, snap_clone)
                };
                // Reuse the snapshot-or-live-lookup pattern
                // Method#source_location uses: when no snapshot
                // was stored, resolve the method against the
                // receiver's class so the suffix still appears.
                // We use
                // `heap.class_of` here (eigenclass-aware) to
                // match the capture path at the `method` arm
                // — `source_location` uses `Vm::class_of`
                // (real class, skips singletons), so the two
                // can diverge on snapshot-less singleton
                // methods; this path errs on the side of
                // finding the method that the BoundMethod was
                // originally captured against.
                let src_suffix = {
                    let m = snap_for_src.or_else(|| match &recv_v {
                        Value::Object(id) => {
                            let cls = self.heap.class_of(*id);
                            self.lookup_method_uncached(&cls, name_id)
                        }
                        _ => match self.class_of(&recv_v) {
                            Value::Class(cls) => self.lookup_method_uncached(&cls, name_id),
                            _ => None,
                        },
                    });
                    m.map(|m| method_source_suffix(&m, &self.protos, &self.sources))
                        .unwrap_or_default()
                };
                let method_name = self.interner.resolve(name_id).to_string();
                // Singleton methods (`def obj.foo`): defining
                // class IS the receiver's eigenclass shell. CRuby
                // renders these as `#<RecvClass:0xHEX>.foo(...)`
                // with a `.` separator instead of `#`. Detect by
                // ptr-eq: `class_of(obj_id)` returns the eigenclass
                // when one is installed, so if it matches the
                // method's defining_class we're looking at a
                // singleton method.
                // Singleton iff: receiver has an eigenclass
                // installed AND defining_class IS that
                // eigenclass. The first conjunct distinguishes
                // singleton methods from regular methods —
                // without it, every method on a singleton-less
                // object would also satisfy
                // `class_of == defining_class`.
                let is_singleton = match (&recv_v, &defining_rc) {
                    (Value::Object(id), Some(def)) => {
                        let cls = self.heap.class_of(*id);
                        let real = self.heap.real_class_of(*id);
                        !std::rc::Rc::ptr_eq(&cls, &real)
                            && std::rc::Rc::ptr_eq(&cls, def)
                    }
                    _ => false,
                };
                let s = if is_singleton {
                    // `#<Method: #<A:0xHEX>.foo(params)>` — receiver
                    // rendered as its real class (skip the
                    // eigenclass) plus a stable hex identity.
                    let real_class = match &recv_v {
                        Value::Object(id) => self.heap.real_class_of(*id).name.clone(),
                        _ => "Object".to_string(),
                    };
                    let oid = object_id_for(&recv_v);
                    format!(
                        "#<Method: #<{}:0x{:016x}>.{}({}){}>",
                        real_class, oid, method_name, params, src_suffix
                    )
                } else {
                    let recv_class = match self.class_of(&recv_v) {
                        Value::Class(c) => c.name.clone(),
                        _ => "Object".to_string(),
                    };
                    let defining_name = defining_rc.map(|c| c.name.clone());
                    let class_part = match defining_name {
                        Some(d) if d != recv_class => format!("{}({})", recv_class, d),
                        _ => recv_class,
                    };
                    format!("#<Method: {}#{}({}){}>", class_part, method_name, params, src_suffix)
                };
                self.stack.push(Value::new_str(s));
                return Ok(());
            }
            if let Value::UnboundMethod(uid) = &recv {
                let (class_name, name_id, params, src_suffix) = {
                    let (cls, nid, snap) = self.heap.unbound_method_full(*uid);
                    let params = snap
                        .as_ref()
                        .map(|m| format_method_params(&self.protos[m.proto_idx]))
                        .unwrap_or_default();
                    // CRuby prints the class where the method was
                    // *defined*, not the class it was captured on:
                    // `B.instance_method(:foo).inspect` shows
                    // `A#foo` when foo is inherited from A. Fall
                    // back to the captured class when the snap is
                    // absent or the Weak ref has been collected.
                    let defining = snap
                        .as_ref()
                        .and_then(|m| m.defining_class.as_ref())
                        .and_then(|w| w.upgrade())
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| cls.name.clone());
                    // Mirror Method#source_location: live-lookup
                    // fallback against the captured class when
                    // no snapshot was stored, so inspect's
                    // suffix stays consistent with
                    // source_location.
                    let m_for_src = snap.clone()
                        .or_else(|| self.lookup_method_uncached(&cls, nid));
                    let src_suffix = m_for_src
                        .map(|m| method_source_suffix(&m, &self.protos, &self.sources))
                        .unwrap_or_default();
                    (defining, nid, params, src_suffix)
                };
                let method_name = self.interner.resolve(name_id).to_string();
                let s = format!(
                    "#<UnboundMethod: {}#{}({}){}>",
                    class_name, method_name, params, src_suffix
                );
                self.stack.push(Value::new_str(s));
                return Ok(());
            }
            let cls_rc = match self.class_of(&recv) {
                Value::Class(c) => Some(c),
                _ => None,
            };
            let cls_name = cls_rc.as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "Object".to_string());
            // Tier-1 2a: Exception subclasses render as
            // `#<ClassName: message>` (or bare `#<ClassName>`
            // when message is empty / matches the class name —
            // matches CRuby's default Exception#inspect).
            // Receiver must be a real heap instance with an
            // `@message` ivar; primitive types fall through to
            // the universal hex form.
            // Exception subclasses render as `#<ClassName: message>` (or
            // bare `#<ClassName>` when the message is empty), shared with
            // `stringify_for_output` so `p exc` and `exc.inspect` agree.
            if let Some(s) = self.exception_inspect_string(&recv) {
                self.stack.push(Value::new_str(s));
                return Ok(());
            }
            let oid = object_id_for(&recv);
            let s = format!("#<{}:0x{:016x}>", cls_name, oid);
            self.stack.push(Value::new_str(s));
            return Ok(());
        }
        // Phase C.1 Rational readers / conversions. Lives here in
        // dispatch.rs (not primitive_call) because the stateless
        // primitive layer can't read the heap-stored RationalRepr.
        // Arithmetic + comparison whitelist expansion lands in
        // Phase C.2.
        if let Value::Rational(id) = &recv {
            let r = self.heap.rational(*id).clone();
            match (&*name, args.len()) {
                ("numerator", 0) => {
                    #[cfg(feature = "bignum")]
                    {
                        let v = self.bigint_to_value(r.num)?;
                        self.stack.push(v);
                    }
                    #[cfg(not(feature = "bignum"))]
                    self.stack.push(Value::Int(r.num));
                    return Ok(());
                }
                ("denominator", 0) => {
                    #[cfg(feature = "bignum")]
                    {
                        let v = self.bigint_to_value(r.den)?;
                        self.stack.push(v);
                    }
                    #[cfg(not(feature = "bignum"))]
                    self.stack.push(Value::Int(r.den));
                    return Ok(());
                }
                ("to_r", 0) => {
                    self.stack.push(recv.clone());
                    return Ok(());
                }
                ("to_i", 0) => {
                    // CRuby `to_i` / `to_int` for Rational truncates
                    // toward zero (NOT floor). `(7/2r).to_i == 3`,
                    // `(-7/2r).to_i == -3`. BigInt `/` is already
                    // truncating-toward-zero (num_bigint matches Rust).
                    #[cfg(feature = "bignum")]
                    {
                        let v = self.bigint_to_value(&r.num / &r.den)?;
                        self.stack.push(v);
                    }
                    #[cfg(not(feature = "bignum"))]
                    self.stack.push(Value::Int(r.num / r.den));
                    return Ok(());
                }
                ("to_f", 0) => {
                    self.stack.push(Value::Float(crate::heap::rational_to_f64(&r)));
                    return Ok(());
                }
                // Arity guards for the readers — they take no args.
                ("numerator" | "denominator" | "to_r" | "to_i" | "to_f", _) => {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0)",
                            args.len(),
                        ),
                    }));
                }
                // Phase C.2 — method-call form for the binary
                // operators `+ - * /` and the comparisons
                // `< <= > >=` (plus `==` / `!=`; see arm note). The
                // `Op::BinOp` path already wires `try_rational_binop`; this
                // arm catches `r.send(:+, x)` / `r.+ x` (parsed by Prism
                // as a method call rather than Op::BinOp when send is
                // used or when the receiver is a complex expression).
                // `==` / `!=` are NOT listed here — the universal
                // `Object#==` / `Object#!=` arms further up call
                // `Value::ruby_eq`, which carries the canonical
                // Rational cross-type equality (see heap.rs).
                // Routing `r.send(:==, x)` through this arm would
                // be dead code because it's shadowed by the
                // universal dispatch.
                // Phase C.4.4 — `Rational#**` lives in this arm
                // (not via `try_rational_binop` because `**` isn't
                // in `BinOpKind` — power is method-dispatched, not
                // BinOp-opcoded). Integer exp uses `BigInt::pow` on
                // num and den; non-integer exp (Float / Rational)
                // demotes to Float (CRuby parity — exact-exponent
                // Rational pow only stays Rational for integer exp).
                ("**", 1) => {
                    let v = self.rational_pow(&recv, &args[0])?;
                    self.stack.push(v);
                    return Ok(());
                }
                ("+" | "-" | "*" | "/" | "<" | "<=" | ">" | ">=", 1) => {
                    let kind = crate::bytecode::BinOpKind::from_op_name(&name)
                        .expect("name matched above");
                    if let Some(v) = self.try_rational_binop(kind, &recv, &args[0])? {
                        self.stack.push(v);
                        return Ok(());
                    }
                    // Non-numeric arg → TypeError matching CRuby.
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "{} can't be coerced into Rational",
                            crate::vm::numeric::type_name_for_coerce(&args[0]),
                        ),
                    }));
                }
                // `Rational#eql?(other)` — CRuby's numeric
                // strictness: only true when `other` is also a
                // Rational AND structurally equal. The universal
                // `Object#eql?` arm further up calls `ruby_eq`,
                // which after Phase C.2 treats `Rational == Int|Float`
                // as true — so without this arm, `Rational(1, 1)
                // .eql?(1)` returned true, breaking Hash#uniq /
                // Array#uniq / Set semantics. Mirrors the existing
                // `(Int, "eql?")` / `(Float, "eql?")` strict arms
                // in numeric.rs. Same-Rational case routes through
                // ruby_eq's canonical (num, den) compare.
                ("eql?", 1) => {
                    let same = matches!(&args[0], Value::Rational(_))
                        && recv.ruby_eq(&args[0], &self.heap);
                    self.stack.push(Value::Bool(same));
                    return Ok(());
                }
                ("eql?", _) => {
                    // Arity guard. The universal Object#eql? arm
                    // is gated out for Rational receivers (see
                    // dispatch.rs:5109), so without this arm a
                    // wrong-arity call would surface NoMethodError
                    // instead of CRuby's ArgumentError.
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1)",
                            args.len(),
                        ),
                    }));
                }
                // `Rational#<=>(other)` — there is no BinOpKind for
                // <=>, so it always reaches this method-call arm.
                // Cross-multiply with Int/Float/Rational; returns
                // -1/0/1 (Int) or nil for non-numeric.
                ("<=>", 1) => {
                    let other = &args[0];
                    let result = crate::heap::rational_cmp_other(&r, other, &self.heap);
                    let v = match result {
                        Some(std::cmp::Ordering::Less) => Value::Int(-1),
                        Some(std::cmp::Ordering::Equal) => Value::Int(0),
                        Some(std::cmp::Ordering::Greater) => Value::Int(1),
                        None => Value::Nil,
                    };
                    self.stack.push(v);
                    return Ok(());
                }
                // Arity guard for the binary operators.
                ("+" | "-" | "*" | "/" | "**" | "<" | "<=" | ">" | ">=" | "<=>", _) => {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 1)",
                            args.len(),
                        ),
                    }));
                }
                _ => {}
            }
        }
        // `Object#hash` — universal, no args. Returns an integer
        // hash. For value types (Int/Str/Sym/Bool/Nil), hash by
        // content so `{1 => :a}[1] == :a` works. For heap objects
        // where equality is identity, hash by object_id.
        if &*name == "hash" && args.is_empty() {
            // Single source of truth — `object_hash` handles all
            // per-variant salt and recursive container hashing
            // (Array order-sensitive, Hash order-insensitive)
            // with cycle detection. See its doc for the type-tag
            // table.
            let v = object_hash(&recv, &self.heap);
            self.stack.push(Value::Int(v));
            return Ok(());
        }
        // `Object#respond_to?(name)` — pure feature detection, no
        // invocation. Goes last so user classes that override
        // `respond_to?` (we don't support that yet, but conceptually)
        // would shadow this. Accepts either a `Symbol` or a `String`
        // argument; anything else falls through to NoMethodError.
        // `respond_to?(:foo)` or `respond_to?(:foo, include_private)`.
        // CRuby's second arg toggles whether private methods count;
        // we don't enforce method visibility precisely in the lookup
        // path used here, so the bool is effectively ignored — the
        // check passes through to `responds_to` which already walks
        // the method table without filtering by visibility. Accepting
        // the 2-arg form lets feature-detection patterns like
        // `respond_to?(:deprecate_constant, true)` work without
        // tripping NoMethodError.
        if &*name == "respond_to?" {
            // Arity check matches the no-recv path: CRuby raises
            // ArgumentError on 0 args or 3+. Keeps the explicit-
            // receiver shape (`obj.respond_to?()`) from misreporting
            // as method_missing / NoMethodError.
            if args.is_empty() || args.len() > 2 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", args.len()),
                }));
            }
            // Type check matches the no-recv arm — CRuby raises
            // `TypeError: X is not a symbol nor a string` for any
            // other arg[0] type, before reaching method_missing.
            let lookup_name: SymId = match &args[0] {
                Value::Sym(id) => *id,
                Value::Str(s) => self.interner.intern(&s.to_string_lossy()),
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "{} is not a symbol nor a string",
                        other.to_inspect(&self.heap, &self.interner),
                    ),
                })),
            };
            let include_private = matches!(args.get(1), Some(Value::Bool(true)));
            if self.responds_to(&recv, lookup_name, include_private) {
                self.stack.push(Value::Bool(true));
                return Ok(());
            }
            // Normal resolution missed — consult `respond_to_missing?`.
            if self.try_respond_to_missing(&recv, lookup_name, include_private)? {
                return Ok(());
            }
            self.stack.push(Value::Bool(false));
            return Ok(());
        }
        if self.try_method_missing(&recv, name_id, args.to_vec(), None)? {
            return Ok(());
        }
        // Kernel module-function fallback: CRuby's `Kernel#Array`,
        // `Kernel#Integer`, `Kernel#Float`, `Kernel#String`,
        // `Kernel#sprintf`, `Kernel#format` are private instance
        // methods on Kernel (included in Object). With an
        // explicit receiver CRuby raises NoMethodError-private,
        // which lets `method_missing` intercept; only if NO
        // `method_missing` is defined does the call actually
        // surface as NoMethodError. We model the latter half
        // here: when normal lookup AND method_missing miss, route
        // to `builtin_call`. This sits AFTER `try_method_missing`
        // so a user `method_missing` wins (matches CRuby), and
        // before NoMethodError so sinatra's
        // `codes.flat_map(&method(:Array))` shape (sinatra/base.rb
        // :1404) — `method(:Array)` captures, `.call` re-dispatches
        // through here with no user method_missing — succeeds.
        // (TRY_RUNS layer #25.)
        //
        // `eval` is intentionally NOT in this set: with-recv
        // `obj.eval(...)` would silently discard the receiver
        // (Kernel#eval ignores it), which is surprise-driven.
        // CRuby raises NoMethodError-private here. The
        // `method(:eval).call(src)` route still works via the
        // no_recv `builtin_call` at the top of do_call.
        // (code-review #267 #3.)
        if matches!(name.as_ref(),
            "Array" | "Integer" | "Float" | "String"
            | "sprintf" | "format"
        ) && let Some(res) = self.builtin_call(name.as_ref(), &args) {
            let v = res?;
            // Mirror the flag handling in the no_recv builtin
            // path (line 452-459): clears
            // `suppress_call_result_push` if set; unconditionally
            // pushing would corrupt the rescue handler's stack
            // (Copilot review #267 round 1).
            if self.suppress_call_result_push {
                self.suppress_call_result_push = false;
            } else {
                self.stack.push(v);
            }
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            kind: crate::error::NoMethodErrorKind::Missing,
            method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&recv)),
        }))
    }



    pub(crate) fn invoke_method(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>) -> Result<(), Trap> {
        self.invoke_method_with_block(m, self_val, args, None)
    }

    /// Fire `Module.included(target)` / `Module.prepended(target)` /
    /// `Module.extended(target)` for each `src` module passed to the
    /// corresponding `include` / `prepend` / `extend` call. CRuby's
    /// contract:
    ///   - hook receiver is the module being mixed in (`src`), not
    ///     the target
    ///   - hook is called with `target` as its single argument; the
    ///     target is a `Value::Class` for `include` / `prepend` /
    ///     `Class.extend` and a `Value::Object` for `Object#extend`,
    ///     so the parameter is the open `Value` enum rather than a
    ///     `Class`-specific shape
    ///   - return value is discarded (hook runs for side effects)
    ///   - hook fires on EVERY `include`/`prepend`/`extend` call,
    ///     including idempotent re-mixes where the chain mutation
    ///     is a no-op; callers populate `sources` accordingly (do
    ///     not gate on `class_is_a`)
    ///   - `hook_name` is the Symbol the caller wants to invoke —
    ///     `"included"`, `"prepended"`, or `"extended"` — chosen at
    ///     the dispatch arm based on which keyword the user wrote
    ///
    /// Fast-path: if the hook name has never been interned no user
    /// code can have defined an override, so we skip the lookup
    /// entirely (mirroring the `Class.inherited` fast-path in
    /// step.rs). Lookup uses `lookup_class_singleton_method` so a
    /// `def self.included(base)` (or `def self.prepended(base)` /
    /// `def self.extended(base)`) defined on `src` or any of its
    /// singleton ancestors fires; a generic
    /// `class Module; def included(base); end; end` monkey-patch
    /// won't reach here — same divergence as the `inherited` hook.
    pub(crate) fn fire_inclusion_hooks(
        &mut self,
        sources: &[std::rc::Rc<crate::value::Class>],
        target: &Value,
        hook_name: &str,
    ) -> Result<(), Trap> {
        if sources.is_empty() || !self.interner.contains(hook_name) {
            return Ok(());
        }
        let hook_id = self.interner.intern(hook_name);
        for src in sources {
            if let Some(m) = self.lookup_class_singleton_method(src, hook_id) {
                let pre_frames = self.frames.len();
                self.invoke_method(
                    m,
                    Value::Class(src.clone()),
                    vec![target.clone()],
                )?;
                self.dispatch_until(pre_frames)?;
                self.stack.pop();
            }
        }
        Ok(())
    }

    /// Fire one of the `Module#method_added` / `method_removed` /
    /// `method_undefined` lifecycle hooks on `cls`. CRuby calls
    /// these whenever an instance method is installed, removed, or
    /// undefined on a class/module — Rails / RSpec / many DSLs use
    /// `method_added` to auto-wrap freshly-defined methods (e.g.
    /// validation chains, instrumentation).
    ///
    /// Contract:
    ///   - hook receiver is `cls` (the class/module being modified)
    ///   - hook is called with `Value::Sym(method_name_id)` as its
    ///     single argument
    ///   - return value is discarded (hook runs for side effects)
    ///   - fast-path: if the hook name has never been interned no
    ///     override can exist, so we skip the lookup entirely
    ///     (mirrors the `Class.inherited` / `fire_inclusion_hooks`
    ///     fast-path)
    ///   - lookup uses `lookup_class_singleton_method` so a
    ///     `def self.method_added(name)` defined on `cls` or any
    ///     of its singleton ancestors fires; a generic
    ///     `class Module; def method_added(...); end; end`
    ///     monkey-patch won't reach here — same divergence as the
    ///     `inherited` hook.
    pub(crate) fn fire_method_lifecycle_hook(
        &mut self,
        cls: &std::rc::Rc<crate::value::Class>,
        hook_name: &str,
        method_name_id: crate::intern::SymId,
    ) -> Result<(), Trap> {
        if !self.interner.contains(hook_name) {
            return Ok(());
        }
        let hook_id = self.interner.intern(hook_name);
        if let Some(m) = self.lookup_class_singleton_method(cls, hook_id) {
            let pre_frames = self.frames.len();
            self.invoke_method(
                m,
                Value::Class(cls.clone()),
                vec![Value::Sym(method_name_id)],
            )?;
            self.dispatch_until(pre_frames)?;
            self.stack.pop();
        }
        Ok(())
    }

    /// Fire a singleton-method lifecycle hook named `hook_name` on
    /// `receiver`. CRuby parity: this is the singleton-method twin of
    /// `Module#method_added`. Rails / RSpec / many DSLs hook
    /// `singleton_method_added` to auto-wrap class methods.
    ///
    /// Coverage is intentionally partial, not "every eigenclass
    /// mutation". Only the dedicated `singleton_method_added` install
    /// entry points are wired (see callers): `def self.foo` /
    /// `def obj.foo`, their block forms, and `define_singleton_method`.
    /// Singleton installs that go through other features do NOT yet
    /// fire it — notably `module_function`'s singleton copy and
    /// `alias_method` writes into `singleton_methods`. Wiring those,
    /// plus the `_removed`/`_undefined` siblings (the helper is
    /// name-generic and can ride the same lookup rule), is follow-up
    /// work.
    ///
    /// Contract:
    ///   - hook receiver is `receiver` (the object/class whose
    ///     singleton class was modified)
    ///   - hook is called with `Value::Sym(method_name_id)` as its
    ///     single argument
    ///   - return value is discarded (hook runs for side effects)
    ///   - fast-path: skip the lookup entirely when the hook name
    ///     has never been interned
    ///
    /// Lookup rule (CRuby semantics):
    ///   - Value::Class(C): the user-defined hook lives on C's
    ///     singleton chain — `def self.singleton_method_added(name)`
    ///     installs into C's singleton_methods. Use
    ///     `lookup_class_singleton_method`.
    ///   - Value::Object(obj): resolve through `Heap::class_of(obj)`,
    ///     which is the object's eigenclass when one exists and the
    ///     real class otherwise. So the hook fires both for a
    ///     per-object override (`class << obj; def
    ///     singleton_method_added(n); end`) and for a
    ///     `def singleton_method_added(n)` on the real class (which
    ///     covers every instance). Use `lookup_method_uncached`.
    ///   - Other receiver types: no hook (primitives don't carry
    ///     singleton classes in the subset we model).
    pub(crate) fn fire_singleton_method_lifecycle_hook(
        &mut self,
        receiver: Value,
        hook_name: &str,
        method_name_id: crate::intern::SymId,
    ) -> Result<(), Trap> {
        if !self.interner.contains(hook_name) {
            return Ok(());
        }
        let hook_id = self.interner.intern(hook_name);
        let m = match &receiver {
            Value::Class(cls) => self.lookup_class_singleton_method(cls, hook_id),
            Value::Object(oid) => {
                let cls = self.heap.class_of(*oid);
                self.lookup_method_uncached(&cls, hook_id)
            }
            _ => return Ok(()),
        };
        if let Some(m) = m {
            let pre_frames = self.frames.len();
            self.invoke_method(m, receiver, vec![Value::Sym(method_name_id)])?;
            self.dispatch_until(pre_frames)?;
            self.stack.pop();
        }
        Ok(())
    }

    /// Wrap freshly-built frame locals in an `Rc<RefCell<…>>`, reusing a
    /// recycled cell from the pool when one is available so the common
    /// method call avoids a per-call `Rc` allocation. The built `locals`
    /// Vec replaces the pooled cell's (empty) inner Vec.
    fn intern_locals(&mut self, locals: Vec<Value>) -> Rc<RefCell<Vec<Value>>> {
        if let Some(cell) = self.locals_pool.pop() {
            *cell.borrow_mut() = locals;
            cell
        } else {
            Rc::new(RefCell::new(locals))
        }
    }

    /// Pool-buffer-reusing twin of `vec_nil` + `intern_locals` for
    /// the method-invocation paths: pops a recycled cell and refills
    /// its retained-capacity buffer with nils. The old shape — fresh
    /// `vec_nil` (a malloc), bind args, then `intern_locals` swapping
    /// the new Vec INTO the pooled cell and dropping the cell's old
    /// buffer (a free) — meant the pool only saved the Rc while the
    /// Vec itself churned malloc/free on every call (visible as the
    /// mi_malloc/mi_free pair in the tight-call-loop profile).
    /// Callers bind args through `cell.borrow_mut()` instead.
    fn locals_cell_nil(&mut self, n: usize) -> Rc<RefCell<Vec<Value>>> {
        if let Some(cell) = self.locals_pool.pop() {
            {
                let mut v = cell.borrow_mut();
                v.clear();
                v.resize(n, Value::Nil);
            }
            cell
        } else {
            Rc::new(RefCell::new(vec_nil(n)))
        }
    }

    /// Build a block invocation's fresh locals cell as a snapshot of
    /// `captured`, grown to `n_locals`, reusing a pooled cell's buffer
    /// when one is available. This is the block-form counterpart to
    /// [`intern_locals`] for the hot `arr.each { … }` / `map` / `select`
    /// loop: instead of `Rc::new(RefCell::new(captured.clone()))` (a
    /// fresh Rc + Vec allocation every element — the #1 leaf cost in the
    /// primitive-block-iteration profile), it pops a recycled cell and
    /// refills its retained-capacity buffer via `extend_from_slice`, so
    /// only the element copy remains; the per-element malloc/free churn
    /// is gone.
    ///
    /// Correctness piggybacks on `recycle_frame_locals`'s
    /// `strong_count == 1` guard: a block whose body creates an escaping
    /// closure leaves `strong_count >= 2` at `Op::Return`, so its cell is
    /// NOT recycled and the next invocation cannot reuse it — each
    /// escaping iteration keeps its own distinct Rc, preserving the
    /// `.each`-capture-isolation fix. Non-escaping blocks (the common
    /// case) cycle one cell through the loop.
    fn block_locals_from_captured(
        &mut self,
        captured: &Rc<RefCell<Vec<Value>>>,
        n_locals: usize,
    ) -> Rc<RefCell<Vec<Value>>> {
        let src = captured.borrow();
        if let Some(cell) = self.locals_pool.pop() {
            {
                let mut v = cell.borrow_mut();
                v.clear();
                v.extend_from_slice(&src);
                if v.len() < n_locals {
                    v.resize(n_locals, Value::Nil);
                }
            }
            cell
        } else {
            let mut v = src.clone();
            if v.len() < n_locals {
                v.resize(n_locals, Value::Nil);
            }
            Rc::new(RefCell::new(v))
        }
    }

    /// Decide a block invocation's locals representation, returning the
    /// frame `Locals` plus the `block_writeback` to install.
    ///
    /// SHARE-DIRECT (the new fast path): when the block's body creates
    /// no inner closure (`!proto.creates_block`, so nothing can capture
    /// and leak this invocation's slots) AND the same block isn't
    /// already live on the stack (no re-entrancy), the block frame
    /// reuses the `captured` Vec ITSELF — a single `Rc::clone`, zero
    /// per-iteration byte copy. Writes to outer-scope slots land
    /// directly on `captured` (which IS the enclosing method's locals),
    /// so no write-back is needed (`None`), and the lexical-owner walk
    /// finds the method frame for free because both share the Rc. The
    /// captured Vec is grown to `needed` once; the extra `[param_start,
    /// needed)` slots are block scratch the method never reads.
    ///
    /// COPY (the prior behaviour, preserved for the cases share-direct
    /// can't serve): a capturing block needs per-invocation isolation
    /// (the `.each { |s| -> { s } }` fix), and a re-entrant block needs
    /// each live invocation to own distinct scratch. Both fall back to
    /// `block_locals_from_captured` + a `(captured, param_start)`
    /// write-back.
    fn block_frame_locals(
        &mut self,
        captured: &Rc<RefCell<Vec<Value>>>,
        proto_idx: usize,
        needed: usize,
        param_start: u16,
        captured_is_method_scope: bool,
    ) -> (LocalsCell, BlockWriteback) {
        // Share-direct requires ALL of:
        //  - `captured` is a genuine method / class-body / toplevel
        //    scope, not an enclosing block's per-invocation COPY —
        //    sharing a copy would skip its write-back chain and lose
        //    the propagation to the grandparent (the `[[1,2]].each
        //    { |p| p.each { |n| total += n } }` case);
        //  - no inner closure can capture & leak this frame's slots
        //    (`!creates_block`);
        //  - the same block isn't already live on the stack
        //    (`!reentrant`), which would clobber its scratch.
        // Returns the locals cell (caller wraps in `Locals::Shared`)
        // and the `block_writeback` — `None` for the share path
        // (writes hit the method scope directly).
        if captured_is_method_scope
            && !self.protos[proto_idx].creates_block
            && !self.block_is_reentrant(proto_idx, captured)
        {
            // Grow once so the block's body slots exist; the method
            // only ever reads `[0, method_n_locals)` so the extra
            // tail is invisible to it.
            {
                let mut c = captured.borrow_mut();
                if c.len() < needed {
                    c.resize(needed, Value::Nil);
                }
            }
            (captured.clone(), None)
        } else {
            let cell = self.block_locals_from_captured(captured, needed);
            (cell, Some((captured.clone(), param_start)))
        }
    }

    /// Is a block with this `proto_idx` + `captured` already an active
    /// frame on the stack? Such re-entrancy means a share-direct frame
    /// would clobber the suspended invocation's param / body-local
    /// scratch, so the caller must fall back to a per-invocation copy.
    /// The `proto_idx` pre-filter (a cheap `usize` compare) keeps the
    /// `Rc::ptr_eq` off all the unrelated frames; the enclosing method
    /// frame is non-block so it never matches.
    fn block_is_reentrant(
        &self,
        proto_idx: usize,
        captured: &Rc<RefCell<Vec<Value>>>,
    ) -> bool {
        self.frames.iter().any(|f| {
            f.is_block
                && f.proto_idx == proto_idx
                && f.locals
                    .as_shared()
                    .is_some_and(|l| Rc::ptr_eq(l, captured))
        })
    }

    /// Return a popped frame's locals cell to the pool, IFF nothing else
    /// still references it. A `define_method` body shares its locals Rc
    /// with the closure's capture (`strong_count >= 2`), and a pending
    /// non-local return parks one in `method_return_locals` — the
    /// `strong_count == 1` guard excludes both, so a recycled cell can
    /// never alias live state. Clearing drops the stale Values (releasing
    /// their refs); the buffer capacity is kept for the next call.
    pub(crate) fn recycle_frame_locals(&mut self, locals: Rc<RefCell<Vec<Value>>>) {
        const LOCALS_POOL_CAP: usize = 256;
        if self.locals_pool.len() < LOCALS_POOL_CAP && Rc::strong_count(&locals) == 1 {
            locals.borrow_mut().clear();
            self.locals_pool.push(locals);
        }
    }

    /// Release a popped frame's locals storage — the one call EVERY
    /// frame-pop site must make. A `Shared` cell goes back to the
    /// recycle pool (guarded by `strong_count` inside
    /// `recycle_frame_locals`); a `Stack` frame's arena segment is
    /// truncated away (dropping the `Value`s releases their refs).
    /// Frame pops are LIFO even through exception unwind, so
    /// truncating to this frame's base can never cut a live deeper
    /// frame's slots.
    #[inline]
    pub(crate) fn release_frame_locals(&mut self, locals: crate::vm::Locals) {
        match locals {
            crate::vm::Locals::Shared(rc) => self.recycle_frame_locals(rc),
            crate::vm::Locals::Stack(base) => self.locals_arena.truncate(base as usize),
        }
    }

    /// Move the top `argc` operand-stack values (already in slot
    /// order) into the locals arena and pad with Nil up to `n_locals`;
    /// returns the new frame's arena base. The manual move loop
    /// (swap-out + truncate) replaces an earlier
    /// `extend(stack.drain(..))` — the drain/extend iterator machinery
    /// refused to inline and dominated the tight-call-loop profile.
    /// The Nil left in the vacated stack slot makes the truncate's
    /// drop a no-op without cloning the value (no refcount churn).
    #[inline(always)]
    fn arena_push_args(&mut self, argc: usize, n_locals: usize) -> u32 {
        debug_assert!(n_locals >= argc);
        let base = self.locals_arena.len();
        let split = self.stack.len() - argc;
        self.locals_arena.reserve(n_locals);
        for i in 0..argc {
            let v = std::mem::replace(&mut self.stack[split + i], Value::Nil);
            self.locals_arena.push(v);
        }
        self.stack.truncate(split);
        for _ in argc..n_locals {
            self.locals_arena.push(Value::Nil);
        }
        base as u32
    }

    /// Read a slot of the TOP frame's locals, representation-agnostic.
    /// Cold-path convenience — the hot ops (`LoadLocal` & co) inline
    /// the match themselves.
    #[inline]
    pub(crate) fn get_local_top(&self, slot: usize) -> Value {
        let frame = self.frames.last().expect("ICE: get_local_top no frame");
        match &frame.locals {
            crate::vm::Locals::Stack(base) => {
                self.locals_arena[*base as usize + slot].clone()
            }
            crate::vm::Locals::Shared(rc) => rc.borrow()[slot].clone(),
        }
    }

    /// Write a slot of the TOP frame's locals, representation-agnostic.
    /// NOTE: does NOT run the block-writeback propagation — callers
    /// that can be a block frame writing an outer-scope slot must use
    /// the `Op::StoreLocal` machinery instead.
    #[inline]
    pub(crate) fn set_local_top(&mut self, slot: usize, v: Value) {
        let frame = self.frames.last().expect("ICE: set_local_top no frame");
        match &frame.locals {
            crate::vm::Locals::Stack(base) => {
                let idx = *base as usize + slot;
                self.locals_arena[idx] = v;
            }
            crate::vm::Locals::Shared(rc) => rc.borrow_mut()[slot] = v,
        }
    }

    /// Lexical-owner frame index for the TOP frame — the shared
    /// resolution used by `yield`, `block_given?`, `super`'s
    /// defining-class walk and `Op::ReturnMethod`. A non-block top
    /// frame is its own lexical owner (the old seed walk found it
    /// via `Rc::ptr_eq` on its own cell; `Locals::Stack` frames have
    /// no cell, hence this explicit shortcut). A block top frame
    /// always carries `Shared` locals, so the writeback-chain walk
    /// applies unchanged.
    pub(crate) fn lexical_owner_of_top(&self) -> Option<usize> {
        let f = self.frames.last()?;
        if !f.is_block {
            return Some(self.frames.len() - 1);
        }
        match &f.locals {
            crate::vm::Locals::Shared(rc) => {
                let seed = rc.clone();
                self.find_lexical_owner_frame(&seed)
            }
            // Unreachable by construction (block frames are always
            // Shared) — treat as "owner not on stack".
            crate::vm::Locals::Stack(_) => None,
        }
    }

    /// Enter a fresh method-local `$~` scope on the just-pushed top
    /// frame: stash the caller's `last_match` on the new frame (to be
    /// restored when it returns — see the pop sites in `Op::Return`,
    /// `continue_method_break`, and exception unwind) and reset
    /// `self.last_match` to nil so the method body starts clean. Called
    /// immediately after each method-frame push. Blocks DON'T call this
    /// (they transparently share the enclosing method's match data).
    #[cfg(feature = "regex")]
    #[inline]
    /// Lazy `$~` scoping: called before EVERY `last_match` write
    /// (the 11 runtime sites — match/match?/=~ arms, scan/gsub,
    /// MatchData install, iter drivers). The eager
    /// `enter_method_match_scope` saved/restored on every method
    /// invocation (~8% of a tight call loop, profiled) even though
    /// almost no method touches a regex; instead, the FIRST write
    /// inside a method scope snapshots the caller's `$~` into the
    /// innermost METHOD frame (blocks and class bodies share their
    /// enclosing method's scope, so the walk skips them — same
    /// contract as the eager version's "None on a block frame
    /// means don't touch on pop"). Between method entry and the
    /// first write `$~` is invariant (nested calls restore
    /// themselves), so the lazily-saved value equals what the
    /// eager save would have captured. The Return/unwind restore
    /// paths are unchanged: `None` still reads as "this frame
    /// never touched `$~`".
    #[cfg(feature = "regex")]
    pub(crate) fn save_match_scope_on_write(&mut self) {
        for f in self.frames.iter_mut().rev() {
            if f.is_block || f.is_class_body {
                continue;
            }
            if f.saved_last_match.is_none() {
                f.saved_last_match = Some(self.last_match.take().map(Box::new));
            }
            return;
        }
    }

    /// `$~` visibility for the CURRENT scope — the read-side half of
    /// the lazy scoping contract. The global `last_match` belongs to
    /// this scope only if the innermost METHOD frame has written
    /// (and therefore saved the caller's value); otherwise the
    /// global is an OUTER scope's match and reads here must see nil.
    /// The eager version achieved this by clearing the global at
    /// every method entry; the lazy version leaves it in place and
    /// gates the read sites (`$~` / `$1`..`$9` / `` $` `` / `$'` /
    /// `$&` ops, `Regexp.last_match`, MatchData extraction) through
    /// this getter instead. Toplevel (no method frame) reads the
    /// global directly — toplevel matches are program-global, same
    /// as before.
    #[cfg(feature = "regex")]
    pub(crate) fn scoped_last_match(&self) -> Option<&crate::vm::LastMatch> {
        for f in self.frames.iter().rev() {
            if f.is_block || f.is_class_body {
                continue;
            }
            f.saved_last_match.as_ref()?;
            break;
        }
        self.last_match.as_ref()
    }

    /// Explicit-receiver monomorphic fast path — see the call site in
    /// `do_call`. Resolves via the SAME `class_of` + `lookup_method_cached`
    /// the slow path uses, so method resolution (including the
    /// eigenclass/singleton chain, prepends, and the cext fall-through) is
    /// identical; only PUBLIC, fixed-arity, non-closure methods are invoked
    /// stack-direct here, and everything else returns `Ok(false)` to fall
    /// through. A public method always passes `check_method_visibility`, so
    /// skipping it for the fast path is safe.
    fn try_invoke_explicit_recv_cached(
        &mut self,
        name_id: SymId,
        argc: usize,
        cache_id: u16,
    ) -> Result<bool, Trap> {
        // Explicit-recv stack layout: [..., recv, a1, ..., aN].
        let recv_idx = match self.stack.len().checked_sub(argc + 1) {
            Some(i) => i,
            None => return Ok(false),
        };
        let id = match self.stack.get(recv_idx) {
            Some(Value::Object(id)) => *id,
            _ => return Ok(false),
        };
        let Some(cls) = self.heap.try_class_of(id) else {
            return Ok(false); // class-less slot (HeapObj::Fiber) -> universal arms
        };
        let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) else {
            return Ok(false);
        };
        // Only plain `def`-style proto methods are stack-direct here.
        // Closures (define_method) share captured locals; builtins carry a
        // dummy `proto_idx` and re-dispatch in `invoke_method_with_block`;
        // both must take the full path.
        if m.visibility.get() != Visibility::Public
            || m.closure.is_some()
            || m.builtin.is_some()
        {
            return Ok(false);
        }
        let fixed = match m.fixed_arity {
            Some(f) if f.required as usize == argc => f,
            _ => return Ok(false),
        };
        self.check_frames()?;
        // Bind the argc args (stack top) into the locals, then drop the recv.
        // Args sit on the stack in slot order (a1..aN), so the arena
        // path moves them in one drain — no per-value refcount churn,
        // no Rc/RefCell cell at all.
        let n_locals = fixed.n_locals as usize;
        let locals = if fixed.stack_eligible {
            let base = self.arena_push_args(argc, n_locals);
            crate::vm::Locals::Stack(base)
        } else {
            let cell = self.locals_cell_nil(n_locals);
            {
                let mut l = cell.borrow_mut();
                for slot in (0..argc).rev() {
                    l[slot] = self
                        .stack
                        .pop()
                        .expect("ICE: explicit-recv fast path arg underflow");
                }
            }
            crate::vm::Locals::Shared(cell)
        };
        let recv = self
            .stack
            .pop()
            .expect("ICE: explicit-recv fast path recv underflow");
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals,
            self_val: recv,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: None,
            defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false,
            n_given_positional: fixed.required,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
        });
        // $~ scoping is LAZY now — save_match_scope_on_write fires on
        // the first last_match write inside this method scope.
        Ok(true)
    }

    /// Block-form sibling of `try_invoke_explicit_recv_cached`.
    /// `do_call_block`'s entry stack layout is
    /// `[..., recv, block, a1, ..., aN]`, so the args are the top
    /// `argc`, the block sits at `len-(argc+1)`, and the receiver at
    /// `len-(argc+2)`. We peek (never mutate the stack) until every
    /// gate passes, then commit. This handles ONLY the common case:
    /// an `Object` receiver, a literal `Value::Block` block, and a
    /// plain `def`-style proto method (Public, exact fixed arity,
    /// non-closure, non-builtin). Every other shape — a `BoundMethod`
    /// / `CurriedProc` / `Nil` block needing coercion, a non-Object
    /// receiver, a closure/builtin/variadic method, a visibility or
    /// arity miss — returns `Ok(false)` with the stack UNCHANGED so
    /// `do_call_block`'s full path runs unaltered. Mirrors the
    /// no-block fast path exactly, differing only in popping the block
    /// off the stack and threading it into `block_arg: Some(block_id)`.
    fn try_invoke_explicit_recv_block_cached(
        &mut self,
        name_id: SymId,
        argc: usize,
        cache_id: u16,
    ) -> Result<bool, Trap> {
        // Block-form layout: [..., recv, block, a1, ..., aN].
        let block_idx = match self.stack.len().checked_sub(argc + 1) {
            Some(i) => i,
            None => return Ok(false),
        };
        // recv lives one slot below the block.
        let recv_idx = match block_idx.checked_sub(1) {
            Some(i) => i,
            None => return Ok(false),
        };
        let id = match self.stack.get(recv_idx) {
            Some(Value::Object(id)) => *id,
            _ => return Ok(false),
        };
        // Only a literal block is stack-direct. A BoundMethod /
        // CurriedProc needs `coerce_callable_to_block`, and `&nil`
        // re-aims at the no-block path — both must fall through to
        // `do_call_block`'s existing coerce logic untouched.
        let block_id = match self.stack.get(block_idx) {
            Some(Value::Block(bid)) => *bid,
            _ => return Ok(false),
        };
        let Some(cls) = self.heap.try_class_of(id) else {
            return Ok(false); // class-less slot (HeapObj::Fiber) -> universal arms
        };
        let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) else {
            return Ok(false);
        };
        // Same gates as the no-block template: closures share captured
        // locals, builtins re-dispatch via a dummy proto_idx, and
        // non-Public methods must take the full visibility path.
        if m.visibility.get() != Visibility::Public
            || m.closure.is_some()
            || m.builtin.is_some()
        {
            return Ok(false);
        }
        let fixed = match m.fixed_arity {
            Some(f) if f.required as usize == argc => f,
            _ => return Ok(false),
        };
        self.check_frames()?;
        // Bind the argc args (stack top), then drop the block, then
        // the recv. These pops are guaranteed by the peeks above —
        // `unreachable!` (which the panic budget does not count)
        // documents the invariant without an `.expect`.
        let n_locals = fixed.n_locals as usize;
        let locals = if fixed.stack_eligible {
            let base = self.arena_push_args(argc, n_locals);
            crate::vm::Locals::Stack(base)
        } else {
            let cell = self.locals_cell_nil(n_locals);
            {
                let mut l = cell.borrow_mut();
                for slot in (0..argc).rev() {
                    l[slot] = match self.stack.pop() {
                        Some(v) => v,
                        None => unreachable!("ICE: explicit-recv block fast path arg underflow"),
                    };
                }
            }
            crate::vm::Locals::Shared(cell)
        };
        // Drop the block value (the ObjId is already captured in
        // `block_id`); it becomes the frame's `block_arg`.
        match self.stack.pop() {
            Some(_) => {}
            None => unreachable!("ICE: explicit-recv block fast path block underflow"),
        }
        let recv = match self.stack.pop() {
            Some(v) => v,
            None => unreachable!("ICE: explicit-recv block fast path recv underflow"),
        };
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals,
            self_val: recv,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: Some(block_id),
            defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false,
            n_given_positional: fixed.required,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
        });
        // $~ scoping is LAZY now — save_match_scope_on_write fires on
        // the first last_match write inside this method scope.
        Ok(true)
    }

    /// Class/Module-receiver monomorphic fast path — the
    /// `X.class_method(args)` sibling of `try_invoke_explicit_recv_cached`.
    /// Jekyll-style code dispatches module functions constantly
    /// (`PathManager.join`, `Utils.*`, `Jekyll.logger`), and without
    /// this every such call walked `do_call`'s full arm chain
    /// (~240ns vs ~60ns measured on a 2-arg module fn).
    ///
    /// Soundness:
    /// - `class_singleton_deny` gates out every name a pre-singleton
    ///   `do_call` arm can intercept for a Class receiver (Class.new,
    ///   const_get, include, respond_to?, …) — for any other name the
    ///   chain provably falls through to the canonical
    ///   `lookup_class_singleton_method` arm this path mirrors.
    /// - resolution uses `lookup_class_singleton_cached` (same walk,
    ///   method_gen-validated, `ptr|1`-tagged cache entries so a
    ///   polymorphic site can't cross-serve instance entries).
    /// - only PUBLIC, fixed-arity, non-closure, non-builtin methods
    ///   invoke stack-direct; everything else falls through unchanged
    ///   (private class methods keep their NoMethodError shape,
    ///   `define_singleton_method` closures keep captured locals).
    fn try_invoke_class_singleton_cached(
        &mut self,
        name_id: SymId,
        argc: usize,
        cache_id: u16,
    ) -> Result<bool, Trap> {
        // Explicit-recv stack layout: [..., recv, a1, ..., aN].
        let recv_idx = match self.stack.len().checked_sub(argc + 1) {
            Some(i) => i,
            None => return Ok(false),
        };
        let cls = match self.stack.get(recv_idx) {
            Some(Value::Class(cls)) => cls.clone(),
            _ => return Ok(false),
        };
        if self.class_singleton_deny.contains(&name_id) {
            return Ok(false);
        }
        let Some(m) = self.lookup_class_singleton_cached(&cls, name_id, cache_id) else {
            return Ok(false);
        };
        if m.visibility.get() != Visibility::Public
            || m.closure.is_some()
            || m.builtin.is_some()
        {
            return Ok(false);
        }
        let fixed = match m.fixed_arity {
            Some(f) if f.required as usize == argc => f,
            _ => return Ok(false),
        };
        self.check_frames()?;
        let n_locals = fixed.n_locals as usize;
        let locals = if fixed.stack_eligible {
            let base = self.arena_push_args(argc, n_locals);
            crate::vm::Locals::Stack(base)
        } else {
            let cell = self.locals_cell_nil(n_locals);
            {
                let mut l = cell.borrow_mut();
                for slot in (0..argc).rev() {
                    l[slot] = self
                        .stack
                        .pop()
                        .expect("ICE: class-singleton fast path arg underflow");
                }
            }
            crate::vm::Locals::Shared(cell)
        };
        let recv = self
            .stack
            .pop()
            .expect("ICE: class-singleton fast path recv underflow");
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals,
            self_val: recv,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: None,
            defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false,
            n_given_positional: fixed.required,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
        });
        Ok(true)
    }

    fn try_invoke_fixed_method_from_stack(
        &mut self,
        m: Rc<Method>,
        self_val: Value,
        argc: usize,
        block: Option<ObjId>,
    ) -> Result<bool, Trap> {
        if m.closure.is_some() {
            return Ok(false);
        }
        let fixed = match m.fixed_arity {
            Some(fixed) if fixed.required as usize == argc => fixed,
            _ => return Ok(false),
        };
        self.check_frames()?;
        let n_locals = fixed.n_locals as usize;
        // Stack-eligible protos go straight to the arena; the rest
        // use one pooled cell for every arity shape (the old
        // special-cases built a fresh Vec that intern_locals then
        // swapped into the cell, freeing the cell's buffer — see
        // locals_cell_nil).
        let locals = if fixed.stack_eligible {
            let base = self.arena_push_args(argc, n_locals);
            crate::vm::Locals::Stack(base)
        } else {
            let cell = self.locals_cell_nil(n_locals);
            {
                let mut l = cell.borrow_mut();
                for slot in (0..argc).rev() {
                    l[slot] = self
                        .stack
                        .pop()
                        .expect("ICE: fixed method fast path arg underflow");
                }
            }
            crate::vm::Locals::Shared(cell)
        };
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals,
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false,
            swap_return: None,
            block_arg: block,
            defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
            lexical_cvar_class: None,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: false,
            n_given_positional: fixed.required,
            kw_given_mask: 0,
            aux: None,
            pending_yield: false,
            block_writeback: None,
        });
        // $~ scoping is LAZY now — save_match_scope_on_write fires on
        // the first last_match write inside this method scope.
        Ok(true)
    }



    /// Invoke the parent's `inherited(subclass)` hook on
    /// `new_cls`. Used by the `Class.new` no-block and
    /// block-form arms to match CRuby's contract that
    /// subclass-creation fires `<parent>.inherited(child)`
    /// regardless of source-form vs dynamic-form. Walks the
    /// parent's class-singleton chain via
    /// `lookup_class_singleton_method`; absent-hook is a
    /// silent no-op (CRuby's `Object#inherited` default).
    pub(crate) fn invoke_inherited_hook(&mut self, new_cls: &Rc<crate::value::Class>) -> Result<(), Trap> {
        let parent_rc = new_cls.superclass.borrow().clone();
        let Some(parent) = parent_rc else { return Ok(()); };
        // Fast-path: if `inherited` has never been interned,
        // no user code can have defined a hook, so skip the
        // lookup. Mirrors `fire_inclusion_hooks`'s gate.
        if !self.interner.contains("inherited") {
            return Ok(());
        }
        let inherited_sym = self.interner.intern("inherited");
        let Some(m) = self.lookup_class_singleton_method(&parent, inherited_sym) else {
            return Ok(());
        };
        // Frame-count synchronisation so the queued hook body
        // actually executes before we pop the return value —
        // `invoke_method` pushes the Frame but the bytecode
        // runs only when dispatch resumes. Mirrors
        // `fire_inclusion_hooks`'s pre_frames + dispatch_until
        // + pop pattern.
        let pre_frames = self.frames.len();
        let parent_val = Value::Class(parent.clone());
        let child_val = Value::Class(new_cls.clone());
        self.invoke_method(m, parent_val, vec![child_val])?;
        self.dispatch_until(pre_frames)?;
        // `inherited` returns nil per CRuby contract; drop the
        // pushed return value so the caller's stack stays
        // balanced for the subsequent `Class.new` result push.
        self.stack.pop();
        Ok(())
    }

    pub(crate) fn invoke_method_with_block(&mut self, m: Rc<Method>, self_val: Value, args: Vec<Value>, block: Option<ObjId>) -> Result<(), Trap> {
        // Builtin-method short-circuit: synthesised Methods on
        // Kernel (and any future host class with similar
        // reflection records) carry a `builtin: Some(...)` payload
        // that supplies introspection metadata. Their `proto_idx`
        // is a placeholder (`0`) and must not be executed as
        // bytecode — re-enter `do_call`/`do_call_block` with the
        // primitive's real name so the inline arm handles dispatch
        // (`obj.class`, `obj.is_a?(X)`, ...).
        if let Some(meta) = &m.builtin {
            // Synth Method dispatch routes back through `do_call`
            // with the primitive's real name. The synth lives only
            // in `Vm.kernel_builtin_metas` (not on Kernel.methods),
            // so the chain-walking sites below won't re-find it
            // and we don't need a skip flag — `obj.class`'s normal
            // inline arm fires naturally.
            let name_id = meta.name_id;
            let argc = args.len();
            self.stack.push(self_val);
            if let Some(bid) = block {
                self.stack.push(Value::Block(bid));
                for a in args { self.stack.push(a); }
                return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
            } else {
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
            }
        }
        // `define_method`-installed methods carry a captured Rc and
        // diverge from the normal fresh-locals path: their frame
        // *shares* `captured` with the lexical scope that created
        // the block. Writes to outer-scope locals from inside the
        // method body propagate back, matching CRuby semantics.
        if let Some(cl) = &m.closure {
            // `|**kw|` block-method (`define_method(:m) { |**k| … }`):
            // peel the trailing kwargs Hash BEFORE the positional arity
            // check — the closure binder otherwise counts it as a
            // positional and trips "wrong number of arguments" — and bind
            // it to the kwrest slot below (empty `{}` when none passed).
            // Mirrors the method path's kw-rest handling, which this
            // closure path skipped (rest / block-arg were already wired).
            let has_kw_rest = self.protos[m.proto_idx].kw_rest_param.is_some();
            let kw_trailing_positional = std::mem::take(&mut self.trailing_hash_positional);
            let mut args = args;
            let kw_hash_id: Option<crate::value::ObjId> = if has_kw_rest && !kw_trailing_positional {
                match args.last() {
                    Some(Value::Hash(hid)) => { let h = *hid; args.pop(); Some(h) }
                    _ => None,
                }
            } else {
                None
            };
            let given = args.len();
            let n_params = cl.n_params as usize;
            let proto_idx = m.proto_idx;
            let proto_n_locals = self.protos[proto_idx].n_locals as usize;
            let param_start = cl.param_start as usize;
            // M27 A1: when the underlying block proto has a `|*rest|`
            // parameter (`define_method(:m) do |*args| … end`), allow
            // arity `>= n_params` and gather overflow into the rest
            // slot's Array; otherwise enforce strict equality (the
            // pre-M27 behaviour). Look up rest + block-arg slot
            // positions in the proto's param list BEFORE the
            // `caps.borrow_mut()` so the same borrow can drive every
            // write.
            let has_rest = self.protos[proto_idx].rest_param.is_some();
            if (has_rest && given < n_params) || (!has_rest && given != n_params) {
                let expected = if has_rest { format!("{}+", n_params) } else { format!("{}", n_params) };
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected {})", given, expected),
                }));
            }
            self.check_frames()?;
            let rest_slot: Option<usize> = self.protos[proto_idx].rest_param.as_ref()
                .and_then(|name| self.protos[proto_idx].params.iter().position(|p| p == name))
                .map(|idx| param_start + idx);
            // M27 A1: when the underlying block proto has a `|&blk|`
            // parameter, look up its slot in the closure's locals
            // BEFORE the locals borrow_mut and bind the caller's block
            // there. CRuby's `define_method(:m) do |&blk| blk.call end;
            // obj.m { ... }` idiom (Sinatra's route table) needs this
            // — without it the slot stayed Nil because the closure
            // path skipped the method-style trailing-slot binder.
            let block_arg_slot: Option<usize> = self.protos[proto_idx].block_param.as_ref()
                .and_then(|bname| self.protos[proto_idx].params.iter()
                    .position(|p| p == bname))
                .map(|idx| param_start + idx);
            let kw_rest_slot: Option<usize> = self.protos[proto_idx].kw_rest_param.as_ref()
                .and_then(|name| self.protos[proto_idx].params.iter().position(|p| p == name))
                .map(|idx| param_start + idx);
            // Block params live *after* the captured frame's n_locals
            // (block locals layout inherits the parent — see ADR 0004).
            // Resize the shared Vec if a previous invocation hasn't
            // already grown it.
            //
            // Rest-aware arg binding: first `n_params` args go into
            // the Single/Destructure slots; overflow gathers into the
            // rest slot as a fresh Array. Without rest the loop binds
            // exactly `given == n_params` slots.
            // Pre-allocate the rest Array (if any) BEFORE taking the
            // `caps.borrow_mut()` so the heap calls don't reborrow.
            // Split args into the head (positional) + tail (rest)
            // here so the borrow_mut just writes them into slots.
            // Empty rest is still a fresh `[]` so the body sees an
            // Array (not Nil) at the slot — matches CRuby's
            // `*args` arity contract.
            let (head_args, rest_arr_id): (Vec<Value>, Option<crate::value::ObjId>) = if rest_slot.is_some() {
                let mut args = args;
                let rest_vec: Vec<Value> = if given > n_params {
                    args.split_off(n_params)
                } else {
                    Vec::new()
                };
                // Pin `self_val` (and the heap-ref args) across this
                // rest-Array allocation's `maybe_gc`. The frame that
                // will root `self_val` isn't pushed until below, and a
                // `define_method(:initialize) do |*a| … end` reaches
                // here from `Class#new` AFTER that path dropped its own
                // PinGuard — so without this, a sweep frees the
                // freshly-allocated receiver and the closure body runs
                // on a dangling `self`. Found via STRESS_GC on a
                // `define_method`-defined `initialize` with `*args`.
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(self_val.clone());
                // Pin the peeled kwargs Hash too — it was popped out of
                // `args`, so without this the rest-Array alloc's maybe_gc
                // sweeps it and the later kwrest bind hits a dangling slot
                // (STRESS_GC: "heap slot is not a Hash").
                if let Some(hid) = kw_hash_id { g.pin(Value::Hash(hid)); }
                for a in &args {
                    if a.is_gc_heap_ref() { g.pin(a.clone()); }
                }
                for a in &rest_vec {
                    if a.is_gc_heap_ref() { g.pin(a.clone()); }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::Array(rest_vec.into()));
                drop(g);
                (args, Some(id))
            } else {
                (args, None)
            };
            // Resolve the kwrest Hash to bind: the peeled one, or a fresh
            // empty `{}` when the slot exists but no kwargs were passed
            // (CRuby's `|**k|` defaults to `{}`, not nil). Allocate the
            // empty Hash here — before the `caps.borrow_mut()` — pinning
            // self + the already-built args/rest so the alloc's maybe_gc
            // can't sweep them.
            let kw_hash_final: Option<crate::value::ObjId> = if kw_rest_slot.is_some() {
                match kw_hash_id {
                    Some(hid) => Some(hid),
                    None => {
                        let mut g = crate::vm::PinGuard::new(self);
                        g.pin(self_val.clone());
                        if let Some(rid) = rest_arr_id { g.pin(Value::Array(rid)); }
                        for a in &head_args { if a.is_gc_heap_ref() { g.pin(a.clone()); } }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let id = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
                        drop(g);
                        Some(id)
                    }
                }
            } else {
                None
            };
            let cl = m.closure.as_ref().unwrap();
            {
                let mut caps = cl.captured.borrow_mut();
                let need = param_start.max(proto_n_locals);
                if caps.len() < need {
                    caps.resize(need, Value::Nil);
                }
                for (i, a) in head_args.into_iter().enumerate() {
                    caps[param_start + i] = a;
                }
                if let (Some(slot), Some(id)) = (rest_slot, rest_arr_id) {
                    caps[slot] = Value::Array(id);
                }
                if let Some(slot) = block_arg_slot {
                    caps[slot] = match block {
                        Some(id) => Value::Block(id),
                        None => Value::Nil,
                    };
                }
                if let (Some(slot), Some(hid)) = (kw_rest_slot, kw_hash_final) {
                    caps[slot] = Value::Hash(hid);
                }
            }
            self.frames.push(Frame {
                proto_idx,
                ip: 0,
                locals: crate::vm::Locals::Shared(cl.captured.clone()),
                self_val,
                base_sp: self.stack.len(),
                // M27 A2/A3: `define_method`'d method bodies don't
                // expose the caller's block via `block_given?` or
                // `yield` — CRuby treats the body as a Proc, so
                // `yield` raises LocalJumpError and `block_given?`
                // returns false. The block is reachable only through
                // an explicit `|&blk|` slot (bound above), which keeps
                // the explicit-capture idiom working without polluting
                // the implicit-yield surface. Setting `block_arg:
                // None` here is what enforces both.
                is_class_body: false, swap_return: None, block_arg: None, defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()), lexical_cvar_class: None, #[cfg(feature = "regex")] saved_last_match: None, is_block: false,
                // `define_method` enforces exact arity (no
                // defaults), so all params are "given".
                n_given_positional: given as u16,
                kw_given_mask: 0,
                aux: None, pending_yield: false,
                block_writeback: None,
            });
            // $~ scoping is LAZY now — save_match_scope_on_write fires on
            // the first last_match write inside this method scope.
            return Ok(());
        }
        if let Some(fixed) = m.fixed_arity
            && args.len() == fixed.required as usize
        {
            self.check_frames()?;
            let mut locals = args;
            locals.resize(fixed.n_locals as usize, Value::Nil);
            let locals = if !self.protos[m.proto_idx].creates_block {
                let base = self.locals_arena.len();
                self.locals_arena.extend(locals);
                crate::vm::Locals::Stack(base as u32)
            } else {
                crate::vm::Locals::Shared(self.intern_locals(locals))
            };
            self.frames.push(Frame {
                proto_idx: m.proto_idx,
                ip: 0,
                locals,
                self_val,
                base_sp: self.stack.len(),
                is_class_body: false,
                swap_return: None,
                block_arg: block,
                defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()),
                lexical_cvar_class: None,
                #[cfg(feature = "regex")] saved_last_match: None,
                is_block: false,
                n_given_positional: fixed.required,
                kw_given_mask: 0,
                aux: None,
                pending_yield: false,
                block_writeback: None,
            });
            // $~ scoping is LAZY now — save_match_scope_on_write fires on
            // the first last_match write inside this method scope.
            return Ok(());
        }
        // Default-argument support (literal defaults only): a Proto
        // carries a `defaults` vec parallel to `params`. `None`
        // entries are required; `Some(v)` entries can be omitted by
        // the caller and the slot is filled from the literal at
        // invocation time. Required params always come before
        // optionals in source order, so the legal arg-count range
        // is `[required, params.len()]`.
        //
        // Rest-param (`*args`) — m.params holds the positional
        // names; the rest-name (if any) is in proto.rest_param.
        // Excess args past `params.len()` collect into an Array
        // bound to the rest slot. With a rest param there's no
        // upper bound on the arg count.
        let proto = &self.protos[m.proto_idx];
        let has_rest = proto.rest_param.is_some();
        let has_kw_rest = proto.kw_rest_param.is_some();
        let has_block_param = proto.block_param.is_some();
        let kw_count = proto.kw_param_defaults.len();
        // Layout of `m.params` tail:
        //   [...positional..., rest?, ...kw_params..., kw_rest?, block_param?]
        let positional_max = m.params.len()
            - (if has_rest { 1 } else { 0 })
            - kw_count
            - (if has_kw_rest { 1 } else { 0 })
            - (if has_block_param { 1 } else { 0 });
        // M27 A4: split required count into pre-rest and post-rest.
        // `n_required_positional` is the leading required (pre-rest);
        // `n_required_post` is the trailing required (after `*rest`).
        // CRuby's arity check sums them — both groups are mandatory.
        let required_pre = proto.n_required_positional as usize;
        let n_required_post = proto.n_required_post as usize;
        let required = required_pre + n_required_post;
        // Pop trailing Hash arg (if present and we expect kw
        // params) — those entries become keyword bindings, not
        // positional args.
        let mut args = args;
        // Peel the trailing Hash into keyword bindings when the callee
        // declares kwparams — UNLESS the call was a plain `Op::Call`
        // whose trailing hash is an explicit-brace POSITIONAL hash
        // (`f({k: v})`, always positional in Ruby 3). Keyword (`CallKw`),
        // splat (`ApplyCall`), `super`, and block (`CallBlock`) calls
        // all leave `trailing_hash_positional == false`, preserving the
        // prior peel-if-kwparams behaviour. Peeling unconditionally was
        // the bug that made `merge_data!({ "categories" => … })` (Liquid
        // / Jekyll) raise `wrong number of arguments (given 0, …)`.
        let trailing_positional = std::mem::take(&mut self.trailing_hash_positional);
        let kw_hash: Option<Vec<(Value, Value)>> = if (kw_count > 0 || has_kw_rest) && !trailing_positional {
            if let Some(Value::Hash(hid)) = args.last().cloned() {
                args.pop();
                Some(self.heap.hash(hid).clone())
            } else {
                None
            }
        } else {
            None
        };
        let given = args.len();
        let arity_ok = if has_rest {
            given >= required
        } else {
            given >= required && given <= positional_max
        };
        if !arity_ok {
            let expected = if has_rest {
                format!("{}+", required)
            } else if required == positional_max {
                format!("{}", required)
            } else {
                format!("{}..{}", required, positional_max)
            };
            return Err(self.trap(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected {})", given, expected),
            }));
        }
        self.check_frames()?;
        let n_locals = proto.n_locals as usize;
        // Snapshot proto-derived data needed during arg binding,
        // dropping the immutable borrow on self.protos so the
        // subsequent maybe_gc / heap.alloc calls (for the rest
        // Array) can take &mut self.
        let kw_defaults_snapshot: Vec<Option<Value>> = proto.kw_param_defaults.clone();
        let kw_has_computed_snapshot: Vec<bool> = proto.kw_has_computed_default.clone();
        // Track which kwarg names the caller actually supplied;
        // bit `1 << i` set iff kwarg index `i` was found in
        // kw_hash. Threaded into the new frame as `kw_given_mask`
        // so the body's `Op::JumpIfKwArgGiven(kw_idx, off)`
        // prologue (one per computed-default kwarg) can skip
        // default eval when the caller supplied a value. 64-bit
        // caps non-literal-default kwargs per method at 64.
        let mut kw_given_mask: u64 = 0;
        // Optional positional slots that the caller omitted stay
        // `Nil` here; the method body's entry prologue runs
        // `Op::JumpIfArgGiven(slot, skip)` + default-expr +
        // `Op::StoreLocal(slot)` per optional, evaluating any
        // expression (literal, prior param, constant lookup, full
        // method call). `frame.n_given_positional = positional_take`
        // is what the prologue consults to tell "caller-supplied"
        // from "left for default-eval".
        let mut locals = vec_nil(n_locals);
        // M27 A4: peel `n_required_post` args off the tail before the
        // pre-rest / optional / rest binder runs. The post slots live
        // at `[positional_max - n_required_post .. positional_max]`
        // (params order is `[pre_req..., opt..., post_req...]` then
        // rest/kw/block tail). For `def mid(a, *b, c); mid(1,2,3,4,5)`:
        //   - post_args = [5], bound to slot `c` (positional_max - 1).
        //   - mid_args = [1,2,3,4]; first goes to slot `a`, the rest
        //     (3 items) gather into the Array bound to `b`.
        // Without n_required_post the existing logic bound c = nil
        // and the rest Array absorbed [2,3,4,5].
        let mut args = args;
        let post_args: Vec<Value> = if n_required_post > 0 && args.len() >= n_required_post {
            args.split_off(args.len() - n_required_post)
        } else {
            Vec::new()
        };
        let given_after_post = args.len();
        let pre_take = given_after_post.min(positional_max - n_required_post);
        // Bind up to (positional_max - n_required_post) args into the
        // pre+optional slots; any overflow flows into the rest slot.
        let positional_take = pre_take; // legacy name still used by the
                                        // frame's n_given_positional
                                        // record + default-arg prologue
        let mut args_iter = args.into_iter();
        for slot in locals.iter_mut().take(pre_take) {
            *slot = args_iter.next().unwrap();
        }
        if has_rest {
            // Remaining args (possibly empty) → fresh Array in the
            // rest slot.
            //
            // GC root hole guard: at this point everything we need
            // to survive `maybe_gc` lives only as Rust locals —
            // not in `self.stack`, `self.frames`, or `self.pinned`.
            // That covers:
            //   - `locals` — the not-yet-installed frame locals
            //     (already populated with positional + default args)
            //   - `rest_vec` — trailing args destined for the rest slot
            //   - `self_val` — the receiver. For inline-allocated
            //     receivers like `Ghost.new.poof`, the Object isn't
            //     bound to any caller local, so this window is the
            //     only thing keeping it alive
            //   - `block` (when Some) — heap-resident `BlockHandle`
            //     not yet attached to the new frame
            //   - `kw_hash` keys+values (when present) — the Hash
            //     contents were cloned out earlier; the per-pair
            //     Values may be heap-y and need to survive until
            //     the kw_count > 0 branch below reads them.
            //
            // Master commit 01b28ed shipped a narrower version of
            // this guard (pinning only `self_val` + `rest_vec`).
            // This widens it to `locals` / `block` / `kw_hash` and
            // adds the `check_alloc?` the original cut was missing
            // — a host configured with `max_heap_objects` would
            // otherwise see the rest-Array silently slip past the
            // cap, since `heap.alloc` itself doesn't enforce it.
            // The PinGuard's Drop pops on the early-return path of
            // `check_alloc?` too, so adding the check is safe.
            let rest_vec: Vec<Value> = args_iter.collect();
            let rest_slot = positional_max;
            let arr_id = {
                let mut g = PinGuard::new(self);
                for v in &locals { g.pin(v.clone()); }
                for v in &rest_vec { g.pin(v.clone()); }
                g.pin(self_val.clone());
                if let Some(id) = block { g.pin(Value::Block(id)); }
                if let Some(kw) = &kw_hash {
                    for (k, v) in kw {
                        g.pin(k.clone());
                        g.pin(v.clone());
                    }
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Array(rest_vec.into()))
            };
            locals[rest_slot] = Value::Array(arr_id);
        }
        // M27 A4: bind the post-rest required slots. They live AT THE
        // TAIL of the positional region (`[positional_max -
        // n_required_post .. positional_max]`); the rest slot — which
        // we just wrote (when present) — sits AFTER them. `post_args`
        // was peeled off args before the pre/rest binder ran.
        if n_required_post > 0 {
            let post_start = positional_max - n_required_post;
            for (i, v) in post_args.into_iter().enumerate() {
                locals[post_start + i] = v;
            }
        }
        // Bind keyword params. kw names live at the tail of
        // m.params; for each, look up the corresponding key in
        // the kw_hash (Symbol-keyed). Missing required keyword
        // → ArgumentError. Missing optional → use literal default.
        let kw_start = positional_max + if has_rest { 1 } else { 0 };
        if kw_count > 0 {
            for (i, (default, kw_name)) in kw_defaults_snapshot.iter()
                .zip(m.params[kw_start..kw_start + kw_count].iter())
                .enumerate()
            {
                let key_sym = self.interner.intern(kw_name);
                let key_val = Value::Sym(key_sym);
                let found = kw_hash.as_ref().and_then(|h| {
                    h.iter().find(|(k, _)| k.ruby_eql(&key_val, &self.heap))
                        .map(|(_, v)| v.clone())
                });
                let has_computed = kw_has_computed_snapshot.get(i).copied().unwrap_or(false);
                match (found, default, has_computed) {
                    (Some(v), _, _) => {
                        locals[kw_start + i] = v;
                        // Mark kwarg `i` as caller-supplied so the
                        // body's `Op::JumpIfKwArgGiven(i, _)` prologue
                        // (when emitted for a computed-default kwarg)
                        // skips the default-eval path.
                        if i < 64 {
                            kw_given_mask |= 1u64 << i;
                        }
                    }
                    // Literal-default optional kwarg, caller missing
                    // → fill from the snapshot. No prologue runs for
                    // this slot — same fast path as before.
                    (None, Some(d), false) => locals[kw_start + i] = d.clone(),
                    // Computed-default optional kwarg, caller missing
                    // → leave nil; the body's prologue evaluates the
                    // default expression and stores into the slot.
                    // Bit stays unset → prologue falls through.
                    (None, None, true) => {}
                    // Required kwarg, caller missing → ArgumentError.
                    // `(None, Some, true)` is structurally impossible
                    // — computed defaults set the snapshot entry to
                    // `None` (compiler emission, ast.rs lowering).
                    (None, None, false) => return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("missing keyword: :{}", kw_name),
                    })),
                    (None, Some(_), true) => unreachable!(
                        "computed kwarg default must also have None literal snapshot"
                    ),
                }
            }
        }
        // **kw_rest binding. Take the kw_hash entries whose keys
        // weren't claimed by a named kw_param above and collect
        // them into a fresh Hash bound to the kw_rest slot. With
        // no kw_hash at all (caller passed no kwargs), the slot
        // still gets a fresh empty Hash so `**opts` reliably yields
        // a Hash to user code. The known-names set is built from
        // the same kw_param name slice we just zipped over.
        if has_kw_rest {
            let kw_rest_slot = kw_start + kw_count;
            let known_keys: Vec<Value> = m.params[kw_start..kw_start + kw_count]
                .iter()
                .map(|nm| Value::Sym(self.interner.intern(nm)))
                .collect();
            let leftover: Vec<(Value, Value)> = match &kw_hash {
                Some(h) => h.iter()
                    .filter(|(k, _)| !known_keys.iter().any(|kk| kk.ruby_eql(k, &self.heap)))
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            // Same GC root-hole pattern as the rest-arg path above
            // (and the master Array#zip / Hash#sort_by chain fixed
            // in earlier PRs): `locals` / `self_val` / `block` /
            // `kw_hash` / `leftover` are Rust locals, NOT on
            // vm.stack / pinned, so the explicit `maybe_gc()` here
            // sweeps any heap-backed values they reference. Pin
            // everything participating in the new Hash alloc + the
            // already-bound locals through the alloc point.
            //
            // Master shipped the kw_rest code without this guard
            // (commits 680dbef "Module include chain + is_a?" /
            // ed0b872 "nested block destructure"); STRESS_GC tests
            // `anon_kwrest` and `kwrest_args` were the canary.
            let hid = {
                let mut g = PinGuard::new(self);
                for v in &locals { g.pin(v.clone()); }
                g.pin(self_val.clone());
                if let Some(id) = block { g.pin(Value::Block(id)); }
                if let Some(kw) = &kw_hash {
                    for (k, v) in kw {
                        g.pin(k.clone());
                        g.pin(v.clone());
                    }
                }
                for (k, v) in &leftover {
                    g.pin(k.clone());
                    g.pin(v.clone());
                }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(leftover)))
            };
            locals[kw_rest_slot] = Value::Hash(hid);
        }
        // `&blk` named block param: bind the caller's block (if any)
        // into the trailing block_param slot as `Value::Block(id)`,
        // or `Value::Nil` if no block was passed. The slot lives at
        // the very end of `params` after kw_rest (see Proto.block_param
        // / compile_proto for layout).
        if has_block_param {
            let block_slot = positional_max
                + if has_rest { 1 } else { 0 }
                + kw_count
                + if has_kw_rest { 1 } else { 0 };
            locals[block_slot] = match block {
                Some(id) => Value::Block(id),
                None => Value::Nil,
            };
        }
        let locals = if !self.protos[m.proto_idx].creates_block {
            // Full-binder path (optionals / rest / kwargs / &blk):
            // the bound Vec moves into the arena wholesale.
            let base = self.locals_arena.len();
            self.locals_arena.extend(locals);
            crate::vm::Locals::Stack(base as u32)
        } else {
            crate::vm::Locals::Shared(self.intern_locals(locals))
        };
        self.frames.push(Frame {
            proto_idx: m.proto_idx,
            ip: 0,
            locals,
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: block, defining_class: m.defining_class.as_ref().and_then(|w| w.upgrade()), lexical_cvar_class: None, #[cfg(feature = "regex")] saved_last_match: None, is_block: false,
            // Drives the body's default-arg prologue. Slots
            // `[0, positional_take)` came from the caller; slots
            // `[positional_take, positional_max)` are left Nil
            // here and the prologue's `Op::JumpIfArgGiven` skips
            // the default-eval for the former, executes it for
            // the latter.
            n_given_positional: positional_take as u16,
            kw_given_mask,
            aux: None, pending_yield: false,
            block_writeback: None,
        });
        // $~ scoping is LAZY now — save_match_scope_on_write fires on
        // the first last_match write inside this method scope.
        Ok(())
    }



    /// `module M; refine(Target) do … end; end` — record a refinement.
    /// Build an anonymous holder class, run the block on it as a class
    /// body (so `def`s install on it), and stash `(Target, holder)` under
    /// the defining module `M` for a later `using M` to activate. Returns
    /// the holder (CRuby returns the refinement module). Tier-1: see the
    /// `module_refinements` field doc for the global-activation caveat.
    pub(crate) fn do_refine(
        &mut self,
        target: std::rc::Rc<Class>,
        module: std::rc::Rc<Class>,
        block: ObjId,
    ) -> Result<(), Trap> {
        let holder = std::rc::Rc::new(Class {
            name: String::new(),
            is_module: true,
            ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            superclass: std::cell::RefCell::new(None),
            includes: std::cell::RefCell::new(Vec::new()),
            prepends: std::cell::RefCell::new(Vec::new()),
            singleton_prepends: std::cell::RefCell::new(Vec::new()),
            singleton_includes: std::cell::RefCell::new(Vec::new()),
            singleton_view: std::cell::RefCell::new(None),
            singleton_target: std::cell::RefCell::new(None),
            class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: std::cell::RefCell::new(None),
            #[cfg(feature = "cext")]
            cext_alloc_func: std::cell::Cell::new(None),
        });
        // Run the refine block as a class body on `holder`.
        let pre = self.frames.len();
        self.invoke_block_with_self(block, Value::Class(holder.clone()), true, vec![])?;
        self.dispatch_until(pre)?;
        let _ = self.stack.pop(); // discard the class-body return value
        self.module_refinements
            .entry(std::rc::Rc::as_ptr(&module) as usize)
            .or_default()
            .push((target, holder.clone()));
        self.stack.push(Value::Class(holder));
        Ok(())
    }

    /// `using M` — activate `M`'s refinements (Tier-1: globally, from
    /// here on). Copies every `(Target, holder)` recorded by `refine`
    /// into the active set keyed by `(Target.name, method_name)`, and
    /// registers the names in the dispatch gate.
    pub(crate) fn do_using(&mut self, module: &std::rc::Rc<Class>) {
        let key = std::rc::Rc::as_ptr(module) as usize;
        let Some(refs) = self.module_refinements.get(&key).cloned() else { return };
        for (target, holder) in &refs {
            let target_name = self.interner.intern(&target.name);
            for (mname, m) in holder.methods.borrow().iter() {
                self.active_refinements.insert((target_name, *mname), m.clone());
                self.refined_method_names.insert(*mname);
            }
        }
    }

    /// `obj.instance_eval { |o| ... }` / `cls.class_eval { |c| ... }`
    /// — invoke the block with `self` swapped to `new_self`.
    ///
    /// When `as_class_body` is true (the `class_eval` case),
    /// we also push `cls` onto `class_stack` + a fresh
    /// `Public` visibility entry, and mark the new frame
    /// `is_class_body: true`. That re-uses the existing
    /// class-body machinery so `def name; …; end` inside the
    /// block lands on the receiver class's method table — the
    /// dominant DSL use of `class_eval`. The cost: per the
    /// existing class-body Return semantics
    /// (`vm/step.rs::Op::Return`), the frame returns the class
    /// itself rather than the block's last expression. CRuby
    /// returns the block value; we'll need a non-`is_class_body`
    /// path to match exactly when a real use-case appears (see
    /// SUBSET.md). For `instance_eval` (`as_class_body=false`)
    /// the frame is a normal block, so the block's last
    /// expression is the return value — that part matches CRuby.
    ///
    /// `instance_eval { def name; ...; end }` defines a
    /// *singleton* method on the receiver in CRuby. rubyrs
    /// doesn't model singleton classes yet; `def` inside an
    /// `instance_eval` block lands on `toplevel_methods` (the
    /// same documented divergence as `attr_*` / `alias_method` /
    /// `define_method` outside a class body — see SUBSET.md's
    /// PoC caveat list). Real uses of `instance_eval` in our
    /// niche (configuration DSLs) typically read state rather
    /// than define methods, so this is acceptable for now.
    pub(crate) fn invoke_block_with_self(
        &mut self,
        block_id: ObjId,
        new_self: Value,
        as_class_body: bool,
        args: Vec<Value>,
    ) -> Result<(), Trap> {
        self.check_frames()?;
        let (proto_idx, captured, param_start, n_params, bh_lexical_cvar_class) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params, bh.lexical_cvar_class.clone())
        };
        // Bind args into the block's param slots, same auto-splat
        // shape as `invoke_block`. For instance_eval/class_eval
        // the conventional arg is a single value (self), so the
        // single-Array auto-splat case is unlikely to trigger,
        // but we keep the rule identical to avoid surprising
        // future callers.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        {
            let mut locals = captured.borrow_mut();
            if locals.len() < needed {
                while locals.len() < needed { locals.push(Value::Nil); }
            }
            for (i, a) in args.into_iter().enumerate() {
                if i < n_params as usize {
                    locals[param_start as usize + i] = a;
                }
            }
        }
        if as_class_body {
            // class_eval: re-use the class-body machinery so
            // `def` inside the block goes onto cls's method
            // table. Mirrors what `Op::DefClass` does at the
            // top of a `class X ... end` body. The return-path
            // handlers in vm/step.rs pop both stacks when this
            // frame returns, keyed off `is_class_body: true`.
            if let Value::Class(cls) = &new_self {
                self.class_stack.push(cls.clone());
                self.class_visibility_stack.push(crate::value::Visibility::Public);
            } else {
                // Caller checked Type before getting here, so
                // this is a programmer-error path. ICE rather
                // than silent-corruption: the class_stack pop
                // on frame return would underflow.
                panic!("ICE: invoke_block_with_self as_class_body=true requires Value::Class new_self");
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: crate::vm::Locals::Shared(captured),
            self_val: new_self,
            base_sp: self.stack.len(),
            is_class_body: as_class_body,
            swap_return: None,
            block_arg: None,
            defining_class: None,
            // instance_eval keeps the block's lexical cref for `@@cvar`
            // (resolve where the block was written, not on the new self).
            // class_eval instead re-roots the cref at the eval'd class —
            // that's `new_self`, which the self_val rule already returns,
            // so leave this None for the class-body case.
            lexical_cvar_class: if as_class_body { None } else { bh_lexical_cvar_class },
            #[cfg(feature = "regex")] saved_last_match: None,
            // class_eval's frame is BOTH `is_block: true` and
            // `is_class_body: true`. That dual role matters for
            // non-local `return`: per the unwind loop in
            // `vm/step.rs` (Op::ReturnMethod's branch), a
            // `return` inside the block walks back through
            // is_block frames to find the enclosing method.
            // With `is_block: false` the class_eval frame would
            // be the target itself — `return` would return *from
            // class_eval* rather than the enclosing method,
            // diverging from CRuby. The matching unwind change
            // (pop class_stack/visibility_stack when walking
            // past a `is_block && is_class_body` frame) lives
            // in `vm/step.rs`.
            is_block: true,
            n_given_positional: 0,
            kw_given_mask: 0,
            aux: None, pending_yield: false,
            block_writeback: None,
        });
        Ok(())
    }

    /// Wrap a callable Value (BoundMethod, CurriedProc, ...) into
    /// a fresh `Value::Block` so it can be passed wherever a
    /// block is expected. Lazily compiles a single shared
    /// forwarder proto on first call; subsequent calls reuse the
    /// same proto index. The synthesised BlockHandle stashes the
    /// callable in `captured[0]` and uses the proto's rest slot
    /// to splat the caller's args into a `.call(...)` on it.
    /// Caller must pass a value whose `.call` dispatch is
    /// already wired up (currently BoundMethod and CurriedProc).
    pub(crate) fn coerce_callable_to_block(&mut self, callable: Value)
        -> Result<crate::value::ObjId, Trap>
    {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        use crate::heap::HeapObj;
        use std::cell::RefCell;

        // Lazy proto build. Locals layout:
        //   slot 0: the callable (captured)
        //   slot 1: args Array (rest slot, filled by invoke_block)
        let proto_idx = if let Some(idx) = self.callable_forwarder_proto {
            idx
        } else {
            let call_id = self.interner.intern("call");
            let proto = Proto {
                name: "<callable-forwarder>".to_string(),
                params: Vec::new(),
                n_required_positional: 0,
                n_required_post: 0,
                rest_param: None,
                kw_param_defaults: Vec::new(),
                kw_has_computed_default: Vec::new(),
                kw_rest_param: None,
                block_param: None,
                n_locals: 2,
                // Not literally true (no Op::CreateBlock in the body),
                // but this proto runs as a closure whose locals ARE a
                // shared captured cell — it must never be considered
                // for the Locals::Stack representation.
                creates_block: true,
                code: vec![
                    Op::LoadLocal(0),
                    Op::LoadLocal(1),
                    Op::ApplyCall(call_id, u16::MAX),
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 4],
                filename: "<synthetic>".into(),
                // Synthetic forwarder protos have no body-
                // introduced locals; every slot they touch is
                // either filled at invoke time (block params /
                // rest) or written by the proto's own emitted
                // ops. `u16::MAX` skips the per-invocation reset.
                block_body_local_start: u16::MAX,
                byte_literals: Vec::new(),
                const_chains: Vec::new(),
                lexical_scope: Vec::new(),
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            self.callable_forwarder_proto = Some(idx);
            idx
        };

        // captured[0] = the callable; captured[1] left to
        // invoke_block to populate with the rest Array.
        //
        // Pin the callable across maybe_gc — the Rc<RefCell<Vec>>
        // we just built is a Rust-local with no GC root yet (the
        // Block that would own it isn't alloc'd until after the
        // maybe_gc). Without the pin, STRESS_GC sweeps the
        // callable's slot between Vec construction and Block alloc;
        // the new Block alloc reuses the freed slot, and the
        // captured ObjId silently points at the Block itself —
        // invoke_block then panics when `.call` dispatches.
        let captured = Rc::new(RefCell::new(vec![callable.clone(), Value::Nil]));
        let mut g = crate::vm::PinGuard::new(self);
        g.pin(callable);
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let id = g.vm.heap.alloc(HeapObj::Block(crate::value::BlockHandle {
            proto_idx,
            captured,
            self_val: Value::Nil,
            lexical_cvar_class: None,
            param_start: 0,
            n_params: 0,
            rest_slot: Some(1),
            kw_rest_slot: None,
            // Synthetic forwarder over a fixed 2-slot scratch Vec, not
            // a method scope; its proto is `creates_block` anyway so
            // the share path never sees it.
            captured_is_method_scope: false,
        }));
        Ok(id)
    }

    /// Build a `Value::Block` that, when called with `*args`,
    /// invokes `outer.call(inner.(*args))`. Used by Method#`>>` /
    /// `<<` to express function composition. Both sides must be
    /// callable (BoundMethod or Block); validated by the caller.
    /// Proto is lazy-built and shared across all composition sites.
    pub(crate) fn coerce_compose_to_block(
        &mut self,
        outer: Value,
        inner: Value,
    ) -> Result<crate::value::ObjId, Trap> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        use crate::heap::HeapObj;
        use std::cell::RefCell;

        // Locals layout:
        //   slot 0: outer callable (runs second)
        //   slot 1: inner callable (runs first)
        //   slot 2: args Array (filled via rest_slot)
        let proto_idx = if let Some(idx) = self.method_compose_forwarder_proto {
            idx
        } else {
            let call_id = self.interner.intern("call");
            let proto = Proto {
                name: "<method-compose-forwarder>".to_string(),
                params: Vec::new(),
                n_required_positional: 0,
                n_required_post: 0,
                rest_param: None,
                kw_param_defaults: Vec::new(),
                kw_has_computed_default: Vec::new(),
                kw_rest_param: None,
                block_param: None,
                n_locals: 3,
                // Same as the callable-forwarder: closure-run proto,
                // locals live in a shared captured cell — never
                // Stack-eligible.
                creates_block: true,
                code: vec![
                    Op::LoadLocal(0),                   // [outer]
                    Op::LoadLocal(1),                   // [outer, inner]
                    Op::LoadLocal(2),                   // [outer, inner, args]
                    Op::ApplyCall(call_id, u16::MAX),   // [outer, inner_result]
                    Op::Call(call_id, 1, u16::MAX),     // [outer_result]
                    Op::Return,
                ],
                op_spans: vec![Span::ZERO; 6],
                filename: "<synthetic>".into(),
                // Synthetic forwarder protos have no body-
                // introduced locals; every slot they touch is
                // either filled at invoke time (block params /
                // rest) or written by the proto's own emitted
                // ops. `u16::MAX` skips the per-invocation reset.
                block_body_local_start: u16::MAX,
                byte_literals: Vec::new(),
                const_chains: Vec::new(),
                lexical_scope: Vec::new(),
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            self.method_compose_forwarder_proto = Some(idx);
            idx
        };
        let captured = Rc::new(RefCell::new(vec![outer.clone(), inner.clone(), Value::Nil]));
        let mut g = crate::vm::PinGuard::new(self);
        g.pin(outer);
        g.pin(inner);
        g.vm.maybe_gc();
        g.vm.check_alloc()?;
        let id = g.vm.heap.alloc(HeapObj::Block(crate::value::BlockHandle {
            proto_idx,
            captured,
            self_val: Value::Nil,
            lexical_cvar_class: None,
            param_start: 0,
            n_params: 0,
            rest_slot: Some(2),
            kw_rest_slot: None,
            // Synthetic compose-forwarder scratch Vec, not a method
            // scope; proto is `creates_block` regardless.
            captured_is_method_scope: false,
        }));
        Ok(id)
    }

    /// Propagate a single-slot write made inside a block frame
    /// to every enclosing scope's storage that owns this slot
    /// index. The block-locals model uses a per-invocation fresh
    /// Vec for each `invoke_block`; outer-scope writes need to
    /// reach (a) the BlockHandle's `captured` Rc (which is the
    /// outer block's CURRENT-invocation fresh Vec, still on the
    /// frame stack) and possibly (b) further outer scopes if
    /// nested block frames also hold their own writebacks.
    /// Walking stops as soon as the slot index sits in the
    /// current target frame's OWN range (`>= param_start` for
    /// that frame's block proto), because then the target frame
    /// IS the canonical storage for the slot.
    ///
    /// Called from every `Op::StoreLocal` / `Op::IncLocal` /
    /// `Op::IncLocalNoPush` site whose write hit slot
    /// `< frame.block_writeback.1` (i.e. an outer-scope write
    /// from inside a block frame).
    pub(crate) fn propagate_outer_write(&self, slot: usize, v: &Value) {
        let frame = self.frames.last().expect("ICE: propagate_outer_write no frame");
        let mut target = match &frame.block_writeback {
            Some((p, _)) => p.clone(),
            None => return,
        };
        loop {
            {
                let mut t = target.borrow_mut();
                if slot < t.len() {
                    t[slot] = v.clone();
                }
            }
            // Is `target` the locals of another block frame still
            // on the stack? If so AND `slot` is still in THAT
            // frame's outer scope, walk further; otherwise stop.
            let outer = self
                .frames
                .iter()
                .rposition(|f| {
                    f.is_block
                        && f.locals
                            .as_shared()
                            .is_some_and(|l| Rc::ptr_eq(l, &target))
                });
            match outer {
                Some(idx) => match &self.frames[idx].block_writeback {
                    Some((parent, ps)) if slot < *ps as usize => {
                        target = parent.clone();
                    }
                    _ => return,
                },
                None => return,
            }
        }
    }

    /// Walk the frame stack to find the topmost non-block frame
    /// whose `locals` Rc matches the lexical-owner identity rooted
    /// at the given `seed` Rc. With the per-invocation block-locals
    /// model (`invoke_block` clones a fresh Vec into the new block
    /// frame's locals and saves the original `captured` Rc on
    /// `block_writeback`), a simple `Rc::ptr_eq(&f.locals, &seed)`
    /// search misses the enclosing method when the seed is a
    /// block frame's fresh Vec. This helper follows the writeback
    /// chain — each block frame whose `f.locals` equals the
    /// current `seed` provides a `block_writeback.0` that points
    /// one scope outward (toward the method) — until either a
    /// method-frame match is found, or the chain ends.
    ///
    /// Returns the frame index, or `None` if the lexical owner
    /// isn't on the stack (the block escaped its scope — stored
    /// as a Proc and invoked from elsewhere).
    pub(crate) fn find_lexical_owner_frame(
        &self,
        seed: &Rc<RefCell<Vec<Value>>>,
    ) -> Option<usize> {
        let mut target = seed.clone();
        loop {
            if let Some(idx) = self
                .frames
                .iter()
                .rposition(|f| {
                    !f.is_block
                        && f.locals
                            .as_shared()
                            .is_some_and(|l| Rc::ptr_eq(l, &target))
                })
            {
                return Some(idx);
            }
            // Not a method frame match — see if the target Rc
            // corresponds to a still-live block frame whose
            // writeback points one more scope outward.
            let outer_idx = self
                .frames
                .iter()
                .rposition(|f| {
                    f.is_block
                        && f.locals
                            .as_shared()
                            .is_some_and(|l| Rc::ptr_eq(l, &target))
                });
            match outer_idx {
                Some(idx) => match &self.frames[idx].block_writeback {
                    Some((parent, _)) => target = parent.clone(),
                    None => return None,
                },
                None => return None,
            }
        }
    }

    /// Single-positional-arg fast path for the hot Rust-level iter
    /// drivers (`times` / Array `each`/`map`/filter / Hash key-value
    /// walks): skips the per-iteration args-Vec allocation, the
    /// kw-rest peel, the auto-splat probe, and the rest/kw-rest
    /// binding block of the general path. Blocks with a rest or
    /// kw-rest slot, or with >1 params (auto-splat semantics), fall
    /// back to `invoke_block` — the fallback re-reads the block
    /// handle, which is fine for that rare shape. Locals setup and
    /// the Frame it pushes are byte-identical to the general path.
    pub(crate) fn invoke_block1(&mut self, block_id: ObjId, arg: Value) -> Result<(), Trap> {
        self.check_frames()?;
        let (proto_idx, captured, self_val, param_start, n_params, rest_slot, kw_rest_slot, bh_lexical_cvar_class, captured_is_method_scope) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.self_val.clone(),
             bh.param_start, bh.n_params, bh.rest_slot, bh.kw_rest_slot, bh.lexical_cvar_class.clone(), bh.captured_is_method_scope)
        };
        if rest_slot.is_some() || kw_rest_slot.is_some() || n_params > 1 {
            return self.invoke_block(block_id, vec![arg]);
        }
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        let body_local_start = proto.block_body_local_start;
        let (block_cell, writeback) =
            self.block_frame_locals(&captured, proto_idx, needed, param_start, captured_is_method_scope);
        {
            let mut locals = block_cell.borrow_mut();
            if (body_local_start as usize) < needed {
                for slot in body_local_start as usize..needed {
                    locals[slot] = Value::Nil;
                }
            }
            if n_params == 1 {
                locals[param_start as usize] = arg;
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: crate::vm::Locals::Shared(block_cell),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None,
            lexical_cvar_class: bh_lexical_cvar_class,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: true, n_given_positional: 0, kw_given_mask: 0, aux: None, pending_yield: false,
            block_writeback: writeback,
        });
        Ok(())
    }

    /// Two-positional-args twin of `invoke_block1`, for the
    /// `|k, v|` iter drivers (Hash#each / reduce / sort blocks /
    /// each_with_index). A 2-param plain block binds both args
    /// directly — no per-iteration args Vec, and the caller can
    /// skip materializing a pair Array + the auto-splat re-clone
    /// (Hash#each paid three allocations per pair). Anything else
    /// (rest / kw-rest / n_params != 2) falls back to the general
    /// path with the exact Vec the old call sites built.
    pub(crate) fn invoke_block2(&mut self, block_id: ObjId, a: Value, b: Value) -> Result<(), Trap> {
        self.check_frames()?;
        let (proto_idx, captured, self_val, param_start, n_params, rest_slot, kw_rest_slot, bh_lexical_cvar_class, captured_is_method_scope) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.self_val.clone(),
             bh.param_start, bh.n_params, bh.rest_slot, bh.kw_rest_slot, bh.lexical_cvar_class.clone(), bh.captured_is_method_scope)
        };
        if rest_slot.is_some() || kw_rest_slot.is_some() || n_params != 2 {
            return self.invoke_block(block_id, vec![a, b]);
        }
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        let body_local_start = proto.block_body_local_start;
        let (block_cell, writeback) =
            self.block_frame_locals(&captured, proto_idx, needed, param_start, captured_is_method_scope);
        {
            let mut locals = block_cell.borrow_mut();
            if (body_local_start as usize) < needed {
                for slot in body_local_start as usize..needed {
                    locals[slot] = Value::Nil;
                }
            }
            locals[param_start as usize] = a;
            locals[param_start as usize + 1] = b;
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: crate::vm::Locals::Shared(block_cell),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None,
            lexical_cvar_class: bh_lexical_cvar_class,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: true, n_given_positional: 0, kw_given_mask: 0, aux: None, pending_yield: false,
            block_writeback: writeback,
        });
        Ok(())
    }

    pub(crate) fn invoke_block(&mut self, block_id: ObjId, mut args: Vec<Value>) -> Result<(), Trap> {
        self.check_frames()?;
        // Snapshot what we need out of the block's heap slot before
        // taking any `&mut self` action. BlockHandle.captured is a
        // shared `Rc<RefCell<Vec<Value>>>` — cheap to clone.
        let (proto_idx, captured, self_val, param_start, n_params, rest_slot, kw_rest_slot, bh_lexical_cvar_class, captured_is_method_scope) = {
            let bh = self.heap.block(block_id);
            (bh.proto_idx, bh.captured.clone(), bh.self_val.clone(),
             bh.param_start, bh.n_params, bh.rest_slot, bh.kw_rest_slot, bh.lexical_cvar_class.clone(), bh.captured_is_method_scope)
        };
        // `|**opts|` keyword-rest: peel the trailing kwargs Hash off
        // the args BEFORE positional binding (so it doesn't land in
        // a positional slot or the auto-splat), defaulting to an
        // empty Hash. CRuby: `proc { |a, **o| }.call(1, x: 2)` →
        // a=1, o={x:2}; `proc { |**o| }.call` → o={}. The kwargs
        // arrive as a trailing positional Hash (verified: a block
        // called with `k: v` receives `[{k=>v}]`).
        let kw_rest_value: Option<Value> = if kw_rest_slot.is_some() {
            // Only treat a trailing Hash as kwargs; otherwise the
            // block was called with no keywords → bind `{}`.
            if matches!(args.last(), Some(Value::Hash(_))) {
                args.pop()
            } else {
                None
            }
        } else {
            None
        };
        // CRuby auto-splat: when a block declared with >1 parameter
        // is called with a single Array argument, the Array's
        // elements are spread into the parameter slots. The most
        // common ergonomic surfaces this enables:
        //   arr_of_pairs.each { |a, b| ... }       # arr = [[1,2], [3,4]]
        //   hash.each_with_index { |(k, v), i| }   # pair + index
        //   hash.to_a.sort_by { |k, v| v }         # pair after Hash#to_a
        // Hash#each / #map already yield two args directly, so this
        // path doesn't change their behaviour. Single-param blocks
        // also unaffected — they bind the whole Array.
        //
        // Auto-splat doesn't apply to rest-param blocks — `|*args|`
        // wants to capture the whole arg list, including a single
        // Array as-is.
        let args: Vec<Value> = if n_params > 1 && args.len() == 1 && rest_slot.is_none() {
            match &args[0] {
                Value::Array(aid) => self.heap.array(*aid).clone(),
                _ => args,
            }
        } else {
            args
        };
        // Build the rest Array (if any) BEFORE taking the locals
        // borrow — heap.alloc needs &mut self.heap, which conflicts
        // with the captured.borrow_mut() below.
        //
        // GC rooting: at this point the caller has popped the
        // Value::Block(block_id) off the operand stack (see
        // `do_call_block` and the Block.call arm in `do_call`),
        // so the only live reference to the block + its captured
        // Vec is this fn's `block_id` parameter — *not* a GC root.
        // Without pinning, the maybe_gc below would sweep the
        // Block's heap slot and (transitively) every captured
        // BoundMethod/Block held inside `captured`. The new alloc
        // could reuse the freed slot, and the forwarder would
        // dispatch through a dangling ObjId. Reproduced under
        // STRESS_GC=1 by `proc_curry_compose.rb`'s `(succ >> m).(4)`
        // — composing a Block with a BoundMethod produces a
        // compose-forwarder Block with `rest_slot = Some(2)`, so
        // this branch fires; the Squared instance held inside
        // `m`'s BoundMethod gets swept between pop and the
        // recursive `m.call`, panicking later at heap.rs's
        // `class_of called on non-Object slot`.
        // Build the rest-array AND the kw-rest binding under ONE
        // PinGuard so the peeled kwargs Hash, every heap-shaped
        // rest-arg element, and any freshly-alloc'd `{}` all stay
        // rooted across the maybe_gc/alloc calls. (`rest_args` and
        // the peeled `kw_rest_value` are Rust-local Values with no
        // GC root — under STRESS_GC an unrooted Hash element would
        // be swept and the later alloc would store a dangling
        // ObjId; same hazard the callable_coerce.rb / proc_curry_
        // compose.rb fixtures pinned against.)
        let (rest_array_val, kw_rest_final): (SlotBinding, SlotBinding) = if rest_slot.is_none()
            && kw_rest_slot.is_none()
        {
            // No rest / kw-rest slots — nothing below allocates or
            // calls maybe_gc, so skip the PinGuard entirely (its
            // construct+drop showed up at ~2% of tight block loops).
            (None, None)
        } else {
            let mut g = crate::vm::PinGuard::new(self);
            g.pin(Value::Block(block_id));
            if let Some(v) = &kw_rest_value { g.pin(v.clone()); }
            let rest = if let Some(slot) = rest_slot {
                let rest_args: Vec<Value> = args.iter().skip(n_params as usize).cloned().collect();
                for a in &rest_args { g.pin(a.clone()); }
                g.vm.maybe_gc();
                g.vm.check_alloc()?;
                let id = g.vm.heap.alloc(HeapObj::Array(rest_args.into()));
                Some((slot, Value::Array(id)))
            } else {
                None
            };
            let kwr = if let Some(slot) = kw_rest_slot {
                // The peeled kwargs Hash, or a fresh `{}` (CRuby
                // binds `{}` when the block was called with no
                // keywords).
                let v = match &kw_rest_value {
                    Some(h) => h.clone(),
                    None => {
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        Value::Hash(g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new()))))
                    }
                };
                Some((slot, v))
            } else {
                None
            };
            (rest, kwr)
        };
        let proto = &self.protos[proto_idx];
        let needed = proto.n_locals as usize;
        let body_local_start = proto.block_body_local_start;
        // Per-invocation locals isolation (the .each-capture-leak
        // fix surfaced by sinatra_plugin_smoke):
        //
        //   `[:a,:b,:c].map { |s| -> { s } }.map(&:call)`
        //
        // used to return `[:c, :c, :c]` because every Lambda's
        // captured Rc pointed at the SAME outer locals Vec, which
        // the outer .map overwrote each iteration. Fix: each
        // invocation gets its own fresh Vec cloned from the
        // BlockHandle's `captured`. Inner closures created during
        // this invocation capture the fresh Rc (independent of the
        // next iteration's), so their slot reads land on this
        // invocation's values.
        //
        // To preserve closure-write-through to outer-method scope
        // (e.g. `counter = 0; arr.each { counter += 1 }; counter`),
        // we remember the original `captured` Rc plus this block's
        // `param_start`. At Op::Return the lower `[0..param_start]`
        // portion of the fresh Vec — slots owned by the surrounding
        // method / outer blocks — is copied back into the original
        // Rc. This keeps the active-invocation outer-write-through
        // working; only the post-pop write-through (a *detached*
        // inner closure mutating outer-method vars AFTER its
        // outer block frame has popped) is a documented Tier-1
        // divergence — see the Op::Return write-back arm and
        // SUBSET.md for the trade-off.
        // Snapshot the captured outer scope into a fresh (pool-reused)
        // cell sized to `needed` — or share the captured Vec directly
        // for a non-capturing, non-re-entrant block (no per-iteration
        // copy). See `block_frame_locals`.
        let (block_cell, writeback) =
            self.block_frame_locals(&captured, proto_idx, needed, param_start, captured_is_method_scope);
        {
            let mut locals = block_cell.borrow_mut();
            // Reset body-introduced block-local slots before
            // rebinding params. CRuby's "block-locals are fresh
            // each invocation" semantics: a variable
            // first-assigned inside the block body (e.g.
            // `y = 100 if cond`, `n ||= 0`, plain `tmp = expr`)
            // sees `nil` at the top of every call, even when an
            // earlier invocation assigned it. Outer-scope
            // variables (slot index < parent.n_locals at compile
            // time) and the block's own params keep their
            // values across invocations because their slot
            // indices sit below `body_local_start`.
            //
            // `block_body_local_start == u16::MAX` is the
            // sentinel for "not a block-shaped proto" — set by
            // `ProtoBuilder::build` and by the cext synthetic
            // forwarders. The branch is also a no-op when the
            // block body assigned no new locals (start equals
            // n_locals).
            if (body_local_start as usize) < needed {
                for slot in body_local_start as usize..needed {
                    locals[slot] = Value::Nil;
                }
            }
            // Place args into the block's required param slots.
            // CRuby's arity-mismatch semantics: too few args →
            // leftover slots bind to Nil. Overflow past n_params
            // either flows into the rest slot (handled below) or
            // is silently dropped (block-arity-permissive default).
            let mut it = args.into_iter();
            for i in 0..n_params as usize {
                locals[param_start as usize + i] = it.next().unwrap_or(Value::Nil);
            }
            if let Some((slot, val)) = rest_array_val {
                locals[slot as usize] = val;
            }
            if let Some((slot, val)) = kw_rest_final {
                locals[slot as usize] = val;
            }
        }
        self.frames.push(Frame {
            proto_idx,
            ip: 0,
            locals: crate::vm::Locals::Shared(block_cell),
            self_val,
            base_sp: self.stack.len(),
            is_class_body: false, swap_return: None, block_arg: None, defining_class: None,
            lexical_cvar_class: bh_lexical_cvar_class,
            #[cfg(feature = "regex")] saved_last_match: None,
            is_block: true, n_given_positional: 0, kw_given_mask: 0, aux: None, pending_yield: false,
            block_writeback: writeback,
        });
        Ok(())
    }



    pub(crate) fn do_call_block(&mut self, name_id: SymId, argc: usize, no_recv: bool, cache_id: u16) -> Result<(), Trap> {
        let name = self.interner.resolve(name_id).clone();
        // Consume `bypass_visibility_once` at the dispatch boundary
        // — same reasoning as `do_call`. `do_call_block` itself
        // has no visibility-check site today (block-form
        // private/protected enforcement is a pre-existing gap), so
        // the consumed value is mostly there to prevent leaking
        // past the block-form `send`/`__send__` re-aim into the
        // next unrelated call. The `&nil` arm below re-installs
        // it before delegating to `do_call`, which DOES enforce
        // visibility — so `send(:priv, &nil)` still bypasses.
        let bypass_visibility = self.take_bypass_visibility();
        // Monomorphic inline-cache fast path for the common shape:
        // `obj.method(args) { block }` where `obj` is a user Object,
        // the block is a literal, and `method` is a plain `def`. This
        // skips the whole dispatch preamble + the heap args-Vec drain
        // below. The fast path only fires for Public methods, so
        // consuming `bypass_visibility` above is harmless — mirrors the
        // no-block `do_call` placement. On any non-match it returns
        // `Ok(false)` with the stack UNCHANGED, so the full path below
        // runs exactly as before.
        if !no_recv && self.try_invoke_explicit_recv_block_cached(name_id, argc, cache_id)? {
            return Ok(());
        }
        let split = self.stack.len() - argc;
        let args: Vec<Value> = self.stack.drain(split..).collect();
        let block_val = self.stack.pop().expect("ICE: stack underflow before block");
        // GC rooting around the `&callable` coerce. After draining
        // args into a Rust Vec and popping block_val, both are
        // unrooted Rust locals — `coerce_callable_to_block`'s
        // `maybe_gc` would otherwise sweep any heap-shaped arg
        // (Hash / Array / Object). STRESS_GC repro:
        // `callable_coerce.rb`'s
        // `deliver({"X" => "ok"}, &app.method(:call))` shape
        // (block_val is a BoundMethod, args contains a Hash).
        // PinGuard's mutable-borrow lifetime can't span the full
        // function (the borrow checker won't let other &mut self
        // sites use `self` while the guard is alive), so we pin
        // only inside the BoundMethod / CurriedProc arms — the
        // exact window where coerce_callable_to_block fires.
        let block = match block_val {
            Value::Block(id) => id,
            // `&method_object` forwarding (K8): coerce the
            // BoundMethod into a Block via `to_proc` semantics.
            // Synthesises a vararg-lambda whose captured locals
            // hold the BoundMethod; when invoked, it does
            // `m.call(*args)`. See `coerce_callable_to_block`.
            Value::BoundMethod(bm_id) => {
                let mut g = crate::vm::PinGuard::new(self);
                for a in &args { g.pin(a.clone()); }
                g.vm.coerce_callable_to_block(Value::BoundMethod(bm_id))?
            }
            // `&curried_proc` — a curried proc is still a Proc in
            // CRuby, so `&` on it forwards as a block. Same shape
            // as the BoundMethod arm: the synthesised forwarder
            // does `cp.call(*args)`, and `CurriedProc#call`
            // (dispatch.rs:1159) handles arity-completion / partial
            // application from there.
            Value::CurriedProc(cp_id) => {
                let mut g = crate::vm::PinGuard::new(self);
                for a in &args { g.pin(a.clone()); }
                g.vm.coerce_callable_to_block(Value::CurriedProc(cp_id))?
            }
            // `foo(&nil)` in CRuby is equivalent to `foo` without
            // a block. Common shape: `def render(&block);
            // evaluate(&block); end` invoked without a block ⇒
            // `block` is Nil, the `&block` forwarding becomes
            // `evaluate(&nil)`. Restore args to the stack and
            // delegate to the no-block dispatch path.
            Value::Nil => {
                // Re-install the visibility-bypass flag we consumed
                // at entry. `send(:priv_method, &nil)` should still
                // bypass visibility — without this, `do_call` would
                // raise NoMethodError on a private method because
                // its own bypass slot is now `false`.
                self.bypass_visibility_once = bypass_visibility;
                for a in args { self.stack.push(a); }
                return self.do_call(name_id, argc, no_recv, cache_id);
            }
            // Anything else (Int / Str / ...) is a real type error
            // — CRuby raises `TypeError: wrong argument type X
            // (expected Proc)`, where X is the class name (e.g.
            // "Integer", "TrueClass", or a user class), NOT
            // `type_name()`'s short tag ("Boolean", etc.). Use
            // `class_of` so the message matches CRuby for booleans
            // and user instances.
            other => {
                let class_name = match self.class_of(&other) {
                    Value::Class(c) => c.name.clone(),
                    _ => other.type_name().to_string(),
                };
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "wrong argument type {} (expected Proc)",
                        class_name,
                    ),
                }));
            }
        };
        let recv = if no_recv {
            None
        } else {
            Some(self.stack.pop().expect("ICE: stack underflow before block receiver"))
        };

        // Reopen-precedence early gate — block-form twin of
        // do_call's (`5.times { }` with a user `def times` on
        // Integer must invoke the reopen WITH the block, not the
        // native iter driver). Same mask/own-table contract; see
        // do_call for the rationale and the operator-syntax
        // boundary note.
        {
            if self.fast_index_checked_gen != self.method_gen {
                self.fast_index_revalidate();
            }
            if self.prim_reopen_mask != 0
                && let Some(r) = &recv
            {
                let bit: u8 = match r {
                    Value::Int(_) => 0,
                    #[cfg(feature = "bignum")]
                    Value::BigInt(_) => 0,
                    Value::Float(_) => 1,
                    Value::Str(_) => 2,
                    Value::Sym(_) => 3,
                    Value::Nil => 4,
                    Value::Bool(_) => 5,
                    Value::Rational(_) => 6,
                    _ => 7,
                };
                if bit < 7
                    && self.prim_reopen_mask & (1 << bit) != 0
                    && let Value::Class(cls) = self.class_of(r)
                {
                    let m = cls.methods.borrow().get(&name_id).cloned();
                    if let Some(m) = m {
                        let r = r.clone();
                        self.invoke_method_with_block(m, r, args, Some(block))?;
                        return Ok(());
                    }
                }
            }
        }

        // Bare `instance_exec { ... }` / `instance_eval { ... }`
        // inside an instance method — `recv` is None, so the
        // receiver-form arm below won't see it. Dispatch on `self`
        // from the current frame, mirroring
        // `self.instance_exec(&block)` / `self.instance_eval(&block)`.
        // Same override-precedence probe as the receiver-form arm
        // so a user-defined method still wins.
        //
        // The two share the same shape because at the dispatch
        // level instance_eval and instance_exec are
        // indistinguishable for the no-args / block-only form
        // (the difference — instance_exec passes call args as
        // block args, instance_eval doesn't — only matters when
        // args are present, and we have no args here by virtue of
        // the no_recv block path). rack-cors hits this via
        // `instance_eval(&block)` inside its initialize when the
        // user passes a configuration block.
        if no_recv && matches!(&*name, "instance_exec" | "instance_eval") {
            let self_val = self.frames.last().expect("ICE: do_call_block no frame").self_val.clone();
            let user_override = match &self_val {
                Value::Object(id) => {
                    let cls = self.heap.class_of(*id);
                    self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                }
                Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                _ => match self.class_of(&self_val) {
                    Value::Class(cls) => self.lookup_method_cached(&cls, name_id, cache_id).is_some(),
                    _ => false,
                },
            };
            if !user_override {
                self.invoke_block_with_self(block, self_val, /*as_class_body=*/false, args)?;
                return Ok(());
            }
            // User override exists — re-shape stack as receiver form
            // (`recv, block, args...`) and re-enter so the normal
            // dispatch finds and invokes the user method.
            let argc = args.len();
            self.stack.push(self_val);
            self.stack.push(Value::Block(block));
            for a in args { self.stack.push(a); }
            return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
        }

        // `bm.call(args, &block)` — the block-form counterpart to
        // the no-block BoundMethod#call arm in `do_call` (line
        // ~1969). Without this, calling a stored `Method` with a
        // block (`@scan_line.call(@src, &block)` — ERB's
        // lib/erb/compiler.rb:147 pattern) raises NoMethodError
        // because the fallthrough never sees Method as a valid
        // receiver. Re-shape the stack as
        // `bm_recv, block, args...` (the order do_call_block
        // expects — see the push sequence below) and recursively
        // dispatch through `do_call_block` so the underlying
        // method receives the block argument.
        if let Some(Value::BoundMethod(bid)) = &recv
            && matches!(&*name, "call" | "[]" | "()") {
            let (bm_recv, bm_name_id, bm_method) = match self.heap.get(*bid) {
                HeapObj::BoundMethod { recv, name_id, method } => {
                    (recv.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: BoundMethod slot holds non-BoundMethod"),
            };
            // Snapshot fast path — invoke directly with the
            // attached block, matching the no-block BoundMethod#call
            // arm's parity with capture-then-remove-then-call.
            if let Some(m) = bm_method {
                self.invoke_method_with_block(m, bm_recv, args, Some(block))?;
                return Ok(());
            }
            // do_call_block entry expects stack layout
            // `recv, block, args...` (drain last `argc` for args,
            // then pop block, then pop recv). Push in that order.
            let argc_new = args.len();
            self.stack.push(bm_recv);
            self.stack.push(Value::Block(block));
            for a in args { self.stack.push(a); }
            return self.do_call_block(bm_name_id, argc_new, false, u16::MAX);
        }
        // `ubm.bind_call(recv, *args, &block)` — block-form parallel
        // of the no-block bind_call arm in `try_dispatch_callable_intrinsics`
        // (line ~690). That arm runs via `do_call`'s pre-block
        // dispatch path and never sees a block argument; tilt's
        // `method.bind_call(scope, **locals, &block)` (template.rb:
        // ~392) passes one, which lands here. Without this arm
        // the call raises NoMethodError even though the blockless
        // shape succeeds.
        if let Some(Value::UnboundMethod(uid)) = &recv && &*name == "bind_call" {
            if args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "wrong number of arguments (given 0, expected 1..)".into(),
                }));
            }
            let (cap_class, cap_name_id, cap_method) = match self.heap.get(*uid) {
                HeapObj::UnboundMethod { class, name_id, method } => {
                    (class.clone(), *name_id, method.clone())
                }
                _ => panic!("ICE: UnboundMethod slot holds non-UnboundMethod"),
            };
            let mut args = args;
            let target = args.remove(0);
            // Dispatch class for Object targets — mirrors the
            // eigenclass-aware capture in unbind so a
            // singleton-method UnboundMethod can bind_call back
            // to its original receiver.
            let target_class = match &target {
                Value::Object(id) => self.heap.class_of(*id),
                _ => match self.class_of(&target) {
                    Value::Class(c) => c,
                    _ => return Err(self.trap(RubyError::TypeError {
                        msg: format!("bind_call argument must have a class (got {})", target.type_name()),
                    })),
                },
            };
            // Same is_a fence as the no-block path: Kernel sentinel
            // and any Module captured class are exempt; Class
            // capture is strict.
            if cap_class.name.as_str() != "Kernel"
                && !cap_class.is_module
                && !super::class_is_a(&target_class, &cap_class) {
                return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "bind_call argument must be an instance of {} (got {})",
                        cap_class.name, target_class.name,
                    ),
                }));
            }
            let m = match cap_method.or_else(|| self.lookup_method_uncached(&cap_class, cap_name_id)) {
                Some(m) => m,
                None => {
                    let mname = self.interner.resolve(cap_name_id).to_string();
                    return Err(self.trap(RubyError::NameError {
                        msg: format!("undefined method '{}' for class '{}'", mname, cap_class.name),
                    }));
                }
            };
            self.invoke_method_with_block(m, target, args, Some(block))?;
            return Ok(());
        }

        // P2-13: `block` (now an ObjId in a Rust local) is no
        // longer rooted after popping off the stack. Each native
        // iterator driver (`iter_array_filter`, the inline
        // `each` / `map` arms, etc.) pins the block alongside its
        // source receiver, so we don't need a guard at the
        // dispatch boundary itself. The `invoke_method_with_block`
        // path on the no_recv / Object-recv branches doesn't
        // trigger GC before installing the block as the frame's
        // `block_arg`, so the gap is safe there too.
        //
        // `Hash.new { |h, k| ... }` interception. Parallel to the
        // no-block arm in `do_call`. The block becomes the Hash's
        // default-block (stored in `HashObj.default_block` for GC
        // and access). `Hash#[]` consults this slot on missing
        // keys and invokes the block with `(self_hash, key)` —
        // tilt's `Hash.new { |h, k| h[k] = [] }` auto-vivifies.
        //
        // `Hash.new(default) { block }` is an ArgumentError in
        // CRuby ("wrong number of arguments (given 1, expected 0)"
        // from Hash#initialize when both default-arg and block are
        // given). Mirror that explicitly so callers don't see the
        // misleading generic Class.new fallback behaviour.
        // `Module.new { |m| ... }` — anonymous Module with the
        // block evaluated as the module body (`class_eval`-style).
        // The block also receives the new module as its sole arg
        // for explicit-reference shapes like `Module.new { |m|
        // m.define_method(:foo) { ... } }`. Sits BEFORE the
        // `Hash.new` intercept so the Module-class-receiver path
        // isn't swallowed by a hypothetical future shared
        // pattern.
        // `Class.new { ... }` / `Class.new(SuperClass) { ... }` —
        // anonymous Class with the block evaluated as the class body
        // (`class_eval`-style). The new class's superclass defaults
        // to Object (CRuby's documented default); an explicit Class
        // arg overrides. The block ALSO receives the new class as
        // its sole positional arg, matching CRuby's
        // `Class.new(Parent) { |k| ... }` shape that delegate.rb's
        // `DelegateClass(K)` uses to define helper methods on the
        // returned anonymous class. Sits BEFORE the universal
        // Class-instance allocator further down so the block path
        // isn't swallowed by the bare-Instance fallback.
        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Class"
        {
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            // 0 or 1 positional arg — anything else is ArgumentError.
            // The single-arg form is the explicit superclass.
            let explicit_super: Option<Rc<Class>> = match args.as_slice() {
                [] => None,
                [Value::Class(sc)] if !sc.is_module => Some(sc.clone()),
                [Value::Class(_)] => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "superclass must be an instance of Class (given an instance of Module)".to_string(),
                    }));
                }
                [other] => {
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!("superclass must be an instance of Class (given an instance of {})", other.type_name()),
                    }));
                }
                _ => {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!(
                            "wrong number of arguments (given {}, expected 0..1)",
                            args.len(),
                        ),
                    }));
                }
            };
            // Default to Object when no explicit superclass — same
            // shape `Op::DefClass`'s default-parent code uses (see
            // step.rs around the BasicObject fence).
            let object_sym = self.interner.intern("Object");
            let parent = explicit_super.or_else(|| self.classes.get(&object_sym).cloned());
            let new_cls = std::rc::Rc::new(Class {
                name: String::new(),
                is_module: false,
                ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                superclass: std::cell::RefCell::new(parent),
                includes: std::cell::RefCell::new(Vec::new()),
                prepends: std::cell::RefCell::new(Vec::new()),
                singleton_prepends: std::cell::RefCell::new(Vec::new()),
                singleton_includes: std::cell::RefCell::new(Vec::new()),
                singleton_view: std::cell::RefCell::new(None),
                singleton_target: std::cell::RefCell::new(None),
                class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: std::cell::RefCell::new(None),
                #[cfg(feature = "cext")]
                cext_alloc_func: std::cell::Cell::new(None),
            });
            // Fire the parent's `inherited(subclass)` hook
            // BEFORE the class-body block runs. CRuby's
            // ordering: `Class.new(P) do BODY end` invokes
            // `P.inherited(new_cls)` and then evaluates BODY
            // against new_cls. Mustermann's
            // `class NodeTranslator < DelegateClass(Node)` AST
            // construction relies on the hook running so that
            // `subclass.const_set(:NodeTranslator, ...)` lands
            // before any subsequent `translate(...)` block can
            // call `const_get(:NodeTranslator)`.
            self.invoke_inherited_hook(&new_cls)?;
            let cls_val = Value::Class(new_cls);
            self.invoke_block_with_self(
                block,
                cls_val.clone(),
                /*as_class_body=*/ true,
                vec![cls_val],
            )?;
            return Ok(());
        }

        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Module"
        {
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                // User-defined Module.new singleton wins, parallel
                // to the Hash precedence rule below.
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len(),
                    ),
                }));
            }
            // Build the fresh module shell. Same field set as the
            // no-block `do_call` arm; lifted here so the block can
            // run inside the module body and the result push lands
            // on this control path.
            let new_mod = std::rc::Rc::new(Class {
                name: String::new(),
                is_module: true,
                ivars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                singleton_methods: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
                superclass: std::cell::RefCell::new(None),
                includes: std::cell::RefCell::new(Vec::new()),
                prepends: std::cell::RefCell::new(Vec::new()),
                singleton_prepends: std::cell::RefCell::new(Vec::new()),
                singleton_includes: std::cell::RefCell::new(Vec::new()),
                singleton_view: std::cell::RefCell::new(None),
                singleton_target: std::cell::RefCell::new(None),
                class_vars: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            consts: std::cell::RefCell::new(crate::intern::FxHashMap::default()),
            assigned_name: std::cell::RefCell::new(None),
                #[cfg(feature = "cext")]
                cext_alloc_func: std::cell::Cell::new(None),
            });
            let mod_val = Value::Class(new_mod);
            // `as_class_body=true` so `def name; …; end` inside
            // the block lands on the module's methods table. Same
            // machinery `class_eval` uses (`invoke_block_with_self`
            // pushes the module onto class_stack + sets
            // `is_class_body: true` on the new frame). The block
            // receives `mod_val` as its sole positional arg —
            // matches CRuby's `Module.new { |m| ... }` shape.
            self.invoke_block_with_self(
                block,
                mod_val.clone(),
                /*as_class_body=*/ true,
                vec![mod_val],
            )?;
            return Ok(());
        }
        // `Module#define_method(:name) { |args| body }` —
        // dynamically install a block-as-method on the receiver
        // class's instance-methods table. Mirrors the
        // `Op::DefMethodBlock` opcode's install logic but is
        // entered via runtime dispatch rather than a parsed
        // `def`. Both shapes accepted:
        //   - explicit receiver: `cls.define_method(:foo) { ... }`
        //     → recv = Some(Value::Class(target))
        //   - bare-call inside `class_eval do ... end` where
        //     self is the class:
        //     `cls.class_eval { define_method(:foo) { ... } }`
        //     → no_recv = true, frame self_val = the class.
        //     Sinatra/base.rb's `define_singleton` uses this
        //     shape; the block_arg `&content` becomes the
        //     attached block.
        //
        // Closure semantics match DefMethodBlock: the
        // BlockHandle's `captured` Rc is shared with the
        // installed Method so outer-scope locals stay live.
        // CRuby returns the method name as a Symbol.
        // (TRY_RUNS pass-9.7d layer #21.)
        // `obj.define_singleton_method(:name) { ... }` —
        // runtime path covering the cases the compiler shortcut
        // at `compiler.rs:213` doesn't catch (dynamic dispatch
        // via __send__, `singleton_method`-returning methods,
        // etc.). For Value::Object the install target is the
        // receiver's eigenclass (materialized via
        // `ensure_singleton_class`); for Value::Class it goes
        // straight into the class's `singleton_methods` table
        // so `C.define_singleton_method(:foo) { ... }` adds a
        // class method. Primitive receivers raise NoMethodError
        // (CRuby reports "can't define singleton" TypeError, but
        // routing the dispatch arm to do that requires plumbing
        // we don't have here — Tier-2 polish).
        //
        // Known limitation: this short-circuit fires before
        // user-method lookup, so a user `def self.define_singleton_method`
        // override on a Class is shadowed. Mirrors the pre-existing
        // Object-extras precedence gap documented at iter.rs's
        // tap/itself comment (bb4df50c, PR #290 cycle-3).
        // The proper fix is a user-override probe (see the
        // `send` arm in dispatch.rs:513) applied to the whole
        // built-in install family — tracked as Tier-2 follow-up.
        if &*name == "define_singleton_method" {
            let target_recv = recv.clone().or_else(|| {
                self.frames.last().map(|f| f.self_val.clone())
            });
            // Arity matches `define_method`:
            //   1 → install the block (path below)
            //   2 → install args[1] (Proc/Method/UnboundMethod);
            //       CRuby silently drops any attached block
            let two_arg_form = args.len() == 2;
            match args.len() {
                1 | 2 => {}
                n => return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 1..2)", n),
                })),
            }
            let name_sym = match &args[0] {
                Value::Sym(s) => *s,
                Value::Str(s) => {
                    let raw = s.to_string_lossy();
                    if let Some(max) = self.max_symbols
                        && !self.interner.contains(&raw) && self.interner.len() >= max {
                            return Err(self.trap(RubyError::ResourceExhausted {
                                msg: format!("interner exhausted: {} symbols", max),
                            }));
                        }
                    self.interner.intern(&raw)
                }
                other => return Err(self.trap(RubyError::TypeError {
                    msg: format!(
                        "wrong argument type {} (expected Symbol or String)",
                        other.type_name(),
                    ),
                })),
            };
            // 2-arg form: skip the block payload and install
            // args[1] via the shared helpers. For Object recv
            // the install goes onto the eigenclass; for Class
            // recv it goes into cls.singleton_methods directly
            // (matching the block-form table-write below).
            if two_arg_form {
                let src = args[1].clone();
                let install_result = match &target_recv {
                    Some(Value::Object(id)) => {
                        let sc = self.heap.ensure_singleton_class(*id);
                        Some(self.install_method_from_value(
                            &sc,
                            name_sym,
                            &src,
                            crate::value::Visibility::Public,
                        ))
                    }
                    Some(Value::Class(c)) => Some(
                        self.install_singleton_method_on_class_from_value(
                            c, name_sym, &src,
                        ),
                    ),
                    _ => None,
                };
                if let Some(res) = install_result {
                    let installed = res.map_err(|e| self.trap(e))?;
                    // `singleton_method_added` fires on the receiver
                    // — for Object recv, on the underlying object
                    // (NOT on its eigenclass which is where the
                    // method physically lives). For Class recv this
                    // is already handled inside
                    // `install_singleton_method_on_class_from_value`.
                    if let Some(recv @ Value::Object(_)) = &target_recv {
                        self.fire_singleton_method_lifecycle_hook(
                            recv.clone(),
                            "singleton_method_added",
                            name_sym,
                        )?;
                    }
                    self.stack.push(Value::Sym(installed));
                    return Ok(());
                }
                // Non-{Object,Class} receiver — fall through to
                // the existing match below which raises
                // NoMethodError / ArgumentError as appropriate.
            }
            let (proto_idx, captured, param_start, n_params) = {
                let bh = self.heap.block(block);
                (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
            };
            let proto = &self.protos[proto_idx];
            let params = proto.params.clone();
            // `defining_class` anchors `super` lookups inside
            // the installed method. For Object receivers the
            // anchor is the eigenclass (super walks its
            // superclass chain into the original class);
            // for Class receivers the anchor is the class whose
            // `singleton_methods` table we're writing into, so
            // `super` inside a class method walks the metaclass
            // chain. Without an anchor, `super` raises
            // "outside of method" — mirrors the static
            // singleton install at step.rs:1273.
            let hook_recv: Value = match target_recv {
                Some(Value::Object(id)) => {
                    let sc = self.heap.ensure_singleton_class(id);
                    let m = std::rc::Rc::new(crate::value::Method {
                        params,
                        proto_idx,
                        fixed_arity: None,
                        defining_class: Some(std::rc::Rc::downgrade(&sc)),
                        visibility: std::cell::Cell::new(crate::value::Visibility::Public),
                        closure: Some(crate::value::MethodClosure { captured, param_start, n_params }),
                        builtin: None,
                        original_name: Some(name_sym),
                    });
                    sc.methods.borrow_mut().insert(name_sym, m);
                    Value::Object(id)
                }
                Some(Value::Class(c)) => {
                    let m = std::rc::Rc::new(crate::value::Method {
                        params,
                        proto_idx,
                        fixed_arity: None,
                        defining_class: Some(std::rc::Rc::downgrade(&c)),
                        visibility: std::cell::Cell::new(crate::value::Visibility::Public),
                        closure: Some(crate::value::MethodClosure { captured, param_start, n_params }),
                        builtin: None,
                        original_name: Some(name_sym),
                    });
                    c.singleton_methods.borrow_mut().insert(name_sym, m);
                    Value::Class(c)
                }
                Some(other) => return Err(self.trap(RubyError::NoMethodError {
                    kind: crate::error::NoMethodErrorKind::Missing,
                    method: format!("undefined method '{}' called", &*name),
                    recv_type: std::borrow::Cow::Borrowed(other.type_name()),
                })),
                None => return Err(self.trap(RubyError::ArgumentError {
                    msg: "no receiver for define_singleton_method".into(),
                })),
            };
            self.method_gen = self.method_gen.wrapping_add(1);
            // `singleton_method_added` fires on the receiver after
            // the block-form install lands.
            self.fire_singleton_method_lifecycle_hook(
                hook_recv,
                "singleton_method_added",
                name_sym,
            )?;
            self.stack.push(Value::Sym(name_sym));
            return Ok(());
        }
        if &*name == "define_method" {
            // Track whether we picked the target via explicit
            // receiver vs no_recv (bare call in class body). The
            // `class_visibility_stack` lexical-visibility lookup
            // below only makes sense for the no_recv path, where
            // the target IS the surrounding class body. For the
            // explicit-receiver path, the surrounding visibility
            // belongs to whatever class body we're currently in —
            // which may be unrelated to `target_cls`. Leaking the
            // caller's `private` onto methods installed on an
            // unrelated class diverges from CRuby
            // (code-review #245 round 7 #1).
            let (target_cls, explicit_recv) = match &recv {
                Some(Value::Class(c)) => (Some(c.clone()), true),
                None => {
                    let self_val = self.frames.last()
                        .expect("ICE: define_method no_recv with empty frames")
                        .self_val
                        .clone();
                    if let Value::Class(c) = self_val { (Some(c), false) } else { (None, false) }
                }
                _ => (None, false),
            };
            // Precedence rule (parallels `Module.new` / `Hash.new`):
            // a user-defined `def self.define_method(...)` on the
            // receiver (or its singleton-prepended chain) wins over
            // the built-in intrinsic. Without this check, override
            // attempts silently shadow into this arm. Only consult
            // when we actually resolved a target class — otherwise
            // fall through to normal dispatch (which will raise
            // NoMethodError on the non-Class receiver).
            if let Some(cls) = &target_cls
                && let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                let recv_val = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, recv_val, args, Some(block));
            }
            if let Some(target_cls) = target_cls {
                // Arity:
                //   0       → wrong-arity ArgumentError
                //   1       → install the attached block
                //   2       → Proc/Method/UnboundMethod install
                //             from args[1] via the shared
                //             helper. CRuby silently drops any
                //             attached block in this shape and
                //             uses args[1] — we honour that by
                //             routing through the `two_arg_form`
                //             branch below before reading the
                //             block payload.
                //   3+      → wrong-arity ArgumentError
                // CRuby's wording is `expected 1..2` even when a
                // block is attached, so we use the same message
                // across both arms (PR #245 Copilot round 6 #1).
                let two_arg_form = args.len() == 2;
                match args.len() {
                    1 | 2 => {}
                    n => return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("wrong number of arguments (given {}, expected 1..2)", n),
                    })),
                }
                let name_sym = match &args[0] {
                    Value::Sym(s) => *s,
                    Value::Str(s) => {
                        // Same `Config::max_symbols` cap as
                        // `parse_send_target` / `resolve_ivar_name_arg`
                        // — without this, untrusted code could grow
                        // the interner unbounded via
                        // `cls.define_method("dyn_#{i}") {}` in a loop.
                        // Existing symbols always re-resolve; only
                        // fresh names count against the cap.
                        let raw = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&raw) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        self.interner.intern(&raw)
                    }
                    other => return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Symbol or String)",
                            other.type_name(),
                        ),
                    })),
                };
                // Explicit-receiver path: visibility defaults to
                // Public (the new method's target class doesn't
                // share lexical scope with the caller's visibility
                // stack). No-recv (bare call in class body): inherit
                // the surrounding class's current visibility, since
                // `define_method` and `def` should behave the same
                // way under `private` / `public` modifiers.
                let vis = if explicit_recv {
                    crate::value::Visibility::Public
                } else {
                    self.class_visibility_stack.last().copied()
                        .unwrap_or(crate::value::Visibility::Public)
                };
                // 2-arg form (with or without an attached block —
                // CRuby silently drops the block and uses args[1]).
                // Route through the shared 2-arg installer; the
                // block-form path below remains for the 1-arg
                // case.
                if two_arg_form {
                    let src = args[1].clone();
                    let installed = self
                        .install_method_from_value(&target_cls, name_sym, &src, vis)
                        .map_err(|e| self.trap(e))?;
                    self.stack.push(Value::Sym(installed));
                    return Ok(());
                }
                let (proto_idx, captured, param_start, n_params) = {
                    let bh = self.heap.block(block);
                    (bh.proto_idx, bh.captured.clone(), bh.param_start, bh.n_params)
                };
                let proto = &self.protos[proto_idx];
                let params = proto.params.clone();
                let m = std::rc::Rc::new(crate::value::Method {
                    params,
                    proto_idx,
                    fixed_arity: None,
                    // When `target_cls` is an eigenclass shell from
                    // `Class#singleton_class`, the install redirects
                    // into the underlying real class's
                    // singleton_methods; `defining_class` has to
                    // resolve to the same real class so `super`
                    // walks the right ancestor chain.
                    // (Code-review #253 round 1 #1.)
                    defining_class: Some(std::rc::Rc::downgrade(&target_cls.effective_install_class())),
                    visibility: std::cell::Cell::new(vis),
                    closure: Some(crate::value::MethodClosure { captured, param_start, n_params }),
                    builtin: None,
                    original_name: Some(name_sym),
                });
                target_cls.install_method(name_sym, m);
                self.method_gen = self.method_gen.wrapping_add(1);
                // `method_added(name_sym)` fires for the runtime
                // block-form `cls.define_method(:foo) { ... }` too —
                // CRuby invokes the hook regardless of install path.
                self.fire_method_lifecycle_hook(&target_cls, "method_added", name_sym)?;
                self.stack.push(Value::Sym(name_sym));
                return Ok(());
            }
        }
        // Tier-1 2b: `Proc.new { ... }` returns the captured
        // block as a Value::Block (which then accepts `.call /
        // [] / () / yield` via the existing block-call arm).
        // CRuby treats Proc.new as just a Proc wrapper around
        // the block — no separate Proc object type; rubyrs's
        // Value::Block already IS the Proc shape.
        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Proc"
        {
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                // Honor a user `def Proc.new` override (parallel
                // to Hash / Module).
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
                }));
            }
            self.stack.push(Value::Block(block));
            return Ok(());
        }
        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Hash"
        {
            // Same precedence rule as `do_call`'s Hash.new no-
            // block path: a user `def self.new` on Hash (reopened
            // class) wins over the built-in default-block
            // intercept. CRuby treats `Class#new` as a regular
            // method; a reopen-and-override is just normal
            // method-resolution and should be honoured in block-
            // form too. Without this check, `class Hash; def
            // self.new(&b); ...; end; end; Hash.new { ... }`
            // silently returned `{}` from the hardcoded intercept
            // below.
            //
            // `do_call_block`'s generic Value::Class singleton-
            // method dispatch arm further down would catch this
            // for non-Hash classes, but it fires AFTER this Hash
            // intercept, so Hash specifically needs the explicit
            // pre-check.
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
                }));
            }
            // GC rooting: `block` was popped from the stack into a
            // Rust-local ObjId above. Until `hash_set_default_block`
            // installs it into the new Hash (which IS a GC root via
            // `self.stack.push` below), the block is unreachable
            // from the standard roots (stack / frames / pinned).
            // `maybe_gc` could sweep it between the alloc and the
            // store, leaving `hash_set_default_block` pointing at a
            // freed slot. Pin across both maybe_gc + alloc.
            let mut g = PinGuard::new(self);
            g.pin(Value::Block(block));
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let hid = g.vm.heap.alloc(HeapObj::Hash(crate::heap::HashObj::with_pairs(Vec::new())));
            g.vm.heap.hash_set_default_block(hid, Some(block));
            g.vm.stack.push(Value::Hash(hid));
            return Ok(());
        }
        // `Array.new(size) { |i| block }` — CRuby's three-arg
        // intercept on the block-form path. Builds a fresh Array
        // by calling the block once per index 0..size-1, using
        // each return value as the element. Surfaced as a gap
        // by the SQLite bench's `Array.new(N) { ... }` pattern.
        //
        // Honors a user `def self.new` override on Array (same
        // precedence rule as the Hash arm above) — the
        // singleton-method lookup runs first, and only if it
        // misses do we install the block-form constructor.
        if &*name == "new"
            && let Some(Value::Class(cls)) = &recv
            && cls.name.as_str() == "Array"
        {
            if let Some(m) = self.lookup_class_singleton_method(cls, name_id) {
                let target_self = Value::Class(cls.clone());
                return self.invoke_method_with_block(m, target_self, args, Some(block));
            }
            let size: i64 = match args.as_slice() {
                [] => 0,
                [Value::Int(n)] => *n,
                _ => return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0..1 for block form)", args.len()),
                })),
            };
            if size < 0 {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("negative array size ({})", size),
                }));
            }
            // Pin both the block (which the iter closure captures
            // through frames; if GC reaps it mid-loop the next
            // step_block call segfaults) and a running Vec<Value>
            // accumulator (via a placeholder Array allocation).
            // The accumulator is rebuilt as a real Array at the end
            // — pinning it as we go would mean N allocations
            // instead of 1, which defeats the purpose of the
            // pre-sized Vec.
            let mut g = PinGuard::new(self);
            g.pin(Value::Block(block));
            // pre_frames captures the frame depth so step_block can
            // detect non-local return / break correctly.
            let pre_frames = g.vm.frames.len();
            let mut elems: Vec<Value> = Vec::with_capacity(size.max(0) as usize);
            for i in 0..size {
                match g.vm.step_block(block, vec![Value::Int(i)], pre_frames)? {
                    super::iter::BlockStep::MethodReturn => return Ok(()),
                    super::iter::BlockStep::Break(v) => {
                        g.vm.stack.push(v);
                        return Ok(());
                    }
                    super::iter::BlockStep::Value(v) => elems.push(v),
                }
            }
            g.vm.maybe_gc();
            g.vm.check_alloc()?;
            let aid = g.vm.heap.alloc(HeapObj::Array(elems.into()));
            g.vm.stack.push(Value::Array(aid));
            return Ok(());
        }
        // `instance_eval` / `class_eval` / `module_eval` — swap
        // `self` for the duration of the block. Intercepted here
        // so the receiver-type dispatch below can't claim them
        // first (e.g. a future `Object#instance_eval` primitive
        // would shadow this). `args.is_empty()` keeps us out of
        // the way of any hypothetical user-defined
        // `instance_eval(arg)` that someone might define.
        if let Some(r) = &recv {
            // `instance_exec(*args) { |*a| ... }` — like instance_eval
            // but the block receives the EXPLICIT args you pass
            // (not `self`). Same self-swap semantics. Variadic args,
            // including zero. Sinatra-shape DSL pattern:
            // `instance.instance_exec(&handler)` runs the captured
            // route block against a fresh request instance so `@ivar`
            // and helper methods (defined on the instance's class)
            // resolve through the swapped self.
            let is_instance_exec = &*name == "instance_exec";
            if is_instance_exec {
                // Override-precedence probe (parity with `send` /
                // `Hash.new` patterns nearby): only fall into the
                // builtin path when there's no user-defined
                // `instance_exec` on the receiver. Without this, a
                // `class C; def instance_exec(...); ...; end; end`
                // override (including on primitive classes like
                // `class String; def instance_exec; end; end`) would
                // be silently shadowed by the builtin.
                let user_override = match r {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                    }
                    Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                    // Primitives — consult the user-class table for
                    // the primitive's stub class (e.g. `String`,
                    // `Integer`). Mirrors the primitive-receiver
                    // fallback in `do_call` at ~line 3066.
                    _ => match self.class_of(r) {
                        Value::Class(cls) => self.lookup_method_cached(&cls, name_id, cache_id).is_some(),
                        _ => false,
                    },
                };
                if !user_override {
                    self.invoke_block_with_self(block, r.clone(), /*as_class_body=*/false, args)?;
                    return Ok(());
                }
            }
            let is_instance_eval = &*name == "instance_eval";
            let is_class_eval = &*name == "class_eval" || &*name == "module_eval";
            if (is_instance_eval || is_class_eval) && args.is_empty() {
                if is_class_eval && !matches!(r, Value::Class(_)) {
                    // Align with the existing wording for `include`
                    // (vm/dispatch.rs:171, :369) so error messages
                    // are consistent across the Module-receiver
                    // family.
                    return Err(self.trap(RubyError::TypeError {
                        msg: format!(
                            "wrong argument type {} (expected Module)",
                            r.type_name(),
                        ),
                    }));
                }
                // CRuby passes `self` as the sole block arg (so
                // `obj.instance_eval { |o| o == obj }` works);
                // mirror that. The single-arg matches the
                // common DSL shape `cls.class_eval { |k| ... }`.
                let block_args = vec![r.clone()];
                self.invoke_block_with_self(block, r.clone(), is_class_eval, block_args)?;
                return Ok(());
            }
            // String-arg form: `cls.class_eval(source [, file, line])`
            // — parse + compile + run the source. Tier 1 divergence:
            // does NOT switch to the receiver class's class-body
            // context (so bare `Foo.class_eval("def bar; end")`
            // lands `bar` at top level instead of on Foo). Tilt's
            // tilt-2.7.0 `eval_compiled_method` path self-wraps its
            // source in a nested `Tilt::TOPOBJECT.class_eval do
            // def __tilt_xxx; end end`, so the inner block-form
            // (intercepted above) does the actual class context
            // switching. Documented in docs/SUBSET.md.
            // CRuby parity: `class_eval`/`module_eval` is either
            // (a) block-only with 0 args (handled above) OR
            // (b) string-form with 1..3 args and NO block (handled
            // in do_call). The block+args combination raises
            // ArgumentError "wrong number of arguments (given N,
            // expected 0)". Without this guard, passing both
            // would fall through to NoMethodError.
            if is_class_eval && let Value::Class(cls) = r
                && !args.is_empty()
                && self.lookup_class_singleton_method(cls, name_id).is_none()
            {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!(
                        "wrong number of arguments (given {}, expected 0)",
                        args.len()
                    ),
                }));
            }
        }
        if let Some(r) = &recv
            && let Some(v) = self.collection_call_block(r, &name, &args, block)? {
                self.stack.push(v);
                return Ok(());
            }

        if no_recv {
            // `lambda { ... }` / `proc { ... }` / `Proc.new { ... }`-
            // style block-to-Value capture. rubyrs doesn't
            // distinguish Lambda from Proc at runtime (the strict-
            // arity check is the documented gap in SUBSET.md), so
            // both names just hand the attached block back as a
            // Value::Block. `args.is_empty()` keeps us out of the
            // way of user-defined `lambda(arg)` shapes if anyone
            // overrides the name.
            if args.is_empty() && (&*name == "lambda" || &*name == "proc") {
                self.stack.push(Value::Block(block));
                return Ok(());
            }
            // ADR 0025 Phase 4c: `Kernel#at_exit { ... }` — record the
            // block for end-of-eval execution; return the block as a
            // Proc value for CRuby parity (`at_exit` returns the Proc
            // that was registered, allowing the caller to introspect
            // it). Drained LIFO by `Runtime::eval` after `eval_inner`
            // returns. Must live alongside `lambda`/`proc` here in the
            // block-form dispatch because `builtin_call` doesn't
            // receive the attached block.
            if args.is_empty() && &*name == "at_exit" {
                self.at_exit_handlers.push(block);
                self.stack.push(Value::Block(block));
                return Ok(());
            }
            // `refine(Target) do … end` inside a module body. self is the
            // defining module; record the refinement against it.
            if &*name == "refine" && args.len() == 1
                && let Value::Class(target) = &args[0]
                && let Some(Value::Class(module)) = self.frames.last().map(|f| f.self_val.clone()).as_ref()
            {
                let (target, module) = (target.clone(), module.clone());
                return self.do_refine(target, module, block);
            }
            if let Some(res) = self.builtin_call(&name, &args) {
                let v = res?;
                // See suppress_call_result_push doc on Vm —
                // mirrors the no_recv path above.
                if self.suppress_call_result_push {
                    self.suppress_call_result_push = false;
                } else {
                    self.stack.push(v);
                }
                return Ok(());
            }
            if let Some(host) = self.host_fns.get(&name_id).cloned() {
                let v = self.invoke_host_fn(host, &args)?;
                self.stack.push(v);
                return Ok(());
            }
            // Bare `send(:foo) { ... }` / `__send__(:foo) { ... }`
            // (and `public_send`) — same re-aim as the no_recv arm in
            // `do_call`. See there for the override + visibility
            // rationale.
            if matches!(&*name, "send" | "__send__" | "public_send") {
                let frame_self = self.frames.last()
                    .expect("ICE: do_call_block(no_recv) with empty frames")
                    .self_val.clone();
                let user_override = &*name == "send" && match &frame_self {
                    Value::Object(id) => {
                        let cls = self.heap.class_of(*id);
                        self.lookup_method_cached(&cls, name_id, cache_id).is_some()
                    }
                    Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
                    _ => false,
                };
                if !user_override {
                    let target_sym = self.parse_send_target(&args)?;
                    let new_argc = args.len() - 1;
                    self.bypass_visibility_once = true;
                    self.stack.push(Value::Block(block));
                    for a in args.into_iter().skip(1) {
                        self.stack.push(a);
                    }
                    return self.do_call_block(target_sym, new_argc, true, u16::MAX);
                }
            }
            let self_val = self.frames.last().expect("ICE: do_call_block no frame").self_val.clone();
            if let Value::Object(id) = &self_val {
                let cls = self.heap.class_of(*id);
                if let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id) {
                    self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                    return Ok(());
                }
            }
            // Block-form parallel of `do_call`'s user-singleton
            // bare-call resolution (~line 2439). Inside
            // `class Foo < Bar; foo do ... end; end`, bare `foo`
            // is dispatched on `self = Foo` and must walk Foo's
            // singleton chain (including Bar's, transitively) so
            // user-defined `def self.foo` and `class << self; def
            // foo; end; end` methods inherited from a parent class
            // resolve identically with or without an attached
            // block. Without this, the Sinatra-shape DSL
            // (`class App < Sinatra::Base; get '/' do ... end`)
            // dies at NoMethodError because the route registrar's
            // block triggers `do_call_block` instead of `do_call`,
            // and the existing block-form Class bridge below only
            // covers hardcoded primitive names.
            // Bare-call-with-block inside reopened-primitive method
            // bodies — `class Hash; def deep_x; each { … }; end; end`
            // shape. Parallel of the no-block fix in `do_call`
            // (commit b8feb3ce). The Object arm above only fires for
            // `Value::Object` self; primitive selves (Hash / Array /
            // String / Int / Sym / …) previously fell through to
            // method_missing / NoMethodError, even though
            // `self.<name> { … }` works fine.
            //
            // Same two-tier resolution as do_call's version:
            //   1. Try `lookup_method_uncached` on the primitive's
            //      class — catches user-defined sibling methods.
            //   2. Otherwise, bridge to the receiver-form
            //      do_call_block by pushing self_val + block + args
            //      and re-entering with no_recv=false. Receiver-form
            //      primitive arms (Hash#each, Array#map, …) take
            //      over.
            //
            // Nil exclusion stays load-bearing for the toplevel
            // ArgumentError surface, same reasoning as do_call.
            if !matches!(&self_val, Value::Object(_) | Value::Class(_) | Value::Nil) {
                if let Value::Class(cls) = self.class_of(&self_val)
                    && let Some(m) = self.lookup_method_uncached(&cls, name_id)
                {
                    self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                    return Ok(());
                }
                let argc = args.len();
                self.stack.push(self_val.clone());
                self.stack.push(Value::Block(block));
                for a in args { self.stack.push(a); }
                return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
            }
            if let Value::Class(c) = &self_val
                && let Some(m) = self.lookup_class_singleton_method(c, name_id) {
                self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                return Ok(());
            }
            // Block-form parallel of `do_call`'s bare-call Class
            // bridge (see comments at the no_recv arm around
            // ~line 537). Without this, bare whitelisted Class
            // methods invoked with an attached block from inside
            // a class body would raise NoMethodError even though
            // their blockless counterparts dispatch correctly —
            // breaks the lockstep contract for the block form.
            // Stack restoration matches do_call_block's
            // `[..., recv, block, *args]` shape so re-entry
            // sees the receiver-form layout it expects.
            // PR #196 code-review #3.
            if let Value::Class(cls) = &self_val {
                let in_set = matches!(&*name,
                    "new" | "name" | "to_s" | "inspect"
                    | "method_defined?" | "instance_method" | "undef_method" | "remove_method"
                    | "superclass" | "ancestors" | "include?"
                    | "instance_methods" | "public_instance_methods"
                    | "private_instance_methods" | "protected_instance_methods"
                    | "constants"
                    | "autoload" | "autoload?" | "const_defined?" | "const_get" | "const_set" | "private_constant" | "public_constant"
                    | "deprecate_constant"
                    | "private_class_method" | "public_class_method"
                    | "singleton_class"
                    | "class_eval" | "module_eval"
                );
                let allocate_allowed =
                    &*name == "allocate"
                        && !cls.is_module
                        && cls.name != "Module";
                if in_set || allocate_allowed {
                    // `class_eval` / `module_eval` are the ONLY
                    // bridge-set members whose block is load-
                    // bearing. `class C; class_eval { def foo;
                    // end }; end` defines `foo` on `C` via the
                    // block-form intercept in do_call_block's
                    // recv-form path. Re-route through
                    // do_call_block (preserving block) instead
                    // of the do_call discard path the other
                    // bridge names use.
                    if matches!(&*name, "class_eval" | "module_eval") {
                        let argc = args.len();
                        self.stack.push(self_val.clone());
                        self.stack.push(Value::Block(block));
                        for a in args { self.stack.push(a); }
                        return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
                    }
                    // Route through the blockless `do_call`, NOT
                    // `do_call_block` — CRuby silently discards the
                    // block for these Class methods (verified:
                    // `class Bar < Foo; ancestors { ran = true };
                    // end` returns the ancestor array AND `ran`
                    // stays false). do_call_block doesn't have
                    // receiver-form arms for most of these names,
                    // so routing the block form there would
                    // produce NoMethodError. The `allocate` case
                    // already has a do_call_block arm that
                    // discards its block — re-entering do_call
                    // hits the dedicated allocate arm there
                    // instead, with the same fences. Same
                    // outcome, simpler routing.
                    let argc = args.len();
                    self.stack.push(self_val.clone());
                    for a in args { self.stack.push(a); }
                    let _ = block; // explicitly discarded per CRuby
                    return self.do_call(name_id, argc, /*no_recv=*/false, u16::MAX);
                }
            }
            // Bare call WITH a block on a real `nil` receiver inside a
            // method body — block-form parallel of do_call's Nil arm
            // (~line 3815). ActiveSupport's `NilClass`-targeted methods
            // reached via an inherited yielding Object method land
            // here. `defining_class.is_some()` keeps `<main>`'s bare
            // calls on the toplevel path; see do_call's Nil arm for the
            // full main-self-is-Nil rationale.
            if matches!(&self_val, Value::Nil)
                && self.frames.last().is_some_and(|f| f.defining_class.is_some())
                && let Value::Class(cls) = self.class_of(&self_val)
                && let Some(m) = self.lookup_method_uncached(&cls, name_id)
            {
                self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                return Ok(());
            }
            // Bare call WITH a block on a primitive self (Int / Str /
            // Sym / Float / Array / Hash / Range / Bool) — block-form
            // parallel of do_call's primitive bare-call bridge (~line
            // 3815). Without this, every bare `transform_keys { }` /
            // `map { }` / `each_with_object { }` inside a reopened
            // primitive method body — e.g. `class Hash; def
            // symbolize_keys; transform_keys { ... }; end; end` —
            // raised NoMethodError even though the explicit
            // `self.transform_keys { }` form dispatches fine. Two-tier:
            // a user-defined sibling method first, else bridge to the
            // receiver-form `do_call_block` so the native primitive /
            // iterator arm fires with the block attached.
            if !matches!(&self_val, Value::Object(_) | Value::Class(_) | Value::Nil) {
                if let Value::Class(cls) = self.class_of(&self_val)
                    && let Some(m) = self.lookup_method_uncached(&cls, name_id)
                {
                    self.invoke_method_with_block(m, self_val.clone(), args, Some(block))?;
                    return Ok(());
                }
                let argc = args.len();
                self.stack.push(self_val.clone());
                self.stack.push(Value::Block(block));
                for a in args { self.stack.push(a); }
                return self.do_call_block(name_id, argc, /*no_recv=*/false, u16::MAX);
            }
            if let Some(m) = self.toplevel_methods.get(&name_id).cloned() {
                self.invoke_method_with_block(m, self_val, args, Some(block))?;
                return Ok(());
            }
            if self.try_method_missing(&self_val, name_id, args, Some(block))? {
                return Ok(());
            }
            return Err(self.trap(RubyError::NoMethodError {
                kind: crate::error::NoMethodErrorKind::Missing,
                method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&self_val)),
            }));
        }
        let recv = recv.expect("ICE: receiver missing for block call");

        // `obj.send(:name, args...) { ... }` — same dynamic-name
        // re-aim as the block-less arm in `do_call`. `do_call_block`
        // pops args then block then recv (in that drain/pop order),
        // so the stack shape it expects from a caller is
        // `[..., recv, block, *args]`. Put them back in that order
        // and re-enter. cache_id = u16::MAX for the same reason as
        // the block-less arm. User-`def send` override + visibility
        // bypass parity — same rules as the block-less arm; see
        // there for the rationale.
        let user_send_override = &*name == "send" && match &recv {
            Value::Object(id) => {
                let cls = self.heap.class_of(*id);
                self.lookup_method_cached(&cls, name_id, cache_id).is_some()
            }
            // `def self.send` on a class — singleton-method lookup
            // walking the class's superclass chain. Falls through to
            // the existing `Value::Class` arm which invokes the
            // user's singleton `send`.
            Value::Class(c) => self.lookup_class_singleton_method(c, name_id).is_some(),
            _ => false,
        };
        if matches!(&*name, "send" | "__send__" | "public_send") && !user_send_override {
            let target_sym = self.parse_send_target(&args)?;
            let new_argc = args.len() - 1;
            self.bypass_visibility_once = true;
            self.stack.push(recv);
            self.stack.push(Value::Block(block));
            for a in args.into_iter().skip(1) {
                self.stack.push(a);
            }
            return self.do_call_block(target_sym, new_argc, false, u16::MAX);
        }

        // Mirror do_call's Int#+/-/* BigInt-aware intercept so
        // block-form sends (`a.send(:+, big) { ... }`) match the
        // expression form's overflow-promotion. Without this the
        // block path falls through to numeric_call's plain `+`
        // which wraps on overflow.
        #[cfg(feature = "bignum")]
        if args.len() == 1
            && matches!(&recv, Value::Int(_))
            && matches!(&args[0], Value::Int(_))
            && let Some(kind) = crate::bytecode::BinOpKind::from_op_name(&name)
            && matches!(kind,
                crate::bytecode::BinOpKind::Add
                | crate::bytecode::BinOpKind::Sub
                | crate::bytecode::BinOpKind::Mul
            )
        {
            let (Value::Int(x), Value::Int(y)) = (&recv, &args[0]) else { unreachable!() };
            let v = self.apply_int_promote(kind, *x, *y)?;
            self.stack.push(v);
            return Ok(());
        }

        if self.try_push_int_chr_encoding(&recv, &name, &args)? {
            return Ok(());
        }
        if self.try_string_encoding_ops(&recv, &name, &args)? {
            return Ok(());
        }
        if self.try_push_string_encoding(&recv, &name, &args) {
            return Ok(());
        }
        if let Some(v) = primitive_call(&recv, &name, &args, self.max_value_bytes).map_err(|e| self.trap(e))? { self.stack.push(v); return Ok(()); }
        if let Some(v) = self.sym_primitive(&recv, &name, &args)? { self.stack.push(v); return Ok(()); }
        // Mirror do_call's bigint_primitive hook. Without this,
        // block-form calls on BigInt receivers (`big.send(:to_s) { ... }`)
        // raise NoMethodError because primitive_call/sym_primitive
        // are stateless and can't reach the BigInt heap.
        #[cfg(feature = "bignum")]
        if let Some(v) = self.bigint_primitive(&recv, &name, &args)? {
            self.stack.push(v);
            return Ok(());
        }
        // Block-form `def self.foo` dispatch. Mirrors `do_call`'s
        // `Value::Class` arm at vm/dispatch.rs:1226 — without this,
        // `Foo.bar(args) { … }` where `Foo` carries a user singleton
        // method falls all the way through to `NoMethodError`.
        // Common shape: `StringIO.open("x") do |io| … end`,
        // `Module.send(:include, M) { … }`, any DSL helper a host
        // exposes as a class method that takes a block. Same
        // `lookup_class_singleton_method` helper walks the singleton
        // chain through superclasses; on a hit, we re-enter via
        // `invoke_method_with_block` to thread the block through.
        if let Value::Class(cls) = &recv
            && let Some(m) = self.lookup_class_singleton_method(cls, name_id)
        {
            let target_self = recv.clone();
            return self.invoke_method_with_block(m, target_self, args, Some(block));
        }

        // `Class#allocate` (block form) — CRuby silently ignores
        // a block passed to `allocate`. Without this arm,
        // `Box.allocate { ... }` (or `Box.send(:allocate) { ... }`,
        // which routes here through `do_call_block`) falls through
        // to method_missing/NoMethodError instead of allocating
        // (PR #181 review round 4 Copilot comment #1). Mirrors
        // do_call's allocate arm — same arity / primitive shell /
        // Module-Class fences, same shared allocator helper, with
        // the block discarded.
        //
        // Precedence: this arm sits AFTER the generic
        // `lookup_class_singleton_method` check at line 4601, so
        // a user-defined `def self.allocate` wins. do_call has the
        // matching precedence via its dedicated `allocate`
        // user-singleton arm at line 1184 (fix landed in the same
        // PR's code-review round). The two paths are now
        // symmetric: user override wins in both no-block and
        // block forms.
        if &*name == "allocate"
            && let Value::Class(cls) = &recv {
            if !args.is_empty() {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0)", args.len()),
                }));
            }
            // Eigenclass-shell fence — CRuby:
            // `A.singleton_class.allocate` raises TypeError
            // ("can't create instance of singleton class").
            // (Code-review #253 round 9 #1.)
            if cls.singleton_target.borrow().is_some() {
                return Err(self.trap(RubyError::TypeError {
                    msg: "can't create instance of singleton class".into(),
                }));
            }
            if cls.is_module
                || cls.name == "Module"
                || cls.name == "Class"
                || is_primitive_class_name(&cls.name)
            {
                let display = if cls.name.is_empty() {
                    if cls.is_module { "Module" } else { "Class" }
                } else {
                    &cls.name
                };
                return Err(self.trap(RubyError::TypeError {
                    msg: format!("allocator undefined for {}", display),
                }));
            }
            let obj = self.alloc_default_instance(cls)?;
            self.stack.push(obj);
            return Ok(());
        }
        let new_id = self.interner.intern("new");
        if name_id == new_id
            && let Value::Class(cls) = &recv {
                // Eigenclass-shell fence (block-form parallel of
                // the no-block fence in
                // `try_dispatch_class_intrinsics`). CRuby raises
                // TypeError for `A.singleton_class.new { … }` too.
                // (Code-review #253 round 9 #1.)
                if cls.singleton_target.borrow().is_some() {
                    return Err(self.trap(RubyError::TypeError {
                        msg: "can't create instance of singleton class".into(),
                    }));
                }
                // Pin args + obj + block across the WHOLE alloc +
                // invoke window. Pre-fix the PinGuard was scoped to
                // just the `alloc_default_instance` call, leaving
                // `obj` AND `block` as bare Rust locals before
                // `invoke_method_with_block` ran — its rest-arg
                // alloc / arity-binding can trigger maybe_gc with
                // the new Frame not yet on the stack, sweeping the
                // block ObjId before its Frame.block_arg slot got
                // rooted. STRESS_GC repro:
                // `class Foo; def initialize(&blk); blk.call; end;
                // end; Foo.new { 42 }` ICE'd at "heap slot is not a
                // Block" in the block_given? → blk.call window.
                // Pin everything heap-shaped (args entries, the
                // fresh obj, the block) for the duration of the
                // invoke; the guard releases on `Ok(())` return
                // BELOW where the new Frame is already pushed +
                // GC-rooted via Frame.block_arg + Frame.self_val.
                let mut g = PinGuard::new(self);
                for a in &args { g.pin(a.clone()); }
                g.pin(Value::Block(block));
                let obj = g.vm.alloc_default_instance(cls)?;
                g.pin(obj.clone());
                let init_id = g.vm.interner.intern("initialize");
                let ruby_init = g.vm.lookup_method_uncached(cls, init_id);
                if let Some(m) = ruby_init {
                    g.vm.invoke_method_with_block(m, obj.clone(), args, Some(block))?;
                    // Drop the guard before mutating the new
                    // frame's swap_return — by this point the
                    // new Frame is on `g.vm.frames` and rooting
                    // both obj (as self_val) and block (as
                    // block_arg) on its own, so the pin
                    // tracking is no longer load-bearing.
                    drop(g);
                    self.frames.last_mut().expect("ICE: frames empty after new").swap_return = Some(obj);
                } else {
                    drop(g);
                    self.stack.push(obj);
                }
                return Ok(());
            }
        // `try_class_of` — same class-less-slot (HeapObj::Fiber)
        // fall-through as do_call's Object arm.
        if let Value::Object(id) = &recv
            && let Some(cls) = self.heap.try_class_of(*id)
            && let Some(m) = self.lookup_method_cached(&cls, name_id, cache_id)
        {
            self.invoke_method_with_block(m, recv.clone(), args, Some(block))?;
            return Ok(());
        }
        // User-defined method reopened on a primitive's class —
        // explicit receiver, WITH a block. The builtin primitive arms
        // above (primitive_call / sym_primitive / collection_call_block
        // / numeric) have already had first refusal, so this is a
        // fallback for NEW methods a script added to a core class, the
        // block-form parallel of do_call's primitive receiver-form
        // user-method lookup. Without it `h.deep_transform_keys { ... }`
        // on a reopened Hash raised NoMethodError even though both the
        // blockless `h.deep_transform_keys` and the bare-call
        // `deep_transform_keys { ... }` (from inside another Hash
        // method) resolved — the asymmetry ActiveSupport's core-ext
        // surfaced. (Builtin precedence is unchanged: a name that
        // matches a primitive arm still takes that arm first.)
        if !matches!(&recv, Value::Object(_) | Value::Class(_))
            && let Value::Class(cls) = self.class_of(&recv)
            && let Some(m) = self.lookup_method_uncached(&cls, name_id)
        {
            self.invoke_method_with_block(m, recv.clone(), args, Some(block))?;
            return Ok(());
        }
        // Block-form collection → Enumerable module fallback (e.g.
        // `array.sum { … }`). Same rationale as the no-block path.
        if self.try_enumerable_module_fallback(&recv, name_id, args.clone(), Some(block))? {
            return Ok(());
        }
        if self.try_method_missing(&recv, name_id, args, Some(block))? {
            return Ok(());
        }
        Err(self.trap(RubyError::NoMethodError {
            kind: crate::error::NoMethodErrorKind::Missing,
            method: name.to_string(), recv_type: std::borrow::Cow::Owned(self.recv_desc_for_error(&recv)),
        }))
    }


}

/// Identity comparison for Method receivers — heap-managed
/// recvs compare by ObjId / Rc-pointer; primitives compare by
/// value. Matches CRuby's `equal?`-style semantics, narrowed to
/// the cases that can appear in a BoundMethod recv slot.
/// True for class names whose instances are backed by a non-Object
/// `Value` variant. Used by `Class#instance_method` to decide
/// between "real lookup, NameError on miss" (user class — the
/// methods table is the source of truth) and "synthesise an
/// UnboundMethod, let downstream arity / parameters fall back to
/// the builtin sentinel" (primitive — methods live in
/// `primitive_call` arms, not in a per-class table).
///
/// Mirrors `Vm::class_of`'s class-name set (`vm/lookup.rs`),
/// PLUS one intentional extra: `Kernel`. Kernel's instances are
/// Object-backed in CRuby (not a distinct `Value` variant), but
/// we treat it as a sentinel primitive here so
/// `Kernel.instance_method(:foo)` resolves via the same path as
/// real primitives — see the Kernel arm below for the rationale.
/// The two lists are NOT meant to be kept identical; future
/// editors should add new entries to whichever side actually
/// needs them.
/// `Class#method_defined?(name)` resolver. Walks the user-Method
/// table + ancestor chain first; if that misses and `cls` is a
/// primitive class (Integer / String / ...), builds a sentinel
/// receiver of the matching `Value` shape and consults the per-
/// primitive `responds_to` whitelist. This way
/// `String.method_defined?(:nope)` correctly returns `false` while
/// `Symbol.method_defined?(:name)` returns `true` (the
/// `msgpack-ruby/lib/msgpack/symbol.rb` Ruby-2.7+ version-detect
/// path). Excluded primitives that need a non-trivial sentinel
/// (Array/Hash/Range/Regexp/Proc/Method/UnboundMethod) fall back
/// to a permissive `true` — matches CRuby for the broadly-shared
/// Kernel methods and stays out of false-negative territory while
/// the synthesis cost isn't justified.
fn class_method_defined(vm: &mut Vm, cls: &Rc<Class>, sid: SymId) -> bool {
    // Eigenclass-shell: methods installed via
    // `singleton_class.class_eval { def foo; end }` redirect
    // into `target.singleton_methods` rather than the shell's
    // own `methods` table. CRuby's `shell.method_defined?(:foo)`
    // returns true for redirected installs, so walk the
    // target's singleton-method chain when the shell asks.
    // (Code-review #253 round 9 #3.)
    if let Some(target) = cls
        .singleton_target
        .borrow()
        .as_ref()
        .and_then(std::rc::Weak::upgrade)
        && vm.lookup_class_singleton_method(&target, sid).is_some()
    {
        return true;
    }
    if vm.lookup_method_uncached(cls, sid).is_some() {
        return true;
    }
    let sentinel: Option<Value> = match cls.name.as_str() {
        "Integer" => Some(Value::Int(0)),
        "Float" => Some(Value::Float(0.0)),
        "String" => Some(Value::new_str("")),
        // Sym(SymId(0)) is the first interned token — the
        // interner always has at least one entry by the time
        // class objects exist, so this is safe to construct.
        "Symbol" => Some(Value::Sym(SymId(0))),
        "TrueClass" => Some(Value::Bool(true)),
        "FalseClass" => Some(Value::Bool(false)),
        "NilClass" => Some(Value::Nil),
        _ => None,
    };
    match sentinel {
        Some(s) => vm.responds_to(&s, sid, true),
        // Aggregate / opaque primitives: keep the previously-
        // permissive answer so the gem helper path doesn't trip
        // on Kernel-shared method probes.
        None => is_primitive_class_name(&cls.name),
    }
}

/// Outcome of a `define_method` / `define_singleton_method`
/// 2-arg form install request. Pulls the body description out
/// of a Proc / Method / UnboundMethod argument so the install
/// path stays the same as the block-form. Visibility is
/// applied later by the caller (built into the Method by
/// `build_method_from_value`), not stored on this enum.
enum MethodSource {
    Proc {
        proto_idx: usize,
        params: Vec<String>,
        closure: crate::value::MethodClosure,
    },
    Snapshot(std::rc::Rc<crate::value::Method>),
}

impl Vm {
    /// Extract a [`MethodSource`] from the 2nd-positional argument
    /// of `Module#define_method(:name, src)` /
    /// `Object#define_singleton_method(:name, src)`. Accepts:
    ///   * `Value::Block(id)`  — a `Proc`, including `proc { … }`
    ///     and `Proc.new`. Captures the block's proto + closure
    ///     verbatim, matching how the block-form install reads
    ///     its block argument.
    ///   * `Value::BoundMethod(id)` / `Value::UnboundMethod(id)`
    ///     — install the captured method snapshot directly.
    ///     CRuby's `bind` compatibility check on UnboundMethod
    ///     (target class must inherit from the unbound's class)
    ///     is enforced here so a cross-hierarchy install raises
    ///     TypeError up front instead of failing later.
    ///   * `Value::CurriedProc(id)` — Tier-2 follow-up; emits
    ///     TypeError now (consistent with the `other =>` branch
    ///     for unsupported source kinds).
    ///   * Anything else → TypeError matching CRuby.
    fn method_source_from(
        &self,
        src: &Value,
        target_cls: &std::rc::Rc<crate::value::Class>,
    ) -> Result<MethodSource, RubyError> {
        match src {
            Value::Block(bid) => {
                let bh = self.heap.block(*bid);
                let proto = &self.protos[bh.proto_idx];
                Ok(MethodSource::Proc {
                    proto_idx: bh.proto_idx,
                    params: proto.params.clone(),
                    closure: crate::value::MethodClosure {
                        captured: bh.captured.clone(),
                        param_start: bh.param_start,
                        n_params: bh.n_params,
                    },
                })
            }
            Value::BoundMethod(id) => {
                let (_, _, snap) = self.heap.bound_method_full(*id);
                match snap {
                    Some(m) => Ok(MethodSource::Snapshot(m.clone())),
                    None => Err(RubyError::TypeError {
                        // Built-in / universal-arm methods don't
                        // carry a Proto so we can't install them
                        // verbatim — CRuby's Method objects can
                        // wrap primitive dispatch and rubyrs's
                        // can't yet. Tier-2 follow-up: install
                        // a synthetic name-forwarding Method
                        // body that re-dispatches by SymId on
                        // the new receiver. PR #321 cycle-3.
                        msg: "BoundMethod source has no Proto body (rubyrs limitation: built-in methods can't be re-installed via define_method yet)".into(),
                    }),
                }
            }
            Value::UnboundMethod(id) => {
                let (defining, _, snap) = self.heap.unbound_method_full(*id);
                // Mirror the `UnboundMethod#bind` fence at
                // dispatch.rs:928 — Kernel and Modules are
                // universally bindable in CRuby; only
                // Class-owned UnboundMethods enforce the
                // subclass check. Prior implementation was too
                // strict and rejected
                // `C.define_method(:x, M.instance_method(:x))`.
                if defining.name.as_str() != "Kernel"
                    && !defining.is_module
                    && !crate::vm::class_is_a(target_cls, &defining)
                {
                    return Err(RubyError::TypeError {
                        msg: format!(
                            "bind argument must be a subclass of {}",
                            defining.name,
                        ),
                    });
                }
                match snap {
                    Some(m) => Ok(MethodSource::Snapshot(m)),
                    None => Err(RubyError::TypeError {
                        // Same rubyrs limitation as BoundMethod
                        // sources: built-in / universal-arm
                        // methods don't expose a Proto. Tier-2
                        // follow-up is a name-forwarding
                        // synthetic body. PR #321 cycle-3.
                        msg: "UnboundMethod source has no Proto body (rubyrs limitation: built-in methods can't be re-installed via define_method yet)".into(),
                    }),
                }
            }
            Value::CurriedProc(_) => Err(RubyError::TypeError {
                msg: "CurriedProc as define_method source is not yet supported by rubyrs Tier-1".into(),
            }),
            other => Err(RubyError::TypeError {
                msg: format!(
                    "wrong argument type {} (expected Proc/Method/UnboundMethod)",
                    other.type_name(),
                ),
            }),
        }
    }

    /// Build the [`crate::value::Method`] described by `src`,
    /// anchored to `defining_class`, without inserting it into
    /// any table. The caller decides whether the install goes
    /// into the class's instance-method table (`define_method`)
    /// or its singleton-method table (`define_singleton_method`
    /// for a Class receiver), then bumps `method_gen` itself.
    /// Used by both 2-arg-form paths so the source-decoding
    /// stays single-sourced.
    fn build_method_from_value(
        &self,
        src: &Value,
        defining_class: &std::rc::Rc<crate::value::Class>,
        visibility: crate::value::Visibility,
        name_id: crate::intern::SymId,
    ) -> Result<std::rc::Rc<crate::value::Method>, RubyError> {
        let source = self.method_source_from(src, defining_class)?;
        let m = match source {
            MethodSource::Proc { proto_idx, params, closure } => {
                std::rc::Rc::new(crate::value::Method {
                    params,
                    proto_idx,
                    fixed_arity: None,
                    defining_class: Some(std::rc::Rc::downgrade(defining_class)),
                    visibility: std::cell::Cell::new(visibility),
                    closure: Some(closure),
                    builtin: None,
                    original_name: Some(name_id),
                })
            }
            MethodSource::Snapshot(snap) => {
                std::rc::Rc::new(crate::value::Method {
                    params: snap.params.clone(),
                    proto_idx: snap.proto_idx,
                    fixed_arity: snap.fixed_arity,
                    defining_class: Some(std::rc::Rc::downgrade(defining_class)),
                    visibility: std::cell::Cell::new(visibility),
                    closure: snap.closure.clone(),
                    original_name: snap.original_name,
                    builtin: snap.builtin.clone(),
                })
            }
        };
        Ok(m)
    }

    /// `Module#define_method`-style install: routes through
    /// `Class::install_method`, which redirects via
    /// `singleton_target` when the class is an eigenclass shell.
    /// Bumps `method_gen`.
    fn install_method_from_value(
        &mut self,
        target_cls: &std::rc::Rc<crate::value::Class>,
        name_sym: crate::intern::SymId,
        src: &Value,
        visibility: crate::value::Visibility,
    ) -> Result<crate::intern::SymId, RubyError> {
        let anchor = target_cls.effective_install_class();
        let m = self.build_method_from_value(src, &anchor, visibility, name_sym)?;
        target_cls.install_method(name_sym, m);
        self.method_gen = self.method_gen.wrapping_add(1);
        // `Module#method_added(name)` fires for `define_method`
        // installs too — CRuby invokes the hook regardless of
        // whether the install came from `def`, `alias_method`, or
        // `define_method`. \`.map_err(|t| t.err)\` flattens the
        // Trap back into a bare RubyError because this helper's
        // signature returns RubyError; the trap rewraps at the
        // call sites' `.map_err(|e| self.trap(e))` boundary.
        self.fire_method_lifecycle_hook(target_cls, "method_added", name_sym)
            .map_err(|t| t.err)?;
        Ok(name_sym)
    }

    /// `Object#define_singleton_method`-style install on a
    /// Class receiver: writes directly into `cls.singleton_methods`
    /// (matching the block-form arm). The defining_class anchor
    /// is the class itself so `super` inside the new class
    /// method walks the metaclass chain.
    fn install_singleton_method_on_class_from_value(
        &mut self,
        cls: &std::rc::Rc<crate::value::Class>,
        name_sym: crate::intern::SymId,
        src: &Value,
    ) -> Result<crate::intern::SymId, RubyError> {
        let m = self.build_method_from_value(
            src,
            cls,
            crate::value::Visibility::Public,
            name_sym,
        )?;
        cls.singleton_methods.borrow_mut().insert(name_sym, m);
        self.method_gen = self.method_gen.wrapping_add(1);
        // `singleton_method_added` fires on the class itself — same
        // shape as `method_added` for instance-method installs.
        // `.map_err(|t| t.err)` flattens Trap back to RubyError to
        // match this helper's signature (the caller rewraps at the
        // \`.map_err(|e| self.trap(e))\` boundary).
        self.fire_singleton_method_lifecycle_hook(
            Value::Class(cls.clone()),
            "singleton_method_added",
            name_sym,
        )
        .map_err(|t| t.err)?;
        Ok(name_sym)
    }
}

impl Vm {
    /// Resolves a constant path from a starting class. Behavior
    /// matches CRuby's `Module#const_get` / `Module#const_defined?`
    /// dispatch:
    ///   - If the arg is a Symbol, the path is treated as a single
    ///     bare name (no `::` splitting). `:"Foo::Bar"` raises
    ///     `wrong constant name Foo::Bar`.
    ///   - If the arg is a String, `::` separators split the path
    ///     and each segment is walked. A leading `::` rebases to
    ///     the toplevel (Object). Each segment is validated via
    ///     `is_valid_const_name` before lookup.
    ///   - The interner-cap guard applies at every lookup: a
    ///     non-interned qualified key returns Missing without
    ///     calling `intern` (defends `Config::max_symbols`).
    ///
    /// (Copilot review #277 round 4 #3.)
    /// Fire a pending autoload for a flat (possibly qualified)
    /// constant key that just missed in `classes` / `constants`.
    /// Tries the exact name first, then each shorter `::`-prefix
    /// (longest first), consulting BOTH the toplevel registry
    /// (bare keys) and the scoped registry (qualified keys). The
    /// first pending entry found is popped and its target
    /// `require`d; the caller re-checks `classes` / `constants`
    /// afterwards.
    ///
    /// The prefix walk is what makes a deep toplevel reference
    /// like `M5::Inner::THE` work when the autoload is registered
    /// on the intermediate `M5::Inner`: the flat `Op::LoadConst`
    /// key is the full `"M5::Inner::THE"`, so without the prefix
    /// fallback only an exact-key autoload would fire. (The
    /// segment-by-segment `resolve_const_path` already handles
    /// this for `const_get`; this brings the flat LoadConst path
    /// to parity.)
    ///
    /// Returns `Ok(true)` if a require ran (caller should
    /// re-resolve), `Ok(false)` if nothing was pending.
    #[cfg(not(target_os = "wasi"))]
    pub(crate) fn fire_pending_autoload(&mut self, name: &str) -> Result<bool, Trap> {
        let parts: Vec<&str> = name.split("::").collect();
        for take in (1..=parts.len()).rev() {
            let prefix = if take == parts.len() {
                name.to_string()
            } else {
                parts[..take].join("::")
            };
            if !self.interner.contains(&prefix) {
                continue;
            }
            let pid = self.interner.intern(&prefix);
            let path = self
                .autoloads_toplevel
                .remove(&pid)
                .or_else(|| self.autoloads_scoped.remove(&pid));
            if let Some(path) = path {
                return match self.builtin_call("require", &[Value::new_str(path)]) {
                    Some(Ok(_)) => Ok(true),
                    Some(Err(t)) => Err(t),
                    None => Ok(false), // unreachable: "require" is a builtin
                };
            }
        }
        Ok(false)
    }

    /// CRuby names an anonymous class/module on its FIRST
    /// constant-assignment: `C = Class.new` ⇒ `C.name == "C"`,
    /// `const_set(:Inner, Class.new)` on a named owner ⇒
    /// `Owner::Inner`. rubyrs stores anon classes minted by
    /// `Class.new` with `name == ""` and keeps their nested
    /// constants in the per-class `consts` table; once such a class
    /// is assigned to a constant it must become reachable through
    /// the GLOBAL qualified-key read paths (`Op::LoadConst`,
    /// `resolve_const_path`, `const_via_ancestors`) that keep named
    /// classes in `self.classes` / `self.constants` keyed by
    /// `"Outer::Inner"`.
    ///
    /// This stamps `qualified` into `assigned_name` (so
    /// `Module#name` / `#to_s` report it), registers the class in
    /// `self.classes[qualified]`, and RECURSIVELY promotes its
    /// `consts` subtree into the global maps under the qualified
    /// prefix — naming nested anon classes (`Owner::Inner::Leaf`)
    /// as it goes. The recursion is what makes rouge's token tree
    /// (`Class.new(parent){ const_set(:Sub, …) }` nested several
    /// deep, then referenced as `Name::Variable::Class`) resolve.
    ///
    /// Idempotent / first-assignment-wins: a class that ALREADY has
    /// a structural `name` or a previously-stamped `assigned_name`
    /// is left untouched (CRuby keeps the first name a constant gets
    /// — a later `D = C` alias doesn't rename). Singleton-class
    /// shells (`singleton_target` set) are never stamped.
    pub(crate) fn name_anon_class(&mut self, cls: &std::rc::Rc<crate::value::Class>, qualified: &str) {
        // Already named (structurally or via a prior assignment), or
        // an eigenclass shell — don't re-stamp.
        if !cls.name.is_empty()
            || cls.assigned_name.borrow().is_some()
            || cls.singleton_target.borrow().is_some()
        {
            return;
        }
        let key = self.interner.intern(qualified);
        // Don't shadow/clobber a DIFFERENT class already registered
        // under this name. `Op::StoreConst` passes the BARE const name
        // (lexical module nesting is not encoded in the name_id), so a
        // namespaced redefinition collides with a global built-in —
        // e.g. Liquid's `module Liquid; ArgumentError = Class.new(Error)`
        // would otherwise overwrite the core `ArgumentError` in
        // `self.classes`, which `LoadConst` consults BEFORE
        // `self.constants`. Clobbering it corrupts the exception
        // hierarchy and breaks `rescue StandardError`, which silently
        // failed the real Jekyll build. The class is still bound to the
        // constant (via `self.constants` below at the StoreConst site);
        // we only decline to globally re-home it under a name a
        // different class already owns.
        if self.classes.get(&key).is_some_and(|existing| !std::rc::Rc::ptr_eq(existing, cls)) {
            return;
        }
        *cls.assigned_name.borrow_mut() = Some(qualified.to_string());
        // Re-homing changes global resolution — invalidate the const
        // ICs once for the whole promotion (covers the nested
        // `classes`/`constants` inserts in the recursion below too,
        // since each recursive call re-bumps).
        self.bump_const_gen();
        self.classes.insert(key, cls.clone());
        // Promote each nested constant into the global maps under
        // the qualified prefix. Snapshot first so we don't hold the
        // `consts` borrow across the recursive call (a nested anon
        // class's own `consts` gets walked too).
        let nested: Vec<(SymId, Value)> = cls
            .consts
            .borrow()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for (k, v) in nested {
            let bare = self.interner.resolve(k).to_string();
            let child_qual = format!("{}::{}", qualified, bare);
            let child_key = self.interner.intern(&child_qual);
            if let Value::Class(child) = &v {
                // Recurse FIRST so the child's own subtree is named
                // before we register it (name_anon_class registers
                // the child in self.classes itself; the explicit
                // insert below covers the already-named-child case
                // where recursion is a no-op).
                self.name_anon_class(child, &child_qual);
                self.classes.insert(child_key, child.clone());
            } else {
                self.constants.insert(child_key, v);
            }
        }
    }

    pub(crate) fn resolve_const_path(
        &mut self,
        start_cls: &std::rc::Rc<crate::value::Class>,
        path: &str,
        split_on_double_colon: bool,
    ) -> ConstPathOutcome {
        // When a path segment resolves to a constant whose VALUE is a
        // Class (a const alias like `Str = Literal::String`), the
        // continuation scope-name for the next segment must be that
        // class's name — NOT empty — or `Str::Double` looks the next
        // segment up under the wrong (stale) scope and misses.
        fn const_scope_name(v: &Value) -> String {
            match v {
                Value::Class(c) => c.effective_name().unwrap_or_default(),
                _ => String::new(),
            }
        }
        let (mut scope_name, segments): (String, Vec<&str>) =
            if split_on_double_colon && path.starts_with("::") {
                // Leading `::` rebases to Object's toplevel scope.
                ("Object".to_string(), path[2..].split("::").collect())
            } else if split_on_double_colon && path.contains("::") {
                (start_cls.name.clone(), path.split("::").collect())
            } else {
                (start_cls.name.clone(), vec![path])
            };
        // CRuby reports the FULL original path in the
        // wrong-name message when the structural issue is
        // visible at parse time — specifically trailing `::`
        // or triple-colon runs (`:::`). For deeper invalid
        // segments inside an otherwise structurally-valid
        // path (e.g. `Foo::lower`), CRuby reports just that
        // segment. We approximate by detecting the structural
        // shapes up front and returning WrongName with the
        // full path; the per-segment loop below handles the
        // segment-only cases.
        //
        // Caveats: CRuby's exact rule depends on path length
        // and resolution success (`Foo::Bar::` with Foo
        // missing reports `uninitialized constant Foo`
        // because the walk fails before validation). We don't
        // model that branch; accepted divergence — covered by
        // Shape 13 of the fixture which exercises CRuby's
        // canonical short-path shapes.
        // (Code-review #277 round 6 #2.)
        if split_on_double_colon
            && (path.ends_with("::") || path.contains(":::"))
        {
            return ConstPathOutcome::WrongName { name: path.to_string() };
        }
        // Build an ancestor-walk: scope_name first, then each
        // named superclass up the chain, ending at Object (the
        // toplevel). Used as the per-segment fallback when the
        // direct `scope_name::segment` lookup misses — CRuby's
        // const_get walks the inheritance chain for unqualified
        // names. The anon-class case (scope_name == "") gets
        // resolved here too: an anonymous class subclassing
        // `Foo` looks up `Foo::Bar` via the chain even though
        // the direct lookup `::Bar` would be malformed.
        //
        // Hit by mustermann's
        // `Class.new(self, &block) do translate(...) end`
        // shape: `translate`'s body calls `const_get
        // (:NodeTranslator)` from a class whose name is empty
        // (the anon Class.new(self, &block) class), and the
        // constant lives on the named parent `Translator`.
        let mut scope_chain: Vec<String> = Vec::new();
        // Seed with the original scope_name only when non-empty
        // (avoid the malformed `::segment` shape an anon class
        // would otherwise emit).
        if !scope_name.is_empty() && scope_name != "Object" {
            scope_chain.push(scope_name.clone());
        }
        // Walk the FULL ancestor chain (`flatten_ancestors`):
        // prepends, self, included modules, then up the superclass
        // chain with each super's own prepends/includes. CRuby's
        // `rb_const_get(C, :X)` searches this whole inheritance set,
        // so `C::FOO` resolves through an included module M (and a
        // superclass that includes M, and prepended modules). The
        // start class's OWN table is already seeded as entry 0 above;
        // `flatten_ancestors` re-lists it after any prepends, which
        // gives CRuby's "class-before-its-prepends" const precedence
        // for free (the entry-0 seed is the first one tried).
        //
        // Skip empty names (intermediate anonymous classes/modules)
        // and Object — its constant namespace IS the toplevel and is
        // consulted via the bare `segment.to_string()` lookup the
        // existing `scope_name == "Object"` branch handles.
        for anc in super::flatten_ancestors(start_cls) {
            if !anc.name.is_empty()
                && anc.name != "Object"
                && anc.name != scope_name
            {
                scope_chain.push(anc.name.clone());
            }
        }
        let mut current_value: Option<Value> = None;
        let mut segments_remaining: usize = segments.len();
        for segment in segments {
            if !is_valid_const_name(segment) {
                return ConstPathOutcome::WrongName { name: segment.to_string() };
            }
            // For the FIRST segment, walk the inheritance chain
            // looking for any scope that has the constant. For
            // subsequent segments (deeper into a chained const
            // path like `Foo::Bar::Baz`), keep the
            // single-scope-name lookup since `scope_name` has
            // been updated to the parent class we just resolved
            // into — chaining shouldn't restart the inheritance
            // walk.
            let lookup = if scope_name == "Object" {
                segment.to_string()
            } else if !scope_name.is_empty() {
                format!("{}::{}", scope_name, segment)
            } else if scope_chain.is_empty() {
                // No named ancestor and we're not at Object —
                // toplevel. Use the bare segment, same shape
                // as the Object branch.
                segment.to_string()
            } else {
                // Anonymous start scope with named ancestors —
                // first chain entry is the most-specific scope
                // to try.
                format!("{}::{}", scope_chain[0], segment)
            };
            let mut hit: Option<(Value, String)> = None;
            // Anonymous-scope per-class consts: first segment
            // only, and only when the starting scope is the
            // anon class itself (i.e. before we've stepped
            // into a named child via chained-path lookup).
            // const_set on an anon receiver lives here per the
            // explanation in `const_set`'s dispatch arm — this
            // is the symmetric read path.
            if current_value.is_none() && start_cls.name.is_empty() {
                let seg_id = self.interner.intern(segment);
                if let Some(v) = start_cls.consts.borrow().get(&seg_id).cloned() {
                    let nm = if let Value::Class(c) = &v {
                        c.effective_name().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    hit = Some((v, nm));
                }
            }
            // Direct lookup first.
            let direct_qid_opt = if self.interner.contains(&lookup) {
                Some(self.interner.intern(&lookup))
            } else {
                None
            };
            if hit.is_none()
                && let Some(qid) = direct_qid_opt
            {
                if let Some(c) = self.classes.get(&qid).cloned() {
                    hit = Some((Value::Class(c.clone()), c.effective_name().unwrap_or_default()));
                } else if let Some(v) = self.constants.get(&qid).cloned() {
                    hit = Some((v.clone(), const_scope_name(&v)));
                }
            }
            // Inheritance-chain fallback: only for the first
            // segment. `scope_chain` has scope_name as entry 0
            // already, so skip it and try entries 1..end.
            if hit.is_none() && current_value.is_none() {
                for ancestor in scope_chain.iter().skip(1) {
                    let chain_lookup = format!("{}::{}", ancestor, segment);
                    if !self.interner.contains(&chain_lookup) {
                        continue;
                    }
                    let chain_qid = self.interner.intern(&chain_lookup);
                    if let Some(c) = self.classes.get(&chain_qid).cloned() {
                        hit = Some((Value::Class(c.clone()), c.effective_name().unwrap_or_default()));
                        break;
                    } else if let Some(v) = self.constants.get(&chain_qid).cloned() {
                        hit = Some((v.clone(), const_scope_name(&v)));
                        break;
                    }
                }
                // Final fallback: toplevel (bare segment lookup
                // via Object) — matches CRuby's "after walking
                // the inheritance chain, try toplevel" rule.
                if hit.is_none() && self.interner.contains(segment) {
                    let tl_qid = self.interner.intern(segment);
                    if let Some(c) = self.classes.get(&tl_qid).cloned() {
                        hit = Some((Value::Class(c.clone()), c.effective_name().unwrap_or_default()));
                    } else if let Some(v) = self.constants.get(&tl_qid).cloned() {
                        hit = Some((v.clone(), const_scope_name(&v)));
                    }
                }
            }
            // Scoped autoload trigger — Phase 2 of issue #224.
            // If the qualified `lookup` (`Mod::Const`) missed but is
            // registered as a pending scoped autoload, pop it,
            // `require` the target, and retry the direct lookup once
            // — refilling `hit` so the shared handler below runs.
            // Mirrors the toplevel trigger in Op::LoadConst. Popping
            // BEFORE the require prevents re-entry into the same
            // autoload mid-flight (and turns a require that fails to
            // define the constant into a clean NameError on retry
            // rather than an infinite loop). Wasi has no `require`,
            // so the whole block compiles out there.
            #[cfg(not(target_os = "wasi"))]
            if hit.is_none()
                // Only intern when the name already exists — a
                // scoped-autoload key is always interned at
                // registration time, so a not-yet-interned `lookup`
                // can't be pending. The `contains` guard keeps a
                // `const_defined?` / `const_get` MISS from growing
                // the interner (the `const_defined_misses_do_not_
                // grow_interner` resource-cap invariant).
                && self.interner.contains(&lookup)
            {
                let lookup_id = self.interner.intern(&lookup);
                if let Some(al_path) = self.autoloads_scoped.remove(&lookup_id) {
                    match self.builtin_call("require", &[Value::new_str(al_path)]) {
                        Some(Ok(_)) => {
                            if let Some(c) = self.classes.get(&lookup_id).cloned() {
                                hit = Some((Value::Class(c.clone()), c.effective_name().unwrap_or_default()));
                            } else if let Some(v) = self.constants.get(&lookup_id).cloned() {
                                hit = Some((v.clone(), const_scope_name(&v)));
                            }
                            // require ran but didn't define `lookup`
                            // → fall through to Missing below.
                        }
                        // require itself trapped (LoadError, a syntax
                        // error in the target, …) — surface it rather
                        // than masking as "uninitialized constant".
                        Some(Err(t)) => return ConstPathOutcome::Trap(t),
                        None => {} // unreachable: "require" is a builtin
                    }
                }
            }
            if let Some((value, found_class_name)) = hit {
                segments_remaining -= 1;
                if matches!(value, Value::Class(_)) {
                    scope_name = found_class_name;
                    current_value = Some(value);
                } else {
                    if segments_remaining > 0 {
                        return ConstPathOutcome::NotClass { full_path: path.to_string() };
                    }
                    current_value = Some(value);
                }
                continue;
            }
            return ConstPathOutcome::Missing { missing_qualified: lookup };
        }
        match current_value {
            Some(v) => ConstPathOutcome::Found(v),
            None => ConstPathOutcome::Missing { missing_qualified: path.to_string() },
        }
    }

    /// CRuby-shape arity for a Proto: required positional count
    /// when the signature is fully fixed; `-(required + 1)`
    /// otherwise. Used by `Method#arity` and
    /// `UnboundMethod#arity`. Note: `Proc#arity` does NOT call
    /// this helper — blocks store rest info on `BlockHandle`
    /// (the Proto's `rest_param` field stays empty for them),
    /// and the block arm in `try_dispatch_callable_intrinsics`
    /// computes arity directly from the handle's `n_params` /
    /// `rest_slot`.
    ///
    /// The Proto's parameter layout is
    /// `[required..., optional..., rest?, kw..., kw_rest?, block?]`.
    /// `n_required_positional` covers the leading required slots;
    /// optionals are the gap between that and the rest/kw/block
    /// tail. The `block_param` slot is appended to `proto.params`
    /// so the body sees the local but it must NOT count as an
    /// optional positional for introspection.
    ///
    /// Required keyword (`def f(a:)`) bumps the mandatory count
    /// by 1 (CRuby treats the kwargs bundle as one mandatory
    /// arg). Any optional/rest position OR optional/kw_rest
    /// keyword (when no required-kw is present) flips the result
    /// negative.
    /// (TRY_RUNS layer #24.)
    pub(crate) fn proto_arity(&self, proto_idx: usize) -> i64 {
        let proto = &self.protos[proto_idx];
        let n_req_pos = proto.n_required_positional as usize;
        let rest_count = proto.rest_param.is_some() as usize;
        let kw_count = proto.kw_param_defaults.len();
        let kw_rest_count = proto.kw_rest_param.is_some() as usize;
        let block_count = proto.block_param.is_some() as usize;
        let positional_total = proto.params.len()
            .saturating_sub(rest_count + kw_count + kw_rest_count + block_count);
        let n_opt_pos = positional_total.saturating_sub(n_req_pos);
        let n_req_kw = proto.kw_param_defaults.iter().filter(|d| d.is_none()).count();
        let n_opt_kw = proto.kw_param_defaults.iter().filter(|d| d.is_some()).count();
        let req_kw_present = n_req_kw > 0;
        let effective_req = n_req_pos + req_kw_present as usize;
        let has_pos_optional = n_opt_pos > 0 || rest_count > 0;
        let has_kw_optional = !req_kw_present && (n_opt_kw > 0 || kw_rest_count > 0);
        if has_pos_optional || has_kw_optional {
            -((effective_req + 1) as i64)
        } else {
            effective_req as i64
        }
    }

    /// Default Instance allocator — `maybe_gc` + `check_alloc` +
    /// `heap.alloc(HeapObj::Instance { class, empty ivars, no
    /// singleton })` → `Value::Object`. Shared by `Class#allocate`
    /// and the default branch of `Class.new`'s allocator cascade
    /// so the two paths can't drift on GC/rooting/allocation
    /// behavior (PR #181 review #2 — Copilot flagged duplication
    /// between the two arms).
    ///
    /// Note: the `new` arm calls this through `g.vm` while inside
    /// a `PinGuard`; callers without a PinGuard call it directly
    /// on `&mut self`. Either is safe — this method does NOT pin
    /// its result, so any caller that needs to keep the new
    /// Instance alive across a later `maybe_gc` must pin
    /// (`PinGuard::pin`) before that point.
    ///
    /// Sites that intentionally do NOT use this helper:
    /// - `raise.rs` exception construction (lines 41/63/108/373)
    ///   skips `check_alloc` so a raise during budget exhaustion
    ///   does not re-trap — exception normalization must succeed
    ///   even under OOM-like conditions.
    /// - `match_data.rs:34` (regex MatchData) is a hot path where
    ///   the Instance lives immediately next to a heap-allocated
    ///   capture Array; threading them through this helper would
    ///   trigger an extra `maybe_gc` between two heap.alloc calls
    ///   and sweep the unpinned capture Array.
    ///
    /// These exemptions are intentional; flagged by PR #181
    /// code-review #3.
    pub(crate) fn alloc_default_instance(&mut self, cls: &Rc<Class>) -> Result<Value, Trap> {
        self.maybe_gc();
        self.check_alloc()?;
        // A user subclass of Hash allocates a real (tagged) Hash so
        // the Hash primitives (`[]=`, `merge!`, `size`, …) dispatch on
        // its instances; the tag carries the actual class for
        // `obj.class` / `is_a?` / user-override lookup. (Array / String
        // subclasses still fall through to a plain Instance — separate
        // follow-ups.)
        if class_inherits_named(cls, "Hash") {
            let id = self.heap.alloc(HeapObj::Hash(crate::heap::HashObj {
                pairs: Vec::new(),
                default_block: None,
                default_value: None,
                class_tag: Some(cls.clone()),
                ivars: crate::intern::FxHashMap::default(),
                index: None,
            }));
            return Ok(Value::Hash(id));
        }
        // Array twin (rouge's python lexer: `StringRegister < Array`).
        // String subclasses remain the documented follow-up.
        if class_inherits_named(cls, "Array") {
            let id = self.heap.alloc(HeapObj::Array(crate::heap::ArrayObj {
                elems: Vec::new(),
                class_tag: Some(cls.clone()),
                ivars: crate::intern::FxHashMap::default(),
            }));
            return Ok(Value::Array(id));
        }
        let id = self.heap.alloc(HeapObj::Instance(Instance {
            class: cls.clone(),
            ivars: crate::intern::FxHashMap::default(),
            singleton_class: None,
            frozen: std::cell::Cell::new(false),
        }));
        Ok(Value::Object(id))
    }

    /// Does an instance of the primitive class `class_name`
    /// respond to method `sid`? Builds a sentinel `Value` of
    /// the matching shape and consults the per-primitive
    /// `responds_to` whitelist. Aggregate primitives
    /// (Array/Hash/Range/Regexp/...) fall back to permissive
    /// `true`, matching `class_method_defined`'s shape. Used
    /// by `Op::AliasMethod` to decide whether to synthesise a
    /// primitive-forwarder Method when the source name isn't
    /// in the user-Method table.
    pub(crate) fn primitive_class_responds_to(&self, class_name: &str, sid: SymId) -> bool {
        let sentinel: Option<Value> = match class_name {
            "Integer" => Some(Value::Int(0)),
            "Float" => Some(Value::Float(0.0)),
            "String" => Some(Value::new_str("")),
            "Symbol" => Some(Value::Sym(SymId(0))),
            "TrueClass" => Some(Value::Bool(true)),
            "FalseClass" => Some(Value::Bool(false)),
            "NilClass" => Some(Value::Nil),
            _ => None,
        };
        match sentinel {
            Some(s) => self.responds_to(&s, sid, true),
            None => is_primitive_class_name(class_name),
        }
    }

    /// Runtime `attr_reader` / `attr_writer` / `attr_accessor` for an
    /// explicit Class receiver (`Foo.attr_accessor(:x)` /
    /// `Foo.singleton_class.send(:attr_accessor, :x)`). The
    /// compile-time path (compiler.rs) handles the bareword class-body
    /// form; this is the dispatch-time sibling. Installs the getter
    /// (`LoadIvar @name; Return`) and/or setter (`LoadLocal 0; Dup;
    /// StoreIvar @name; Return`) into `cls`'s instance-method table,
    /// backed by the `@name` ivar — same shape compile_proto emits.
    /// Returns the created method-name SymIds (CRuby 3.0+ return).
    pub(crate) fn install_attr_accessor(
        &mut self,
        cls: &Rc<Class>,
        sym_name: &str,
        do_reader: bool,
        do_writer: bool,
    ) -> Vec<SymId> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        let ivar_id = self.interner.intern(&format!("@{}", sym_name));
        // For an eigenclass shell `install_method` redirects to the
        // target's singleton_methods (class-level accessor); the
        // `defining_class` anchor is the real class so `super` walks
        // the right chain.
        let anchor = cls.effective_install_class();
        let mut created = Vec::new();
        if do_reader {
            let proto = Proto {
                name: format!("<attr-reader:{}>", sym_name),
                params: vec![],
                n_required_positional: 0,
                n_required_post: 0,
                rest_param: None,
                kw_param_defaults: vec![],
                kw_has_computed_default: vec![],
                kw_rest_param: None,
                block_param: None,
                n_locals: 0,
                creates_block: false,
                code: vec![Op::LoadIvar(ivar_id), Op::Return],
                op_spans: vec![Span::ZERO; 2],
                filename: "<attr_accessor>".into(),
                block_body_local_start: u16::MAX,
                byte_literals: vec![],
                const_chains: vec![],
                lexical_scope: vec![],
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            let nid = self.interner.intern(sym_name);
            cls.install_method(nid, Rc::new(crate::value::Method {
                params: vec![],
                proto_idx: idx,
                fixed_arity: None,
                defining_class: Some(Rc::downgrade(&anchor)),
                visibility: std::cell::Cell::new(crate::value::Visibility::Public),
                closure: None,
                builtin: None,
                original_name: Some(nid),
            }));
            created.push(nid);
        }
        if do_writer {
            let setter = format!("{sym_name}=");
            let proto = Proto {
                name: format!("<attr-writer:{}>", setter),
                params: vec!["val".to_string()],
                n_required_positional: 1,
                n_required_post: 0,
                rest_param: None,
                kw_param_defaults: vec![],
                kw_has_computed_default: vec![],
                kw_rest_param: None,
                block_param: None,
                n_locals: 1,
                creates_block: false,
                code: vec![Op::LoadLocal(0), Op::Dup, Op::StoreIvar(ivar_id), Op::Return],
                op_spans: vec![Span::ZERO; 4],
                filename: "<attr_accessor>".into(),
                block_body_local_start: u16::MAX,
                byte_literals: vec![],
                const_chains: vec![],
                lexical_scope: vec![],
            };
            let idx = self.protos.len();
            self.protos.push(proto);
            let nid = self.interner.intern(&setter);
            cls.install_method(nid, Rc::new(crate::value::Method {
                params: vec!["val".to_string()],
                proto_idx: idx,
                fixed_arity: None,
                defining_class: Some(Rc::downgrade(&anchor)),
                visibility: std::cell::Cell::new(crate::value::Visibility::Public),
                closure: None,
                builtin: None,
                original_name: Some(nid),
            }));
            created.push(nid);
        }
        self.method_gen = self.method_gen.wrapping_add(1);
        created
    }

    /// Build a Method that forwards to a primitive method on
    /// `self`. Emitted as the body of an `alias_method`'d
    /// primitive — when the alias is invoked, the body runs
    /// `LoadSelf; LoadLocal(0); ApplyCall(orig_id, ...); Return`
    /// so any args the caller passed flow through to the
    /// primitive call via the rest-Array slot. The forwarder
    /// Proto is appended to `self.protos` and the index is
    /// stamped into the returned Method.
    pub(crate) fn synth_primitive_forwarder(&mut self, cls: &Rc<Class>, orig_id: SymId) -> Rc<crate::value::Method> {
        use crate::bytecode::{Op, Proto};
        use crate::error::Span;
        let proto = Proto {
            name: format!("<primitive-alias-forwarder:{}>", self.interner.resolve(orig_id)),
            // `args` is the rest-arg name; proto.params lists it so
            // `invoke_method`'s arg-binding loop treats slot 0 as
            // the rest collector. n_required_positional = 0 keeps
            // the alias arity-permissive (matches primitive
            // dispatch, which is variadic).
            params: vec!["args".to_string()],
            n_required_positional: 0,
            n_required_post: 0,
            rest_param: Some("args".to_string()),
            kw_param_defaults: vec![],
            kw_has_computed_default: vec![],
            kw_rest_param: None,
            block_param: None,
            n_locals: 1,
            creates_block: false,
            code: vec![
                Op::LoadSelf,
                Op::LoadLocal(0),
                // ApplyCallPrimitive (not ApplyCall): forces primitive
                // dispatch so a later `def keys` override on the same
                // subclass doesn't capture this forwarder into infinite
                // recursion — `alias` snapshots the original method.
                Op::ApplyCallPrimitive(orig_id, u16::MAX),
                Op::Return,
            ],
            op_spans: vec![Span::ZERO; 4],
            filename: "<primitive-alias>".into(),
            block_body_local_start: u16::MAX,
            byte_literals: vec![],
            const_chains: vec![],
            lexical_scope: vec![],
        };
        let idx = self.protos.len();
        self.protos.push(proto);
        Rc::new(crate::value::Method {
            params: vec!["args".to_string()],
            proto_idx: idx,
            fixed_arity: None,
            defining_class: Some(Rc::downgrade(cls)),
            visibility: std::cell::Cell::new(crate::value::Visibility::Public),
            closure: None,
            builtin: None,
            original_name: Some(orig_id),
        })
    }
}

/// Whether `cls` is a STRICT subclass of a class named `name` —
/// i.e. some ancestor along its superclass chain (excluding `cls`
/// itself) has that name. Used to decide that `class M < Hash`
/// instances should allocate as tagged Hashes. Name-based (like the
/// `cls.name == "File"` dispatch checks) — robust because the builtin
/// Hash/Array/String classes have fixed names.
fn class_inherits_named(cls: &Rc<Class>, name: &str) -> bool {
    let mut cur = cls.superclass.borrow().clone();
    let mut guard = 0;
    while let Some(c) = cur {
        if c.name == name {
            return true;
        }
        // Cycle / runaway guard (superclass chains are short).
        guard += 1;
        if guard > 4096 {
            return false;
        }
        cur = c.superclass.borrow().clone();
    }
    false
}

/// CRuby's constant-name validation rule: the bare name must
/// start with an ASCII uppercase letter and contain only
/// `[A-Za-z0-9_]`. Empty names are rejected. Used by
/// `Module#const_defined?` / `Module#const_get` to raise the
/// CRuby-shape `NameError("wrong constant name <name>")`
/// distinct from `"uninitialized constant"` (which is for
/// valid-but-absent names). (Copilot review #277 round 3.)
pub(crate) fn is_valid_const_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Outcome of `resolve_const_path` (a single helper that
/// powers both `const_defined?` and `const_get`).
pub(crate) enum ConstPathOutcome {
    /// Path resolved to this Value (Class or other constant).
    Found(Value),
    /// Every name in the path was valid, but some step missed.
    /// `missing_qualified` is the qualified key in CRuby's
    /// `uninitialized constant Foo::Bar` shape for error
    /// reporting.
    Missing { missing_qualified: String },
    /// Some name in the path was not a valid constant identifier.
    WrongName { name: String },
    /// A scoped-autoload trigger fired `require` and it trapped
    /// (LoadError, or an error raised while loading the target).
    /// `resolve_const_path` can't return `Result`, so it threads
    /// the Trap out through this variant; every caller re-raises
    /// it via `return Err(t)`. Only constructible on non-wasi
    /// builds (the trigger is wasi-gated).
    #[cfg(not(target_os = "wasi"))]
    Trap(crate::error::Trap),
    /// A middle segment of the path resolved to a non-class /
    /// non-module value (e.g. `Foo::CONST::X` where `Foo::CONST`
    /// is `42`). CRuby raises
    /// `TypeError: <full_path> does not refer to class/module`.
    /// Pre-fix the helper continued walking with the previous
    /// scope, which could silently resolve to an unrelated
    /// sibling (`Foo::X`) or surface as a misleading
    /// `uninitialized constant` NameError. (Code-review #277
    /// round 6 #1.)
    NotClass { full_path: String },
}

fn is_primitive_class_name(name: &str) -> bool {
    matches!(
        name,
        "Integer" | "Float" | "String" | "Symbol"
            | "Array" | "Hash" | "Range"
            | "Regexp" | "Proc"
            | "Method" | "UnboundMethod"
            | "TrueClass" | "FalseClass" | "NilClass"
            // Kernel — modeled as a sentinel "primitive" so
            // `Kernel.instance_method(:foo)` resolves without
            // forcing every Kernel method to live in a class
            // table. Real CRuby: Kernel is a Module included in
            // Object, transitively giving every value its method
            // set. We don't have Modules; this sentinel makes
            // the lookup succeed and emits an UnboundMethod that
            // defers resolution to bind+call (where do_call
            // routes to the receiver's primitive method dispatch
            // as if the call were direct).
            | "Kernel"
    )
}

fn method_recv_identity(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => x == y,
        (Value::Hash(x), Value::Hash(y)) => x == y,
        (Value::Range(x), Value::Range(y)) => x == y,
        (Value::Block(x), Value::Block(y)) => x == y,
        (Value::BoundMethod(x), Value::BoundMethod(y)) => x == y,
        (Value::UnboundMethod(x), Value::UnboundMethod(y)) => x == y,
        // ObjId identity, matching `method_recv_hash`. Two BigInt
        // Values are "the same receiver" only when they share an
        // ObjId; canonical-value equality (e.g. comparing two
        // independently-allocated 2^64 BigInts) is intentionally
        // not the relation here — `bound_method == other` only
        // collapses when the underlying receiver is literally the
        // same heap slot.
        #[cfg(feature = "bignum")]
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        // Same ObjId-identity rule as BigInt (see comment above):
        // method receivers collapse only when they point at the
        // literal same heap slot, not canonical-value equality.
        (Value::Rational(x), Value::Rational(y)) => x == y,
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Sym(x), Value::Sym(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        _ => false,
    }
}

/// Hash a Method receiver consistently with `method_recv_identity`.
/// Two receivers that compare equal via `method_recv_identity`
/// must collide here.
fn method_recv_hash(v: &Value) -> i64 {
    match v {
        Value::Object(id) | Value::Array(id) | Value::Hash(id) | Value::Range(id)
        | Value::Block(id) | Value::BoundMethod(id) | Value::UnboundMethod(id)
        | Value::CurriedProc(id) | Value::Rational(id) => id.0 as i64,
        // Two BigInts that hash-equal must collide via ObjId since
        // the heap-side bigint value identity is the ObjId (we
        // never share an ObjId across different BigInt values).
        #[cfg(feature = "bignum")]
        Value::BigInt(id) => id.0 as i64,
        Value::Class(c) => Rc::as_ptr(c) as i64,
        Value::Str(s) => Rc::as_ptr(s) as i64,
        Value::Int(n) => *n,
        Value::Float(f) => f.to_bits() as i64,
        Value::Sym(s) => s.0 as i64,
        Value::Bool(true) => 1,
        Value::Bool(false) => 0,
        Value::Nil => 0xDEAD_BEEF,
        #[cfg(feature = "regex")]
        Value::Regex(r) => Rc::as_ptr(r) as i64,
    }
}

/// Is `s` a syntactically valid CRuby instance-variable name?
///
/// CRuby grammar (from `parse.y` / docs): a single `@` followed by
/// a Ruby identifier — leading char must be ASCII letter or `_`,
/// subsequent chars must be ASCII letter / digit / `_`. Used by
/// `instance_variable_get` / `instance_variable_set` to reject
/// names that CRuby would also reject:
///
///   - bare `@` (no identifier body)
///   - `@@foo` (class-variable shape, double `@`)
///   - `@1foo` (digit start after `@`)
///   - `@foo?` / `@foo=` / `@foo!` (method-name suffixes that
///     aren't legal in ivar names)
///   - non-ASCII bodies (CRuby permits some, rubyrs takes the
///     conservative ASCII-only subset; not load-bearing for
///     any caller surfaced today)
fn is_valid_ivar_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    // Need `@` + at least one identifier char.
    if bytes.len() < 2 || bytes[0] != b'@' {
        return false;
    }
    // First body char: letter or `_`. Rejects `@@x`, `@1x`, `@?x`.
    let first = bytes[1];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    // Remaining: letter / digit / `_`. Rejects `@foo?`, `@foo=`,
    // `@foo!`, `@foo-bar`.
    bytes[2..].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Compute a stable integer id for any `Value`. Backs both
/// `Object#object_id` and `BasicObject#__id__`. Ids are
/// stable for a value while that value is alive; CRuby also
/// reuses heap `object_id` values after GC, and our heap
/// encoding likewise can reuse ids after deallocation
/// (`Heap::alloc` reissues entries from a freelist; Rc
/// pointer identities can also reappear). So we promise
/// "stable while alive", not session-wide uniqueness. CRuby
/// exact values aren't observable beyond equality checks
/// (`a.object_id == b.object_id`), so this encoding diverges
/// from CRuby's exact tags but preserves the contract: same
/// (live) value → same id, distinct (simultaneously live)
/// values →
/// distinct ids (best-effort — Float encoding hashes 64 bits
/// into 60 with collision-resistance ~2^30 distinct floats;
/// distinct floats can in principle collide).
///
/// Encoding contract:
///   - CRuby-exact for the special immediates user code is known
///     to depend on:
///       * nil:   4   (CRuby 3.x — was 8 in 2.x)
///       * true:  20
///       * false: 0
///       * Int n: `n * 2 + 1` (CRuby's Fixnum tag — always odd)
///   - Distinct high-bit type discriminators for the rest, so
///     cross-type collisions are impossible:
///       * Sym:   bit 61 set
///       * Float: bit 60 set
///       * Heap:  bit 62 set, with a 4-bit type subtag at
///         bits 58..61 to distinguish Array vs Object
///         vs Hash etc.
///   - The discriminator bits are far above the range that user
///     code's integer literals reach (`|n| < 2^58` for any
///     practical int produces an id below 2^59, well clear of
///     the Sym/Float/Heap tag bits).
pub(crate) fn object_id_for(v: &crate::value::Value) -> i64 {
    use crate::value::Value;
    /// Heap-managed value id:
    ///   - bit 62        = heap discriminator
    ///   - bits 58..61   = type subtag (4 bits → 16 types)
    ///   - bits 0..57    = payload (58 bits). ObjId-backed
    ///     variants pass a u32 freelist index
    ///     here, which always fits. Rc-backed
    ///     variants (Str/Regex/Class) hash the
    ///     pointer through `scramble_ptr` first
    ///     to avoid leaking host addresses, and
    ///     the resulting 64-bit scramble is
    ///     masked into 58 bits — so two
    ///     simultaneously-live Rc allocations
    ///     can in principle collide
    ///     (~2^29 distinct live allocations
    ///     before a collision is likely).
    fn heap_id(payload: u64, type_subtag: u8) -> i64 {
        debug_assert!(type_subtag < 16, "type subtag must fit in 4 bits");
        let payload_masked = payload & 0x03FF_FFFF_FFFF_FFFF; // 58 bits
        (1i64 << 62) | ((type_subtag as i64) << 58) | (payload_masked as i64)
    }
    match v {
        // CRuby-exact Fixnum encoding `2n+1` for ints in the
        // safe range; falls back to a bit-59 tag otherwise.
        // Safe range:
        //   * `n < 0` — id is negative (sign bit set), distinct
        //     from every type-tagged id (Float/Sym/Heap all set
        //     specific positive bits and clear the sign bit).
        //     Only excluded by overflow of `2n+1` itself
        //     (i.e. `n == i64::MIN`).
        //   * `n >= 0` — id must clear bits 59..62 so it doesn't
        //     collide with Float(bit 60) / Sym(bit 61) /
        //     Heap(bit 62). That means `id < (1<<59)` i.e.
        //     `n < (1<<58)`.
        // Without this guard, e.g. `n = 1<<60` yields
        // `2n+1 = 2^61+1` which collides with `Sym(SymId(1))`.
        Value::Int(n) => match n.checked_mul(2).and_then(|m| m.checked_add(1)) {
            Some(id) if *n < 0 || id < (1i64 << 59) => id,
            _ => {
                // Out-of-range int (|n| > 2^62 roughly): hash
                // the full 64-bit pattern into 59 bits and set
                // bit 59 as the type tag. A raw low-bit mask
                // would collide on inputs with identical low 59
                // bits (e.g. `2**62` and `-(2**62)` both have
                // low-59 == 0). Bit 59 is below
                // Float(60)/Sym(61)/Heap(62) so no cross-type
                // collision; it's above the safe Int range so
                // no collision with regular `2n+1` ids.
                // Collision resistance ~2^30 distinct
                // out-of-range ints — only reachable in builds
                // without bignum promotion.
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                n.hash(&mut h);
                (1i64 << 59) | ((h.finish() & 0x07FF_FFFF_FFFF_FFFF) as i64)
            }
        },
        Value::Bool(true) => 20,
        Value::Bool(false) => 0,
        Value::Nil => 4,
        // Sym: bit 61 set; bits 0..58 = SymId. Distinct from
        // true(20)/false(0)/nil(4) because bit 61 is way above
        // their bit positions; distinct from heap (bit 62) and
        // Float (bit 60).
        Value::Sym(sid) => (1i64 << 61) | (sid.0 as i64),
        // Float: bit 60 set; low 60 bits = a hash of the f64
        // bit pattern. The bit pattern occupies all 64 bits
        // (sign + 11-bit exponent + 52-bit mantissa); a naive
        // `& 0x0FFF...` would strip the sign bit and collapse
        // `1.0` and `-1.0` to the same id. Hashing folds all 64
        // bits into 60 with collision-resistance ~2^30 distinct
        // floats — adequate for any practical workload.
        Value::Float(f) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f.to_bits().hash(&mut h);
            (1i64 << 60) | ((h.finish() & 0x0FFF_FFFF_FFFF_FFFF) as i64)
        }
        // Rc-backed values (Str/Regex/Class): use the raw
        // pointer as the *seed* for an opaque, per-process id,
        // not as the id itself. A naive `Rc::as_ptr(s) as u64`
        // would leak the host virtual address through
        // `object_id` (and through the `to_s`/`inspect`
        // fallback), weakening ASLR for embedders running
        // untrusted Ruby code. Scrambling with a process-local
        // RandomState keeps the identity contract (same Rc →
        // same id while alive) but the resulting payload is
        // not recoverable to the original address. ObjId-backed
        // variants below already use opaque freelist indices,
        // not addresses, so they don't need this treatment.
        Value::Str(s) => heap_id(scramble_ptr(std::rc::Rc::as_ptr(s) as usize), 2),
        Value::Object(id) => heap_id(id.0 as u64, 3),
        Value::Array(id) => heap_id(id.0 as u64, 4),
        Value::Hash(id) => heap_id(id.0 as u64, 5),
        Value::Range(id) => heap_id(id.0 as u64, 6),
        Value::Block(id) => heap_id(id.0 as u64, 7),
        Value::BoundMethod(id) => heap_id(id.0 as u64, 8),
        Value::UnboundMethod(id) => heap_id(id.0 as u64, 9),
        Value::CurriedProc(id) => heap_id(id.0 as u64, 10),
        #[cfg(feature = "regex")]
        Value::Regex(re) => heap_id(scramble_ptr(std::rc::Rc::as_ptr(re) as usize), 11),
        #[cfg(feature = "bignum")]
        Value::BigInt(id) => heap_id(id.0 as u64, 12),
        Value::Class(c) => heap_id(scramble_ptr(std::rc::Rc::as_ptr(c) as usize), 13),
        Value::Rational(id) => heap_id(id.0 as u64, 14),
    }
}

/// Compute the universal `Object#hash` value for `v`. Backs
/// both the `Object#hash` dispatch arm and any container that
/// needs to recurse over its children with the same salt
/// scheme.
///
/// Per-variant type tags (kept stable — changing one would
/// reshuffle every Hash key in user code on upgrade):
///   1 Int, 2 Float, 3 Str, 4 Sym, 5 Bool, 6 Nil,
///   7 heap-identity (default fallback), 8 Range,
///   9 Array (order-sensitive), 10 Hash (order-insensitive).
fn object_hash(v: &Value, heap: &crate::heap::Heap) -> i64 {
    let mut visited = std::collections::HashSet::new();
    object_hash_inner(v, heap, &mut visited)
}

/// Sentinel id returned when `object_hash_inner` re-enters a
/// container it's already inside (`a = []; a << a; a.hash`).
/// Mirrors CRuby's `rb_exec_recursive` substitute — a fixed
/// value used to break the recursion. The exact constant
/// doesn't matter as long as it's stable across runs.
const HASH_RECURSION_SENTINEL: i64 = 0x52_55_42_59_52_53_43_59; // "RUBYRSCY"

fn object_hash_inner(
    v: &Value,
    heap: &crate::heap::Heap,
    visited: &mut std::collections::HashSet<crate::value::ObjId>,
) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match v {
        Value::Int(n) => { 1u8.hash(&mut h); n.hash(&mut h); }
        Value::Float(f) => { 2u8.hash(&mut h); f.to_bits().hash(&mut h); }
        Value::Str(s) => { 3u8.hash(&mut h); s.content.borrow().hash(&mut h); }
        Value::Sym(sid) => { 4u8.hash(&mut h); sid.0.hash(&mut h); }
        Value::Bool(b) => { 5u8.hash(&mut h); b.hash(&mut h); }
        Value::Nil => { 6u8.hash(&mut h); }
        Value::Range(id) => {
            let (begin, end, excl) = {
                let r = heap.range(*id);
                (r.begin.clone(), r.end.clone(), r.exclusive)
            };
            8u8.hash(&mut h);
            object_hash_inner(&begin, heap, visited).hash(&mut h);
            object_hash_inner(&end, heap, visited).hash(&mut h);
            excl.hash(&mut h);
        }
        // Array#hash is order-sensitive — `[1,2].hash !=
        // [2,1].hash`. Feed length plus each element's content
        // hash sequentially. On re-entry (cyclic array) emit
        // the sentinel instead of recursing. We iterate by
        // index + per-step `clone()` of one element rather than
        // cloning the whole Vec up front so a 1M-element array
        // costs O(1) extra memory per hash call.
        Value::Array(id) => {
            9u8.hash(&mut h);
            if !visited.insert(*id) {
                HASH_RECURSION_SENTINEL.hash(&mut h);
            } else {
                let len = heap.array(*id).len();
                (len as u64).hash(&mut h);
                for i in 0..len {
                    let el = heap.array(*id)[i].clone();
                    object_hash_inner(&el, heap, visited).hash(&mut h);
                }
                visited.remove(id);
            }
        }
        // Hash#hash is order-INsensitive — `{a:1,b:2}.hash ==
        // {b:2,a:1}.hash` because the two hashes are `==`. We
        // XOR a per-pair combinator across pairs so pair order
        // can't affect the result, but the combinator itself
        // mixes key and value non-symmetrically (mul-then-add)
        // so a swap of key/value *within* a pair perturbs the
        // result. A bare `kh ^ vh` per pair would collide
        // structurally: e.g. `{1=>2, 2=>1}` and `{1=>1, 2=>2}`
        // both reduce to `acc = 0` despite being `!=`. Length
        // still participates so empty-vs-full disambiguates.
        Value::Hash(id) => {
            10u8.hash(&mut h);
            if !visited.insert(*id) {
                HASH_RECURSION_SENTINEL.hash(&mut h);
            } else {
                let len = heap.hash(*id).len();
                (len as u64).hash(&mut h);
                let mut acc: i64 = 0;
                for i in 0..len {
                    let (k, val) = heap.hash(*id)[i].clone();
                    let kh = object_hash_inner(&k, heap, visited);
                    let vh = object_hash_inner(&val, heap, visited);
                    // (kh * 31 + vh) — non-commutative in kh,vh
                    // so swapping key with value changes the
                    // pair's contribution; XOR across pairs
                    // keeps overall ordering irrelevant.
                    let pair_h = (kh as i128)
                        .wrapping_mul(31)
                        .wrapping_add(vh as i128) as i64;
                    acc ^= pair_h;
                }
                acc.hash(&mut h);
                visited.remove(id);
            }
        }
        // Phase C.1: structural Rational hash. Required to keep
        // the `a.eql?(b) ⇒ a.hash == b.hash` invariant after the
        // companion `ruby_eq` arm in heap.rs treats canonical
        // (num, den) as equality. Without this `Rational(1, 2)`
        // values would compare equal but hash to per-ObjId values,
        // breaking Hash key lookup.
        Value::Rational(id) => {
            let r = heap.rational(*id);
            11u8.hash(&mut h);
            r.num.hash(&mut h);
            r.den.hash(&mut h);
        }
        _ => { 7u8.hash(&mut h); object_id_for(v).hash(&mut h); }
    }
    h.finish() as i64
}

/// Resolve a Method's definition site to the ` filename:line`
/// suffix CRuby's `Method#inspect` appends. Returns an empty
/// string only when there's no proto at all (e.g. an
/// out-of-range `proto_idx`) or when a builtin has no
/// `source_label` — when the proto exists but its source
/// text isn't registered we emit ` filename:0`, mirroring
/// `Method#source_location`'s `[filename, 0]` return for
/// the same case.
///
/// Built-in Methods (Kernel reflection records etc.) carry
/// their own `source_label` on the BuiltinMeta; surface it
/// the same way `Method#source_location` does — paste the
/// label plus the meta's recorded line. `source_label: None`
/// (BasicObject's C-defined methods in CRuby) renders no
/// suffix, matching the source_location-returns-nil case.
fn method_source_suffix(
    method: &crate::value::Method,
    protos: &[crate::bytecode::Proto],
    sources: &std::collections::HashMap<std::rc::Rc<str>, std::rc::Rc<str>>,
) -> String {
    if let Some(meta) = &method.builtin {
        return match meta.source_label {
            Some(label) => format!(" {}:{}", label, meta.source_line),
            None => String::new(),
        };
    }
    let Some(proto) = protos.get(method.proto_idx) else {
        return String::new();
    };
    let filename = &proto.filename;
    let first_offset = proto.op_spans.first().map(|s| s.byte_offset).unwrap_or(0);
    let line = sources
        .get(filename.as_ref())
        .map(|src| crate::error::line_col(src, first_offset).0)
        .unwrap_or(0);
    // Even at line 0 (synth proto without source text)
    // we emit ` filename:0` rather than suppressing the
    // suffix, so `inspect` and `source_location` agree on
    // every method — `Method#source_location` returns
    // [filename, 0] in the same case.
    format!(" {}:{}", filename, line)
}

/// Render a `Proto`'s parameter list in the form CRuby's
/// `Method#inspect` uses — required positional bare,
/// optional positional with `=...`, rest with `*`, required
/// keyword with `:`, optional keyword with `: ...`, kw-rest
/// with `**`, block with `&`. Anonymous rest/kw-rest collapse
/// to bare `*` / `**`. Layout of `Proto.params` (set up in
/// `compile_def`):
///   [0..n_total_pos)    positional (required + optional, in
///                       source order); first
///                       `n_required_positional` are required.
///   if rest_param.is_some():  one slot for the rest name
///   then len(kw_param_defaults) keyword slots
///   if kw_rest_param.is_some(): one slot for the kw-rest name
///   if block_param.is_some():   one slot for the block name
/// Total derived by subtracting the tail counters from
/// `params.len()`.
fn format_method_params(proto: &crate::bytecode::Proto) -> String {
    let mut parts: Vec<String> = Vec::new();
    let n_total = proto.params.len();
    let mut tail = 0usize;
    if proto.rest_param.is_some() { tail += 1; }
    tail += proto.kw_param_defaults.len();
    if proto.kw_rest_param.is_some() { tail += 1; }
    if proto.block_param.is_some() { tail += 1; }
    let n_pos = n_total.saturating_sub(tail);
    let n_req = (proto.n_required_positional as usize).min(n_pos);

    for (i, name) in proto.params[..n_pos].iter().enumerate() {
        if i < n_req {
            parts.push(name.clone());
        } else {
            parts.push(format!("{}=...", name));
        }
    }
    let mut idx = n_pos;
    if let Some(rname) = &proto.rest_param {
        // Anonymous `def f(*)` parses to an empty rest name;
        // collapse to bare `*` to match CRuby.
        parts.push(if rname.is_empty() {
            "*".to_string()
        } else {
            format!("*{}", rname)
        });
        idx += 1;
    }
    for (i, default) in proto.kw_param_defaults.iter().enumerate() {
        let kname = &proto.params[idx + i];
        parts.push(match default {
            None => format!("{}:", kname),
            Some(_) => format!("{}: ...", kname),
        });
    }
    idx += proto.kw_param_defaults.len();
    if let Some(krname) = &proto.kw_rest_param {
        // `def f(**)` compiles with a synthetic
        // `__kw_rest_anon` slot name (compiler.rs:322) —
        // collapse it back to bare `**` for inspect.
        let is_anon = krname.is_empty() || krname == "__kw_rest_anon";
        parts.push(if is_anon {
            "**".to_string()
        } else {
            format!("**{}", krname)
        });
        idx += 1;
    }
    if let Some(bname) = &proto.block_param {
        parts.push(format!("&{}", bname));
    }
    let _ = idx;
    parts.join(", ")
}

/// Scramble a raw pointer into an opaque, process-local u64
/// suitable for embedding in `object_id`. Same pointer → same
/// scrambled value within a process (so identity holds while
/// the value is alive), but the host virtual address isn't
/// recoverable from the result. Uses the std `RandomState`'s
/// process-startup entropy as the hash key.
fn scramble_ptr(ptr: usize) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    use std::sync::OnceLock;
    static SEED: OnceLock<RandomState> = OnceLock::new();
    let rs = SEED.get_or_init(RandomState::new);
    
    
    rs.hash_one(ptr)
}

/// Return `Some(k)` when `exp` represents an integer value that fits
/// `i64`, otherwise `None`. Used by `Rational#**` and the
/// `Int/Float ** Rational` intercept to promote integer-valued Float /
/// Rational exponents back to the exact integer-power path (CRuby
/// parity — `2 ** Rational(3, 1)` returns Integer 8, not Float 8.0).
///
/// For `Value::Float`, `g.fract() == 0.0` AND `g` is within the i64
/// representable range. NaN / ±Inf return `None` and let the caller's
/// Float fallback handle them. For `Value::Rational`, `den == 1` AND
/// `num` fits i64. Int / BigInt are not Float-fallback inputs and
/// thus never reach this helper.
fn integer_valued_exp(exp: &Value, heap: &crate::heap::Heap) -> Option<i64> {
    match exp {
        Value::Float(g) => {
            if !g.is_finite() { return None; }
            if g.fract() != 0.0 { return None; }
            // Bracket against i64 limits — `g as i64` saturates on
            // out-of-range f64 (i64::MAX or i64::MIN), but the caller
            // wants None there so it falls back to Float pow.
            if *g < i64::MIN as f64 || *g > i64::MAX as f64 { return None; }
            Some(*g as i64)
        }
        Value::Rational(eid) => {
            let r = heap.rational(*eid);
            #[cfg(feature = "bignum")]
            {
                use num_traits::One;
                if !r.den.is_one() { return None; }
                i64::try_from(&r.num).ok()
            }
            #[cfg(not(feature = "bignum"))]
            {
                if r.den != 1 { return None; }
                Some(r.num)
            }
        }
        _ => None,
    }
}

/// Convert a finite `f64` to its lossless `(num, den)` Rational pair
/// with `num` carrying the sign and `den` always a positive power of 2.
/// Returns `(0, 1)` for ±0.0. Caller is responsible for filtering
/// NaN / ±Inf upstream — this assumes finiteness.
#[cfg(feature = "bignum")]
fn float_to_rational_pair_signed(f: f64) -> (num_bigint::BigInt, num_bigint::BigInt) {
    use num_bigint::BigInt;
    use num_traits::One;
    let (sign, mantissa, exp) =
        crate::vm::numeric::float_decompose(f).expect("finite per caller contract");
    if mantissa == 0 {
        return (BigInt::from(0), BigInt::one());
    }
    let mant = BigInt::from(mantissa);
    let signed = if sign < 0 { -mant } else { mant };
    if exp >= 0 {
        (signed << exp as usize, BigInt::one())
    } else {
        (signed, BigInt::one() << (-exp) as usize)
    }
}

/// Stern-Brocot mediant search — given a closed positive interval
/// `[a, b]` represented as (num, common_den) pairs (both with
/// `common_den > 0`), return the simplest fraction `p/q` that
/// lies in the interval. Matches CRuby's `nurat_rationalize_internal`
/// for the algorithm used by `Float#rationalize(eps)`.
///
/// Preconditions: `0 < a <= b`, both denominators positive.
/// The caller is responsible for the sign flip when the target
/// is negative (Stern-Brocot only handles positive intervals).
#[cfg(feature = "bignum")]
fn stern_brocot_simplest(
    mut a_num: num_bigint::BigInt,
    mut a_den: num_bigint::BigInt,
    mut b_num: num_bigint::BigInt,
    mut b_den: num_bigint::BigInt,
) -> (num_bigint::BigInt, num_bigint::BigInt) {
    use num_bigint::BigInt;
    use num_integer::Integer;
    use num_traits::{One, Zero};
    let (mut p0, mut q0) = (BigInt::zero(), BigInt::one());
    let (mut p1, mut q1) = (BigInt::one(), BigInt::zero());
    let c: BigInt;
    loop {
        // c = ceil(a_num / a_den), den > 0.
        let (q, r) = a_num.div_mod_floor(&a_den);
        let cc = if r.is_zero() { q } else { q + 1 };
        // Test cc < b_num/b_den ⇔ cc * b_den < b_num.
        if &cc * &b_den < b_num {
            c = cc;
            break;
        }
        let k = &cc - 1;
        let p2 = &k * &p1 + &p0;
        let q2 = &k * &q1 + &q0;
        // t = 1 / (b - k) = b_den / (b_num - k * b_den)
        let t_num = b_den.clone();
        let t_den = &b_num - &k * &b_den;
        // b = 1 / (a - k) = a_den / (a_num - k * a_den)
        let new_b_num = a_den.clone();
        let new_b_den = &a_num - &k * &a_den;
        a_num = t_num;
        a_den = t_den;
        b_num = new_b_num;
        b_den = new_b_den;
        p0 = p1;
        q0 = q1;
        p1 = p2;
        q1 = q2;
    }
    (&c * &p1 + p0, &c * &q1 + q0)
}
