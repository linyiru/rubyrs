//! Printf-style formatter for `String#%`. Standalone — depends
//! only on `Value`, `Heap`, `Interner`, and `RubyError`, no `Vm`
//! state. Pulled out of `vm.rs` (which was past 6500 lines) as
//! the first structural-refactor step; the implementation hasn't
//! changed.

use crate::error::RubyError;
use crate::heap::Heap;
use crate::intern::Interner;
use crate::value::Value;

/// Minimal printf-style formatter used by `String#%`. Supports the
/// flag set [- + 0 space #], optional width and precision (decimal
/// integers only — `*` for argument-driven width is not yet
/// supported), and conversion specifiers d/i, f, s, x, X, o, b, B,
/// c, p, plus the literal `%%`. Positional (`%1$d`) and named
/// (`%<name>s`) directives are out of scope; encountering them
/// raises ArgumentError so the caller can `rescue`.
pub(crate) fn ruby_sprintf(
    fmt: &str,
    args: &[Value],
    heap: &Heap,
    interner: &Interner,
) -> Result<String, RubyError> {
    let mut out = String::new();
    let mut idx: usize = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut flag_minus = false;
        let mut flag_plus = false;
        let mut flag_zero = false;
        let mut flag_space = false;
        let mut flag_hash = false;
        loop {
            match chars.peek() {
                Some('-') => { flag_minus = true; chars.next(); }
                Some('+') => { flag_plus = true; chars.next(); }
                Some('0') => { flag_zero = true; chars.next(); }
                Some(' ') => { flag_space = true; chars.next(); }
                Some('#') => { flag_hash = true; chars.next(); }
                _ => break,
            }
        }
        let mut width: Option<usize> = None;
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() {
                width = Some(width.unwrap_or(0) * 10 + (d as usize - '0' as usize));
                chars.next();
            } else { break; }
        }
        let mut precision: Option<usize> = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p: usize = 0;
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    p = p * 10 + (d as usize - '0' as usize);
                    chars.next();
                } else { break; }
            }
            precision = Some(p);
        }
        let spec = chars.next().ok_or_else(|| RubyError::ArgumentError {
            msg: "malformed format string - %".into(),
        })?;
        if spec == '%' {
            out.push('%');
            continue;
        }
        let arg = args.get(idx).ok_or_else(|| RubyError::ArgumentError {
            msg: "too few arguments".into(),
        })?;
        idx += 1;
        let mut body = match spec {
            'd' | 'i' => {
                // BigInt fast path: render the decimal directly via
                // num_bigint's Display so `'%d' % (2**100)` works.
                // Base specifiers (%x/X/o/b/B) now route through
                // `format_radix_any` which has its own BigInt arm
                // — see those format-spec match arms below.
                #[cfg(feature = "bignum")]
                let big_decimal: Option<String> = match arg {
                    Value::BigInt(id) => Some(heap.bigint(*id).to_string()),
                    _ => None,
                };
                #[cfg(not(feature = "bignum"))]
                let big_decimal: Option<String> = None;
                let mut body = if let Some(s) = big_decimal {
                    let abs_digits = s.strip_prefix('-').unwrap_or(&s);
                    if s.starts_with('-') {
                        format!("-{abs_digits}")
                    } else if flag_plus {
                        format!("+{abs_digits}")
                    } else if flag_space {
                        format!(" {abs_digits}")
                    } else {
                        abs_digits.to_string()
                    }
                } else {
                    let n = coerce_int(arg)?;
                    if n < 0 {
                        format!("-{}", n.unsigned_abs())
                    } else if flag_plus {
                        format!("+{n}")
                    } else if flag_space {
                        format!(" {n}")
                    } else {
                        n.to_string()
                    }
                };
                if let Some(p) = precision {
                    // Precision on an integer pads with zeros to
                    // that many digit positions, ignoring any sign.
                    let (sign, digits) = match body.chars().next() {
                        Some(c @ ('-' | '+' | ' ')) => (c, &body[1..]),
                        _ => (' ', body.as_str()),
                    };
                    if digits.len() < p {
                        let pad = "0".repeat(p - digits.len());
                        body = if sign == ' ' && matches!(body.chars().next(), Some('-' | '+' | ' ')) {
                            format!("{sign}{pad}{digits}")
                        } else if sign == ' ' {
                            format!("{pad}{digits}")
                        } else {
                            format!("{sign}{pad}{digits}")
                        };
                    }
                }
                body
            }
            // Base-N specifiers route through `format_radix_any`,
            // which dispatches on arg shape: BigInt args render
            // via `num_bigint::BigInt::to_str_radix` on the
            // magnitude; everything else coerces to i64 and
            // defers to `format_radix_int`. Both branches render
            // negative magnitudes as `-<digits>` rather than
            // CRuby's `..f`-prefixed two's-complement form —
            // documented divergence (see `format_radix_int`
            // comment). For BigInt, the divergence applies the
            // same way.
            'x' => format_radix_any(arg, heap, 16, false, flag_hash)?,
            'X' => format_radix_any(arg, heap, 16, true, flag_hash)?,
            'o' => format_radix_any(arg, heap, 8, false, flag_hash)?,
            'b' => format_radix_any(arg, heap, 2, false, flag_hash)?,
            'B' => format_radix_any(arg, heap, 2, true, flag_hash)?,
            'f' => {
                let f = coerce_float(arg)?;
                let prec = precision.unwrap_or(6);
                let mut body = format!("{f:.*}", prec);
                if !body.starts_with('-') {
                    if flag_plus { body.insert(0, '+'); }
                    else if flag_space { body.insert(0, ' '); }
                }
                body
            }
            's' => {
                let mut body = arg.to_display(heap, interner);
                if let Some(p) = precision {
                    let truncated: String = body.chars().take(p).collect();
                    body = truncated;
                }
                body
            }
            'p' => arg.to_inspect(heap, interner),
            'c' => match arg {
                Value::Int(n) => {
                    char::from_u32(*n as u32).map(|c| c.to_string()).ok_or_else(|| {
                        RubyError::ArgumentError {
                            msg: format!("invalid character code {n} for %c"),
                        }
                    })?
                }
                Value::Str(s) => s.to_string_lossy().chars().next().map(|c| c.to_string()).unwrap_or_default(),
                _ => return Err(RubyError::TypeError {
                    msg: format!("no implicit conversion to %c from {}", arg.type_name()),
                }),
            },
            other => return Err(RubyError::ArgumentError {
                msg: format!("malformed format string - %{other}"),
            }),
        };
        // Apply width / padding. Precision was already applied
        // per-spec (d uses leading zeros to `precision`; s truncates).
        if let Some(w) = width
            && body.chars().count() < w {
                let pad_n = w - body.chars().count();
                let pad_char = if flag_zero && !flag_minus && precision.is_none()
                    && matches!(spec, 'd' | 'i' | 'x' | 'X' | 'o' | 'b' | 'B' | 'f') {
                    '0'
                } else {
                    ' '
                };
                let pad: String = std::iter::repeat_n(pad_char, pad_n).collect();
                if flag_minus {
                    body.push_str(&pad);
                } else if pad_char == '0' {
                    // Zero-pad goes inside the sign for numbers.
                    if let Some(first) = body.chars().next() {
                        if matches!(first, '-' | '+' | ' ') {
                            let rest: String = body.chars().skip(1).collect();
                            body = format!("{first}{pad}{rest}");
                        } else {
                            body = format!("{pad}{body}");
                        }
                    }
                } else {
                    body = format!("{pad}{body}");
                }
            }
        out.push_str(&body);
    }
    Ok(out)
}

