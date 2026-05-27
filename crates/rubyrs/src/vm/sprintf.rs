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
    max_value_bytes: Option<usize>,
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
                    Value::BigInt(id) => {
                        // Same pre-allocation cap rationale as
                        // `format_radix_any`: `to_string()` on a
                        // 10M-bit BigInt allocates ~3 MB before the
                        // post-format `out.len() > max` check in
                        // kernel.rs / string.rs can fire — host can
                        // OOM first. Estimate the decimal length from
                        // `bits()` and trap BEFORE the alloc via the
                        // same shared helper that protects the base-N
                        // arms. `sign_byte` accounts for the `-` /
                        // `+` / ` ` byte the formatting below may
                        // prepend (radix=10 has no `0x`/`0b` prefix).
                        let b = heap.bigint(*id);
                        let digits_est = super::bignum::bignum_digits_upper_bound(b.bits(), 10);
                        let sign_byte: u64 = if b.sign() == num_bigint::Sign::Minus
                            || flag_plus
                            || flag_space
                        { 1 } else { 0 };
                        let est = digits_est.saturating_add(sign_byte);
                        let cap = max_value_bytes.unwrap_or(1 << 20) as u64;
                        if est > cap {
                            return Err(RubyError::ResourceExhausted {
                                msg: format!("sprintf value size ~{} bytes > cap {}", est, cap),
                            });
                        }
                        Some(b.to_string())
                    },
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
            'x' => format_radix_any(arg, heap, 16, false, flag_hash, max_value_bytes)?,
            'X' => format_radix_any(arg, heap, 16, true, flag_hash, max_value_bytes)?,
            'o' => format_radix_any(arg, heap, 8, false, flag_hash, max_value_bytes)?,
            'b' => format_radix_any(arg, heap, 2, false, flag_hash, max_value_bytes)?,
            'B' => format_radix_any(arg, heap, 2, true, flag_hash, max_value_bytes)?,
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
                    // Zero-pad goes inside (a) the sign and
                    // (b) the alt-form prefix (`0x`/`0X`/`0b`/`0B`)
                    // for numbers. CRuby's `'%#08x' % 255` is
                    // `0x0000ff`, not `00000xff`. Octal's `0`
                    // alt prefix is itself a digit and CRuby's
                    // output happens to match unconditional
                    // zero-padding (both produce `00000007` for
                    // `'%#08o' % 7`), so we skip prefix detection
                    // for octal.
                    let bytes = body.as_bytes();
                    let sign_len = if matches!(bytes.first(), Some(b'-' | b'+' | b' ')) { 1 } else { 0 };
                    let prefix_len = if bytes.len() >= sign_len + 2
                        && bytes[sign_len] == b'0'
                        && matches!(bytes[sign_len + 1], b'x' | b'X' | b'b' | b'B')
                    {
                        2
                    } else {
                        0
                    };
                    let head_len = sign_len + prefix_len;
                    if head_len > 0 {
                        let head = &body[..head_len];
                        let rest = &body[head_len..];
                        body = format!("{head}{pad}{rest}");
                    } else {
                        body = format!("{pad}{body}");
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
fn format_radix_any(
    arg: &Value,
    heap: &Heap,
    radix: u32,
    upper: bool,
    alt: bool,
    max_value_bytes: Option<usize>,
) -> Result<String, RubyError> {
    use num_bigint::Sign;
    if let Value::BigInt(id) = arg {
        let b = heap.bigint(*id);
        // CRuby suppresses the alt-form prefix for zero values:
        // `'%#x' % 0`, `'%#o' % 0`, `'%#b' % 0` all render as
        // just `"0"`, not `"0x0"` / `"00"` / `"0b0"`. Match that
        // for both BigInt(0) and (downstream via the i64 path)
        // Int(0).
        let alt = alt && b.sign() != Sign::NoSign;
        // Pre-allocation cap check: `to_str_radix(2)` on a 10M-bit
        // BigInt allocates a ~10 MB string in one go. Estimate
        // the rendered length from the BigInt's bit count and
        // trap BEFORE the alloc — `String#%` / `Kernel#sprintf`'s
        // post-format cap check only sees the already-allocated
        // result string and can't unwind a host OOM.
        //
        // Bound: `digits_upper_bound(bits, radix) + sign_byte + prefix`
        // via the shared [`super::bignum::bignum_digits_upper_bound`]
        // helper, which uses `floor(log2(radix) * 64)` (power-of-two
        // exact path + f64 fallback) as a tight integer lower
        // bound on `log2(base)` — within ±1 char of the true digit
        // count across radix 2..=36. Earlier revisions used the
        // integer `floor(log2(radix))` which over-estimated by
        // ~10% for radix 10 — enough to false-trap rendered values
        // that would actually fit under a tight cap.
        // `sign_byte` is 1 iff negative, `prefix` is 0 / 1 (octal
        // `#`) / 2 (`0x`/`0b` `#`).
        let digits_est = super::bignum::bignum_digits_upper_bound(b.bits(), radix);
        let sign_byte: u64 = if b.sign() == Sign::Minus { 1 } else { 0 };
        let prefix_len: u64 = if !alt { 0 } else {
            match radix { 16 | 2 => 2, 8 => 1, _ => 0 }
        };
        let est = digits_est.saturating_add(sign_byte).saturating_add(prefix_len);
        let cap = max_value_bytes.unwrap_or(1 << 20) as u64;
        if est > cap {
            return Err(RubyError::ResourceExhausted {
                msg: format!("sprintf value size ~{} bytes > cap {}", est, cap),
            });
        }
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
        // uppercases in-place via `make_ascii_uppercase` rather
        // than `to_uppercase()` — the latter allocates a second
        // full-size String, doubling peak memory for large
        // BigInt formats (a `%X` of a near-cap value would push
        // us past the cap during formatting). All `to_str_radix`
        // output is ASCII, so byte-level uppercase is safe.
        let mut mag = b.magnitude().to_str_radix(radix);
        if upper { mag.make_ascii_uppercase(); }
        let sign = if b.sign() == Sign::Minus { "-" } else { "" };
        return Ok(format!("{sign}{prefix}{mag}"));
    }
    Ok(format_radix_int(coerce_int(arg)?, radix, upper, alt))
}

#[cfg(not(feature = "bignum"))]
fn format_radix_any(
    arg: &Value,
    _heap: &Heap,
    radix: u32,
    upper: bool,
    alt: bool,
    _max_value_bytes: Option<usize>,
) -> Result<String, RubyError> {
    Ok(format_radix_int(coerce_int(arg)?, radix, upper, alt))
}

fn format_radix_int(n: i64, radix: u32, upper: bool, alt: bool) -> String {
    // CRuby suppresses the alt-form prefix for `n == 0` (`'%#x' % 0`
    // → `"0"`, not `"0x0"`). Apply once before the prefix lookup
    // so both the negative and non-negative arms below see the
    // adjusted flag.
    let alt = alt && n != 0;
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
        let mut mag = match radix {
            16 => format!("{:x}", abs_n),
            8 => format!("{:o}", abs_n),
            2 => format!("{:b}", abs_n),
            _ => unreachable!(),
        };
        if upper { mag.make_ascii_uppercase(); }
        format!("-{prefix}{mag}")
    } else {
        let mut mag = match radix {
            16 => format!("{:x}", n as u64),
            8 => format!("{:o}", n as u64),
            2 => format!("{:b}", n as u64),
            _ => unreachable!(),
        };
        if upper { mag.make_ascii_uppercase(); }
        format!("{prefix}{mag}")
    }
}
