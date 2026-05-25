//! `String` primitive methods. Mirrors CRuby's `string.c` —
//! the per-method match arms that don't need heap allocation
//! (concat / sub / gsub / tr already produce String results via
//! `Value::new_str`, which wraps a fresh Rc<RStr>; nothing here
//! reaches into the GC heap directly).
//!
//! Called from `primitive_call` (vm.rs) after numeric dispatch.
//! Stateless — no Vm access, just receiver + args + the
//! resource cap.

use crate::error::RubyError;
use crate::value::Value;

/// Try the Str primitive arms. Returns `Ok(Some(v))` on a
/// handled call, `Ok(None)` if the receiver/method shape
/// doesn't match.
pub(crate) fn string_call(
    recv: &Value,
    name: &str,
    args: &[Value],
    max_value_bytes: Option<usize>,
) -> Result<Option<Value>, RubyError> {
    // Helper: enforce the per-value byte cap at every
    // string-growing arm. Returns Err if the projected size
    // would exceed the cap; callers wrap it in `Trap`.
    let check = |new_len: usize| -> Result<(), RubyError> {
        if let Some(max) = max_value_bytes {
            if new_len > max {
                return Err(RubyError::ResourceExhausted {
                    msg: format!("value size {new_len} bytes > cap {max}"),
                });
            }
        }
        Ok(())
    };
    Ok(match (recv, name, args) {
        (Value::Str(a), "+", [Value::Str(b)]) => {
            check(a.borrow().len().saturating_add(b.borrow().len()))?;
            let mut s = a.borrow().clone();
            s.push_str(&b.borrow());
            Some(Value::new_str(s))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() == *b.borrow())),
        (Value::Str(a), "!=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() != *b.borrow())),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        (Value::Str(a), "length", []) | (Value::Str(a), "size", []) => Some(Value::Int(a.borrow().chars().count() as i64)),
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.borrow().is_empty())),
        (Value::Str(a), "upcase", []) => Some(Value::new_str(a.borrow().to_uppercase())),
        (Value::Str(a), "downcase", []) => Some(Value::new_str(a.borrow().to_lowercase())),
        (Value::Str(a), "reverse", []) => Some(Value::new_str(a.borrow().chars().rev().collect::<String>())),
        (Value::Str(a), "strip", []) => Some(Value::new_str(a.borrow().trim().to_string())),
        (Value::Str(a), "lstrip", []) => Some(Value::new_str(a.borrow().trim_start().to_string())),
        (Value::Str(a), "rstrip", []) => Some(Value::new_str(a.borrow().trim_end().to_string())),
        (Value::Str(a), "include?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().contains(&*b.borrow()))),
        // Literal-substring `match?` — true iff the receiver
        // contains the argument as a substring. CRuby additionally
        // accepts a Regexp here; we only handle String, in line
        // with the rest of our regex-free subset. Calls with a
        // non-String argument fall through to NoMethodError.
        (Value::Str(a), "match?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().contains(&*b.borrow()))),
        // String#match? with a Regex — proper regex match. Returns
        // bool without populating any match-data side state.
        (Value::Str(a), "match?", [Value::Regex(re)]) => {
            Some(Value::Bool(re.is_match(&a.borrow())))
        }
        // `index(substr)` / `rindex(substr)` — return the byte
        // offset where the substring first / last appears, or
        // nil if it's absent. CRuby reports a *character* index
        // for non-ASCII receivers; we report `String::find`'s
        // byte index, which matches for ASCII (the common case
        // for our test fixtures) and diverges for multibyte —
        // documented in SUBSET.md.
        (Value::Str(a), "index", [Value::Str(b)]) => {
            Some(match a.borrow().find(&*b.borrow()) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })
        }
        (Value::Str(a), "rindex", [Value::Str(b)]) => {
            Some(match a.borrow().rfind(&*b.borrow()) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })
        }
        // Literal-substring sub/gsub. Regex forms (`gsub(/pat/, ...)`)
        // are out of scope until we add a regex engine — documented
        // in SUBSET.md. CRuby's `gsub("", "x")` on a non-empty
        // string inserts at every character boundary; we replicate
        // that via `Rust`'s `str::replace` for non-empty patterns
        // and a hand-rolled walk for the empty-pattern case.
        (Value::Str(a), "sub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let pat_ref = pat.borrow();
            let repl_ref = repl.borrow();
            let out = if pat_ref.is_empty() {
                // CRuby: sub("", repl) inserts `repl` at index 0.
                let mut s = repl_ref.clone();
                s.push_str(&a_ref);
                s
            } else if let Some(idx) = a_ref.find(&*pat_ref) {
                let mut s = String::with_capacity(a_ref.len() + repl_ref.len());
                s.push_str(&a_ref[..idx]);
                s.push_str(&repl_ref);
                s.push_str(&a_ref[idx + pat_ref.len()..]);
                s
            } else {
                a_ref.clone()
            };
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "gsub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.borrow();
            let pat_ref = pat.borrow();
            let repl_ref = repl.borrow();
            let out = if pat_ref.is_empty() {
                // CRuby: gsub("", repl) wraps `repl` around every
                // character — `"abc".gsub("", "X") == "XaXbXcX"`.
                let mut s = repl_ref.clone();
                for c in a_ref.chars() {
                    s.push(c);
                    s.push_str(&repl_ref);
                }
                s
            } else {
                a_ref.replace(&*pat_ref, &repl_ref)
            };
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // String#tr — character-by-character translation. Each
        // char in `from` maps to the same-index char in `to`; if
        // `to` is shorter, characters past its length map to its
        // LAST char (CRuby's "stretch" behaviour). If `to` is
        // empty, those chars are deleted. Character-range syntax
        // (`"a-z"`) is intentionally NOT expanded — flagged in
        // SUBSET.md.
        (Value::Str(a), "tr", [Value::Str(from), Value::Str(to)]) => {
            let a_ref = a.borrow();
            let from_ref = from.borrow();
            let to_ref = to.borrow();
            let from_chars: Vec<char> = from_ref.chars().collect();
            let to_chars: Vec<char> = to_ref.chars().collect();
            let mut out = String::with_capacity(a_ref.len());
            for ch in a_ref.chars() {
                if let Some(idx) = from_chars.iter().position(|c| *c == ch) {
                    if to_chars.is_empty() {
                        // Delete: skip this character entirely.
                    } else if idx < to_chars.len() {
                        out.push(to_chars[idx]);
                    } else {
                        out.push(*to_chars.last().unwrap());
                    }
                } else {
                    out.push(ch);
                }
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "start_with?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().starts_with(&*b.borrow()))),
        (Value::Str(a), "end_with?", [Value::Str(b)]) => Some(Value::Bool(a.borrow().ends_with(&*b.borrow()))),
        (Value::Str(a), "to_i", []) => {
            // CRuby's `String#to_i` is famously lenient: leading
            // whitespace, optional sign, then as many digits as it
            // can read; non-numeric tail (or empty input) gives 0.
            let a_ref = a.borrow();
            let s = a_ref.trim_start();
            let (sign, rest) = match s.as_bytes().first() {
                Some(b'-') => (-1i64, &s[1..]),
                Some(b'+') => (1i64, &s[1..]),
                _ => (1i64, s),
            };
            let mut n: i64 = 0;
            let mut saw_digit = false;
            for c in rest.chars() {
                if let Some(d) = c.to_digit(10) {
                    saw_digit = true;
                    n = n.wrapping_mul(10).wrapping_add(d as i64);
                } else { break; }
            }
            Some(Value::Int(if saw_digit { sign.wrapping_mul(n) } else { 0 }))
        }
        (Value::Str(a), "to_f", []) => {
            // CRuby's leniency: trim leading whitespace, parse what
            // we can, return 0.0 for "garbage". Rust's stdlib
            // `f64::from_str` is stricter (rejects trailing junk),
            // so we scan a Ruby-shaped prefix ourselves.
            let a_ref = a.borrow();
            let s = a_ref.trim_start();
            let bytes = s.as_bytes();
            let mut end = 0usize;
            if bytes.first() == Some(&b'-') || bytes.first() == Some(&b'+') {
                end += 1;
            }
            let mut saw_digit = false;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                saw_digit = true;
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'.' {
                end += 1;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    saw_digit = true;
                    end += 1;
                }
            }
            // Optional exponent
            if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
                let mut e = end + 1;
                if e < bytes.len() && (bytes[e] == b'+' || bytes[e] == b'-') { e += 1; }
                let exp_start = e;
                while e < bytes.len() && bytes[e].is_ascii_digit() { e += 1; }
                if e > exp_start { end = e; }
            }
            let parsed = if saw_digit {
                s[..end].parse::<f64>().unwrap_or(0.0)
            } else { 0.0 };
            Some(Value::Float(parsed))
        }
        (Value::Str(a), "*", [Value::Int(n)]) => {
            let n = (*n).max(0) as usize;
            check(a.borrow().len().saturating_mul(n))?;
            Some(Value::new_str(a.borrow().repeat(n)))
        }
        (Value::Str(a), "<", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() < *b.borrow())),
        (Value::Str(a), "<=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() <= *b.borrow())),
        (Value::Str(a), ">", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() > *b.borrow())),
        (Value::Str(a), "<=>", [Value::Str(b)]) => Some(Value::Int(a.borrow().cmp(&*b.borrow()) as i64)),
        (Value::Str(a), ">=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() >= *b.borrow())),
        // Regex#match? mirror — same semantics either side.
        (Value::Regex(re), "match?", [Value::Str(s)]) => {
            Some(Value::Bool(re.is_match(&s.borrow())))
        }
        // Regex#source — the raw pattern string.
        (Value::Regex(re), "source", []) => Some(Value::new_str(re.as_str().to_string())),
        (Value::Regex(re), "to_s", []) => Some(Value::new_str(format!("(?-mix:{})", re.as_str()))),
        (Value::Regex(re), "inspect", []) => Some(Value::new_str(format!("/{}/", re.as_str()))),
        // String#inspect — wrap in double quotes, escape `\`,
        // `"`, and common control characters. Matches CRuby for
        // printable ASCII + the standard escape set; exotic
        // Unicode escapes (`\u{...}`) are out of scope.
        (Value::Str(s), "inspect", []) => {
            let raw = s.borrow();
            let mut out = String::with_capacity(raw.len() + 2);
            out.push('"');
            for c in raw.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"'  => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\0' => out.push_str("\\0"),
                    _ => out.push(c),
                }
            }
            out.push('"');
            Some(Value::new_str(out))
        }
        _ => None,
    })
}
