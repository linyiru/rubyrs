//! `String` primitive methods. Mirrors CRuby's `string.c` —
//! the per-method match arms that don't need heap allocation
//! (concat / sub / gsub / tr already produce String results via
//! `Value::new_str`, which wraps a fresh Rc<RStr>; nothing here
//! reaches into the GC heap directly).
//!
//! Called from `primitive_call` (vm.rs) after numeric dispatch.
//! Stateless — no Vm access, just receiver + args + the
//! resource cap.

use std::rc::Rc;

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::{RStr, Value};

use super::{ruby_sprintf, Vm};
// PinGuard is only referenced from the `("scan", [Value::Regex(...)])`
// arm, which is itself `cfg(feature = "regex")` via `Value::Regex`.
// Without this gate, `--no-default-features` (the wasm32-wasip1
// shape) sees an unused import and `-D warnings` blocks the build.
#[cfg(feature = "regex")]
use super::PinGuard;

/// CRuby's strip-family whitespace predicate. Matches the
/// exact set CRuby's `String#strip` / `#lstrip` / `#rstrip`
/// treat as strippable: SP, HT, LF, VT, FF, CR, plus the NUL
/// byte. Two gaps from Rust's stdlib idioms we'd otherwise
/// reach for:
///   - `char::is_ascii_whitespace` covers SP/HT/LF/FF/CR but
///     NOT VT (`\x0B`); CRuby strips VT, so a 'goodbye\v'
///     left-untouched would be observable.
///   - `char::is_whitespace` covers VT but is Unicode-aware
///     (e.g. NBSP `\u{00A0}`), which CRuby does NOT strip;
///     using it would over-strip.
///
/// Enumerate the byte set explicitly to match CRuby exactly.
/// Divergence pinned by `tests/fixtures/divergence_string_strip_nul.rb`
/// (PR #193) is the gap this predicate closes.
#[inline]
fn strip_ws_or_nul(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r' | '\0')
}

/// `String#capitalize` core — ASCII-only case fold. First
/// char uppercase, remaining chars lowercase. Non-letters at
/// position 0 are left as-is (`"1hello".capitalize` → same).
/// Empty input returns empty.
///
/// **Diverges from CRuby on non-ASCII letters.** CRuby's
/// `String#capitalize` (no options) has been Unicode-aware
/// since 2.4 — `"über".capitalize == "Über"`. Here
/// `to_ascii_uppercase` / `to_ascii_lowercase` no-op on
/// non-ASCII chars, so `"über".capitalize == "über"`. The
/// gap covers both the option form (`:turkic` etc.) AND the
/// default case-fold for non-ASCII letters; full Unicode
/// support is gated on ADR 0020 Tier-2 Encoding.
fn capitalize_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    if let Some(first) = chars.next() {
        out.push(first.to_ascii_uppercase());
        for c in chars { out.push(c.to_ascii_lowercase()); }
    }
    out
}

/// `String#swapcase` core — flip ASCII letter case on each
/// char; non-letters pass through unchanged.
///
/// **Diverges from CRuby on non-ASCII letters.** CRuby's
/// `String#swapcase` (no options) flips Unicode letters too
/// since 2.4: `"Café".swapcase == "cAFÉ"`. Here only ASCII
/// letters flip, so `"Café".swapcase == "cAFé"`. Full
/// Unicode support is gated on ADR 0020 Tier-2 Encoding.
fn swapcase_ascii(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_uppercase() { c.to_ascii_lowercase() }
            else if c.is_ascii_lowercase() { c.to_ascii_uppercase() }
            else { c }
        })
        .collect()
}

/// `String#sub` core — first-match string replacement.
/// Empty pattern prepends `repl` (CRuby quirk). Shared by
/// `sub` / `sub!` so the destructive arm can compare the
/// computed result against the original to decide between
/// mutate-and-return-self vs return-nil.
fn sub_str_str_core(a: &str, pat: &str, repl: &str) -> String {
    if pat.is_empty() {
        let mut s = String::with_capacity(repl.len() + a.len());
        s.push_str(repl);
        s.push_str(a);
        s
    } else if let Some(idx) = a.find(pat) {
        let mut s = String::with_capacity(a.len() + repl.len());
        s.push_str(&a[..idx]);
        s.push_str(repl);
        s.push_str(&a[idx + pat.len()..]);
        s
    } else {
        a.to_string()
    }
}

