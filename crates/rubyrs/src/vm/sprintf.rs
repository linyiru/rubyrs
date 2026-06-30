//! Printf-style formatter for `String#%`. Standalone — depends
//! only on `Value`, `Heap`, `Interner`, and `RubyError`, no `Vm`
//! state. Pulled out of `vm.rs` (which was past 6500 lines) as
//! the first structural-refactor step; the implementation hasn't
//! changed.

use crate::error::RubyError;
use crate::heap::{Heap, HeapObj};
use crate::intern::Interner;
use crate::value::Value;

/// Resolve a `%<name>…` / `%{name}` reference against the single hash
/// argument (CRuby: named directives take their value from a sole Hash
/// arg keyed by Symbol or String). A missing key raises `KeyError`
/// (`key<name> not found` / `key{name} not found`); a non-Hash sole
/// arg raises ArgumentError "one hash required". Returns a clone so the
/// caller isn't tied to the heap borrow.
fn lookup_named_value(
    name: &str,
    args: &[Value],
    heap: &Heap,
    interner: &Interner,
    brace: bool,
) -> Result<Value, RubyError> {
    let id = match args.first() {
        Some(Value::Hash(id)) => *id,
        _ => {
            return Err(RubyError::ArgumentError {
                msg: "one hash required".into(),
            })
        }
    };
    let HeapObj::Hash(h) = heap.get(id) else {
        return Err(RubyError::ArgumentError {
            msg: "one hash required".into(),
        });
    };
    for (k, v) in h.pairs.iter() {
        let hit = match k {
            Value::Sym(sid) => interner.resolve(*sid).as_ref() == name,
            Value::Str(s) => s.to_string_lossy() == name,
            _ => false,
        };
        if hit {
            return Ok(v.clone());
        }
    }
    let msg = if brace {
        format!("key{{{name}}} not found")
    } else {
        format!("key<{name}> not found")
    };
    Err(RubyError::KeyError { msg })
}

