//! `String` primitive methods. Mirrors CRuby's `string.c` —
//! the per-method match arms that don't need heap allocation
//! (concat / sub / gsub / tr already produce String results via
//! `Value::new_str`, which wraps a fresh Rc<RStr>; nothing here
//! reaches into the GC heap directly).
//!
//! Called from `primitive_call` (vm.rs) after numeric dispatch.
//! Stateless — no Vm access, just receiver + args + the
//! resource cap.

#[cfg(feature = "regex")]
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
#[cfg(feature = "regex")]
use crate::value::Instance;
use crate::value::{RStr, Value};

use super::{ruby_sprintf, Vm};
// PinGuard is only referenced from the `("scan", [Value::Regex(...)])`
// arm, which is itself `cfg(feature = "regex")` via `Value::Regex`.
// Without this gate, `--no-default-features` (the wasm32-wasip1
// shape) sees an unused import and `-D warnings` blocks the build.
#[cfg(feature = "regex")]
use super::PinGuard;

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
        if let Some(max) = max_value_bytes
            && new_len > max {
                return Err(RubyError::ResourceExhausted {
                    msg: format!("value size {new_len} bytes > cap {max}"),
                });
            }
        Ok(())
    };
    Ok(match (recv, name, args) {
        (Value::Str(a), "+", [Value::Str(b)]) => {
            check(a.borrow().len().saturating_add(b.borrow().len()))?;
            let mut s = a.borrow().clone();
            s.extend_from_slice(&b.borrow());
            Some(Value::new_str_bytes(s))
        }
        (Value::Str(a), "==", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() == *b.borrow())),
        (Value::Str(a), "!=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() != *b.borrow())),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        // PR #53 review #1: `length`/`size` return UTF-8 character
        // count (lossy on invalid UTF-8 — non-UTF-8 bytes count as
        // one U+FFFD char each). Matches CRuby's "length on a
        // UTF-8-encoded String" behavior. For raw byte count, use
        // `bytesize` (added below); for binary protocol gems the
        // bytesize semantic is the meaningful one.
        (Value::Str(a), "length", []) | (Value::Str(a), "size", []) => {
            Some(Value::Int(a.with_str_lossy(|s| s.chars().count()) as i64))
        }
        (Value::Str(a), "bytesize", []) => Some(Value::Int(a.borrow().len() as i64)),
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.borrow().is_empty())),
        (Value::Str(a), "upcase", []) => Some(Value::new_str(a.to_string_lossy().to_uppercase())),
        (Value::Str(a), "downcase", []) => Some(Value::new_str(a.to_string_lossy().to_lowercase())),
        (Value::Str(a), "reverse", []) => Some(Value::new_str(a.to_string_lossy().chars().rev().collect::<String>())),
        // `String#succ` / `#next` — Ruby's "alphanumeric successor".
        // We support the common single-letter case (`'a'.succ == 'b'`,
        // `'Z'.succ == 'AA'`) plus the general "rightmost alnum
        // rolls over with carry" rule via `str_succ`. The pure-
        // digit / non-alnum and bracketed-string edge cases are
        // documented gaps; CRuby diff fixtures pin the supported
        // shape.
        (Value::Str(a), "succ", []) | (Value::Str(a), "next", []) => {
            Some(Value::new_str(a.with_str_lossy(str_succ)))
        }
        // `center` / `ljust` / `rjust` — pad to `width` with the
        // optional pad-string (default " "). The pad cycles when
        // multichar. If `width` is ≤ receiver length, the receiver
        // is returned unchanged. Empty pad raises ArgumentError
        // (caught by the early arg-shape guard via `pad_len == 0`).
        // CRuby: when `center` produces odd-total padding, the
        // extra char goes on the RIGHT.
        (Value::Str(a), "center" | "ljust" | "rjust", pad_args)
            if matches!(pad_args.first(), Some(Value::Int(_)))
                && (pad_args.len() == 1
                    || (pad_args.len() == 2 && matches!(pad_args[1], Value::Str(_)))) => {
            let width = match &pad_args[0] {
                Value::Int(w) => *w,
                _ => unreachable!(),
            };
            let pad: String = match pad_args.get(1) {
                None => " ".to_string(),
                Some(Value::Str(s)) => s.to_string_lossy(),
                _ => unreachable!(),
            };
            if pad.is_empty() {
                return Err(RubyError::ArgumentError {
                    msg: "zero width padding".into(),
                });
            }
            let a_str = a.to_string_lossy();
            let recv_chars: Vec<char> = a_str.chars().collect();
            let recv_len = recv_chars.len() as i64;
            if width <= recv_len {
                return Ok(Some(Value::Str(a.clone())));
            }
            let pad_chars: Vec<char> = pad.chars().collect();
            let total_pad = (width - recv_len) as usize;
            let take_from_pad = |n: usize| -> String {
                let mut out = String::with_capacity(n);
                for i in 0..n { out.push(pad_chars[i % pad_chars.len()]); }
                out
            };
            let result: String = match name {
                "ljust" => {
                    let mut s = a_str.clone();
                    s.push_str(&take_from_pad(total_pad));
                    s
                }
                "rjust" => {
                    let mut s = take_from_pad(total_pad);
                    s.push_str(&a_str);
                    s
                }
                "center" => {
                    let left = total_pad / 2;
                    let right = total_pad - left;
                    let mut s = take_from_pad(left);
                    s.push_str(&a_str);
                    s.push_str(&take_from_pad(right));
                    s
                }
                _ => unreachable!(),
            };
            check(result.len())?;
            Some(Value::new_str(result))
        }
        // `String#encode(target)` / `#force_encoding(target)` —
        // the subset stores raw bytes with no per-string encoding
        // tag, so both are near-no-ops. `encode` returns the
        // receiver (Rc-shared, no copy) for compatibility with
        // CRuby's "if source encoding == target encoding,
        // re-encode is the identity" rule. `force_encoding`
        // similarly returns the receiver. The argument is
        // accepted as either a String or any value with a `to_s`
        // already on the Value (we just don't validate it
        // against the known encoding list). Documented in
        // SUBSET.md: cross-encoding conversion isn't modelled.
        (Value::Str(a), "encode", [_]) | (Value::Str(a), "force_encoding", [_]) => {
            Some(Value::Str(a.clone()))
        }
        // `String#valid_encoding?` — rubyrs stores raw bytes
        // viewed via `String::from_utf8_lossy` (invalid sequences
        // become U+FFFD), so the effective character stream is
        // always well-formed UTF-8 by construction. Returning
        // true matches that observable behaviour. tilt's
        // template.rb:120 reads this to decide whether to raise
        // `Encoding::InvalidByteSequenceError`; with `true` the
        // raise never fires and template loading proceeds.
        (Value::Str(_), "valid_encoding?", []) => Some(Value::Bool(true)),
        // `String#encoding` — CRuby returns an `Encoding` object;
        // we return the name as a String since there's no per-
        // string encoding tag and no Encoding class in scope.
        // Real codebases commonly use `str.encoding.to_s` for
        // formatting; that works in both. Direct
        // `str.encoding == Encoding::UTF_8` comparisons are NOT
        // supported — even if `Encoding::UTF_8` were added later,
        // the comparison would compare String vs Encoding-object
        // and diverge from CRuby. Sticking to `.to_s` or
        // `.to_s == "UTF-8"` is the portable form.
        (Value::Str(_), "encoding", []) => Some(Value::new_str("UTF-8")),
        (Value::Str(a), "strip", []) => Some(Value::new_str(a.to_string_lossy().trim().to_string())),
        (Value::Str(a), "lstrip", []) => Some(Value::new_str(a.to_string_lossy().trim_start().to_string())),
        (Value::Str(a), "rstrip", []) => Some(Value::new_str(a.to_string_lossy().trim_end().to_string())),
        // PR #53 review #3: use with_str_lossy (Cow-backed) so the
        // valid-UTF-8 hot path is zero-alloc — only the invalid-
        // UTF-8 branch allocates. to_string_lossy() unconditionally
        // owns the String even when from_utf8_lossy returns
        // Cow::Borrowed.
        (Value::Str(a), "include?", [Value::Str(b)]) => {
            Some(Value::Bool(a.with_str_lossy(|sa| b.with_str_lossy(|sb| sa.contains(sb)))))
        }
        // Literal-substring `match?` — true iff the receiver
        // contains the argument as a substring. CRuby additionally
        // accepts a Regexp here; we only handle String, in line
        // with the rest of our regex-free subset. Calls with a
        // non-String argument fall through to NoMethodError.
        (Value::Str(a), "match?", [Value::Str(b)]) => {
            Some(Value::Bool(a.with_str_lossy(|sa| b.with_str_lossy(|sb| sa.contains(sb)))))
        }
        // String#match? with a Regex — proper regex match. Returns
        // bool without populating any match-data side state.
        #[cfg(feature = "regex")]
        (Value::Str(a), "match?", [Value::Regex(re)]) => {
            Some(Value::Bool(a.with_str_lossy(|s| re.is_match(s))))
        }
        // `index(substr)` / `rindex(substr)` — return the byte
        // offset where the substring first / last appears, or
        // nil if it's absent. CRuby reports a *character* index
        // for non-ASCII receivers; we report `String::find`'s
        // byte index, which matches for ASCII (the common case
        // for our test fixtures) and diverges for multibyte —
        // documented in SUBSET.md.
        (Value::Str(a), "index", [Value::Str(b)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| match sa.find(sb) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })))
        }
        (Value::Str(a), "rindex", [Value::Str(b)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| match sa.rfind(sb) {
                Some(i) => Value::Int(i as i64),
                None => Value::Nil,
            })))
        }
        // Literal-substring sub/gsub. Regex forms (`gsub(/pat/, ...)`)
        // are out of scope until we add a regex engine — documented
        // in SUBSET.md. CRuby's `gsub("", "x")` on a non-empty
        // string inserts at every character boundary; we replicate
        // that via `Rust`'s `str::replace` for non-empty patterns
        // and a hand-rolled walk for the empty-pattern case.
        (Value::Str(a), "sub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.to_string_lossy();
            let pat_ref = pat.to_string_lossy();
            let repl_ref = repl.to_string_lossy();
            let out = if pat_ref.is_empty() {
                // CRuby: sub("", repl) inserts `repl` at index 0.
                let mut s = repl_ref.clone();
                s.push_str(&a_ref);
                s
            } else if let Some(idx) = a_ref.find(pat_ref.as_str()) {
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
        // Regex form: `s.sub(/pat/, "repl")`. Replacement string
        // supports Ruby backrefs `\0` / `\1` / ... — translate to
        // the `regex` crate's `$0` / `$1` syntax. `\\` escapes a
        // literal backslash. Block form
        // (`s.sub(/pat/) { |m| ... }`) is the higher-value but
        // separately-dispatched path; not handled here.
        #[cfg(feature = "regex")]
        (Value::Str(a), "sub", [Value::Regex(re), Value::Str(repl)]) => {
            let a_ref = a.to_string_lossy();
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            let out = re.replace(&a_ref, repl_xlated.as_str()).into_owned();
            check(out.len())?;
            Some(Value::new_str(out))
        }
        #[cfg(feature = "regex")]
        (Value::Str(a), "gsub", [Value::Regex(re), Value::Str(repl)]) => {
            let a_ref = a.to_string_lossy();
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            let out = re.replace_all(&a_ref, repl_xlated.as_str()).into_owned();
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "gsub", [Value::Str(pat), Value::Str(repl)]) => {
            let a_ref = a.to_string_lossy();
            let pat_ref = pat.to_string_lossy();
            let repl_ref = repl.to_string_lossy();
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
                a_ref.replace(pat_ref.as_str(), &repl_ref)
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
            let a_ref = a.to_string_lossy();
            let from_ref = from.to_string_lossy();
            let to_ref = to.to_string_lossy();
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
        // `String#squeeze` — collapse consecutive runs of the same
        // character. With a char-set arg, only chars in the set
        // are squeezed. Char-set ranges (`"a-z"`) and ^-negation
        // are NOT expanded here — same conservative semantics as
        // `tr`. Documented in SUBSET.md.
        (Value::Str(a), "squeeze", rest) if rest.is_empty()
            || (rest.len() == 1 && matches!(rest[0], Value::Str(_))) => {
            let a_str = a.to_string_lossy();
            let set: Option<Vec<char>> = match rest.first() {
                None => None,
                Some(Value::Str(s)) => Some(s.to_string_lossy().chars().collect()),
                _ => unreachable!(),
            };
            let mut out = String::with_capacity(a_str.len());
            let mut prev: Option<char> = None;
            for ch in a_str.chars() {
                let in_set = match &set {
                    Some(s) => s.contains(&ch),
                    None => true,
                };
                if in_set && Some(ch) == prev {
                    continue;
                }
                out.push(ch);
                prev = Some(ch);
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        (Value::Str(a), "start_with?", [Value::Str(b)]) => {
            Some(Value::Bool(a.with_str_lossy(|sa| b.with_str_lossy(|sb| sa.starts_with(sb)))))
        }
        (Value::Str(a), "end_with?", [Value::Str(b)]) => {
            Some(Value::Bool(a.with_str_lossy(|sa| b.with_str_lossy(|sb| sa.ends_with(sb)))))
        }
        (Value::Str(a), "to_i", []) => {
            // CRuby's `String#to_i` is famously lenient: leading
            // whitespace, optional sign, then as many digits as it
            // can read; non-numeric tail (or empty input) gives 0.
            let a_ref = a.to_string_lossy();
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
            let a_ref = a.to_string_lossy();
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
            Some(Value::new_str_bytes(a.borrow().repeat(n)))
        }
        (Value::Str(a), "<", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() < *b.borrow())),
        (Value::Str(a), "<=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() <= *b.borrow())),
        (Value::Str(a), ">", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() > *b.borrow())),
        (Value::Str(a), "<=>", [Value::Str(b)]) => Some(Value::Int(a.borrow().cmp(&*b.borrow()) as i64)),
        (Value::Str(a), ">=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() >= *b.borrow())),
        // Regex#match? mirror — same semantics either side.
        #[cfg(feature = "regex")]
        (Value::Regex(re), "match?", [Value::Str(s)]) => {
            Some(Value::Bool(s.with_str_lossy(|s| re.is_match(s))))
        }
        // Regex#source — the raw pattern string.
        #[cfg(feature = "regex")]
        (Value::Regex(re), "source", []) => Some(Value::new_str(re.as_str().to_string())),
        #[cfg(feature = "regex")]
        (Value::Regex(re), "to_s", []) => Some(Value::new_str(format!("(?-mix:{})", re.as_str()))),
        #[cfg(feature = "regex")]
        (Value::Regex(re), "inspect", []) => Some(Value::new_str(format!("/{}/", re.as_str()))),
        // String#inspect — wrap in double quotes, escape `\`,
        // `"`, and common control characters. Matches CRuby for
        // printable ASCII + the standard escape set; exotic
        // Unicode escapes (`\u{...}`) are out of scope.
        (Value::Str(s), "inspect", []) => {
            let raw = s.to_string_lossy();
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

impl Vm {
    /// String methods that need heap access — slice, scan, []=,
    /// %, freeze / frozen? / dup, and all the in-place
    /// mutators. Mirrors the heap-aware half of CRuby's
    /// `string.c`; the rest lives in `string_call` above.
    /// Dispatched from `Vm::collection_call`'s `Value::Str` arm.
    pub(crate) fn string_collection_call(
        &mut self,
        s: Rc<RStr>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        Ok({
                let s = s.clone();
                // In-place mutation methods. All return the
                // receiver (same Rc, so aliases observe the
                // change). The variadic shape (`concat`, `prepend`
                // take *args) doesn't fit the inner-match
                // `[Value::Str(b)]` pattern; we dispatch by name
                // first, then validate the args.
                // freeze / frozen? / dup — the per-string immutability
                // controls. CRuby raises FrozenError on any mutating
                // method against a frozen string; we route that
                // through a Trap so `rescue FrozenError` catches it.
                if name == "frozen?" && args.is_empty() {
                    return Ok(Some(Value::Bool(s.frozen.get())));
                }
                if name == "freeze" && args.is_empty() {
                    s.frozen.set(true);
                    return Ok(Some(Value::Str(s)));
                }
                if name == "dup" && args.is_empty() {
                    // Fresh Rc, fresh RefCell, NOT frozen — `dup`
                    // copies content but resets the frozen bit.
                    let copy = s.content.borrow().clone();
                    return Ok(Some(Value::new_str_bytes(copy)));
                }
                // Helper closure: bail out of any mutating method
                // if `s` was frozen. Used by `<<`, `concat`,
                // `prepend`, `replace`, `[]=`.
                let check_unfrozen = |vm: &Vm| -> Result<(), Trap> {
                    if s.frozen.get() {
                        Err(vm.trap(RubyError::FrozenError {
                            msg: format!("can't modify frozen String: {:?}", s.content.borrow()),
                        }))
                    } else {
                        Ok(())
                    }
                };
                if name == "<<" && args.len() == 1 {
                    check_unfrozen(self)?;
                    match &args[0] {
                        Value::Str(other) => {
                            let to_push = other.borrow().clone();
                            s.borrow_mut().extend_from_slice(&to_push);
                        }
                        // CRuby's String#<< also accepts Integer
                        // (treated as a codepoint). Support it
                        // since Rake / Sinatra builders rely on it
                        // for fast char-by-char concatenation.
                        Value::Int(n) => {
                            if let Some(c) = char::from_u32(*n as u32) {
                                let mut buf = [0u8; 4];
                                let bs = c.encode_utf8(&mut buf);
                                s.borrow_mut().extend_from_slice(bs.as_bytes());
                            } else {
                                return Err(self.trap(RubyError::ArgumentError {
                                    msg: format!("{} out of char range", n),
                                }));
                            }
                        }
                        other => return Err(self.trap(RubyError::TypeError {
                            msg: format!("no implicit conversion of {} into String", other.type_name()),
                        })),
                    }
                    return Ok(Some(Value::Str(s)));
                }
                if name == "concat" {
                    check_unfrozen(self)?;
                    for a in args {
                        match a {
                            Value::Str(o) => {
                                let to_push = o.borrow().clone();
                                s.borrow_mut().extend_from_slice(&to_push);
                            }
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into String", a.type_name()),
                            })),
                        }
                    }
                    return Ok(Some(Value::Str(s)));
                }
                if name == "prepend" {
                    check_unfrozen(self)?;
                    // Concatenate args in order, then prepend to
                    // existing content. CRuby's `prepend("a","b")`
                    // results in `"a" + "b" + self`, not the
                    // reverse — verified against MRI.
                    let mut prefix: Vec<u8> = Vec::new();
                    for a in args {
                        match a {
                            Value::Str(o) => prefix.extend_from_slice(&o.borrow()),
                            _ => return Err(self.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into String", a.type_name()),
                            })),
                        }
                    }
                    let mut buf = prefix;
                    buf.extend_from_slice(&s.borrow());
                    *s.borrow_mut() = buf;
                    return Ok(Some(Value::Str(s)));
                }
                if name == "replace" && args.len() == 1 {
                    check_unfrozen(self)?;
                    match &args[0] {
                        Value::Str(o) => {
                            let new_content = o.borrow().clone();
                            *s.borrow_mut() = new_content;
                        }
                        other => return Err(self.trap(RubyError::TypeError {
                            msg: format!("no implicit conversion of {} into String", other.type_name()),
                        })),
                    }
                    return Ok(Some(Value::Str(s)));
                }
                // String#[] / #slice — char-indexed slicing.
                // CRuby's semantics:
                //   s[i]           -> single-char String, or nil
                //   s[i, n]        -> substring of n chars from i,
                //                     or nil if i out of bounds
                //                     (i == len is OK and gives "")
                //   s[Range]       -> substring; nil for invalid start
                // Negative indices count from the end; out-of-bounds
                // returns nil. Multibyte strings are sliced by char,
                // not by byte.
                fn str_index_char(chars: &[char], i: i64) -> Option<usize> {
                    let len = chars.len() as i64;
                    let idx = if i < 0 { len + i } else { i };
                    if idx < 0 || idx > len { None }
                    else { Some(idx as usize) }
                }
                fn str_slice(chars: &[char], start: usize, n: usize) -> String {
                    chars.iter().skip(start).take(n).collect()
                }
                // String#match(regex) — returns a MatchData
                // instance with @whole = whole match and
                // @caps = numbered captures (Strings, or nil
                // for groups that didn't participate). Returns
                // nil if no match. CRuby additionally accepts
                // a String (interpreted as a literal regex) and
                // a starting offset; both out of scope here.
                #[cfg(feature = "regex")]
                if name == "match" && args.len() == 1 {
                    if let Value::Regex(re) = &args[0] {
                        let bound = s.to_string_lossy();
                        let captures = re.captures(&bound);
                        match captures {
                            None => return Ok(Some(Value::Nil)),
                            Some(caps) => {
                                let whole = caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
                                let mut group_vals: Vec<Value> = Vec::with_capacity(caps.len().saturating_sub(1));
                                for i in 1..caps.len() {
                                    group_vals.push(match caps.get(i) {
                                        Some(m) => Value::new_str(m.as_str().to_string()),
                                        None => Value::Nil,
                                    });
                                }
                                self.maybe_gc();
                                let caps_arr = self.heap.alloc(HeapObj::Array(group_vals));
                                let cls_id = self.interner.intern("MatchData");
                                let cls = match self.classes.get(&cls_id).cloned() {
                                    Some(c) => c,
                                    None => return Ok(Some(Value::Nil)),
                                };
                                let obj_id = self.heap.alloc(HeapObj::Instance(Instance {
                                    class: cls,
                                    ivars: HashMap::new(),
                                    singleton_class: None,
                                }));
                                let whole_ivar = self.interner.intern("@whole");
                                let caps_ivar = self.interner.intern("@caps");
                                {
                                    let inst = self.heap.instance_mut(obj_id);
                                    inst.ivars.insert(whole_ivar, Value::new_str(whole));
                                    inst.ivars.insert(caps_ivar, Value::Array(caps_arr));
                                }
                                return Ok(Some(Value::Object(obj_id)));
                            }
                        }
                    }
                    return Ok(None);
                }
                if (name == "[]" || name == "slice") && args.len() == 1 {
                    let chars: Vec<char> = s.to_string_lossy().chars().collect();
                    let len = chars.len() as i64;
                    return Ok(Some(match &args[0] {
                        Value::Int(i) => {
                            let idx = if *i < 0 { len + *i } else { *i };
                            if idx < 0 || idx >= len {
                                Value::Nil
                            } else {
                                let ch = chars[idx as usize].to_string();
                                Value::new_str(ch)
                            }
                        }
                        Value::Range(rid) => {
                            // Endless / beginless: a Nil endpoint
                            // means "from index 0" or "to len". So
                            // (`s[6..]` / `s[..5]` / `s[..]` all
                            // resolve via this branch.
                            let r = self.heap.range(*rid);
                            let excl = r.exclusive;
                            let bi: i64 = match &r.begin {
                                Value::Int(a) => *a,
                                Value::Nil => 0,
                                _ => return Ok(None),
                            };
                            let ei: i64 = match &r.end {
                                Value::Int(c) => *c,
                                Value::Nil => len, // exclusive of len-1 below
                                _ => return Ok(None),
                            };
                            let endless_end = matches!(&r.end, Value::Nil);
                            let start = match str_index_char(&chars, bi) {
                                Some(s) => s,
                                None => return Ok(Some(Value::Nil)),
                            };
                            // End index: positive raw; negative
                            // relative to len. Out-of-range high
                            // clamps to len; exclusive drops one.
                            // Nil end is always "to len" (no
                            // exclusive adjustment).
                            let mut end = if endless_end { len } else if ei < 0 { len + ei } else { ei };
                            if !excl && !endless_end { end += 1; }
                            let end = end.clamp(start as i64, len) as usize;
                            let slice: String = str_slice(&chars, start, end.saturating_sub(start));
                            Value::new_str(slice)
                        }
                        _ => return Ok(None),
                    }));
                }
                if (name == "[]" || name == "slice") && args.len() == 2 {
                    if let (Value::Int(i), Value::Int(n)) = (&args[0], &args[1]) {
                        let chars: Vec<char> = s.to_string_lossy().chars().collect();
                        let len = chars.len() as i64;
                        let start_raw = if *i < 0 { len + *i } else { *i };
                        if start_raw < 0 || start_raw > len || *n < 0 {
                            return Ok(Some(Value::Nil));
                        }
                        let start = start_raw as usize;
                        let n = (*n as usize).min(chars.len() - start);
                        let slice = str_slice(&chars, start, n);
                        return Ok(Some(Value::new_str(slice)));
                    }
                    return Ok(None);
                }
                // String#[]= — in-place mutation. Three shapes:
                //   s[i]      = x   → replace one char at char-index i
                //   s[i, n]   = x   → replace n chars from char-index i
                //   s[range]  = x   → replace the slice covered by the range
                // Negative indices count from the end. Out-of-range
                // raises IndexError, matching CRuby (we surface that
                // through the Trap-to-rescue path).
                //
                // The mutation works because Value::Str holds an
                // Rc<RStr> whose `content` is RefCell<Vec<u8>>:
                // every clone of this Value shares the same
                // RefCell, so writes through `borrow_mut` are
                // visible to all aliases. Char-indexed `[]=` goes
                // through `to_string_lossy → mutate → into_bytes`,
                // which scrubs a previously-binary String to
                // lossy UTF-8 (documented tradeoff — CRuby's
                // char-index semantics aren't defined for binary
                // content; use setbyte for byte-level writes).
                if name == "[]=" && args.len() == 2 {
                    check_unfrozen(self)?;
                    if let (Value::Int(i), Value::Str(repl)) = (&args[0], &args[1]) {
                        let chars: Vec<char> = s.to_string_lossy().chars().collect();
                        let len = chars.len() as i64;
                        let idx = if *i < 0 { len + *i } else { *i };
                        if idx < 0 || idx >= len {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {i} out of string"),
                            }));
                        }
                        let mut buf: String = chars[..idx as usize].iter().collect();
                        buf.push_str(&repl.to_string_lossy());
                        buf.extend(chars[idx as usize + 1..].iter());
                        *s.borrow_mut() = buf.into_bytes();
                        return Ok(Some(args[1].clone()));
                    }
                    return Ok(None);
                }
                if name == "[]=" && args.len() == 3 {
                    check_unfrozen(self)?;
                    if let (Value::Int(i), Value::Int(n), Value::Str(repl)) = (&args[0], &args[1], &args[2]) {
                        let chars: Vec<char> = s.to_string_lossy().chars().collect();
                        let len = chars.len() as i64;
                        let start_raw = if *i < 0 { len + *i } else { *i };
                        if start_raw < 0 || start_raw > len || *n < 0 {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {i} out of string"),
                            }));
                        }
                        let start = start_raw as usize;
                        let take = (*n as usize).min(chars.len() - start);
                        let mut buf: String = chars[..start].iter().collect();
                        buf.push_str(&repl.to_string_lossy());
                        buf.extend(chars[start + take..].iter());
                        *s.borrow_mut() = buf.into_bytes();
                        return Ok(Some(args[2].clone()));
                    }
                    return Ok(None);
                }
                match (name, args) {
                    ("chars", []) => {
                        let elems: Vec<Value> = s.to_string_lossy().chars()
                            .map(|c| Value::new_str(c.to_string()))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", []) => {
                        // No-arg `split` matches CRuby's `split(nil)`:
                        // splits on runs of whitespace, drops the
                        // leading empty token.
                        let src = s.to_string_lossy();
                        let elems: Vec<Value> = src.split_whitespace()
                            .map(Value::new_str)
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("split", [Value::Str(sep)]) => {
                        let sep_s = sep.to_string_lossy();
                        let src = s.to_string_lossy();
                        let elems: Vec<Value> = if sep_s.is_empty() {
                            // CRuby: empty-sep split returns each character.
                            src.chars().map(|c| Value::new_str(c.to_string())).collect()
                        } else {
                            src.split(sep_s.as_str()).map(Value::new_str).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    ("%", [single_arg]) => {
                        // Build the argument list. A single Array
                        // splats into positional args; everything
                        // else is a one-element list. This matches
                        // CRuby's `format`/`String#%` calling
                        // convention.
                        let owned;
                        let fmt_args: &[Value] = match single_arg {
                            Value::Array(arr_id) => {
                                owned = self.heap.array(*arr_id).clone();
                                owned.as_slice()
                            }
                            _ => std::slice::from_ref(single_arg),
                        };
                        let fmt_str = s.to_string_lossy();
                        let out = ruby_sprintf(&fmt_str, fmt_args, &self.heap, &self.interner)
                            .map_err(|e| self.trap(e))?;
                        if let Some(max) = self.max_value_bytes
                            && out.len() > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("String#% would exceed {max} bytes"),
                                }));
                            }
                        Some(Value::new_str(out))
                    }
                    // Literal-substring `scan` — returns a fresh
                    // Array containing one copy of the pattern for
                    // every non-overlapping occurrence in the
                    // receiver. CRuby's full `scan` accepts a
                    // Regexp and yields capture groups; literal
                    // patterns are the degenerate case where every
                    // match is the pattern itself, exactly what we
                    // implement. An empty pattern returns
                    // `[""] * (chars + 1)` to match CRuby; this is
                    // unusual but well-defined and cheap.
                    // `String#scan(/pat/)` — Regex form. Returns
                    // either an Array of matched strings (no
                    // capture groups in the pattern) or an Array
                    // of capture-group Arrays (one or more
                    // groups). CRuby's behaviour: with groups the
                    // FULL match is dropped and only captures
                    // appear; without groups the full match is
                    // the element.
                    #[cfg(feature = "regex")]
                    ("scan", [Value::Regex(re)]) => {
                        // regex crate is &str-only; lossy view at
                        // iteration entry (binary input degrades to
                        // lossy UTF-8 here — regex itself only
                        // matches UTF-8 anyway).
                        let s_owned = s.to_string_lossy();
                        let has_groups = re.captures_len() > 1;
                        // GC rooting: under STRESS_GC=1 each per-match
                        // sub-Array alloc'd in the has_groups branch
                        // is unreachable until the wrapping result
                        // Array is built — pin each push so it
                        // survives subsequent maybe_gc's. The no-
                        // groups branch alloc's only Strings (which
                        // are Rc-based, not heap-managed by ObjId),
                        // so no pin is needed there. See
                        // `array.rs::combination` for the symmetric
                        // pattern and `proc_curry_compose` / earlier
                        // STRESS_GC commit for the broader fix.
                        let mut g = PinGuard::new(self);
                        let mut out: Vec<Value> = Vec::new();
                        if has_groups {
                            for caps in re.captures_iter(&s_owned) {
                                let mut group_vec: Vec<Value> = Vec::with_capacity(caps.len() - 1);
                                for i in 1..caps.len() {
                                    let g_val = caps.get(i)
                                        .map(|m| Value::new_str(m.as_str()))
                                        .unwrap_or(Value::Nil);
                                    group_vec.push(g_val);
                                }
                                g.vm.maybe_gc();
                                g.vm.check_alloc()?;
                                let gid = g.vm.heap.alloc(HeapObj::Array(group_vec));
                                let v = Value::Array(gid);
                                g.pin(v.clone());
                                out.push(v);
                            }
                        } else {
                            for m in re.find_iter(&s_owned) {
                                out.push(Value::new_str(m.as_str()));
                            }
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let id = g.vm.heap.alloc(HeapObj::Array(out));
                        Some(Value::Array(id))
                    }
                    ("scan", [Value::Str(pat)]) => {
                        let parts: Vec<Value> = if pat.borrow().is_empty() {
                            std::iter::repeat_with(|| Value::new_str(""))
                                .take(s.to_string_lossy().chars().count() + 1)
                                .collect()
                        } else {
                            let mut out: Vec<Value> = Vec::new();
                            let mut i = 0;
                            let s_ref = s.borrow();
                            let bytes: &[u8] = &s_ref;
                            let pat_ref = pat.borrow();
                            let pat_bytes: &[u8] = &pat_ref;
                            let plen = pat_bytes.len();
                            while i + plen <= bytes.len() {
                                if &bytes[i..i + plen] == pat_bytes {
                                    out.push(Value::Str(pat.clone()));
                                    i += plen;
                                } else {
                                    i += 1;
                                }
                            }
                            out
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(parts));
                        Some(Value::Array(id))
                    }
                    // `String#bytes` — Array of byte values (Int
                    // per byte). Trivially derived from the raw
                    // backing Vec<u8>. Useful for inspecting
                    // `Array#pack` output without round-tripping
                    // through `unpack("C*")`.
                    ("bytes", []) => {
                        let elems: Vec<Value> = s.borrow().iter()
                            .map(|b| Value::Int(*b as i64))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems));
                        Some(Value::Array(id))
                    }
                    // `String#unpack(format)` — binary unpacking
                    // with a subset of CRuby's directives. Supported:
                    //   C / c — 8-bit unsigned/signed byte
                    //   n / N — 16/32-bit big-endian unsigned
                    //   v / V — 16/32-bit little-endian unsigned
                    //   q / Q — 64-bit native (LE on our targets)
                    //   a / A / Z — strings (raw / trim trailing
                    //                space+null / null-terminated)
                    // Counts: digits or `*`. Documented divergence:
                    // exotic directives (m, U, w, f/d/e/E/g/G, etc.)
                    // raise ArgumentError instead of CRuby's wider
                    // table. See SUBSET.md.
                    ("unpack", [Value::Str(fmt)]) => {
                        let bytes = s.borrow().clone();
                        let fmt_str = fmt.to_string_lossy();
                        let result = unpack_bytes(&bytes, &fmt_str)
                            .map_err(|m| self.trap(RubyError::ArgumentError { msg: m }))?;
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(result));
                        Some(Value::Array(id))
                    }
                    // `String#unpack1(fmt)` — same engine as `#unpack`
                    // but returns just the first directive's result
                    // (or `nil` if the result is empty). Idiomatic
                    // when a binary-protocol parser knows the format
                    // produces one value (msgpack-ruby's per-frame
                    // header reads, `bcrypt`'s salt extraction, etc.)
                    // and wants to skip the `.first` boilerplate.
                    //
                    // Offset kwarg (`unpack1(fmt, offset: N)`, Ruby
                    // 3.1+) is not implemented; the 1-arg form is
                    // what real-world Ruby code uses overwhelmingly.
                    ("unpack1", [Value::Str(fmt)]) => {
                        let bytes = s.borrow().clone();
                        let fmt_str = fmt.to_string_lossy();
                        let mut result = unpack_bytes(&bytes, &fmt_str)
                            .map_err(|m| self.trap(RubyError::ArgumentError { msg: m }))?;
                        Some(if result.is_empty() { Value::Nil } else { result.swap_remove(0) })
                    }
                    ("to_sym", []) => {
                        // P2-14b: cap the interner before a hot loop
                        // (`arr.map { |x| x.to_s.to_sym }` and similar)
                        // can quietly grow it without bound. Existing
                        // symbols always re-resolve; only fresh strings
                        // count against the cap.
                        let s_str = s.to_string_lossy();
                        if let Some(max) = self.max_symbols
                            && !self.interner.contains(&s_str) && self.interner.len() >= max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("interner exhausted: {} symbols", max),
                                }));
                            }
                        let sym = self.interner.intern(&s_str);
                        Some(Value::Sym(sym))
                    }
                    _ => None,
                }
        })
    }
}

