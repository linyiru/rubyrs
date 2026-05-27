//! Integer / Float primitive methods. Mirrors CRuby's `numeric.c`
//! (which holds both Integer and Float, plus Numeric and the
//! Rational/Complex stubs we don't model).
//!
//! Called from `primitive_call` (vm.rs) before the per-type
//! collection arms. Stateless — no Vm access, just receiver +
//! args + the resource cap.

use crate::error::RubyError;
use crate::value::Value;

/// Try the Int / Float / mixed-numeric arms. Returns
/// `Ok(Some(v))` on a handled call, `Ok(None)` if the receiver
/// or method shape doesn't match and `primitive_call` should
/// keep looking.
pub(crate) fn numeric_call(
    recv: &Value,
    name: &str,
    args: &[Value],
    _max_value_bytes: Option<usize>,
) -> Result<Option<Value>, RubyError> {
    Ok(match (recv, name, args) {
        // `Integer#[](i)` / `Integer#[](offset, length)` — bit
        // access. Lives BEFORE the broad `(Int, op, [Int])`
        // coercion arm below; otherwise that arm's inner-match
        // None-fallthrough would shadow these (same lesson as the
        // `Float#round(n)` shadow note earlier in this file).
        //
        // `i`-form: returns 0 / 1 for the bit at position `i`
        // (0 = LSB). Negative `i` returns 0. `i >= 64` returns
        // 0 for non-neg receiver, 1 for negative (two's-complement
        // sign extension).
        (Value::Int(a), "[]", [Value::Int(i)]) => {
            let n = *a;
            let i = *i;
            let bit: i64 = if i < 0 {
                0
            } else if i >= 64 {
                if n < 0 { 1 } else { 0 }
            } else {
                // Signed shift preserves the sign bit; arithmetic
                // shift fills high bits with the sign, matching
                // CRuby's two's-complement view of negatives.
                (n >> (i as u32)) & 1
            };
            Some(Value::Int(bit))
        }
        // `Integer#[](offset, length)` — bitfield of width `length`
        // starting at bit `offset`. `length` up to 63 fits safely
        // in i64; `length == 64` with a negative receiver returns
        // -1 (the signed bit pattern of all-ones) where CRuby
        // returns `2**64 - 1` — documented saturation, doesn't
        // block the bigint.rb `bigint[off, 32]` path that motivates
        // this commit.
        //
        // Negative offset / length both return 0.
        (Value::Int(a), "[]", [Value::Int(offset), Value::Int(length)]) => {
            let n = *a;
            let off = *offset;
            let len = *length;
            if len <= 0 || off < 0 {
                return Ok(Some(Value::Int(0)));
            }
            let result: i64 = if off >= 64 {
                if n < 0 {
                    if len >= 64 { -1 } else { (1i64 << len) - 1 }
                } else { 0 }
            } else {
                let actual_len = len.min(64 - off);
                let shifted = n >> (off as u32);
                if actual_len >= 64 {
                    shifted
                } else {
                    shifted & ((1i64 << actual_len) - 1)
                }
            };
            Some(Value::Int(result))
        }
        // `Integer#to_s(radix)` — base-`radix` rendering, lowercase
        // for digits >= 10. Radix must be 2..=36 (CRuby's accepted
        // range); anything outside raises ArgumentError. Negative
        // receivers get a leading `-` followed by the magnitude in
        // the requested radix. `unsigned_abs` avoids overflow on
        // `i64::MIN`. Lives BEFORE the broad `(Int, op, [Int])`
        // coercion arm because that arm's inner-op match would
        // otherwise shadow this (None-fallthrough inside the
        // inner match doesn't surface to the outer pattern walk
        // — same lesson as the `Float#round(n)` shadow note and
        // the `Integer#[]` placement above).
        (Value::Int(a), "to_s", [Value::Int(radix)]) => {
            let r = *radix;
            if !(2..=36).contains(&r) {
                return Err(RubyError::ArgumentError {
                    msg: format!("invalid radix {}", r),
                });
            }
            let r = r as u32;
            let mut n: u64 = a.unsigned_abs();
            let neg = *a < 0;
            if n == 0 {
                return Ok(Some(Value::new_str("0")));
            }
            let mut buf = Vec::<u8>::new();
            while n > 0 {
                let d = (n % r as u64) as u32;
                let ch = std::char::from_digit(d, r).expect("digit < radix");
                buf.push(ch as u8);
                n /= r as u64;
            }
            if neg { buf.push(b'-'); }
            buf.reverse();
            Some(Value::new_str(
                String::from_utf8(buf).expect("ASCII digits + sign"),
            ))
        }
        // `Integer#to_s(non_integer)` — radix must be Integer.
        // Without this arm `5.to_s("x")` would fall through to
        // `NoMethodError`, diverging from the BigInt path (which
        // raises `TypeError` with the same wording) and from
        // CRuby. Mirrors `bigint_primitive`'s non-Int radix arm
        // so the unified Integer#to_s API stays consistent across
        // Int vs BigInt receivers. Lives BEFORE the broad
        // `(Int, op, [Int])` coercion arm for the same shadow
        // reason as the 1-arg `to_s` above.
        //
        // BigInt radix is a special sub-case: by the canonical-
        // BigInt invariant any `Value::BigInt` is out of i64 range,
        // so it can never be in the 2..=36 valid radix range, but
        // it IS an Integer — raising TypeError "no implicit
        // conversion of Integer into Integer" (the literal output
        // of `type_name_for_coerce`) is nonsensical. CRuby raises
        // `RangeError: bignum too big to convert into 'long'` for
        // this exact shape; match that.
        #[cfg(feature = "bignum")]
        (Value::Int(_), "to_s", [Value::BigInt(_)]) => {
            return Err(RubyError::RangeError {
                msg: "bignum too big to convert into `long'".to_string(),
            });
        }
        (Value::Int(_), "to_s", [other]) => {
            return Err(RubyError::TypeError {
                msg: format!(
                    "no implicit conversion of {} into Integer",
                    type_name_for_coerce(other),
                ),
            });
        }
        // `Integer#pow(exp)` — 1-arg form is an alias for `**` for
        // numeric exponents (Int / Float / BigInt under bignum).
        // Sits BEFORE the broader `(Int, op, [Int])` arm because
        // that arm's inner-match `_ => None` fallthrough would
        // otherwise consume the (Int, "pow", [Int]) shape and
        // prevent the top-level alias from firing. Delegating to
        // `**` keeps ZeroDivisionError / identity short-circuits /
        // demote-on-fit centralised. Non-numeric exponents (String,
        // Symbol, nil, …) raise TypeError matching CRuby — the
        // `**` operator's own dispatch would otherwise surface
        // NoMethodError, which is the wrong error class.
        (Value::Int(_), "pow", [arg]) => {
            let acceptable = match arg {
                Value::Int(_) | Value::Float(_) => true,
                #[cfg(feature = "bignum")]
                Value::BigInt(_) => true,
                _ => false,
            };
            if !acceptable {
                return Err(RubyError::TypeError {
                    msg: format!(
                        "{} can't be coerced into Integer",
                        type_name_for_coerce(arg),
                    ),
                });
            }
            return numeric_call(recv, "**", args, _max_value_bytes);
        }
        // Arity guard for `pow` — CRuby raises ArgumentError with
        // the exact "wrong number of arguments (given N, expected
        // 1..2)" message for 0 or >2 args. Without this arm those
        // shapes fall through to NoMethodError despite
        // `respond_to?(:pow)` returning true. The 2-arg arms below
        // catch the valid `[exp, mod]` shape; this guard catches
        // 0, 3+ (1-arg is handled above). The pattern matches any
        // `args` slice because the count check is in the guard.
        (Value::Int(_), "pow", args_slice) if args_slice.len() != 2 => {
            return Err(RubyError::ArgumentError {
                msg: format!(
                    "wrong number of arguments (given {}, expected 1..2)",
                    args_slice.len(),
                ),
            });
        }
        (Value::Int(a), op, [Value::Int(b)]) => match op {
            "+" => Some(Value::Int(a + b)),
            "-" => Some(Value::Int(a - b)),
            "*" => Some(Value::Int(a * b)),
            "/" => {
                if *b == 0 {
                    return Err(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    });
                }
                Some(Value::Int(a / b))
            }
            "%" => {
                if *b == 0 {
                    return Err(RubyError::ZeroDivisionError {
                        msg: "divided by 0".to_string(),
                    });
                }
                Some(Value::Int(a % b))
            }
            "==" => Some(Value::Bool(a == b)),
            "!=" => Some(Value::Bool(a != b)),
            "<"  => Some(Value::Bool(a < b)),
            "<=" => Some(Value::Bool(a <= b)),
            ">"  => Some(Value::Bool(a > b)),
            ">=" => Some(Value::Bool(a >= b)),
            "<=>" => Some(Value::Int(a.cmp(b) as i64)),
            // Integer exponentiation. Positive exponent uses
            // `checked_pow` so overflow is detectable — with
            // `bignum` on we DECLINE (return None) and let
            // bigint_primitive promote to BigInt. Without
            // `bignum`, fall back to `saturating_pow` so the
            // pre-Phase-B behaviour is preserved.
            // Negative exponent promotes to Float for the
            // reciprocal, since we don't have Rational — CRuby
            // would give `(1/2)`, we give `0.5`. Documented
            // divergence.
            "**" => {
                if *b < 0 {
                    // 0 ** negative is a divide-by-zero in CRuby
                    // (the reciprocal of 0 is undefined). Match
                    // by raising before falling into the Float
                    // reciprocal arm — otherwise `(0_u64 as f64)
                    // .powf(-1.0)` silently returns +Infinity,
                    // which then propagates through user code
                    // without surfacing the error.
                    if *a == 0 {
                        return Err(RubyError::ZeroDivisionError {
                            msg: "divided by 0".to_string(),
                        });
                    }
                    // Negative exponent → Float reciprocal.
                    // ±1 bases have exact ±1.0 results decided by
                    // exponent parity, but `(*b as f64)` loses
                    // parity beyond 2**53 — short-circuit before
                    // powf to keep (-1) ** large_odd correct.
                    if *a == 1 {
                        Some(Value::Float(1.0))
                    } else if *a == -1 {
                        Some(Value::Float(if *b & 1 == 0 { 1.0 } else { -1.0 }))
                    } else {
                        // Compute powf on |a| so a negative base
                        // doesn't combine with an f64-rounded
                        // exponent to yield NaN (libm `powf`
                        // returns NaN when the base is negative
                        // and the exp isn't exactly representable
                        // as an integer in f64). Re-apply the
                        // sign from the original i64 parity so
                        // `(-2) ** large_odd_neg` stays negative
                        // regardless of f64 rounding past 2**53.
                        let mag = (a.unsigned_abs() as f64).powf(*b as f64);
                        let signed = if *a < 0 && *b & 1 != 0 { -mag } else { mag };
                        Some(Value::Float(signed))
                    }
                } else if *a == 0 {
                    // 0**0 == 1; 0**n (n>0) == 0. Exact regardless
                    // of exp size — short-circuit before u32 cast.
                    Some(Value::Int(if *b == 0 { 1 } else { 0 }))
                } else if *a == 1 {
                    Some(Value::Int(1))
                } else if *a == -1 {
                    // Parity decides: even exp → 1, odd → -1.
                    Some(Value::Int(if *b & 1 == 0 { 1 } else { -1 }))
                } else {
                    // |a| > 1: any exp that doesn't fit u32
                    // overflows i64 anyway. With bignum we decline
                    // on either u32-overflow or i64-overflow so
                    // bigint_primitive can produce the real value
                    // (or trap ResourceExhausted with an honest
                    // estimate). Without bignum, saturate.
                    match u32::try_from(*b) {
                        Ok(exp) => {
                            #[cfg(feature = "bignum")]
                            { a.checked_pow(exp).map(Value::Int) }
                            #[cfg(not(feature = "bignum"))]
                            { Some(Value::Int(a.saturating_pow(exp))) }
                        }
                        Err(_) => {
                            #[cfg(feature = "bignum")]
                            { None }
                            #[cfg(not(feature = "bignum"))]
                            { Some(Value::Int(a.saturating_pow(u32::MAX))) }
                        }
                    }
                }
            }
            // Bitwise. Ruby uses arbitrary-precision Integer; we
            // truncate to i64. `<<` on a negative shift count is
            // CRuby's right-shift (and vice versa) — we mirror with
            // a sign check rather than panicking on negative shifts.
            "&" => Some(Value::Int(a & b)),
            "|" => Some(Value::Int(a | b)),
            "^" => Some(Value::Int(a ^ b)),
            "<<" => Some(Value::Int(
                if *b >= 0 { a.wrapping_shl((*b as u32).min(63)) }
                else { a.wrapping_shr(((-b) as u32).min(63)) }
            )),
            ">>" => Some(Value::Int(
                if *b >= 0 { a.wrapping_shr((*b as u32).min(63)) }
                else { a.wrapping_shl(((-b) as u32).min(63)) }
            )),
            _ => None,
        },
        // 2-arg form `pow(exp, mod)` — under `bignum`, declined here
        // so bigint_primitive's modpow path handles it (full
        // Integer×Integer×Integer coverage including BigInt).
        // Without `bignum`, implement square-and-multiply with i128
        // intermediates so `respond_to?(:pow)` stays consistent with
        // dispatch on the no-bignum profile. CRuby semantics:
        // ZeroDivisionError on mod==0, RangeError on neg exp,
        // floor-mod (same sign as modulus) on the result.
        #[cfg(not(feature = "bignum"))]
        (Value::Int(a), "pow", [Value::Int(exp), Value::Int(modulus)]) => {
            if *modulus == 0 {
                return Err(RubyError::ZeroDivisionError { msg: "divided by 0".to_string() });
            }
            if *exp < 0 {
                return Err(RubyError::RangeError {
                    msg: "Integer#pow() 1st argument cannot be negative when 2nd argument specified".to_string(),
                });
            }
            // i128 arithmetic: b ∈ [0, |m|) and b² fits since
            // |m| ≤ 2^63 ⇒ b² ≤ 2^126 < i128::MAX.
            let m_abs: i128 = (*modulus as i128).unsigned_abs() as i128;
            let mut result: i128 = if m_abs == 1 { 0 } else { 1 };
            let mut base: i128 = (*a as i128).rem_euclid(m_abs);
            let mut e: i64 = *exp;
            while e > 0 {
                if e & 1 == 1 {
                    result = (result * base).rem_euclid(m_abs);
                }
                e >>= 1;
                if e > 0 {
                    base = (base * base).rem_euclid(m_abs);
                }
            }
            // Adjust for floor-mod: result has same sign as modulus.
            let adjusted = if *modulus < 0 && result != 0 { result - m_abs } else { result };
            Some(Value::Int(adjusted as i64))
        }
        // Non-Integer exponent (with mod given) — match CRuby's
        // distinct "1st argument is integer" message. Kept ahead
        // of the "all arguments are integers" arm so the more
        // specific message wins.
        #[cfg(not(feature = "bignum"))]
        (Value::Int(_), "pow", [exp, _]) if !matches!(exp, Value::Int(_)) => {
            return Err(RubyError::TypeError {
                msg: "Integer#pow() 2nd argument not allowed unless a 1st argument is integer".to_string(),
            });
        }
        // Non-Integer modulus (exp is Int) — CRuby's "all arguments
        // are integers" message. This fires when the exponent passed
        // the integer check above but the modulus did not.
        #[cfg(not(feature = "bignum"))]
        (Value::Int(_), "pow", [_, _]) => {
            return Err(RubyError::TypeError {
                msg: "Integer#pow() 2nd argument not allowed unless all arguments are integers".to_string(),
            });
        }
        (Value::Int(a), "to_s", []) | (Value::Int(a), "inspect", []) => {
            Some(Value::new_str(a.to_string()))
        }
        (Value::Int(a), "to_i", []) => Some(Value::Int(*a)),
        // `abs` / `-@`: i64::MIN.abs() and -i64::MIN both overflow
        // i64 (CRuby promotes to Bignum). With `bignum` on, decline
        // here so bigint_primitive's unary arm produces the
        // BigInt(2^63). Without `bignum`, keep the historical
        // wrapping behaviour (returns i64::MIN unchanged).
        (Value::Int(a), "abs", []) => {
            #[cfg(feature = "bignum")]
            { if *a == i64::MIN { None } else { Some(Value::Int(a.wrapping_abs())) } }
            #[cfg(not(feature = "bignum"))]
            { Some(Value::Int(a.wrapping_abs())) }
        }
        (Value::Int(a), "-@", []) => {
            #[cfg(feature = "bignum")]
            { if *a == i64::MIN { None } else { Some(Value::Int(a.wrapping_neg())) } }
            #[cfg(not(feature = "bignum"))]
            { Some(Value::Int(a.wrapping_neg())) }
        }
        (Value::Int(a), "+@", []) => Some(Value::Int(*a)),
        (Value::Int(a), "~", []) => Some(Value::Int(!a)),
        (Value::Int(a), "even?", []) => Some(Value::Bool(a % 2 == 0)),
        (Value::Int(a), "odd?", []) => Some(Value::Bool(a % 2 != 0)),
        (Value::Int(a), "zero?", []) => Some(Value::Bool(*a == 0)),
        (Value::Int(a), "positive?", []) => Some(Value::Bool(*a > 0)),
        (Value::Int(a), "negative?", []) => Some(Value::Bool(*a < 0)),
        // `Integer#bit_length` — number of bits required to
        // represent the magnitude. For negatives, CRuby uses
        // two's-complement semantics: bit_length(-1) = 0,
        // bit_length(-256) = 8. Equivalent to `bit_length(~n)`
        // for negative `n`. For non-negatives, it's the position
        // of the most-significant 1 bit.
        (Value::Int(a), "bit_length", []) => {
            let magnitude: u64 = if *a >= 0 { *a as u64 } else { !(*a as u64) };
            let bits = 64 - magnitude.leading_zeros();
            Some(Value::Int(bits as i64))
        }
        (Value::Int(a), "succ", []) | (Value::Int(a), "next", []) => Some(Value::Int(a.wrapping_add(1))),
        (Value::Int(a), "pred", []) => Some(Value::Int(a.wrapping_sub(1))),
        (Value::Int(a), "to_f", []) => Some(Value::Float(*a as f64)),
        // `Integer#chr` — single-byte binary String for the 0..255
        // range. CRuby supports `chr(Encoding)` to widen the range
        // (Unicode codepoints up to U+10FFFF for UTF-8); the
        // encoding-aware form depends on an encoding model we don't
        // model in Tier 1 (ADR 0017 row "Refinements, full pattern
        // matching, full encoding model, ..." is Tier 3/4). The
        // 0..255 byte form is the one msgpack / pack-style binary
        // protocols actually reach for; out-of-range raises
        // RangeError to match CRuby's message shape.
        (Value::Int(a), "chr", []) => {
            let n = *a;
            if !(0..=255).contains(&n) {
                return Err(RubyError::RangeError {
                    msg: format!("{} out of char range", n),
                });
            }
            Some(Value::new_str_bytes(vec![n as u8]))
        }

        // Float × Float
        (Value::Float(a), op, [Value::Float(b)]) => match op {
            "+" => Some(Value::Float(a + b)),
            "-" => Some(Value::Float(a - b)),
            "*" => Some(Value::Float(a * b)),
            // Float / 0.0 == ±Infinity (or NaN), not an exception —
            // matches IEEE 754 and CRuby.
            "/" => Some(Value::Float(a / b)),
            "%" => Some(Value::Float(a % b)),
            "==" => Some(Value::Bool(a == b)),
            "!=" => Some(Value::Bool(a != b)),
            "<"  => Some(Value::Bool(a < b)),
            "<=" => Some(Value::Bool(a <= b)),
            ">"  => Some(Value::Bool(a > b)),
            ">=" => Some(Value::Bool(a >= b)),
            // `partial_cmp` returns None on NaN-involved
            // comparisons; CRuby's spec is the same: `(0.0/0.0)
            // <=> 1.0 == nil`.
            "<=>" => Some(match a.partial_cmp(b) {
                Some(o) => Value::Int(o as i64),
                None => Value::Nil,
            }),
            "**" => Some(Value::Float(a.powf(*b))),
            _ => None,
        },
        // Precision-arg form of Float#round / #truncate. Lives
        // BEFORE the mixed-numeric coercion arm below — otherwise
        // that broader `(Float, op, [Int])` arm shadows these.
        // Positive `n` keeps `n` digits after the decimal point
        // and returns a Float; negative `n` zeros out the
        // low-order Integer digits and returns Int. Mirrors
        // CRuby's `Float#round(n)` / `#truncate(n)` shape.
        (Value::Float(a), "round", [Value::Int(n)]) => {
            if *n == 0 {
                Some(Value::Int(a.round() as i64))
            } else if *n > 0 {
                let p = 10f64.powi((*n).min(15) as i32);
                Some(Value::Float((a * p).round() / p))
            } else {
                let p = 10f64.powi((-*n).min(18) as i32);
                Some(Value::Int(((a / p).round() * p) as i64))
            }
        }
        (Value::Float(a), "truncate", [Value::Int(n)]) => {
            if *n == 0 {
                Some(Value::Int(a.trunc() as i64))
            } else if *n > 0 {
                let p = 10f64.powi((*n).min(15) as i32);
                Some(Value::Float((a * p).trunc() / p))
            } else {
                let p = 10f64.powi((-*n).min(18) as i32);
                Some(Value::Int(((a / p).trunc() * p) as i64))
            }
        }
        // Mixed Int/Float — CRuby's "Float wins" coercion.
        (Value::Float(a), op, [Value::Int(b)]) => {
            let b = *b as f64;
            match op {
                "+" => Some(Value::Float(a + b)),
                "-" => Some(Value::Float(a - b)),
                "*" => Some(Value::Float(a * b)),
                "/" => Some(Value::Float(a / b)),
                "%" => Some(Value::Float(a % b)),
                "==" => Some(Value::Bool(*a == b)),
                "!=" => Some(Value::Bool(*a != b)),
                "<"  => Some(Value::Bool(*a < b)),
                "<=" => Some(Value::Bool(*a <= b)),
                ">"  => Some(Value::Bool(*a > b)),
                ">=" => Some(Value::Bool(*a >= b)),
                "<=>" => Some(match a.partial_cmp(&b) {
                    Some(o) => Value::Int(o as i64),
                    None => Value::Nil,
                }),
                "**" => Some(Value::Float(a.powf(b))),
                _ => None,
            }
        }
        (Value::Int(a), op, [Value::Float(b)]) => {
            let a = *a as f64;
            match op {
                "+" => Some(Value::Float(a + b)),
                "-" => Some(Value::Float(a - b)),
                "*" => Some(Value::Float(a * b)),
                "/" => Some(Value::Float(a / b)),
                "%" => Some(Value::Float(a % b)),
                "==" => Some(Value::Bool(a == *b)),
                "!=" => Some(Value::Bool(a != *b)),
                "<"  => Some(Value::Bool(a < *b)),
                "<=" => Some(Value::Bool(a <= *b)),
                ">"  => Some(Value::Bool(a > *b)),
                ">=" => Some(Value::Bool(a >= *b)),
                "<=>" => Some(match a.partial_cmp(b) {
                    Some(o) => Value::Int(o as i64),
                    None => Value::Nil,
                }),
                "**" => Some(Value::Float(a.powf(*b))),
                _ => None,
            }
        }
        // Float predicates and conversions.
        (Value::Float(a), "to_s", []) | (Value::Float(a), "inspect", []) => {
            Some(Value::new_str(crate::heap::format_float(*a)))
        }
        (Value::Float(a), "to_f", []) => Some(Value::Float(*a)),
        (Value::Float(a), "to_i", []) => Some(Value::Int(*a as i64)),
        (Value::Float(a), "abs", []) => Some(Value::Float(a.abs())),
        (Value::Float(a), "-@", []) => Some(Value::Float(-*a)),
        (Value::Float(a), "+@", []) => Some(Value::Float(*a)),
        (Value::Float(a), "zero?", []) => Some(Value::Bool(*a == 0.0)),
        (Value::Float(a), "positive?", []) => Some(Value::Bool(*a > 0.0)),
        (Value::Float(a), "negative?", []) => Some(Value::Bool(*a < 0.0)),
        (Value::Float(a), "nan?", []) => Some(Value::Bool(a.is_nan())),
        (Value::Float(a), "infinite?", []) => {
            // CRuby's `Float#infinite?` returns 1 / -1 / nil, not bool.
            if a.is_infinite() {
                Some(Value::Int(if *a > 0.0 { 1 } else { -1 }))
            } else {
                Some(Value::Nil)
            }
        }
        (Value::Float(a), "finite?", []) => Some(Value::Bool(a.is_finite())),
        (Value::Float(a), "floor", []) => Some(Value::Int(a.floor() as i64)),
        (Value::Float(a), "ceil", []) => Some(Value::Int(a.ceil() as i64)),
        (Value::Float(a), "round", []) => Some(Value::Int(a.round() as i64)),
        (Value::Float(a), "truncate", []) => Some(Value::Int(a.trunc() as i64)),
        _ => None,
    })
}