/// `String#gsub` core — every-match string replacement.
/// Empty pattern wraps `repl` around every char
/// (`"abc".gsub("", "X") == "XaXbXcX"`). Shared by `gsub`
/// and `gsub!`.
fn gsub_str_str_core(a: &str, pat: &str, repl: &str) -> String {
    if pat.is_empty() {
        let mut s = String::with_capacity(repl.len() * (a.chars().count() + 1) + a.len());
        s.push_str(repl);
        for c in a.chars() {
            s.push(c);
            s.push_str(repl);
        }
        s
    } else {
        a.replace(pat, repl)
    }
}

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
            Some(Value::Int(a.char_count() as i64))
        }
        // `String#count(sel, ...)` — count chars matching every
        // selector (multi-arg = intersection). Each selector
        // supports CRuby's tr-style mini-syntax: `^X` negates,
        // `a-z` expands a range. Matches CRuby spec for the
        // shapes ERB (`content.count("\n")` for line offsets)
        // and similar consumers use. Empty selector matches no
        // chars; multi-arg intersection means a char must match
        // EVERY selector to count.
        //
        // Motivating use: MRI lib/erb/compiler.rb:312 — counts
        // newlines in template content to keep line offsets
        // accurate in the compiled output.
        (Value::Str(a), "count", sels) if !sels.is_empty() => {
            let mut parsed_sels: Vec<(std::collections::HashSet<char>, bool)> =
                Vec::with_capacity(sels.len());
            for sel in sels {
                let s = match sel {
                    Value::Str(s) => s.to_string_lossy(),
                    _ => return Ok(None), // Type-error path: let
                                          // generic dispatch handle.
                };
                let parsed = parse_count_selector(&s).map_err(|msg| {
                    crate::error::RubyError::ArgumentError { msg: msg.to_string() }
                })?;
                parsed_sels.push(parsed);
            }
            let total: i64 = a.with_str_lossy(|input| {
                input.chars().filter(|c| {
                    parsed_sels.iter().all(|(set, negate)| {
                        let in_set = set.contains(c);
                        if *negate { !in_set } else { in_set }
                    })
                }).count() as i64
            });
            Some(Value::Int(total))
        }
        // `String#hash` — Integer hash derived from the byte
        // contents. CRuby guarantees: equal strings hash equal
        // (we satisfy this — same byte slice hashes identically
        // via DefaultHasher). The reverse is not promised by
        // either implementation. tilt/string.rb:17 uses this for
        // a heredoc tag (`"TILT#{@data.hash.abs}"`); generally
        // any code performing explicit `obj.hash` lookups on a
        // String. (Hash key dispatch internally uses a separate
        // mechanism — this primitive is only the script-visible
        // `#hash` method.)
        (Value::Str(a), "hash", []) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            a.borrow().hash(&mut h);
            Some(Value::Int(h.finish() as i64))
        }
        (Value::Str(a), "bytesize", []) => Some(Value::Int(a.borrow().len() as i64)),
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.borrow().is_empty())),
        (Value::Str(a), "upcase", []) => Some(Value::new_str(a.to_string_lossy().to_uppercase())),
        (Value::Str(a), "downcase", []) => Some(Value::new_str(a.to_string_lossy().to_lowercase())),
        (Value::Str(a), "reverse", []) => Some(Value::new_str(a.to_string_lossy().chars().rev().collect::<String>())),
        // `String#capitalize` — first char uppercase, rest
        // lowercase. ASCII-only fold (Unicode options out of
        // subset). Empty string is a no-op. First non-letter
        // (digit / punctuation) stays as-is.
        (Value::Str(a), "capitalize", []) => Some(Value::new_str(
            a.with_str_lossy(capitalize_ascii)
        )),
        // `String#swapcase` — every letter has its case
        // flipped; non-letters pass through.
        (Value::Str(a), "swapcase", []) => Some(Value::new_str(
            a.with_str_lossy(swapcase_ascii)
        )),
        // Wrong-arity arms: CRuby accepts an optional Unicode
        // case-mapping option symbol (`:ascii` / `:turkic` /
        // `:lithuanian` / `:fold`); we don't support the option
        // form (ADR 0020 Tier-2 Encoding), so any positional
        // arg raises ArgumentError with the standard "wrong
        // number of arguments" shape. Without these arms the
        // dispatcher falls through to NoMethodError, which lies
        // about feature availability since `respond_to?` returns
        // true for these names.
        (Value::Str(_), "capitalize" | "swapcase" | "capitalize!" | "swapcase!", many) if !many.is_empty() => {
            return Err(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", many.len()),
            });
        }
        // Destructive `!` siblings — mutate the receiver in
        // place and return self when changed, nil when the
        // input already matched the result. The frozen check
        // mirrors the mutating arms further down (`<<` /
        // `concat` etc.). Length-changing mutations honour
        // `max_value_bytes` via `check`.
        (Value::Str(a), "upcase!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s| s.to_uppercase().into_bytes());
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "downcase!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s| s.to_lowercase().into_bytes());
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "capitalize!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s| capitalize_ascii(s).into_bytes());
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "swapcase!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s| swapcase_ascii(s).into_bytes());
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "reverse!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            // reverse! always mutates (even when palindrome —
            // CRuby returns self, not nil, for reverse!).
            let new_bytes: Vec<u8> = a.with_str_lossy(|s|
                s.chars().rev().collect::<String>().into_bytes()
            );
            check(new_bytes.len())?;
            *a.borrow_mut() = new_bytes;
            Some(Value::Str(a.clone()))
        }
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
        // `String#encoding` is intercepted in dispatch.rs before
        // this function, so it can hand back the
        // `Encoding::UTF_8` instance from the preamble (needs Vm
        // access for the constants table). The string_call
        // free-function context can't reach Vm state.
        //
        // `String#b` — CRuby: a NEW String (a copy of the
        // receiver's bytes) with ASCII-8BIT encoding. Returning
        // `recv.clone()` would share the underlying RStr Rc and
        // make `.b` an alias — mutations to the result would leak
        // back to the original, and a frozen receiver would yield
        // a frozen result. Copy the bytes into a fresh RStr so
        // the result is independent and unfrozen. We don't tag
        // encodings per-string, so the ASCII-8BIT distinction is
        // a no-op for our subset.
        (Value::Str(a), "b", []) => Some(Value::new_str_bytes(a.content.borrow().clone())),
        // CRuby's strip family treats `\x00` as part of the
        // strippable whitespace set (along with space, tab, NL,
        // CR, FF, VT). Rust's `is_whitespace()` excludes NUL,
        // so a bare `.trim()` would leave NUL bytes on the ends
        // — a divergence pinned in
        // `tests/fixtures/divergence_string_strip_nul.rb` (PR
        // #193) until this fix. Use a predicate that matches
        // CRuby's set exactly.
        (Value::Str(a), "strip", []) => Some(Value::new_str(
            a.to_string_lossy().trim_matches(strip_ws_or_nul).to_string()
        )),
        (Value::Str(a), "lstrip", []) => Some(Value::new_str(
            a.to_string_lossy().trim_start_matches(strip_ws_or_nul).to_string()
        )),
        (Value::Str(a), "rstrip", []) => Some(Value::new_str(
            a.to_string_lossy().trim_end_matches(strip_ws_or_nul).to_string()
        )),
        // Destructive strip siblings — return self on change,
        // nil otherwise. The frozen check + check() guard mirror
        // the other `!` variants in this file.
        (Value::Str(a), "strip!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s|
                s.trim_matches(strip_ws_or_nul).as_bytes().to_vec()
            );
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "lstrip!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s|
                s.trim_start_matches(strip_ws_or_nul).as_bytes().to_vec()
            );
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "rstrip!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let new_bytes = a.with_str_lossy(|s|
                s.trim_end_matches(strip_ws_or_nul).as_bytes().to_vec()
            );
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
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
        // `String#index(needle, offset)` — start scanning at `offset`.
        // CRuby accepts negative offsets (counted from the end) and
        // returns nil when offset > length. Returns nil when needle
        // isn't found at or after `offset`. The returned index is
        // ABSOLUTE in the receiver (not relative to `offset`),
        // matching CRuby's contract; this is what makes
        // `String#index` chainable for streaming readers like
        // StringIO#gets.
        (Value::Str(a), "index", [Value::Str(b), Value::Int(off)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| {
                let len = sa.len() as i64;
                let start = if *off < 0 { len + *off } else { *off };
                // CRuby: out-of-range offsets (either side) return
                // nil rather than clamping. `len + offset < 0` is
                // the "negative offset past the start" case;
                // `offset > len` is the "offset past the end" case.
                if !(0..=len).contains(&start) {
                    return Value::Nil;
                }
                let start = start as usize;
                match sa[start..].find(sb) {
                    Some(i) => Value::Int((start + i) as i64),
                    None => Value::Nil,
                }
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
            let out = sub_str_str_core(
                &a.to_string_lossy(),
                &pat.to_string_lossy(),
                &repl.to_string_lossy(),
            );
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
            let out = gsub_str_str_core(
                &a.to_string_lossy(),
                &pat.to_string_lossy(),
                &repl.to_string_lossy(),
            );
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // Destructive `!` siblings for sub / gsub — share the
        // frozen-check pattern with the case-fold `!` arms
        // above, but the nil-vs-self decision is gated on
        // MATCH PRESENCE, not on byte equality: CRuby returns
        // `self` whenever a match occurred (even when the
        // replacement produced bytes identical to the input,
        // e.g. `"a".sub!("a", "a")` → `"a"`); only no-match
        // returns nil. An empty Str pattern always matches
        // (sub prepends, gsub wraps every char) — both
        // preserve that quirk for parity with the non-bang
        // arms. The post-compute equality check is kept as a
        // cheap guard to avoid an unnecessary buffer swap
        // when the bytes happen to be identical.
        (Value::Str(a), "sub!", [Value::Str(pat), Value::Str(repl)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let a_ref = a.to_string_lossy();
            let pat_ref = pat.to_string_lossy();
            let matched = pat_ref.is_empty() || a_ref.contains(pat_ref.as_str());
            if !matched { return Ok(Some(Value::Nil)); }
            let new_bytes = sub_str_str_core(
                &a_ref, &pat_ref, &repl.to_string_lossy(),
            ).into_bytes();
            if *a.borrow() != new_bytes {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
            }
            Some(Value::Str(a.clone()))
        }
        (Value::Str(a), "gsub!", [Value::Str(pat), Value::Str(repl)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let a_ref = a.to_string_lossy();
            let pat_ref = pat.to_string_lossy();
            let matched = pat_ref.is_empty() || a_ref.contains(pat_ref.as_str());
            if !matched { return Ok(Some(Value::Nil)); }
            let new_bytes = gsub_str_str_core(
                &a_ref, &pat_ref, &repl.to_string_lossy(),
            ).into_bytes();
            if *a.borrow() != new_bytes {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
            }
            Some(Value::Str(a.clone()))
        }
        #[cfg(feature = "regex")]
        (Value::Str(a), "sub!", [Value::Regex(re), Value::Str(repl)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let a_ref = a.to_string_lossy();
            if !re.is_match(&a_ref) { return Ok(Some(Value::Nil)); }
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            let new_bytes = re.replace(&a_ref, repl_xlated.as_str()).into_owned().into_bytes();
            if *a.borrow() != new_bytes {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
            }
            Some(Value::Str(a.clone()))
        }
        #[cfg(feature = "regex")]
        (Value::Str(a), "gsub!", [Value::Regex(re), Value::Str(repl)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let a_ref = a.to_string_lossy();
            if !re.is_match(&a_ref) { return Ok(Some(Value::Nil)); }
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            let new_bytes = re.replace_all(&a_ref, repl_xlated.as_str()).into_owned().into_bytes();
            if *a.borrow() != new_bytes {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
            }
            Some(Value::Str(a.clone()))
        }
        // String#tr — character-by-character translation. Each
        // char in `from` maps to the same-index char in `to`; if
        // `to` is shorter, characters past its length map to its
        // LAST char (CRuby's "stretch" behaviour). If `to` is
        // empty, those chars are deleted.
        //
        // `from` and `to` both go through `parse_tr_set`. `from`
        // honours a leading `^` as set negation ("translate every
        // char NOT in the set" — non-set chars all map to `to`'s
        // LAST char, or are deleted if `to` is empty). `to`
        // treats `^` as a literal — `tr("a", "^b")` translates
        // `a` to `^`. Range overflow (set > TR_SET_MAX_CHARS)
        // raises ArgumentError, bounding intermediate Vec growth
        // against `tr("\u{0}-\u{10FFFF}", "*")` style inputs.
        (Value::Str(a), "tr", [Value::Str(from), Value::Str(to)]) => {
            let a_ref = a.to_string_lossy();
            let from_ref = from.to_string_lossy();
            let to_ref = to.to_string_lossy();
            let (from_chars, from_negated) = match parse_tr_set(&from_ref, true) {
                Ok(t) => t,
                Err(msg) => return Err(crate::error::RubyError::ArgumentError {
                    msg: msg.to_string(),
                }),
            };
            let (to_chars, _) = match parse_tr_set(&to_ref, false) {
                Ok(t) => t,
                Err(msg) => return Err(crate::error::RubyError::ArgumentError {
                    msg: msg.to_string(),
                }),
            };
            // Pre-build a `char → index` lookup so the per-input-
            // char hot loop is O(1) instead of O(from_chars.len()).
            // For large expanded sets (`tr("a-z", ...)` etc.) the
            // prior `position()` scan would be O(n*m).
            //
            // Duplicate chars in `from`: LAST occurrence wins.
            // CRuby builds the translation table by iterating
            // `from`/`to` and overwriting the per-char entry on
            // each step, so e.g. `"a".tr("aa", "12") == "2"`.
            // `insert()` overwrites on hit; if we used
            // `entry().or_insert(i)` the first occurrence would
            // win, which diverges from CRuby.
            let mut from_index: std::collections::HashMap<char, usize> =
                std::collections::HashMap::with_capacity(from_chars.len());
            for (i, c) in from_chars.iter().enumerate() {
                from_index.insert(*c, i);
            }
            let mut out = String::with_capacity(a_ref.len());
            for ch in a_ref.chars() {
                let idx_opt = from_index.get(&ch).copied();
                let translate = if from_negated { idx_opt.is_none() } else { idx_opt.is_some() };
                if !translate {
                    out.push(ch);
                    continue;
                }
                if to_chars.is_empty() {
                    // Delete: skip this character entirely.
                    continue;
                }
                if from_negated {
                    // Every translated char maps to `to`'s LAST char.
                    out.push(*to_chars.last().unwrap());
                } else {
                    // Position-based: same index in `to`, or last
                    // char if `from` is longer than `to`.
                    let idx = idx_opt.unwrap();
                    if idx < to_chars.len() {
                        out.push(to_chars[idx]);
                    } else {
                        out.push(*to_chars.last().unwrap());
                    }
                }
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // `String#tr!` — destructive sibling of `tr`. Runs the
        // same translation logic but mutates the receiver in
        // place, returning self on change and nil when the
        // result matches the input. Forwards parse errors
        // (reversed range, set too large) as ArgumentError.
        (Value::Str(a), "tr!", [Value::Str(from), Value::Str(to)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let a_ref = a.to_string_lossy();
            let from_ref = from.to_string_lossy();
            let to_ref = to.to_string_lossy();
            let (from_chars, from_negated) = parse_tr_set(&from_ref, true).map_err(|msg| {
                RubyError::ArgumentError { msg: msg.to_string() }
            })?;
            let (to_chars, _) = parse_tr_set(&to_ref, false).map_err(|msg| {
                RubyError::ArgumentError { msg: msg.to_string() }
            })?;
            let mut from_index: std::collections::HashMap<char, usize> =
                std::collections::HashMap::with_capacity(from_chars.len());
            for (i, c) in from_chars.iter().enumerate() {
                from_index.insert(*c, i);
            }
            let mut out = String::with_capacity(a_ref.len());
            for ch in a_ref.chars() {
                let idx_opt = from_index.get(&ch).copied();
                let translate = if from_negated { idx_opt.is_none() } else { idx_opt.is_some() };
                if !translate { out.push(ch); continue; }
                if to_chars.is_empty() { continue; }
                if from_negated {
                    out.push(*to_chars.last().unwrap());
                } else {
                    let idx = idx_opt.unwrap();
                    if idx < to_chars.len() { out.push(to_chars[idx]); }
                    else { out.push(*to_chars.last().unwrap()); }
                }
            }
            let new_bytes = out.into_bytes();
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        // `String#squeeze` — collapse consecutive runs of the same
        // character. With a char-set arg, only chars in the set
        // are squeezed. Char-set selectors go through the shared
        // `parse_count_selector` (which delegates to `parse_tr_set`),
        // so range shorthand (`"a-z"`) and `^`-negation work and the
        // `TR_SET_MAX_CHARS` DoS cap applies. Membership XORs
        // against the negation flag.
        (Value::Str(a), "squeeze", rest) if rest.is_empty()
            || (rest.len() == 1 && matches!(rest[0], Value::Str(_))) => {
            let a_str = a.to_string_lossy();
            let parsed: Option<(std::collections::HashSet<char>, bool)> = match rest.first() {
                None => None,
                Some(Value::Str(s)) => {
                    let s_ref = s.to_string_lossy();
                    Some(parse_count_selector(&s_ref).map_err(|msg| {
                        RubyError::ArgumentError { msg: msg.to_string() }
                    })?)
                }
                _ => unreachable!(),
            };
            let mut out = String::with_capacity(a_str.len());
            let mut prev: Option<char> = None;
            for ch in a_str.chars() {
                let in_set = match &parsed {
                    Some((set, negated)) => set.contains(&ch) != *negated,
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
        // `String#squeeze!` — destructive sibling of `squeeze`.
        (Value::Str(a), "squeeze!", rest) if rest.is_empty()
            || (rest.len() == 1 && matches!(rest[0], Value::Str(_))) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {:?}", a.content.borrow()),
                });
            }
            let a_str = a.to_string_lossy();
            let parsed: Option<(std::collections::HashSet<char>, bool)> = match rest.first() {
                None => None,
                Some(Value::Str(s)) => {
                    let s_ref = s.to_string_lossy();
                    Some(parse_count_selector(&s_ref).map_err(|msg| {
                        RubyError::ArgumentError { msg: msg.to_string() }
                    })?)
                }
                _ => unreachable!(),
            };
            let mut out = String::with_capacity(a_str.len());
            let mut prev: Option<char> = None;
            for ch in a_str.chars() {
                let in_set = match &parsed {
                    Some((set, negated)) => set.contains(&ch) != *negated,
                    None => true,
                };
                if in_set && Some(ch) == prev { continue; }
                out.push(ch);
                prev = Some(ch);
            }
            let new_bytes = out.into_bytes();
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
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
        // `String#to_i(radix)` — same lenient parse as the no-arg
        // form but reading digits in the given base. Radix 0 means
        // "auto-detect" from a `0x`/`0o`/`0b`/`0d` prefix (CRuby).
        // Radix 2..=36 parses with that exact base (lowercase or
        // uppercase digits). Out-of-range raises ArgumentError to
        // match `Integer#to_s`'s shape. Mirrors the `Integer#to_s
        // (radix)` arm in `numeric.rs` so `to_s(r).to_i(r)` round-
        // trips for the supported range.
        (Value::Str(a), "to_i", [Value::Int(radix)]) => {
            let r = *radix;
            // CRuby accepts 0 (auto-detect) and 2..=36; anything
            // else raises ArgumentError.
            if r != 0 && !(2..=36).contains(&r) {
                return Err(RubyError::ArgumentError {
                    msg: format!("invalid radix {}", r),
                });
            }
            let a_ref = a.to_string_lossy();
            let s = a_ref.trim_start();
            let (sign, mut rest) = match s.as_bytes().first() {
                Some(b'-') => (-1i64, &s[1..]),
                Some(b'+') => (1i64, &s[1..]),
                _ => (1i64, s),
            };
            // Resolve the actual radix. Radix 0 inspects the
            // optional CRuby prefix; explicit radices skip the
            // prefix unless it matches (e.g. `to_i(16)` accepts
            // `"0xff"`, `to_i(2)` accepts `"0b1010"`).
            let mut effective_r: u32 = if r == 0 { 10 } else { r as u32 };
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[0] == b'0' {
                let (prefix_r, prefix_len) = match bytes[1] {
                    b'x' | b'X' => (16u32, 2),
                    b'b' | b'B' => (2u32, 2),
                    b'o' | b'O' => (8u32, 2),
                    b'd' | b'D' => (10u32, 2),
                    _ => (0u32, 0),
                };
                if prefix_r != 0 && (r == 0 || r as u32 == prefix_r) {
                    effective_r = prefix_r;
                    rest = &rest[prefix_len..];
                }
            }
            let mut n: i64 = 0;
            let mut saw_digit = false;
            for c in rest.chars() {
                if let Some(d) = c.to_digit(effective_r) {
                    saw_digit = true;
                    n = n.wrapping_mul(effective_r as i64)
                         .wrapping_add(d as i64);
                } else if c == '_' && saw_digit {
                    // CRuby tolerates `_` as a digit separator
                    // INSIDE a numeric literal (e.g. `"1_000".to_i`
                    // → 1000). Leading `_` is treated as garbage.
                    continue;
                } else {
                    break;
                }
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
        // `Regexp#freeze` / `frozen?` — Regexp values are immutable
        // by construction (no mutating instance methods exist), so
        // freezing has nothing to enforce. CRuby still defines the
        // method for compatibility, so user code's `/pat/.freeze`
        // (a common idiom in constant tables like sinatra/base.rb's
        // `HEADER_PARAM = /.../.freeze`) doesn't trip on a missing
        // method. Distinct from `String#freeze`, which rubyrs
        // implements with real frozen-flag tracking — strings have
        // mutators that need enforcement; regexes don't. Returns
        // the receiver to support chaining `/pat/.freeze.match?(s)`.
        // `frozen?` returns true to match the immutable surface.
        // Both arms surfaced as TRY_RUNS pass-7 layer #5 — closing
        // it lets sinatra/base.rb:32's `HEADER_PARAM` constant
        // initialiser execute.
        #[cfg(feature = "regex")]
        (Value::Regex(_), "freeze", []) => Some(recv.clone()),
        #[cfg(feature = "regex")]
        (Value::Regex(_), "frozen?", []) => Some(Value::Bool(true)),
        // Wrong-arity arms: CRuby's `Regexp#freeze` / `frozen?`
        // take zero args; any positional arg raises ArgumentError
        // with the standard "wrong number of arguments" shape.
        // Without these, the dispatcher would fall through to
        // NoMethodError ("undefined method 'freeze' for Regexp"),
        // diverging from CRuby on the error class.
        #[cfg(feature = "regex")]
        (Value::Regex(_), "freeze" | "frozen?", many) if !many.is_empty() => {
            return Err(RubyError::ArgumentError {
                msg: format!("wrong number of arguments (given {}, expected 0)", many.len()),
            });
        }
        // String#inspect — wrap in double quotes, escape `\`,
        // `"`, and common control characters. Matches CRuby for
        // printable ASCII + the standard escape set; exotic
        // Unicode escapes (`\u{...}`) are out of scope.
        (Value::Str(s), "inspect", []) => {
            let raw = s.to_string_lossy();
            let mut out = String::with_capacity(raw.len() + 2);
            out.push('"');
            crate::heap::inspect_escape_into(&raw, &mut out);
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
                // `+@` — unfreeze idiom. CRuby 3.x ALWAYS returns
                // a fresh non-frozen String (the older docs said
                // it returns the receiver when not frozen — the
                // actual behaviour, verified empirically against
                // CRuby 3.4, is to always dup). Drives the `+''`
                // literal-then-mutate pattern (used by ERB's
                // `lib/erb/compiler.rb:282` and countless other
                // gems that build content strings from frozen-
                // string-literal mode).
                if name == "+@" && args.is_empty() {
                    let copy = s.content.borrow().clone();
                    return Ok(Some(Value::new_str_bytes(copy)));
                }
                // `-@` — freeze idiom. CRuby: returns the receiver
                // when already frozen, otherwise a frozen dup. The
                // hash-table-dedupe optimisation CRuby applies is
                // out of scope for our subset (no frozen-string
                // table), but the observable contract — same value,
                // frozen state guaranteed — matches.
                if name == "-@" && args.is_empty() {
                    if s.frozen.get() {
                        return Ok(Some(Value::Str(s)));
                    }
                    let copy = s.content.borrow().clone();
                    let frozen = Value::new_str_bytes(copy);
                    if let Value::Str(ref ns) = frozen {
                        ns.frozen.set(true);
                    }
                    return Ok(Some(frozen));
                }
                // `String#dump` — round-trippable string literal
                // representation. Wraps in double quotes; escapes
                // CRuby's short controls (`\a` `\b` `\t` `\n` `\v`
                // `\f` `\r` `\e` `\"` `\\`); writes other control
                // bytes (0x00..=0x1F, 0x7F) as `\xNN` (uppercase);
                // escapes `#` ONLY when followed by `{` / `@` / `$`
                // (the interpolation triggers — round-trip parity);
                // non-ASCII codepoints become `\uHHHH` (BMP, fixed
                // 4 digits) or `\u{H...}` (above BMP, 5-6
                // uppercase hex digits — Unicode scalar values
                // top out at U+10FFFF).
                //
                // Motivating use: MRI lib/erb/compiler.rb:312
                // (`add_put_cmd`) writes template-content chunks
                // into the compiled source via
                // `"#{@put_cmd} #{content.dump}.freeze"`. Without
                // dump, ERB compile crashes inside compile_stag.
                if name == "dump" && args.is_empty() {
                    // Walk bytes directly so binary strings
                    // (Value::new_str_bytes, File.read with non-
                    // UTF-8 content, cext-allocated bytes) keep
                    // their invalid sequences as `\xNN` instead
                    // of being smudged to U+FFFD by a lossy decode.
                    // Output reconstructs the exact bytes via eval.
                    //
                    // Direct `push_str` / `write!` into the
                    // output String — no per-byte intermediate
                    // allocation. Projection check before each
                    // write traps ResourceExhausted before
                    // pathological 9x-expansion inputs balloon
                    // the buffer.
                    use std::fmt::Write;
                    let bytes = s.content.borrow();
                    let max_cap = self.max_value_bytes;
                    // Project current+add+closing-quote against
                    // cap. Macro avoids the `&mut self` borrow
                    // hassle of a closure that also wants to call
                    // `self.trap`.
                    macro_rules! ensure_room {
                        ($out:expr, $add:expr) => {
                            if let Some(max) = max_cap
                                && $out.len().saturating_add($add).saturating_add(1) > max {
                                return Err(self.trap(RubyError::ResourceExhausted {
                                    msg: format!("String#dump output exceeds {max} bytes"),
                                }));
                            }
                        };
                    }
                    // Pre-check the minimum 2-byte cap (opening
                    // and closing quotes) — even an empty input
                    // dumps to `""`. ensure_room reserves the
                    // closing quote on every push; the opening
                    // quote needs its own check.
                    let mut out = String::with_capacity(bytes.len().saturating_add(2));
                    ensure_room!(out, 1);
                    out.push('"');
                    let mut i = 0;
                    while i < bytes.len() {
                        let b = bytes[i];
                        if b < 0x80 {
                            let short: Option<&'static str> = match b {
                                0x07 => Some("\\a"),
                                0x08 => Some("\\b"),
                                0x09 => Some("\\t"),
                                0x0A => Some("\\n"),
                                0x0B => Some("\\v"),
                                0x0C => Some("\\f"),
                                0x0D => Some("\\r"),
                                0x1B => Some("\\e"),
                                b'"' => Some("\\\""),
                                b'\\' => Some("\\\\"),
                                _ => None,
                            };
                            if let Some(s) = short {
                                ensure_room!(out, s.len());
                                out.push_str(s);
                            } else if b == b'#' {
                                let next = bytes.get(i + 1).copied();
                                if matches!(next, Some(b'{') | Some(b'@') | Some(b'$')) {
                                    ensure_room!(out, 2);
                                    out.push_str("\\#");
                                } else {
                                    ensure_room!(out, 1);
                                    out.push('#');
                                }
                            } else if b < 0x20 || b == 0x7F {
                                ensure_room!(out, 4);
                                let _ = write!(out, "\\x{:02X}", b);
                            } else {
                                ensure_room!(out, 1);
                                out.push(b as char);
                            }
                            i += 1;
                        } else {
                            // Try to decode a 2-4 byte UTF-8
                            // sequence; fall back to \xNN for the
                            // leading byte on failure.
                            let max_seq = (bytes.len() - i).min(4);
                            let mut decoded: Option<(u32, usize)> = None;
                            for n in 2..=max_seq {
                                if let Ok(s) = std::str::from_utf8(&bytes[i..i + n])
                                    && let Some(c) = s.chars().next()
                                    && c.len_utf8() == n {
                                    decoded = Some((c as u32, n));
                                    break;
                                }
                            }
                            match decoded {
                                Some((cp, n)) if cp <= 0xFFFF => {
                                    ensure_room!(out, 6);
                                    let _ = write!(out, "\\u{:04X}", cp);
                                    i += n;
                                }
                                Some((cp, n)) => {
                                    // Output width is exact: 4
                                    // overhead bytes (\u{ and })
                                    // plus the hex-digit count. cp
                                    // is in 0x10000..=0x10FFFF here,
                                    // so 5 or 6 digits. Compute the
                                    // precise projection so a tight
                                    // max_value_bytes can't false-
                                    // trap codepoints that would
                                    // actually fit.
                                    let hex_digits = if cp <= 0xFFFFF { 5 } else { 6 };
                                    ensure_room!(out, 4 + hex_digits);
                                    let _ = write!(out, "\\u{{{:X}}}", cp);
                                    i += n;
                                }
                                None => {
                                    ensure_room!(out, 4);
                                    let _ = write!(out, "\\x{:02X}", b);
                                    i += 1;
                                }
                            }
                        }
                    }
                    // Closing quote reserved on every ensure_room
                    // check above — push unconditionally.
                    out.push('"');
                    return Ok(Some(Value::new_str(out)));
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
                            None => {
                                // CRuby parity: a failed `match`
                                // wipes the prior match's globals.
                                self.last_match = None;
                                return Ok(Some(Value::Nil));
                            }
                            Some(caps) => {
                                let m0 = caps.get(0).unwrap();
                                let (m_start, m_end) = (m0.start(), m0.end());
                                let whole = m0.as_str().to_string();
                                let mut group_vals: Vec<Value> = Vec::with_capacity(caps.len().saturating_sub(1));
                                let mut last_caps: Vec<Option<String>> = Vec::with_capacity(caps.len().saturating_sub(1));
                                for i in 1..caps.len() {
                                    let m = caps.get(i);
                                    last_caps.push(m.map(|m| m.as_str().to_string()));
                                    group_vals.push(match m {
                                        Some(m) => Value::new_str(m.as_str().to_string()),
                                        None => Value::Nil,
                                    });
                                }
                                drop(caps);
                                // Side-channel for `$~` / `$1`..`$N`
                                // (numbered) AND `$&` / `$+` / `` $` ``
                                // / `$'` (BackReferenceReadNode) — the
                                // input + span lets us derive
                                // pre/post-match without re-running
                                // the regex.
                                self.last_match = Some(crate::vm::LastMatch {
                                    whole: whole.clone(),
                                    caps: last_caps,
                                    input: bound,
                                    m_start,
                                    m_end,
                                });
                                return Ok(Some(self.materialize_match_data(whole, group_vals)?));
                            }
                        }
                    }
                    return Ok(None);
                }
                // String#[regex] / String#[regex, n] — Regex
                // overloads of `[]` / `slice`. Tilt uses
                // `script[/.../n, 1]` in `extract_magic_comment`
                // to pull the encoding name out of a magic
                // comment, so the (Regex, Int) form is the
                // motivating consumer. Returns the whole match
                // (1-arg) or the n-th capture group (2-arg with
                // n>=0, where 0 is the whole match), or nil if
                // there's no match / the requested group didn't
                // participate. Side-effect: updates `last_match`
                // (`$~`, `$1..$N`, `$&`, `$``, `$'`, `$+`) the
                // same way `String#match` does.
                #[cfg(feature = "regex")]
                if (name == "[]" || name == "slice") && args.len() == 1
                    && let Value::Regex(re) = &args[0]
                {
                    return Ok(Some(self.str_bracket_regex(&s, re, 0)?));
                }
                #[cfg(feature = "regex")]
                if (name == "[]" || name == "slice") && args.len() == 2
                    && let (Value::Regex(re), Value::Int(n)) = (&args[0], &args[1])
                {
                    return Ok(Some(self.str_bracket_regex(&s, re, *n)?));
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
                        let out = ruby_sprintf(&fmt_str, fmt_args, &self.heap, &self.interner, self.max_value_bytes)
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

    /// Shared backend for `String#[regex]` and `String#[regex, n]`.
    /// Returns the n-th capture group (0 = whole match) as a String,
    /// or Nil if the regex didn't match / the requested group didn't
    /// participate / `n` is out of range. Out-of-range parity with
    /// CRuby (which also returns nil — `String#[regex, n]` does NOT
    /// raise on out-of-range indices).
    ///
    /// Divergence: negative `n` (CRuby supports `-1` for "last
    /// group", `-2` for next-to-last, etc.) is not modeled — any
    /// `n < 0` falls through to the Nil branch instead of indexing
    /// from the end.
    ///
    /// Side-effect: mirrors `String#match` in updating `last_match`
    /// so `$~`, `$&`, `$1..$N`, `` $` ``, `$'`, `$+` stay correct.
    #[cfg(feature = "regex")]
    fn str_bracket_regex(
        &mut self,
        s: &std::rc::Rc<RStr>,
        re: &std::rc::Rc<regex::Regex>,
        n: i64,
    ) -> Result<Value, Trap> {
        let bound = s.to_string_lossy();
        let captures = re.captures(&bound);
        let caps = match captures {
            None => {
                self.last_match = None;
                return Ok(Value::Nil);
            }
            Some(c) => c,
        };
        let m0 = caps.get(0).unwrap();
        let (m_start, m_end) = (m0.start(), m0.end());
        let whole = m0.as_str().to_string();
        let mut last_caps: Vec<Option<String>> = Vec::with_capacity(caps.len().saturating_sub(1));
        for i in 1..caps.len() {
            last_caps.push(caps.get(i).map(|m| m.as_str().to_string()));
        }
        let picked = if n == 0 {
            Some(whole.clone())
        } else if n > 0 && (n as usize) <= last_caps.len() {
            last_caps[(n as usize) - 1].clone()
        } else {
            None
        };
        drop(caps);
        self.last_match = Some(crate::vm::LastMatch {
            whole,
            caps: last_caps,
            input: bound,
            m_start,
            m_end,
        });
        Ok(match picked {
            Some(s) => Value::new_str(s),
            None => Value::Nil,
        })
    }
}

/// Hard cap on the expanded char count for a single tr set.
/// Well above any legitimate human-written usage (full ASCII +
/// punctuation + a few BMP runs is < 1k chars; the full Unicode
/// range is ~1.1M codepoints). Bounds the intermediate Vec so
/// `tr("\u{0}-\u{10FFFF}", "*")` over untrusted input can't OOM
/// the host. Past the cap we raise ArgumentError rather than
/// silently truncate.
const TR_SET_MAX_CHARS: usize = 65_536;

/// Parse a `String#tr` / `#count` style selector into an
/// order-preserving (char-vec, negate) pair. Supports CRuby's
/// mini-syntax:
/// - leading `^` (first char only) → negate the set
/// - `a-z` → expand range inclusive (any `x-y` where `x <= y`,
///   including ranges whose endpoint is `^` like `A-^` — `^` is
///   only special as the leading-position negation prefix)
/// - reversed range (`z-a`) → ArgumentError (matches CRuby)
/// - everything else → literal char
///
/// CRuby's tr-syntax has a few finer corners (backslash escapes
/// inside the selector, octal forms) that real-world consumers
/// rarely hit; we omit those here. Add as motivating cases
/// appear.
///
/// Ordering matters for `tr`: it maps each source-set position
/// to the same dest-set position; using a HashSet would
/// collapse the ordering and break the index-based mapping.
/// `count` doesn't need ordering and `parse_count_selector`
/// delegates here and dedupes into a HashSet at no asymptotic
/// cost — keeping a single source of truth for both the mini-
/// syntax and the `TR_SET_MAX_CHARS` DoS cap.
///
/// `allow_negation` gates the leading-`^` interpretation:
/// `String#tr`'s `from_str` accepts negation but `to_str`
/// treats `^` as a literal char — so callers pass `true` for
/// the source set and `false` for the dest.
///
/// Returns `Err` with a CRuby-shaped message on range overflow
/// (`TR_SET_MAX_CHARS`).
pub(crate) fn parse_tr_set(sel: &str, allow_negation: bool) -> Result<(Vec<char>, bool), &'static str> {
    let mut chars: Vec<char> = sel.chars().collect();
    let negate = allow_negation && chars.first() == Some(&'^') && chars.len() > 1;
    if negate {
        chars.remove(0);
    }
    // Bound the initial capacity by `TR_SET_MAX_CHARS` so a
    // long selector (post-cap, before expansion) doesn't trigger
    // an oversized allocation. The output length is bounded by
    // the per-step length checks below.
    let initial_cap = chars.len().min(TR_SET_MAX_CHARS);
    let mut out: Vec<char> = Vec::with_capacity(initial_cap);
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let start = chars[i] as u32;
            let end = chars[i + 2] as u32;
            if start <= end {
                let span = (end - start + 1) as usize;
                if out.len().saturating_add(span) > TR_SET_MAX_CHARS {
                    return Err("invalid range in string transliteration (set too large)");
                }
                for cp in start..=end {
                    if let Some(c) = char::from_u32(cp) {
                        out.push(c);
                    }
                }
                i += 3;
                continue;
            }
            // Reversed range (e.g. `"c-a"`) — CRuby raises
            // ArgumentError "invalid range \"X-Y\" in string
            // transliteration". rubyrs previously silently
            // treated this as 3 literal chars, which diverged.
            return Err("invalid range in string transliteration");
        }
        if out.len() >= TR_SET_MAX_CHARS {
            return Err("invalid range in string transliteration (set too large)");
        }
        out.push(chars[i]);
        i += 1;
    }
    Ok((out, negate))
}

pub(crate) fn parse_count_selector(sel: &str) -> Result<(std::collections::HashSet<char>, bool), &'static str> {
    // Single source of truth for the mini-set syntax + the
    // TR_SET_MAX_CHARS DoS cap — delegate to parse_tr_set and
    // dedupe into a HashSet (count doesn't need ordering, but
    // tr does; the order-preserving Vec just gets collapsed
    // here at no asymptotic cost).
    let (chars, negate) = parse_tr_set(sel, true)?;
    Ok((chars.into_iter().collect(), negate))
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
    // Collapse (directive, endian) pairs to a single internal
    // sentinel character the engine arms then match on. The
    // unsigned forms (`L`/`S`/`Q`) alias to the existing
    // unsigned-endian directives (`N`/`V`/`n`/`v`/`J`/`Q`).
    // The signed forms (`l`/`s`) and the older `q` use
    // dedicated sentinels — see the helper-comment table below.
    //
    // Native endian: rubyrs targets aarch64-apple-darwin (LE)
    // and x86_64-linux (LE) — both little-endian — so omitting
    // the modifier resolves to LE. CRuby itself returns
    // platform-native here; matching the CI host's behaviour is
    // sufficient for the Tier 1 protocol-compat scope (binary
    // gem formats fix BE/LE explicitly with `>` / `<` anyway).
    let dir = match (dir_raw, endian) {
        // Unsigned 32-bit.
        ('L', Some('>')) => 'N',
        ('L', Some('<')) => 'V',
        ('L', None)      => 'V',
        // Signed 32-bit. Uppercase sentinel = BE, lowercase = LE
        // (`T` and `t` are free — CRuby doesn't use them).
        ('l', Some('>')) => 'T',
        ('l', Some('<')) => 't',
        ('l', None)      => 't',
        // Unsigned 16-bit.
        ('S', Some('>')) => 'n',
        ('S', Some('<')) => 'v',
        ('S', None)      => 'v',
        // Signed 16-bit. `K` / `k` are free.
        ('s', Some('>')) => 'K',
        ('s', Some('<')) => 'k',
        ('s', None)      => 'k',
        // 64-bit (existing).
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
            'n' | 'v' | 'K' | 'k' => {
                // n/v: unsigned 16-bit BE/LE. K/k: signed 16-bit
                // BE/LE (from `s>`/`s<`/`s` — see parse_directive
                // sentinel table).
                let take = if n == usize::MAX { (input.len() - i) / 2 } else { n };
                for _ in 0..take {
                    if i + 2 > input.len() { out.push(Value::Nil); break; }
                    let bytes = [input[i], input[i+1]];
                    let v: i64 = match dir {
                        'n' => u16::from_be_bytes(bytes) as i64,
                        'v' => u16::from_le_bytes(bytes) as i64,
                        'K' => i16::from_be_bytes(bytes) as i64,
                        'k' => i16::from_le_bytes(bytes) as i64,
                        _ => unreachable!(),
                    };
                    i += 2;
                    out.push(Value::Int(v));
                }
            }
            'N' | 'V' | 'T' | 't' => {
                // N/V: unsigned 32-bit BE/LE. T/t: signed 32-bit
                // BE/LE (from `l>`/`l<`/`l`).
                let take = if n == usize::MAX { (input.len() - i) / 4 } else { n };
                for _ in 0..take {
                    if i + 4 > input.len() { out.push(Value::Nil); break; }
                    let bytes = [input[i], input[i+1], input[i+2], input[i+3]];
                    let v: i64 = match dir {
                        'N' => u32::from_be_bytes(bytes) as i64,
                        'V' => u32::from_le_bytes(bytes) as i64,
                        'T' => i32::from_be_bytes(bytes) as i64,
                        't' => i32::from_le_bytes(bytes) as i64,
                        _ => unreachable!(),
                    };
                    i += 4;
                    out.push(Value::Int(v));
                }
            }
            // `H*` / `H<n>` — hex string, high nibble first (e.g.
            // `"\xAB\xCD".unpack1("H*")` → `"abcd"`). `h` is the
            // low-nibble-first variant. CRuby's count semantics:
            // the count is in *nibbles*, not bytes; `*` consumes
            // all bytes. Output is one String (multi-byte input
            // produces a single hex String, NOT one per byte).
            'H' | 'h' => {
                let avail_bytes = input.len() - i;
                let nibble_count = if n == usize::MAX { avail_bytes * 2 } else { n };
                let byte_count = nibble_count.div_ceil(2).min(avail_bytes);
                let mut hex = String::with_capacity(nibble_count);
                let mut written = 0usize;
                for b in &input[i..i + byte_count] {
                    let (hi, lo) = if dir == 'H' { (b >> 4, b & 0xF) } else { (b & 0xF, b >> 4) };
                    if written < nibble_count {
                        hex.push(char::from_digit(hi as u32, 16).unwrap());
                        written += 1;
                    }
                    if written < nibble_count {
                        hex.push(char::from_digit(lo as u32, 16).unwrap());
                        written += 1;
                    }
                }
                i += byte_count;
                out.push(Value::new_str(hex));
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
            'n' | 'v' | 'K' | 'k' => {
                // K/k: signed 16-bit BE/LE — same bit pattern as
                // the unsigned versions for the same i64
                // narrowing (low-16 bits), so the only difference
                // would surface on the unpack side. Kept distinct
                // here for symmetry with unpack_bytes.
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let i = match v {
                        Value::Int(n) => n,
                        _ => return Err("pack: expected Integer for n/v/s/S".into()),
                    };
                    let bytes: [u8; 2] = match dir {
                        'n' | 'K' => (i as u16).to_be_bytes(),
                        'v' | 'k' => (i as u16).to_le_bytes(),
                        _ => unreachable!(),
                    };
                    out.extend_from_slice(&bytes);
                }
            }
            'N' | 'V' | 'T' | 't' => {
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let i = match v {
                        Value::Int(n) => n,
                        _ => return Err("pack: expected Integer for N/V/l/L".into()),
                    };
                    let bytes: [u8; 4] = match dir {
                        'N' | 'T' => (i as u32).to_be_bytes(),
                        'V' | 't' => (i as u32).to_le_bytes(),
                        _ => unreachable!(),
                    };
                    out.extend_from_slice(&bytes);
                }
            }
            // `H*` / `H<n>` — hex string, high nibble first.
            // CRuby ignores non-hex characters silently (treated
            // as 0). Trailing nibble on odd-length string left-
            // shifts (high nibble = the lone digit). `*` packs
            // every nibble; explicit count truncates.
            'H' | 'h' => {
                let v = values.get(vi).cloned().unwrap_or(Value::new_str(""));
                vi += 1;
                let s = match v {
                    Value::Str(s) => s.to_string_lossy(),
                    _ => return Err("pack: expected String for H/h".into()),
                };
                let nibbles: Vec<u8> = s
                    .chars()
                    .map(|c| c.to_digit(16).unwrap_or(0) as u8)
                    .collect();
                let want = if n == usize::MAX { nibbles.len() } else { n.min(nibbles.len()) };
                let mut byte_idx = 0;
                while byte_idx * 2 < want {
                    let hi = nibbles[byte_idx * 2];
                    let lo = if byte_idx * 2 + 1 < want { nibbles[byte_idx * 2 + 1] } else { 0 };
                    let packed = if dir == 'H' { (hi << 4) | lo } else { (lo << 4) | hi };
                    out.push(packed);
                    byte_idx += 1;
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