fn coerce_int(v: &Value) -> Result<i64, RubyError> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Float(f) => Ok(*f as i64),
        Value::Str(s) => Ok(s.to_string_lossy().trim().parse::<i64>().unwrap_or(0)),
        Value::Nil => Err(RubyError::TypeError {
            msg: "no implicit conversion from nil to Integer".into(),
        }),
        _ => Err(RubyError::TypeError {
            msg: format!("no implicit conversion of {} to Integer", v.type_name()),
        }),
    }
}

fn coerce_float(v: &Value) -> Result<f64, RubyError> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Str(s) => Ok(s.to_string_lossy().trim().parse::<f64>().unwrap_or(0.0)),
        Value::Nil => Err(RubyError::TypeError {
            msg: "no implicit conversion from nil to Float".into(),
        }),
        _ => Err(RubyError::TypeError {
            msg: format!("no implicit conversion of {} to Float", v.type_name()),
        }),
    }
}

/// Render `arg` in `radix` (2 / 8 / 16). Dispatches:
/// - `Value::BigInt` → num_bigint's `to_str_radix(radix)` on
///   the magnitude, prefixed with sign and (if `alt`) the
///   conventional `0x` / `0X` / `0` / `0b` / `0B` prefix.
/// - All other shapes → coerce to i64 (TypeError on incoerciable
///   types) and defer to `format_radix_int`.
///
/// Negative values render as `-<digits>` rather than CRuby's
/// `..f`-prefixed two's-complement form for BOTH Int and BigInt —
/// documented divergence shared with `format_radix_int`.
#[cfg(feature = "bignum")]
fn format_radix_any(arg: &Value, heap: &Heap, radix: u32, upper: bool, alt: bool) -> Result<String, RubyError> {
    use num_bigint::Sign;
    if let Value::BigInt(id) = arg {
        let b = heap.bigint(*id);
        let prefix: &str = if !alt { "" } else {
            match radix {
                16 => if upper { "0X" } else { "0x" },
                8 => "0",
                2 => if upper { "0B" } else { "0b" },
                _ => "",
            }
        };
        // `to_str_radix` on the magnitude (positive BigUint) gives
        // lowercase digits 10..35 as 'a'..'z'. Uppercase variant
        // post-processes the same way the i64 path does.
        let mag = b.magnitude().to_str_radix(radix);
        let mag = if upper { mag.to_uppercase() } else { mag };
        let sign = if b.sign() == Sign::Minus { "-" } else { "" };
        return Ok(format!("{sign}{prefix}{mag}"));
    }
    Ok(format_radix_int(coerce_int(arg)?, radix, upper, alt))
}

