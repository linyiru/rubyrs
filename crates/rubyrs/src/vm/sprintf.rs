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
                let n = coerce_int(arg)?;
                let mut body = if n < 0 {
                    format!("-{}", n.unsigned_abs())
                } else if flag_plus {
                    format!("+{n}")
                } else if flag_space {
                    format!(" {n}")
                } else {
                    n.to_string()
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
            'x' => format_radix_int(coerce_int(arg)?, 16, false, flag_hash),
            'X' => format_radix_int(coerce_int(arg)?, 16, true, flag_hash),
            'o' => format_radix_int(coerce_int(arg)?, 8, false, flag_hash),
            'b' => format_radix_int(coerce_int(arg)?, 2, false, flag_hash),
            'B' => format_radix_int(coerce_int(arg)?, 2, true, flag_hash),
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
                Value::Str(s) => s.borrow().chars().next().map(|c| c.to_string()).unwrap_or_default(),
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
        if let Some(w) = width {
            if body.chars().count() < w {
                let pad_n = w - body.chars().count();
                let pad_char = if flag_zero && !flag_minus && precision.is_none()
                    && matches!(spec, 'd' | 'i' | 'x' | 'X' | 'o' | 'b' | 'B' | 'f') {
                    '0'
                } else {
                    ' '
                };
                let pad: String = std::iter::repeat(pad_char).take(pad_n).collect();
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
        }
        out.push_str(&body);
    }
    Ok(out)
}

fn coerce_int(v: &Value) -> Result<i64, RubyError> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Float(f) => Ok(*f as i64),
        Value::Str(s) => Ok(s.borrow().trim().parse::<i64>().unwrap_or(0)),
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
        Value::Str(s) => Ok(s.borrow().trim().parse::<f64>().unwrap_or(0.0)),
        Value::Nil => Err(RubyError::TypeError {
            msg: "no implicit conversion from nil to Float".into(),
        }),
        _ => Err(RubyError::TypeError {
            msg: format!("no implicit conversion of {} to Float", v.type_name()),
        }),
    }
}

fn format_radix_int(n: i64, radix: u32, upper: bool, alt: bool) -> String {
    let prefix: &str = if !alt { "" } else {
        match radix { 16 => if upper { "0X" } else { "0x" }, 8 => "0", 2 => if upper { "0B" } else { "0b" }, _ => "" }
    };
    let body = if n < 0 {
        // CRuby: %x on a negative int produces a two's-complement
        // representation prefixed with "..f" (an infinite ones).
        // We render just `-<unsigned digits>` which is close
        // enough for the common test cases and avoids dragging
        // in BigNum-style "..f" notation. Documented divergence.
        let mag = match radix {
            16 => format!("{:x}", (-n) as u64),
            8 => format!("{:o}", (-n) as u64),
            2 => format!("{:b}", (-n) as u64),
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
    };
    body
}