/// Minimal printf-style formatter used by `String#%`. Supports the
/// flag set [- + 0 space #], optional width and precision (decimal
/// integers or `*` for argument-driven values — a negative `*` width
/// left-justifies, a negative `*` precision is ignored), and
/// conversion specifiers d/i, f, s, x, X, o, b, B,
/// c, p, plus the literal `%%`. Also supports positional (`%1$d`) and
/// named references — `%<name>s` (full flags/width/precision/conv on the
/// named hash value) and the self-contained `%{name}` (the value's
/// `to_s`). Named directives read the sole hash argument; a missing key
/// raises KeyError. An unknown conversion char raises ArgumentError.
pub(crate) fn ruby_sprintf(
    fmt: &str,
    args: &[Value],
    heap: &Heap,
    interner: &Interner,
    max_value_bytes: Option<usize>,
    // Per-arg pre-rendered `%p` forms (parallel to `args`; None =
    // use the stateless to_inspect). The engine can't dispatch a
    // user/singleton `inspect`, so `sprintf_prepare_args` renders
    // those through Vm::inspect_value up front (minitest's
    // `"%p" % [act]` where act embeds a singleton-inspect object).
    inspect_overrides: &[Option<String>],
) -> Result<String, RubyError> {
    let mut out = String::new();
    let mut idx: usize = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // `%{name}` — a self-contained directive: substitute the named
        // hash value rendered as a string (CRuby's `to_s`), no flags /
        // width / conversion. `"%{x}" % {x: 1}` → "1".
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                name.push(ch);
            }
            let v = lookup_named_value(&name, args, heap, interner, true)?;
            out.push_str(&v.to_display(heap, interner));
            continue;
        }
        // `%<name>…` — a reference whose value comes from the sole hash
        // arg; the rest of the directive (flags/width/precision/conv) is
        // parsed normally below. The name may also appear AFTER the
        // flags/width/precision (e.g. `%06.2<f>f`); that second position
        // is handled just before the conversion char is read.
        let mut named_key: Option<String> = None;
        if chars.peek() == Some(&'<') {
            chars.next();
            let mut name = String::new();
            for ch in chars.by_ref() {
                if ch == '>' {
                    break;
                }
                name.push(ch);
            }
            named_key = Some(name);
        }
        // Positional argument reference: `%N$…` makes this spec use the
        // N-th (1-based) argument instead of the next sequential one —
        // `"%2$s %1$s" % ["a","b"]` → "b a". Distinguished from a width
        // (`%5d`) by the trailing `$`; peek via a cloned iterator so a
        // plain width isn't consumed when there's no `$`.
        let mut explicit_idx: Option<usize> = None;
        {
            let mut look = chars.clone();
            let mut digits = 0usize;
            let mut val: usize = 0;
            while let Some(&d) = look.peek() {
                if d.is_ascii_digit() {
                    val = val * 10 + (d as usize - '0' as usize);
                    digits += 1;
                    look.next();
                } else {
                    break;
                }
            }
            if digits > 0 && look.peek() == Some(&'$') {
                // Commit: advance the real iterator past the digits + `$`.
                for _ in 0..=digits {
                    chars.next();
                }
                if val == 0 {
                    return Err(RubyError::ArgumentError {
                        msg: "invalid absolute reference - 0$".into(),
                    });
                }
                explicit_idx = Some(val - 1);
            }
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
        if chars.peek() == Some(&'*') {
            // `%*d` — argument-driven width: the NEXT arg (before the
            // value) is the width; a negative width left-justifies
            // (CRuby), folding into the `-` flag.
            chars.next();
            let w_arg = args.get(idx).ok_or_else(|| RubyError::ArgumentError {
                msg: "too few arguments".into(),
            })?;
            idx += 1;
            let w = match w_arg {
                Value::Int(n) => *n,
                other => {
                    return Err(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into Integer",
                            other.type_name()
                        ),
                    });
                }
            };
            if w < 0 {
                flag_minus = true;
                width = Some(w.unsigned_abs() as usize);
            } else {
                width = Some(w as usize);
            }
        } else {
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    width = Some(width.unwrap_or(0) * 10 + (d as usize - '0' as usize));
                    chars.next();
                } else { break; }
            }
        }
        let mut precision: Option<usize> = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            if chars.peek() == Some(&'*') {
                // `%.*f` — argument-driven precision; a negative value
                // means "no precision" (CRuby).
                chars.next();
                let p_arg = args.get(idx).ok_or_else(|| RubyError::ArgumentError {
                    msg: "too few arguments".into(),
                })?;
                idx += 1;
                match p_arg {
                    Value::Int(n) if *n >= 0 => precision = Some(*n as usize),
                    Value::Int(_) => {} // negative → unset
                    other => {
                        return Err(RubyError::TypeError {
                            msg: format!(
                                "no implicit conversion of {} into Integer",
                                other.type_name()
                            ),
                        });
                    }
                }
            } else {
                let mut p: usize = 0;
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        p = p * 10 + (d as usize - '0' as usize);
                        chars.next();
                    } else { break; }
                }
                precision = Some(p);
            }
        }
        // Second valid position for a `%<name>` reference: after the
        // flags/width/precision, immediately before the conversion char
        // (e.g. `%06.2<f>f`). Only honoured if one wasn't already seen
        // right after the `%`.
        if named_key.is_none() && chars.peek() == Some(&'<') {
            chars.next();
            let mut name = String::new();
            for ch in chars.by_ref() {
                if ch == '>' {
                    break;
                }
                name.push(ch);
            }
            named_key = Some(name);
        }
        let spec = chars.next().ok_or_else(|| RubyError::ArgumentError {
            msg: "malformed format string - %".into(),
        })?;
        if spec == '%' {
            out.push('%');
            continue;
        }
        // Argument selection. A `%<name>` reference pulls from the sole
        // hash arg (does not advance the sequential cursor). Otherwise an
        // explicit `%N$` reference selects args[N-1] (also no advance),
        // and a plain directive consumes the next sequential argument.
        let named_value: Option<Value> = match &named_key {
            Some(name) => Some(lookup_named_value(name, args, heap, interner, false)?),
            None => None,
        };
        // `arg_idx` only matters for `%p`'s pre-rendered inspect override
        // (positional); a named arg has none, so point past the slice.
        let arg_idx = if named_value.is_some() {
            usize::MAX
        } else {
            explicit_idx.unwrap_or(idx)
        };
        let arg: &Value = match &named_value {
            Some(v) => v,
            None => {
                let a = args.get(arg_idx).ok_or_else(|| RubyError::ArgumentError {
                    msg: "too few arguments".into(),
                })?;
                if explicit_idx.is_none() {
                    idx += 1;
                }
                a
            }
        };
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
            'e' | 'E' => {
                let f = coerce_float(arg)?;
                let prec = precision.unwrap_or(6);
                let mut body = fmt_scientific(f, prec, spec == 'E');
                if !body.starts_with('-') {
                    if flag_plus { body.insert(0, '+'); }
                    else if flag_space { body.insert(0, ' '); }
                }
                body
            }
            'g' | 'G' => {
                let f = coerce_float(arg)?;
                // %g precision is significant digits; default 6, 0 → 1.
                let prec = precision.unwrap_or(6);
                let mut body = fmt_general(f, prec, spec == 'G');
                if !body.starts_with('-') {
                    if flag_plus { body.insert(0, '+'); }
                    else if flag_space { body.insert(0, ' '); }
                }
                body
            }
            'a' | 'A' => {
                let f = coerce_float(arg)?;
                let mut body = fmt_hex_float(f, precision, spec == 'A');
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
            'p' => match inspect_overrides.get(arg_idx).and_then(|o| o.as_ref()) {
                Some(pre) => pre.clone(),
                None => arg.to_inspect(heap, interner),
            },
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
                // The `0` flag zero-pads to width. For FLOAT conversions
                // it applies even with a precision (`%05.2f` of 3.14 is
                // `03.14`). For INTEGER conversions a precision overrides
                // it — CRuby ignores `0` then (`%05.2d` of 1 is `   01`),
                // because the precision already controls the digit count.
                // The sign / alt-prefix placement below keeps the zeros
                // inside the sign (`%+08.2f` → `+0003.14`).
                let pad_char = if flag_zero && !flag_minus
                    && (matches!(spec, 'f' | 'e' | 'E' | 'g' | 'G' | 'a' | 'A')
                        || (precision.is_none()
                            && matches!(spec, 'd' | 'i' | 'x' | 'X' | 'o' | 'b' | 'B'))) {
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
        // CRuby raises `FloatDomainError: NaN/Infinity` for
        // `sprintf("%d", Float::NAN)` etc. — matches the
        // `Float#to_i` / `Kernel#Integer` traps wired in this
        // PR so the helper's "every Float→Integer trap site"
        // docstring actually holds. Finite Float still casts
        // via `as i64` (CRuby's `%d` truncates toward zero
        // for finite operands).
        Value::Float(f) if f.is_nan() || f.is_infinite() => {
            Err(RubyError::FloatDomainError {
                msg: crate::vm::numeric::float_domain_label(*f).to_string(),
            })
        }
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

/// `%e` / `%E` — C-style scientific notation. Rust's `{:e}` yields
/// `1.23e4`; reformat the exponent to a sign + at least two digits
/// (`1.23e+04`), and uppercase the `e` for `%E`. Non-finite values
/// (`Inf`/`NaN`) pass through Rust's text.
/// `%a` / `%A` — C99 hexadecimal floating-point. Renders the IEEE-754
/// value as `[-]0x1.<hexfrac>p<±exp>` (normalised: a leading `1` and a
/// binary exponent), or `0x0p+0` for zero. `prec` (the `%.Na` form)
/// fixes the fraction to N hex digits with round-half-to-even; the
/// default is the shortest exact fraction (trailing zeros stripped).
/// `%A` upper-cases the whole rendering. Non-finite values render as
/// `Inf` / `-Inf` / `NaN` (CRuby spelling), case-insensitive.
fn fmt_hex_float(f: f64, prec: Option<usize>, upper: bool) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-Inf".to_string() } else { "Inf".to_string() };
    }
    let bits = f.to_bits();
    let sign = if bits >> 63 == 1 { "-" } else { "" };
    let exp_field = ((bits >> 52) & 0x7ff) as i64;
    let mantissa = bits & 0x000f_ffff_ffff_ffff; // low 52 bits

    let body = if exp_field == 0 && mantissa == 0 {
        // ±0.0
        match prec {
            Some(p) if p > 0 => format!("0x0.{}p+0", "0".repeat(p)),
            _ => "0x0p+0".to_string(),
        }
    } else {
        // (lead, frac52, exp): the leading hex digit is always 1 for a
        // normalised value; a subnormal is normalised by shifting its
        // mantissa up to a leading 1 and lowering the exponent.
        let (mut frac52, mut exp): (u64, i64) = if exp_field == 0 {
            // Subnormal: value = mantissa * 2^-1074. Shift the top set
            // bit into the implicit-1 position (bit 52).
            let shift = 52 - (63 - mantissa.leading_zeros()) as i64; // >0
            (
                (mantissa << shift) & 0x000f_ffff_ffff_ffff,
                -1022 - shift,
            )
        } else {
            (mantissa, exp_field - 1023)
        };
        // The 52-bit fraction renders as 13 hex nibbles (MSB first).
        let frac_hex = match prec {
            None => {
                let full = format!("{frac52:013x}");
                let trimmed = full.trim_end_matches('0');
                trimmed.to_string()
            }
            Some(p) if p >= 13 => format!("{frac52:013x}{}", "0".repeat(p - 13)),
            Some(p) => {
                // Round the 52-bit fraction to 4*p bits, half-to-even.
                let drop = 52 - 4 * p;
                let keep = frac52 >> drop;
                let rem = frac52 & ((1u64 << drop) - 1);
                let half = 1u64 << (drop - 1);
                let round_up = rem > half || (rem == half && keep & 1 == 1);
                let mut k = keep + u64::from(round_up);
                if k >> (4 * p) != 0 {
                    // Carry past the leading 1 → 1.fff… rounds to 2.0,
                    // i.e. 1.0 × 2^(exp+1) with a zero fraction.
                    k = 0;
                    exp += 1;
                    frac52 = 0;
                }
                let _ = frac52;
                format!("{k:0width$x}", width = p)
            }
        };
        let exp_sign = if exp < 0 { '-' } else { '+' };
        if frac_hex.is_empty() {
            format!("0x1p{exp_sign}{}", exp.abs())
        } else {
            format!("0x1.{frac_hex}p{exp_sign}{}", exp.abs())
        }
    };

    let out = format!("{sign}{body}");
    if upper { out.to_uppercase() } else { out }
}