/// Ruby class-name for the `"<X> can't be coerced into Integer"`
/// TypeError that `Integer#pow(non_numeric)` raises (matches CRuby
/// exactly for the common types). Stateless so it lives here next
/// to the pow alias; the bigint_primitive path uses the same fn
/// via `super::numeric::type_name_for_coerce` rather than
/// duplicating. Symbols fall back to the class name `"Symbol"`
/// instead of CRuby's `:symname` (inspect form) because numeric.rs
/// has no heap access to resolve the SymId — minor divergence
/// limited to the error message text on a non-numeric exponent.
pub(crate) fn type_name_for_coerce(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "Integer",
        Value::Float(_) => "Float",
        #[cfg(feature = "bignum")]
        Value::BigInt(_) => "Integer",
        Value::Str(_) => "String",
        Value::Sym(_) => "Symbol",
        Value::Nil => "nil",
        Value::Bool(true) => "true",
        Value::Bool(false) => "false",
        Value::Array(_) => "Array",
        Value::Hash(_) => "Hash",
        Value::Range(_) => "Range",
        _ => "Object",
    }
}

/// Like `type_name_for_coerce` but returns the CRuby **class name**
/// instead of the inspect-friendly token, and preserves real class
/// names for the heap-managed variants that `type_name_for_coerce`
/// collapses to its generic `"Object"` fallback.
///
/// Use this when building error messages that should match CRuby's
/// exact text — `"can't modify frozen NilClass: nil"` rather than
/// `"can't modify frozen nil: nil"`, `"frozen Proc: ..."` rather
/// than `"frozen Object: ..."`. Divergences from
/// `type_name_for_coerce`:
///
///   - `Nil` → `NilClass` (not `nil`)
///   - `Bool(true)` → `TrueClass`; `Bool(false)` → `FalseClass`
///     (not `true` / `false`)
///   - `Block` / `CurriedProc` → `Proc` (CRuby class for closures)
///   - `BoundMethod` → `Method`
///   - `UnboundMethod` → `UnboundMethod`
///   - `Regex` → `Regexp` (under the `regex` feature)
///
/// Numeric / String / Sym / Array / Hash / Range share their names
/// with `type_name_for_coerce`. `Value::Object` (user-class
/// instance) still falls back to `"Object"` here because the
/// helper doesn't have heap access to resolve the per-instance
/// class — callers that need the precise class name on Object
/// receivers should use `Vm::class_of` directly.
pub(crate) fn class_name_for_error(v: &Value) -> &'static str {
    match v {
        Value::Nil => "NilClass",
        Value::Bool(true) => "TrueClass",
        Value::Bool(false) => "FalseClass",
        Value::Block(_) | Value::CurriedProc(_) => "Proc",
        Value::BoundMethod(_) => "Method",
        Value::UnboundMethod(_) => "UnboundMethod",
        #[cfg(feature = "regex")]
        Value::Regex(_) => "Regexp",
        other => type_name_for_coerce(other),
    }
}

