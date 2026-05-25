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
            // Integer exponentiation. Positive exponent stays in i64
            // (saturating on overflow, matching i64::saturating_pow).
            // Negative exponent promotes to Float for the reciprocal,
            // since we don't have Rational — CRuby would give `(1/2)`,
            // we give `0.5`. Documented divergence.
            "**" => Some(if *b >= 0 {
                let exp = (*b as u64).min(u32::MAX as u64) as u32;
                Value::Int(a.saturating_pow(exp))
            } else {
                let f = (*a as f64).powi(*b as i32);
                Value::Float(f)
            }),
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
        (Value::Int(a), "abs", []) => Some(Value::Int(a.wrapping_abs())),
        (Value::Int(a), "-@", []) => Some(Value::Int(a.wrapping_neg())),
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
        _ => None,
    })
}

// Float#inspect — kept private here because it's a single-line
// inspect that just defers to to_s; if it grows we'll promote
// it to a method.