/// Ruby's `String#succ` / `#next` — the "alphanumeric successor".
/// Walks right-to-left looking for the first alnum char, increments
/// it; on rollover ('z'→'a', 'Z'→'A', '9'→'0') carries into the
/// next char left. If the leftmost alnum rolls over, a new char of
/// the same class is prepended ('z' → 'aa', '9' → '10', 'Az' → 'Ba'
/// — wait actually 'Az' → 'Ba'? Yes: carry pushes 'A'→'B').
///
/// Used both directly (`String#succ` primitive) and by Range#each
/// over String endpoints for the canonical `('a'..'z').to_a`
/// iteration. CRuby's full spec covers a few more edge cases
/// (bracketed-string forms, all-non-alnum) which we don't reach
/// in the subset; those return the input unchanged.
pub(crate) fn str_succ(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = chars.clone();
    let mut carry_kind: Option<char> = None; // 'a' / 'A' / '0' if we ran off the front

    let mut i = out.len();
    loop {
        if i == 0 {
            // Walked past the leftmost char with the carry still pending —
            // prepend a fresh char of the same class.
            if let Some(k) = carry_kind {
                out.insert(0, k);
            }
            return out.into_iter().collect();
        }
        i -= 1;
        let c = out[i];
        match c {
            'a'..='y' | 'A'..='Y' | '0'..='8' => {
                out[i] = (c as u8 + 1) as char;
                return out.into_iter().collect();
            }
            'z' => { out[i] = 'a'; carry_kind = Some('a'); /* continue carry */ }
            'Z' => { out[i] = 'A'; carry_kind = Some('A'); }
            '9' => { out[i] = '0'; carry_kind = Some('1'); }
            _ => {
                // Non-alnum: no increment here; if we were in a carry,
                // CRuby pushes a fresh char of the carry class in front
                // of the current position. We just continue scanning
                // — eventually we run off the front and insert. For
                // pure-non-alnum inputs this returns the input unchanged,
                // matching CRuby for the common subset.
                if carry_kind.is_some() { continue; }
                // No alnum found yet — just bump this char's byte.
                // CRuby's behaviour here is "use the rightmost char's
                // succ", which for non-alnum bytes is byte+1. Good
                // enough for the niche.
                out[i] = (c as u32 + 1) as u8 as char;
                return out.into_iter().collect();
            }
        }
    }
}