#[cfg(not(feature = "bignum"))]
fn format_radix_any(arg: &Value, _heap: &Heap, radix: u32, upper: bool, alt: bool) -> Result<String, RubyError> {
    Ok(format_radix_int(coerce_int(arg)?, radix, upper, alt))
}

fn format_radix_int(n: i64, radix: u32, upper: bool, alt: bool) -> String {
    let prefix: &str = if !alt { "" } else {
        match radix { 16 => if upper { "0X" } else { "0x" }, 8 => "0", 2 => if upper { "0B" } else { "0b" }, _ => "" }
    };
    
    if n < 0 {
        // CRuby: %x on a negative int produces a two's-complement
        // representation prefixed with "..f" (an infinite ones).
        // We render just `-<unsigned digits>` which is close
        // enough for the common test cases and avoids dragging
        // in BigNum-style "..f" notation. Documented divergence.
        // `unsigned_abs()` (vs `-n`) survives `i64::MIN`, whose
        // negation would overflow i64 and panic in debug builds.
        let abs_n = n.unsigned_abs();
        let mag = match radix {
            16 => format!("{:x}", abs_n),
            8 => format!("{:o}", abs_n),
            2 => format!("{:b}", abs_n),
            _ => unreachable!(),
        };
        let mag = if upper { mag.to_uppercase() } else { mag };
        format!("-{prefix}{mag}")
    } else {
        let mag = match radix {
            16 => format!("{:x}", n as u64),
            8 => format!("{:o}", n as u64),
            2 => format!("{:b}", n as u64),
            _ => unreachable!(),
        };
        let mag = if upper { mag.to_uppercase() } else { mag };
        format!("{prefix}{mag}")
    }
}