fn fmt_scientific(f: f64, prec: usize, upper: bool) -> String {
    if !f.is_finite() {
        let s = format!("{f}");
        return if upper { s.to_uppercase() } else { s };
    }
    let s = format!("{f:.prec$e}");
    let Some(epos) = s.find('e') else { return s };
    let mantissa = &s[..epos];
    let exp: i32 = s[epos + 1..].parse().unwrap_or(0);
    let e_char = if upper { 'E' } else { 'e' };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mantissa}{e_char}{sign}{:02}", exp.abs())
}

/// `%g` / `%G` — pick `%e` or `%f` by magnitude (`%e` when the decimal
/// exponent is < -4 or >= the significant-digit precision), then strip
/// trailing zeros (and a bare `.`). Precision is significant digits
/// (default 6; 0 treated as 1), per C/Ruby.
fn fmt_general(f: f64, prec: usize, upper: bool) -> String {
    if !f.is_finite() {
        let s = format!("{f}");
        return if upper { s.to_uppercase() } else { s };
    }
    let p = prec.max(1);
    if f == 0.0 {
        return "0".to_string();
    }
    // True decimal exponent via Rust's scientific formatting.
    let sci = format!("{f:e}");
    let exp: i32 = sci.find('e').map(|i| sci[i + 1..].parse().unwrap_or(0)).unwrap_or(0);
    if exp >= -4 && exp < p as i32 {
        // %f form with precision (p - 1 - exp); strip trailing zeros.
        let fprec = (p as i32 - 1 - exp).max(0) as usize;
        let s = format!("{f:.fprec$}");
        strip_float_zeros(&s)
    } else {
        // %e form with precision (p - 1); strip mantissa trailing zeros.
        let raw = fmt_scientific(f, p - 1, upper);
        let e_char = if upper { 'E' } else { 'e' };
        match raw.find(e_char) {
            Some(epos) => {
                let mantissa = strip_float_zeros(&raw[..epos]);
                format!("{mantissa}{}", &raw[epos..])
            }
            None => raw,
        }
    }
}

/// Drop trailing zeros after a decimal point, and a bare trailing `.`.
/// `"1.230" → "1.23"`, `"100.000" → "100"`, `"12" → "12"`.
fn strip_float_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('0');
    trimmed.trim_end_matches('.').to_string()
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
        //
        // Sign + prefix are prepended in-place via `String::insert*`
        // for the same peak-memory reason: `format!("{sign}{prefix}{mag}")`
        // would allocate a second ~`est`-byte String while `mag`
        // is still live, doubling the resident footprint past the
        // cap we just validated. Insertion at offset 0 IS O(n)
        // (memmove of `mag`), but stays within the single
        // ~`est`-byte allocation.
        let mut mag = b.magnitude().to_str_radix(radix);
        if upper { mag.make_ascii_uppercase(); }
        if !prefix.is_empty() { mag.insert_str(0, prefix); }
        if b.sign() == Sign::Minus { mag.insert(0, '-'); }
        return Ok(mag);
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