/// Translate Ruby's `\0` / `\1` / … backref syntax in a
/// String#gsub replacement template into the `regex` crate's
/// `$0` / `$1` / … convention. Doubled backslash (`\\`) escapes
/// a literal backslash. `\&` is the entire match (CRuby alias
/// for `\0`); `\'` (post-match) / `\`` (pre-match) are NOT
/// supported in our subset — they'd need MatchData state we
/// don't currently carry.
///
/// Also escapes any literal `$` in the template so the regex
/// crate doesn't interpret it as its own backref form.
#[cfg(feature = "regex")]
pub(crate) fn ruby_backref_to_dollar(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some(&n) if n.is_ascii_digit() => {
                    chars.next();
                    out.push('$');
                    out.push(n);
                }
                Some(&'&') => {
                    chars.next();
                    out.push('$');
                    out.push('0');
                }
                Some(&'\\') => {
                    chars.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            },
            // Escape `$` so the regex crate doesn't capture it.
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

/// Parse a pack/unpack format directive (single char + optional
/// endian modifier + optional count). Returns the *canonical*
/// directive char and count (`None` ≡ `Some(usize::MAX)` for `*`,
/// `Some(1)` if no count given, `Some(n)` otherwise).
///
/// Endian modifier handling (CRuby compat):
///   `L>` / `L<`  →  `N` / `V`   (32-bit BE/LE unsigned)
///   `L`          →  `V`         (platform-native; LE on our targets)
///   `S>` / `S<`  →  `n` / `v`   (16-bit BE/LE unsigned)
///   `S`          →  `v`         (platform-native; LE on our targets)
///   `Q>` / `Q<`  →  `J` / `Q`   (64-bit BE/LE unsigned;
///                                `J` is the internal sentinel
///                                we use for BE-Q since CRuby
///                                doesn't expose a single-char
///                                form. `j` mirrors for signed.)
///   `q>` / `q<`  →  `j` / `q`
///
/// `J` / `j` are otherwise unused in CRuby's format-string
/// grammar, so the sentinel doesn't shadow a real directive.
fn parse_directive(it: &mut std::str::Chars<'_>) -> Option<(char, Option<usize>)> {
    let dir_raw = it.next()?;
    // Look ahead for endian modifier `>` or `<`.
    let endian: Option<char> = {
        let mut peek = it.clone();
        match peek.next() {
            Some(c @ ('>' | '<')) => {
                let _ = it.next(); // consume the modifier
                Some(c)
            }
            _ => None,
        }
    };
    let dir = match (dir_raw, endian) {
        ('L', Some('>')) => 'N',
        ('L', Some('<')) => 'V',
        ('L', None)      => 'V', // native = LE on our targets
        ('S', Some('>')) => 'n',
        ('S', Some('<')) => 'v',
        ('S', None)      => 'v',
        ('Q', Some('>')) => 'J',
        ('Q', Some('<')) => 'Q',
        ('q', Some('>')) => 'j',
        ('q', Some('<')) => 'q',
        // Other directives with `>` / `<` aren't supported (CRuby
        // also doesn't define endian modifiers on `C` / `c` /
        // `a` / `A` / `Z`). Drop the modifier silently — the
        // `>` / `<` won't match a known directive on its own,
        // so the outer loop will surface "unsupported" if the
        // caller really wanted endian semantics on a non-S/L/Q
        // directive.
        (c, _) => c,
    };
    let mut peek = it.clone();
    let mut count: Option<usize> = Some(1);
    let mut consumed = 0usize;
    match peek.next() {
        Some('*') => {
            // `*` sentinel: use Some(usize::MAX) so callers can
            // branch via `n == usize::MAX` while keeping the
            // return type a plain `Option<usize>`.
            count = Some(usize::MAX);
            consumed = 1;
        }
        Some(c) if c.is_ascii_digit() => {
            let mut n: usize = 0;
            let mut cur = c;
            let mut p = peek.clone();
            loop {
                if cur.is_ascii_digit() {
                    n = n.saturating_mul(10).saturating_add((cur as u8 - b'0') as usize);
                    consumed += 1;
                    match p.next() { Some(nc) => cur = nc, None => break }
                } else { break; }
            }
            count = Some(n);
        }
        _ => {}
    }
    for _ in 0..consumed { it.next(); }
    Some((dir, count))
}

/// Subset of CRuby's `String#unpack` — see the per-directive
/// table in the call-site comment. Returns Err with a CRuby-
/// ish message on unsupported directives or malformed input.
pub(crate) fn unpack_bytes(input: &[u8], fmt: &str) -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0usize;
    let mut it = fmt.chars();
    while let Some((dir, count)) = parse_directive(&mut it) {
        // count = Some(usize::MAX) means "*" (rest of input)
        let n = count.unwrap_or(1);
        match dir {
            'C' | 'c' => {
                let take = if n == usize::MAX { input.len() - i } else { n };
                for _ in 0..take {
                    if i >= input.len() { out.push(Value::Nil); continue; }
                    let b = input[i]; i += 1;
                    out.push(Value::Int(if dir == 'c' { (b as i8) as i64 } else { b as i64 }));
                }
            }
            'n' | 'v' => {
                let take = if n == usize::MAX { (input.len() - i) / 2 } else { n };
                for _ in 0..take {
                    if i + 2 > input.len() { out.push(Value::Nil); break; }
                    let v = if dir == 'n' {
                        u16::from_be_bytes([input[i], input[i+1]])
                    } else {
                        u16::from_le_bytes([input[i], input[i+1]])
                    };
                    i += 2;
                    out.push(Value::Int(v as i64));
                }
            }
            'N' | 'V' => {
                let take = if n == usize::MAX { (input.len() - i) / 4 } else { n };
                for _ in 0..take {
                    if i + 4 > input.len() { out.push(Value::Nil); break; }
                    let v = if dir == 'N' {
                        u32::from_be_bytes([input[i], input[i+1], input[i+2], input[i+3]])
                    } else {
                        u32::from_le_bytes([input[i], input[i+1], input[i+2], input[i+3]])
                    };
                    i += 4;
                    out.push(Value::Int(v as i64));
                }
            }
            'q' | 'Q' | 'j' | 'J' => {
                // q = i64 LE, Q = u64 LE, j = i64 BE, J = u64 BE.
                // Internal sentinels `j` / `J` come from the
                // `q>` / `Q>` endian-modifier parse path.
                let take = if n == usize::MAX { (input.len() - i) / 8 } else { n };
                for _ in 0..take {
                    if i + 8 > input.len() { out.push(Value::Nil); break; }
                    let b = [input[i], input[i+1], input[i+2], input[i+3],
                             input[i+4], input[i+5], input[i+6], input[i+7]];
                    i += 8;
                    let v: i64 = match dir {
                        'q' => i64::from_le_bytes(b),
                        'Q' => u64::from_le_bytes(b) as i64,
                        'j' => i64::from_be_bytes(b),
                        'J' => u64::from_be_bytes(b) as i64,
                        _ => unreachable!(),
                    };
                    out.push(Value::Int(v));
                }
            }
            'a' | 'A' | 'Z' => {
                let take = if n == usize::MAX { input.len() - i } else { n.min(input.len() - i) };
                let slice = &input[i..i + take];
                i += take;
                let s_bytes: Vec<u8> = match dir {
                    'a' => slice.to_vec(),
                    'A' => {
                        // Trim trailing space + null.
                        let mut end = slice.len();
                        while end > 0 && matches!(slice[end-1], b' ' | b'\0') { end -= 1; }
                        slice[..end].to_vec()
                    }
                    'Z' => {
                        // Up to (but excluding) the first null.
                        let pos = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
                        slice[..pos].to_vec()
                    }
                    _ => unreachable!(),
                };
                out.push(Value::new_str_bytes(s_bytes));
            }
            // Whitespace inside the format is ignored, per CRuby.
            ' ' | '\t' | '\n' => {}
            _ => return Err(format!("unsupported pack/unpack directive '{}'", dir)),
        }
    }
    Ok(out)
}

