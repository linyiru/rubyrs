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

// Float#inspect — kept private here because it's a single-line
// inspect that just defers to to_s; if it grows we'll promote
// it to a method.