// Float#inspect — kept private here because it's a single-line
// inspect that just defers to to_s; if it grows we'll promote
// it to a method.

// ---------------------------------------------------------------
// Stateful counterparts: the helpers above are pure (no Vm), but
// the reduce-style accumulators in `Array#sum`/`Range#sum`/
// `Array#inject`/`Range#inject` need a path that can allocate a
// BigInt result when an Int×Int op overflows. That requires Vm
// (heap + interner), so it lives as an `impl Vm` block. The main
// `Op::BinOp` step.rs path doesn't go through this helper — it
// inlines the same logic with BinOpInt/BinOp fast paths because
// each instruction already has the operands unwrapped in locals;
// routing through a helper would add an avoidable match on the
// i64 fast path. If those two paths ever drift apart, refactor
// both onto this helper.
impl crate::vm::Vm {
    /// Apply an Int×Int op, promoting to BigInt on Add/Sub/Mul
    /// overflow when `bignum` is on. Use this instead of calling
    /// `kind.apply_int` directly anywhere the result needs to be
    /// pushed back as a Value.
    pub(crate) fn apply_int_promote(
        &mut self,
        kind: crate::bytecode::BinOpKind,
        x: i64,
        y: i64,
    ) -> Result<Value, crate::error::Trap> {
        if let Some(v) = kind.apply_int(x, y) {
            return Ok(v);
        }
        // None can only happen under `feature = "bignum"`.
        #[cfg(feature = "bignum")]
        {
            self.bigint_arith(kind, &Value::Int(x), &Value::Int(y))
                .expect("ICE: bigint_arith None for Int operands")
        }
        #[cfg(not(feature = "bignum"))]
        unreachable!("apply_int returns None only when bignum is on");
    }
}