/// Subset of CRuby's `Array#pack`. Mirror of `unpack_bytes`;
/// see the call-site comment for the supported directive list.
pub(crate) fn pack_values(values: &[Value], fmt: &str) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    let mut vi = 0usize;
    let mut it = fmt.chars();
    while let Some((dir, count)) = parse_directive(&mut it) {
        let n = count.unwrap_or(1);
        match dir {
            'C' | 'c' => {
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let i = match v {
                        Value::Int(n) => n,
                        _ => return Err("pack: expected Integer for C/c".into()),
                    };
                    out.push((i & 0xff) as u8);
                }
            }
            'n' | 'v' => {
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let i = match v {
                        Value::Int(n) => n as u16,
                        _ => return Err("pack: expected Integer for n/v".into()),
                    };
                    let b = if dir == 'n' { i.to_be_bytes() } else { i.to_le_bytes() };
                    out.extend_from_slice(&b);
                }
            }
            'N' | 'V' => {
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let i = match v {
                        Value::Int(n) => n as u32,
                        _ => return Err("pack: expected Integer for N/V".into()),
                    };
                    let b = if dir == 'N' { i.to_be_bytes() } else { i.to_le_bytes() };
                    out.extend_from_slice(&b);
                }
            }
            'q' | 'Q' | 'j' | 'J' => {
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let i = match v {
                        Value::Int(n) => n,
                        _ => return Err("pack: expected Integer for q/Q/j/J".into()),
                    };
                    let b: [u8; 8] = match dir {
                        'q' => i.to_le_bytes(),
                        'Q' => (i as u64).to_le_bytes(),
                        'j' => i.to_be_bytes(),
                        'J' => (i as u64).to_be_bytes(),
                        _ => unreachable!(),
                    };
                    out.extend_from_slice(&b);
                }
            }
            'a' | 'A' | 'Z' => {
                let v = values.get(vi).cloned().unwrap_or(Value::new_str(""));
                vi += 1;
                let bytes: Vec<u8> = match v {
                    Value::Str(s) => s.borrow().clone(),
                    _ => return Err("pack: expected String for a/A/Z".into()),
                };
                let want = if n == usize::MAX { bytes.len() } else { n };
                if bytes.len() >= want {
                    out.extend_from_slice(&bytes[..want]);
                } else {
                    out.extend_from_slice(&bytes);
                    let pad: u8 = if dir == 'A' { b' ' } else { 0 };
                    out.extend(std::iter::repeat_n(pad, want - bytes.len()));
                }
            }
            ' ' | '\t' | '\n' => {}
            _ => return Err(format!("unsupported pack/unpack directive '{}'", dir)),
        }
    }
    Ok(out)
}
