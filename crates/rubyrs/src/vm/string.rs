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
/// Ruby `String#strip`'s trim set, byte-level. The set is pure
/// ASCII, and ASCII bytes never appear inside a UTF-8 multi-byte
/// sequence — so byte-level trimming is exactly equivalent for
/// valid UTF-8 AND stops mangling binary content (the old
/// `to_string_lossy` route rewrote invalid sequences to U+FFFD
/// bytes in the result). E1 slice 3.
fn strip_b(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | 0)
}

fn trim_bytes(bytes: &[u8], start: bool, end: bool) -> &[u8] {
    let mut lo = 0;
    let mut hi = bytes.len();
    if start {
        while lo < hi && strip_b(bytes[lo]) {
            lo += 1;
        }
    }
    if end {
        while hi > lo && strip_b(bytes[hi - 1]) {
            hi -= 1;
        }
    }
    &bytes[lo..hi]
}

/// Copy `tag` onto a fresh `Value::Str` — the one-liner the
/// byte-preserving ops use to propagate the receiver's encoding.
fn with_tag(v: Value, tag: crate::value::EncodingTag) -> Value {
    if let Value::Str(ref ns) = v {
        ns.encoding.set(tag);
    }
    v
}

/// `String#lines` / `#each_line` splitting: break `src` at each `sep`,
/// KEEPING the separator on the end of every piece — `"a\nb\nc".lines`
/// → `["a\n", "b\n", "c"]`. A trailing separator yields no empty tail
/// piece (`"a\n".lines` → `["a\n"]`); an empty string yields `[]`.
/// An empty separator (CRuby paragraph mode is out of scope) returns
/// the whole string as a single piece.
pub(crate) fn split_lines_keep_sep(src: &str, sep: &str) -> Vec<String> {
    if sep.is_empty() {
        return if src.is_empty() { Vec::new() } else { vec![src.to_string()] };
    }
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(pos) = rest.find(sep) {
        let end = pos + sep.len();
        out.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// `String#chomp` (no arg) — strip exactly one trailing record
/// separator. CRuby tries `\r\n` first (so the EOL pair is
/// removed atomically), then bare `\n`, then bare `\r`.
fn chomp_default(bytes: &[u8]) -> Vec<u8> {
    bytes[..chomp_default_keep_len(bytes)].to_vec()
}

/// `String#chop` keep-length: drop a trailing `\r\n` pair, else the last
/// CHARACTER. For UTF-8 that means backing over continuation bytes to the
/// last char's lead byte; Binary / US-ASCII / other encodings chop one
/// byte (full multibyte-width chop for `Other` is ADR 0020 Tier-2
/// territory). Empty string keeps 0.
fn chop_keep_len(bytes: &[u8], tag: crate::value::EncodingTag) -> usize {
    let n = bytes.len();
    if n == 0 {
        return 0;
    }
    if bytes.ends_with(b"\r\n") {
        return n - 2;
    }
    if matches!(tag, crate::value::EncodingTag::Utf8) {
        let mut i = n - 1;
        while i > 0 && (bytes[i] & 0xC0) == 0x80 {
            i -= 1;
        }
        i
    } else {
        n - 1
    }
}

/// Allocation-free sibling of `chomp_default`: returns the
/// number of leading bytes to keep. Used by `chomp!` to avoid
/// allocating a fresh `Vec<u8>` for the common no-change case.
/// (Copilot review #298 round 2.)
fn chomp_default_keep_len(bytes: &[u8]) -> usize {
    if bytes.ends_with(b"\r\n") {
        bytes.len() - 2
    } else if bytes.ends_with(b"\n") || bytes.ends_with(b"\r") {
        bytes.len() - 1
    } else {
        bytes.len()
    }
}

/// `String#chomp(sep)` with an explicit String separator.
/// Special-cases the `"\n"` argument: CRuby treats it as the
/// "universal record separator" and atomically eats a trailing
/// `"\r\n"` pair, then any bare `"\n"` / `"\r"`. Other separators
/// are matched as an exact suffix only.
fn chomp_with_sep(bytes: &[u8], sep: &[u8]) -> Vec<u8> {
    bytes[..chomp_with_sep_keep_len(bytes, sep)].to_vec()
}

/// Allocation-free sibling of `chomp_with_sep`: returns the
/// number of leading bytes to keep. (Copilot review #298
/// round 2.)
fn chomp_with_sep_keep_len(bytes: &[u8], sep: &[u8]) -> usize {
    if sep.is_empty() {
        chomp_paragraph_keep_len(bytes)
    } else if sep == b"\n" {
        chomp_default_keep_len(bytes)
    } else if bytes.ends_with(sep) {
        bytes.len() - sep.len()
    } else {
        bytes.len()
    }
}

/// `String#chomp("")` paragraph mode — strip ALL trailing
/// `\n` / `\r\n` sequences. CRuby's `$/ = ""` (paragraph
/// record separator) semantics applied on demand.
/// Returns the number of leading bytes to keep so callers
/// can decide whether to allocate. (Copilot review #298
/// round 2.)
fn chomp_paragraph_keep_len(bytes: &[u8]) -> usize {
    let mut end = bytes.len();
    loop {
        if end >= 2 && &bytes[end - 2..end] == b"\r\n" {
            end -= 2;
        } else if end >= 1 && bytes[end - 1] == b'\n' {
            end -= 1;
        } else {
            break;
        }
    }
    end
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
/// CRuby's case methods (`upcase`/`downcase`/`capitalize`/`swapcase`
/// and their `!` forms) raise `ArgumentError: input string invalid`
/// when the receiver's bytes are invalid for its encoding — e.g. a
/// UTF-8 string holding a stray `\xFF`. rack's `MethodOverride` leans
/// on this: it does `method.to_s.upcase rescue ArgumentError` and
/// writes "Invalid string for method" to `rack.errors`. Binary
/// (ASCII-8BIT) strings are byte soup and always valid, so they pass
/// through (the case arms convert ASCII letters byte-wise for those).
/// A no-op for the common case (valid strings), so this only adds the
/// missing raise without touching successful conversions.
fn case_validity_guard(a: &crate::value::RStr) -> Result<(), RubyError> {
    use crate::value::EncodingTag;
    let b = a.content.borrow();
    let valid = match a.encoding.get() {
        EncodingTag::Binary => true,
        EncodingTag::Utf8 => std::str::from_utf8(&b).is_ok(),
        EncodingTag::UsAscii => b.iter().all(|&x| x < 0x80),
        #[cfg(feature = "_encoding_full")]
        EncodingTag::Other(idx) => crate::encoding_full::valid(idx, &b),
        #[cfg(not(feature = "_encoding_full"))]
        EncodingTag::Other(_) => true,
    };
    if valid {
        Ok(())
    } else {
        Err(RubyError::ArgumentError {
            msg: "input string invalid".into(),
        })
    }
}

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
/// `sub` / `sub!`. The destructive arm gates nil-vs-self on
/// match presence (`pat.is_empty() || a.contains(pat)`) at
/// the call site — this helper just produces the rewritten
/// string and is oblivious to the destructive contract.
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
        // Avoid `chars().count()` for an exact pre-allocation
        // (a full extra O(n) pass over the receiver). A
        // length-byte upper-bound is `repl.len() * (a.len() + 1)
        // + a.len()`, but that over-allocates wildly when `a` is
        // long and `repl` is small. Pick a cheap initial
        // capacity equal to `a.len() + repl.len()` instead — the
        // String will grow at most a handful of times on the
        // common short-repl path. Trade O(n) precount for at
        // most log₂(n) reallocs.
        let mut s = String::with_capacity(a.len() + repl.len());
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
            // Two guards before `extend_from_slice` (same shape
            // as String#* below): (1) `checked_add` for usize
            // overflow, (2) `> isize::MAX` for the Vec capacity
            // ceiling. Both raise CRuby-byte-identical
            // `ArgumentError "argument too big"`. Without these,
            // when `max_value_bytes` is None (no cap) two
            // near-isize::MAX strings panic the host VM at
            // `extend_from_slice`'s capacity-overflow assert.
            let new_len = a.borrow().len().checked_add(b.borrow().len()).filter(|&n| n <= isize::MAX as usize).ok_or_else(|| {
                RubyError::ArgumentError { msg: "argument too big".to_string() }
            })?;
            check(new_len)?;
            // E1 slice 2: encoding compatibility decides the result
            // tag; incompatible operands raise CRuby's
            // CompatibilityError (string_call can't trap with a
            // custom class — signal via the RubyError and let the
            // caller's HostException mapping carry the name).
            let tag = crate::value::enc_compat(
                a.encoding.get(), &a.content.borrow(),
                b.encoding.get(), &b.content.borrow(),
            )
            .ok_or_else(|| RubyError::HostException {
                class_name: "Encoding::CompatibilityError".to_string(),
                message: format!(
                    "incompatible character encodings: {} and {}",
                    a.encoding.get().display(),
                    b.encoding.get().display()
                ),
            })?;
            let mut s = a.borrow().clone();
            s.extend_from_slice(&b.borrow());
            let v = Value::new_str_bytes(s);
            if let Value::Str(ref ns) = v {
                ns.encoding.set(tag);
            }
            Some(v)
        }
        // E1 slice 2: tag-compatible equality — equal bytes share
        // ascii-only-ness, so cross-tag equality holds iff the bytes
        // are pure ASCII (same rule as heap.rs's ruby_eq arm).
        (Value::Str(a), "==", [Value::Str(b)]) => {
            let ab = a.content.borrow();
            let bb = b.content.borrow();
            let eq = *ab == *bb
                && (a.encoding.get() == b.encoding.get()
                    || ab.iter().all(|&x| x < 0x80));
            Some(Value::Bool(eq))
        }
        (Value::Str(a), "!=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() != *b.borrow())),
        (Value::Str(a), "to_s", []) => Some(Value::Str(a.clone())),
        // `String#to_str` — explicit-conversion alias. CRuby uses
        // `to_str` for "I really am a String"-style implicit coercion
        // checks (`respond_to?(:to_str)` is the duck-type probe lots
        // of gems use to distinguish String from Symbol / Regexp).
        // For our subset it's identical to `to_s` on a real String.
        (Value::Str(a), "to_str", []) => Some(Value::Str(a.clone())),
        // `String#chr` — returns a one-character string at the beginning of the string.
        (Value::Str(a), "chr", []) => {
            if a.encoding.get() == crate::value::EncodingTag::Binary {
                let bytes = a.content.borrow();
                if bytes.is_empty() {
                    Some(Value::new_str(String::new()))
                } else {
                    Some(Value::new_str_bytes_binary(vec![bytes[0]]))
                }
            } else {
                let s = a.to_string_lossy();
                let mut chars = s.chars();
                if let Some(ch) = chars.next() {
                    Some(Value::new_str(ch.to_string()))
                } else {
                    Some(Value::new_str(String::new()))
                }
            }
        }
        // PR #53 review #1: `length`/`size` return UTF-8 character
        // count (lossy on invalid UTF-8 — non-UTF-8 bytes count as
        // one U+FFFD char each). Matches CRuby's "length on a
        // UTF-8-encoded String" behavior. For raw byte count, use
        // `bytesize` (added below); for binary protocol gems the
        // bytesize semantic is the meaningful one.
        (Value::Str(a), "length", []) | (Value::Str(a), "size", []) => {
            // Registry encodings count per THAT encoding's units
            // (multi-byte aware via the registry; broken sequences
            // fall back to byte length, CRuby's lenient shape).
            #[cfg(feature = "_encoding_full")]
            if let crate::value::EncodingTag::Other(idx) = a.encoding.get() {
                let b = a.content.borrow();
                let n = crate::encoding_full::char_count(idx, &b).unwrap_or(b.len());
                return Ok(Some(Value::Int(n as i64)));
            }
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
            let b = a.borrow();
            b.hash(&mut h);
            // E1 slice 2: same tag-sensitivity rule as the internal
            // ruby_hash — non-ASCII content folds the encoding in,
            // so ==-unequal cross-encoding strings hash apart while
            // ASCII content stays encoding-blind.
            if !b.iter().all(|&x| x < 0x80) {
                let tag = match a.encoding.get() {
                    crate::value::EncodingTag::Binary => 0u8,
                    crate::value::EncodingTag::Utf8 => 1,
                    crate::value::EncodingTag::UsAscii => 2,
                    crate::value::EncodingTag::Other(n) => 3u8.wrapping_add(n),
                };
                tag.hash(&mut h);
            }
            Some(Value::Int(h.finish() as i64))
        }
        (Value::Str(a), "bytesize", []) => Some(Value::Int(a.borrow().len() as i64)),
        // `byteindex(substr[, byte_offset])` — Ruby 3.2's byte-level
        // index (substring form; the regexp form is out of subset).
        // Powers the File-veneer's byte-based line splitting, which
        // must not run a registry/BINARY-tagged buffer through the
        // lossy char view.
        (Value::Str(a), "byteindex", [Value::Str(n)])
        | (Value::Str(a), "byteindex", [Value::Str(n), Value::Int(_)]) => {
            let hay = a.borrow();
            let needle = n.borrow();
            let start = match args.get(1) {
                Some(Value::Int(s)) if *s >= 0 => (*s as usize).min(hay.len()),
                Some(Value::Int(_)) => return Ok(Some(Value::Nil)),
                _ => 0,
            };
            if needle.is_empty() {
                return Ok(Some(Value::Int(start as i64)));
            }
            let found = hay[start..]
                .windows(needle.len())
                .position(|w| w == &needle[..])
                .map(|i| (start + i) as i64);
            Some(found.map(Value::Int).unwrap_or(Value::Nil))
        }
        // `String#byteslice(start[, len])` — byte-level substring,
        // encoding preserved (CRuby contract; the result may be
        // broken in that encoding — that's the caller's business,
        // mirrored by valid_encoding?). Negative start counts from
        // the end; out-of-range → nil.
        (Value::Str(a), "byteslice", [Value::Int(st)])
        | (Value::Str(a), "byteslice", [Value::Int(st), _]) => {
            let b = a.borrow();
            let len_total = b.len() as i64;
            let start = if *st < 0 { len_total + *st } else { *st };
            if start < 0 || start > len_total {
                return Ok(Some(Value::Nil));
            }
            let take = match args.get(1) {
                // One-arg form returns a SINGLE byte (CRuby), not
                // the rest of the string — and nil at the very end
                // (start == bytesize is only valid for the two-arg
                // form, where it yields "").
                None => {
                    if start >= len_total {
                        return Ok(Some(Value::Nil));
                    }
                    1
                }
                Some(Value::Int(n)) => {
                    if *n < 0 {
                        return Ok(Some(Value::Nil));
                    }
                    *n
                }
                Some(other) => {
                    return Err(RubyError::TypeError {
                        msg: format!(
                            "no implicit conversion of {} into Integer",
                            other.type_name()
                        ),
                    });
                }
            };
            let start = start as usize;
            let end = (start + take as usize).min(b.len());
            Some(with_tag(
                Value::new_str_bytes(b[start..end].to_vec()),
                a.encoding.get(),
            ))
        }
        (Value::Str(a), "empty?", []) => Some(Value::Bool(a.borrow().is_empty())),
        // `String#ascii_only?` — byte-level (Tier 1 strings are
        // byte-oriented, so the byte scan equals CRuby's encoding-aware
        // answer for the supported encodings). Uses the cached ASCII
        // flag (`is_ascii_cached`): O(n) once, O(1) after, invalidated
        // on any content write. The previous pure-Ruby
        // `each_byte { ... }` was O(n) per CALL and uncached — kramdown's
        // `current_line_number` rebuilds a full-string StringScanner per
        // element (its `refresh_byte_addressable` calls `ascii_only?`),
        // so an uncached scan turned a document parse O(n²).
        (Value::Str(a), "ascii_only?", []) => Some(Value::Bool(a.content.is_ascii_cached())),
        (Value::Str(a), "upcase", []) => {
            case_validity_guard(a)?;
            // Registry-tagged strings get real per-encoding case
            // mapping (decode → Unicode case → re-encode, original
            // bytes kept for unmappables); the lossy route below
            // would U+FFFD-mangle them AND drop the tag.
            #[cfg(feature = "_encoding_full")]
            if let crate::value::EncodingTag::Other(idx) = a.encoding.get()
                && let Some(out) = crate::encoding_full::case_other(
                    idx, &a.content.borrow(), crate::encoding_full::CaseMode::Up)
            {
                return Ok(Some(with_tag(Value::new_str_bytes(out), a.encoding.get())));
            }
            Some(Value::new_str(a.to_string_lossy().to_uppercase()))
        }
        (Value::Str(a), "downcase", []) => {
            case_validity_guard(a)?;
            #[cfg(feature = "_encoding_full")]
            if let crate::value::EncodingTag::Other(idx) = a.encoding.get()
                && let Some(out) = crate::encoding_full::case_other(
                    idx, &a.content.borrow(), crate::encoding_full::CaseMode::Down)
            {
                return Ok(Some(with_tag(Value::new_str_bytes(out), a.encoding.get())));
            }
            Some(Value::new_str(a.to_string_lossy().to_lowercase()))
        }
        (Value::Str(a), "reverse", []) => Some(Value::new_str(a.to_string_lossy().chars().rev().collect::<String>())),
        // `String#capitalize` — first char uppercase, rest
        // lowercase. ASCII-only fold (Unicode options out of
        // subset). Empty string is a no-op. First non-letter
        // (digit / punctuation) stays as-is.
        (Value::Str(a), "capitalize", []) => {
            case_validity_guard(a)?;
            #[cfg(feature = "_encoding_full")]
            if let crate::value::EncodingTag::Other(idx) = a.encoding.get()
                && let Some(out) = crate::encoding_full::case_other(
                    idx, &a.content.borrow(), crate::encoding_full::CaseMode::Capitalize)
            {
                return Ok(Some(with_tag(Value::new_str_bytes(out), a.encoding.get())));
            }
            Some(Value::new_str(a.with_str_lossy(capitalize_ascii)))
        }
        // `String#swapcase` — every letter has its case
        // flipped; non-letters pass through.
        (Value::Str(a), "swapcase", []) => {
            case_validity_guard(a)?;
            #[cfg(feature = "_encoding_full")]
            if let crate::value::EncodingTag::Other(idx) = a.encoding.get()
                && let Some(out) = crate::encoding_full::case_other(
                    idx, &a.content.borrow(), crate::encoding_full::CaseMode::Swap)
            {
                return Ok(Some(with_tag(Value::new_str_bytes(out), a.encoding.get())));
            }
            Some(Value::new_str(a.with_str_lossy(swapcase_ascii)))
        }
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            case_validity_guard(a)?;
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            case_validity_guard(a)?;
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            case_validity_guard(a)?;
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            case_validity_guard(a)?;
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
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
        // `succ!` / `next!` — in-place successor, returns self. Tilt
        // generates unique compiled-method names by `succ!`-ing a
        // counter String (template.rb's compiled_method_name).
        (Value::Str(a), "succ!", []) | (Value::Str(a), "next!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let next = a.with_str_lossy(str_succ).into_bytes();
            check(next.len())?;
            *a.borrow_mut() = next;
            Some(Value::Str(a.clone()))
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
        // `String#encode` / `#force_encoding` are intercepted in
        // dispatch.rs (try_string_encoding_ops) — resolving the
        // encoding argument may need to read a preamble Encoding
        // instance's ivars, which this free-function context
        // can't reach. (E1: real tag semantics — see ADR 0020.)
        //
        // `String#valid_encoding?` — E1: judged against the TAG.
        // Binary (ASCII-8BIT) accepts any bytes; UTF-8 demands
        // well-formed UTF-8; US-ASCII demands all bytes < 0x80.
        (Value::Str(a), "valid_encoding?", []) => {
            use crate::value::EncodingTag;
            let b = a.content.borrow();
            let ok = match a.encoding.get() {
                EncodingTag::Binary => true,
                EncodingTag::Utf8 => std::str::from_utf8(&b).is_ok(),
                EncodingTag::UsAscii => b.iter().all(|&x| x < 0x80),
                #[cfg(feature = "_encoding_full")]
                EncodingTag::Other(idx) => crate::encoding_full::valid(idx, &b),
                #[cfg(not(feature = "_encoding_full"))]
                EncodingTag::Other(_) => true,
            };
            Some(Value::Bool(ok))
        }
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
        (Value::Str(a), "b", []) => Some(Value::new_str_bytes_binary(a.content.borrow().clone())),
        // CRuby's strip family treats `\x00` as part of the
        // strippable whitespace set (along with space, tab, NL,
        // CR, FF, VT). Rust's `is_whitespace()` excludes NUL,
        // so a bare `.trim()` would leave NUL bytes on the ends
        // — a divergence pinned in
        // `tests/fixtures/divergence_string_strip_nul.rb` (PR
        // #193) until this fix. Use a predicate that matches
        // CRuby's set exactly.
        (Value::Str(a), "strip", []) => {
            let b = a.borrow();
            Some(with_tag(Value::new_str_bytes(trim_bytes(&b, true, true).to_vec()), a.encoding.get()))
        }
        (Value::Str(a), "lstrip", []) => {
            let b = a.borrow();
            Some(with_tag(Value::new_str_bytes(trim_bytes(&b, true, false).to_vec()), a.encoding.get()))
        }
        (Value::Str(a), "rstrip", []) => {
            let b = a.borrow();
            Some(with_tag(Value::new_str_bytes(trim_bytes(&b, false, true).to_vec()), a.encoding.get()))
        }
        // Destructive strip siblings — return self on change,
        // nil otherwise. The frozen check + check() guard mirror
        // the other `!` variants in this file.
        (Value::Str(a), "strip!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let new_bytes = {
                let b = a.borrow();
                trim_bytes(&b, true, true).to_vec()
            };
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let new_bytes = {
                let b = a.borrow();
                trim_bytes(&b, true, false).to_vec()
            };
            if *a.borrow() == new_bytes { Some(Value::Nil) }
            else {
                check(new_bytes.len())?;
                *a.borrow_mut() = new_bytes;
                Some(Value::Str(a.clone()))
            }
        }
        // `String#chomp` — strip ONE trailing record separator.
        // CRuby semantics:
        //   - no arg: strip a single trailing "\r\n", "\n", or
        //     "\r" (whichever matches; "\r\n" preferred over
        //     "\n" so the EOL pair is removed atomically).
        //   - "" arg: strip ALL trailing "\n" / "\r\n" sequences
        //     ("paragraph mode" — CRuby `$/` set to "").
        //   - String suffix arg: strip exactly that suffix iff
        //     the receiver ends with it.
        //   - nil arg: returns the receiver unchanged.
        // tilt-2.7.0 `StringTemplate#prepare` embeds a literal
        // ".chomp" call in the heredoc-wrapped source it evals
        // at render time; the missing method blocked rendering.
        // (TRY_RUNS pass-10 layer #7.)
        (Value::Str(a), "chomp", args) => {
            let bytes = a.borrow();
            let trimmed: Vec<u8> = match args {
                [] => chomp_default(&bytes),
                [Value::Nil] => bytes.clone(),
                [Value::Str(sep)] => chomp_with_sep(&bytes, &sep.borrow()),
                [other] => return Err(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", other.type_name()),
                }),
                _ => return Err(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0..1)", args.len()),
                }),
            };
            Some(with_tag(Value::new_str_bytes(trimmed), a.encoding.get()))
        }
        (Value::Str(a), "chomp!", args) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            // Validate arg shape BEFORE consulting the receiver
            // so a non-String/non-nil arg raises TypeError even
            // when the receiver is empty and short-circuits would
            // return early. (Copilot review #298 round 1.)
            match args {
                [] | [Value::Nil] | [Value::Str(_)] => {}
                [other] => return Err(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", other.type_name()),
                }),
                _ => return Err(RubyError::ArgumentError {
                    msg: format!("wrong number of arguments (given {}, expected 0..1)", args.len()),
                }),
            }
            // Cheap no-change detection: read the keep-length
            // without allocating. Returns nil on no-op (the
            // common case), only truncates in place when there's
            // actually a separator to strip. (Copilot review
            // #298 round 2.)
            let keep_len = {
                let bytes = a.borrow();
                match args {
                    [] => chomp_default_keep_len(&bytes),
                    [Value::Nil] => return Ok(Some(Value::Nil)),
                    [Value::Str(sep)] => chomp_with_sep_keep_len(&bytes, &sep.borrow()),
                    _ => unreachable!(),
                }
            };
            if keep_len == a.borrow().len() {
                Some(Value::Nil)
            } else {
                a.borrow_mut().truncate(keep_len);
                Some(Value::Str(a.clone()))
            }
        }
        (Value::Str(a), "chop", []) => {
            let bytes = a.borrow();
            let keep = chop_keep_len(&bytes, a.encoding.get());
            Some(with_tag(Value::new_str_bytes(bytes[..keep].to_vec()), a.encoding.get()))
        }
        (Value::Str(a), "rstrip!", []) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let new_bytes = {
                let b = a.borrow();
                trim_bytes(&b, false, true).to_vec()
            };
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
            // Known-valid-UTF-8 fast path: borrowed view, no per-call
            // O(n) lossy validation.
            if let Some(m) = is_match_at_char_pos(a, 0, re) {
                return Ok(Some(Value::Bool(
                    m.map_err(|e| e.to_ruby_error(re.as_str()))?,
                )));
            }
            // BINARY subjects match byte-wise (CRuby ASCII-8BIT); fall
            // back to the UTF-8 engine when there's no byte engine.
            let matched = if matches!(a.encoding.get(), crate::value::EncodingTag::Binary) {
                match re.is_match_bytes(&a.content.borrow()) {
                    Some(m) => m,
                    None => a.with_str_lossy(|s| re.is_match_from(s))
                        .map_err(|e| e.to_ruby_error(re.as_str()))?,
                }
            } else {
                a.with_str_lossy(|s| re.is_match_from(s))
                    .map_err(|e| e.to_ruby_error(re.as_str()))?
            };
            Some(Value::Bool(matched))
        }
        // `String#match?(re, pos)` — predicate match starting at char
        // offset `pos` (negative counts from the end); out-of-range → false.
        // Honours a `\G` anchor (match exactly at `pos`). No `$~` update.
        #[cfg(feature = "regex")]
        (Value::Str(a), "match?", [Value::Regex(re), Value::Int(pos)]) => {
            // Fast path (valid UTF-8, non-BINARY): O(1) pos resolve +
            // borrowed view — the lossy path below copies the subject
            // and walks its chars per call (13µs on a 21KB buffer).
            if let Some(m) = is_match_at_char_pos(a, *pos, re) {
                return Ok(Some(Value::Bool(
                    m.map_err(|e| e.to_ruby_error(re.as_str()))?,
                )));
            }
            let lossy = a.to_string_lossy();
            let char_len = lossy.chars().count() as i64;
            let cpos = if *pos < 0 { char_len + *pos } else { *pos };
            let matched = if cpos < 0 || cpos > char_len {
                false
            } else {
                let byte_off = lossy
                    .char_indices()
                    .nth(cpos as usize)
                    .map(|(b, _)| b)
                    .unwrap_or(lossy.len());
                re.is_match_from(&lossy[byte_off..])
                    .map_err(|e| e.to_ruby_error(re.as_str()))?
            };
            Some(Value::Bool(matched))
        }
        // String#match with a String needle — CRuby treats the
        // needle as a regex pattern (`Regexp.new(needle)` + match).
        // Returns a MatchData (or nil). Because materialising
        // MatchData requires `&mut self`, the actual conversion
        // happens in `string_collection_call` further down — this
        // arm returns `None` to defer dispatch there. (Pre-fix
        // this arm returned the matched substring as a String,
        // which was good enough for predicate-style use
        // (`if s.match(needle)`) but diverged from CRuby for any
        // call site reading `.captures` / `.pre_match` etc.)
        // Empty match left as the dispatch target.
        // `index(substr)` / `rindex(substr)` — return the byte
        // offset where the substring first / last appears, or
        // nil if it's absent. CRuby reports a *character* index
        // for non-ASCII receivers; we report `String::find`'s
        // byte index, which matches for ASCII (the common case
        // for our test fixtures) and diverges for multibyte —
        // documented in SUBSET.md.
        (Value::Str(a), "index", [Value::Str(b)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| match sa.find(sb) {
                // `str::find` yields a BYTE offset; CRuby's
                // `String#index` returns a CHARACTER offset. Count
                // the chars before the match so the result is
                // consistent with `String#length` / `String#[]`
                // (both char-based). ASCII is unaffected (byte ==
                // char); multibyte previously diverged from CRuby.
                Some(byte_i) => Value::Int(sa[..byte_i].chars().count() as i64),
                None => Value::Nil,
            })))
        }
        // `String#index(needle, offset)` — start scanning at `offset`.
        // CRuby accepts negative offsets (counted from the end) and
        // returns nil when offset > length. Returns nil when needle
        // isn't found at or after `offset`. Both the `offset`
        // argument AND the returned index are CHARACTER positions
        // (absolute in the receiver, not relative to `offset`),
        // matching CRuby's contract; this is what makes
        // `String#index` chainable for streaming readers like
        // StringIO#gets / File#gets.
        (Value::Str(a), "index", [Value::Str(b), Value::Int(off)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| {
                let char_len = sa.chars().count() as i64;
                let start_char = if *off < 0 { char_len + *off } else { *off };
                // CRuby: out-of-range offsets (either side) return
                // nil rather than clamping. `char_len + offset < 0`
                // is the "negative offset past the start" case;
                // `offset > char_len` is the "offset past the end".
                if !(0..=char_len).contains(&start_char) {
                    return Value::Nil;
                }
                // Char offset → byte offset for the actual scan.
                // `start_char == char_len` (offset at the very end)
                // maps past the last byte, i.e. `sa.len()`.
                let start_byte = sa
                    .char_indices()
                    .nth(start_char as usize)
                    .map(|(b, _)| b)
                    .unwrap_or(sa.len());
                match sa[start_byte..].find(sb) {
                    // Absolute byte index of the match → char index.
                    Some(byte_i) => {
                        let abs_byte = start_byte + byte_i;
                        Value::Int(sa[..abs_byte].chars().count() as i64)
                    }
                    None => Value::Nil,
                }
            })))
        }
        (Value::Str(a), "rindex", [Value::Str(b)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| match sa.rfind(sb) {
                // Byte offset → char offset, as in `index` above.
                Some(byte_i) => Value::Int(sa[..byte_i].chars().count() as i64),
                None => Value::Nil,
            })))
        }
        // `String#rindex(needle, offset)` — rightmost match whose
        // START is <= offset. Char units like the 1-arg form;
        // negative offsets count from the end. rack's multipart
        // parser bounds its tail scans with this.
        (Value::Str(a), "rindex", [Value::Str(b), Value::Int(off)]) => {
            Some(a.with_str_lossy(|sa| b.with_str_lossy(|sb| {
                let char_len = sa.chars().count() as i64;
                let limit_char = if *off < 0 { char_len + *off } else { *off };
                if limit_char < 0 {
                    return Value::Nil;
                }
                let limit_char = limit_char.min(char_len);
                let limit_byte = sa
                    .char_indices()
                    .nth(limit_char as usize)
                    .map(|(byte, _)| byte)
                    .unwrap_or(sa.len());
                if sb.is_empty() {
                    return Value::Int(limit_char);
                }
                // The match may EXTEND past the limit; only its
                // start is bounded. Search the prefix that can
                // hold a start <= limit_byte.
                let hi = (limit_byte + sb.len()).min(sa.len());
                match sa[..hi].rfind(sb) {
                    Some(byte_i) => Value::Int(sa[..byte_i].chars().count() as i64),
                    None => Value::Nil,
                }
            })))
        }
        // `String#scrub` / `#scrub!` — replace invalid UTF-8 byte
        // runs with U+FFFD (or the given replacement). Receivers
        // tagged BINARY / registry encodings are always "valid"
        // in their own encoding, so they scrub to nil/self-copy
        // unchanged (CRuby agrees for BINARY; per-encoding
        // validity for registry tags lives behind
        // `_encoding_full`'s valid_encoding? — the scrub here
        // stays UTF-8-centric, documented). Block form is out of
        // subset.
        (Value::Str(a), "scrub", args2 @ ([] | [Value::Str(_)]))
        | (Value::Str(a), "scrub!", args2 @ ([] | [Value::Str(_)])) => {
            let bang = name == "scrub!";
            if bang && a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let rep = match args2.first() {
                Some(Value::Str(r)) => r.to_string_lossy(),
                _ => "\u{FFFD}".to_string(),
            };
            let bytes = a.borrow().clone();
            let mut out = String::with_capacity(bytes.len());
            let mut changed = false;
            for chunk in bytes.utf8_chunks() {
                out.push_str(chunk.valid());
                if !chunk.invalid().is_empty() {
                    changed = true;
                    out.push_str(&rep);
                }
            }
            if bang {
                // CRuby's scrub! ALWAYS returns self (unlike the
                // select!-family nil-when-unchanged convention).
                if changed {
                    check(out.len())?;
                    *a.borrow_mut() = out.into_bytes();
                }
                Some(Value::Str(a.clone()))
            } else {
                check(out.len())?;
                Some(with_tag(Value::new_str(out), a.encoding.get()))
            }
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
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            // BINARY (ASCII-8BIT) subjects replace byte-wise so the raw
            // bytes survive — a lossy UTF-8 round-trip would expand each
            // invalid byte to a 3-byte U+FFFD, corrupting AND growing the
            // result (rack strips a trailing boundary from a binary file
            // body via `body.sub(@body_regex_at_end, '')`). Scoped to
            // ASCII-8BIT: a genuinely UTF-8-tagged-but-invalid receiver
            // would RAISE ArgumentError in CRuby, not byte-replace.
            if matches!(a.encoding.get(), crate::value::EncodingTag::Binary)
                && let Some(out) = re.replace_bytes(&a.content.borrow(), repl_xlated.as_bytes())
            {
                check(out.len())?;
                return Ok(Some(with_tag(Value::new_str_bytes(out), a.encoding.get())));
            }
            let a_ref = a.to_string_lossy();
            let out = re
                .replace(&a_ref, repl_xlated.as_str())
                .map_err(|e| e.to_ruby_error(re.as_str()))?
                .into_owned();
            check(out.len())?;
            Some(Value::new_str(out))
        }
        #[cfg(feature = "regex")]
        (Value::Str(a), "gsub", [Value::Regex(re), Value::Str(repl)]) => {
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            if matches!(a.encoding.get(), crate::value::EncodingTag::Binary)
                && let Some(out) = re.replace_all_bytes(&a.content.borrow(), repl_xlated.as_bytes())
            {
                check(out.len())?;
                return Ok(Some(with_tag(Value::new_str_bytes(out), a.encoding.get())));
            }
            let a_ref = a.to_string_lossy();
            let out = re
                .replace_all(&a_ref, repl_xlated.as_str())
                .map_err(|e| e.to_ruby_error(re.as_str()))?
                .into_owned();
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let a_ref = a.to_string_lossy();
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            // `regex::Regex::replace` returns `Cow::Borrowed`
            // when there's no match — use that to detect the
            // no-match case in a single scan instead of running
            // a separate `is_match` first.
            match re
                .replace(&a_ref, repl_xlated.as_str())
                .map_err(|e| e.to_ruby_error(re.as_str()))?
            {
                std::borrow::Cow::Borrowed(_) => Some(Value::Nil),
                std::borrow::Cow::Owned(new_str) => {
                    let new_bytes = new_str.into_bytes();
                    if *a.borrow() != new_bytes {
                        check(new_bytes.len())?;
                        *a.borrow_mut() = new_bytes;
                    }
                    Some(Value::Str(a.clone()))
                }
            }
        }
        #[cfg(feature = "regex")]
        (Value::Str(a), "gsub!", [Value::Regex(re), Value::Str(repl)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let a_ref = a.to_string_lossy();
            let repl_ref = repl.to_string_lossy();
            let repl_xlated = ruby_backref_to_dollar(&repl_ref);
            // Same single-scan no-match detection via the Cow
            // returned by `replace_all`.
            match re
                .replace_all(&a_ref, repl_xlated.as_str())
                .map_err(|e| e.to_ruby_error(re.as_str()))?
            {
                std::borrow::Cow::Borrowed(_) => Some(Value::Nil),
                std::borrow::Cow::Owned(new_str) => {
                    let new_bytes = new_str.into_bytes();
                    if *a.borrow() != new_bytes {
                        check(new_bytes.len())?;
                        *a.borrow_mut() = new_bytes;
                    }
                    Some(Value::Str(a.clone()))
                }
            }
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
        // `String#tr_s(from, to)` — tr, then squeeze runs of identical
        // chars WITHIN the translated regions only: consecutive equal
        // output chars collapse iff both were produced by translation
        // (`"bookkeeper".tr_s("ok", "_") == "b_eeper"`); untranslated
        // runs are untouched (`"aabb".tr_s("a","b") == "bbb"`) and a
        // translated char never merges with an equal untranslated
        // neighbour (`"al".tr_s("l","a") == "aa"`). Empty `to` deletes,
        // same as tr.
        (Value::Str(a), "tr_s", [Value::Str(from), Value::Str(to)]) => {
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
            // Same last-occurrence-wins table as `tr`.
            let mut from_index: std::collections::HashMap<char, usize> =
                std::collections::HashMap::with_capacity(from_chars.len());
            for (i, c) in from_chars.iter().enumerate() {
                from_index.insert(*c, i);
            }
            let mut out = String::with_capacity(a_ref.len());
            // (char, was_translated) of the last char pushed to `out`.
            let mut last: Option<(char, bool)> = None;
            for ch in a_ref.chars() {
                let idx_opt = from_index.get(&ch).copied();
                let translate = if from_negated { idx_opt.is_none() } else { idx_opt.is_some() };
                if !translate {
                    out.push(ch);
                    last = Some((ch, false));
                    continue;
                }
                if to_chars.is_empty() {
                    continue; // delete, same as tr
                }
                let mapped = if from_negated {
                    to_chars.last().copied()
                } else {
                    idx_opt
                        .and_then(|i| to_chars.get(i).copied())
                        .or_else(|| to_chars.last().copied())
                };
                let Some(m) = mapped else { continue };
                if last == Some((m, true)) {
                    continue; // squeeze within the translated run
                }
                out.push(m);
                last = Some((m, true));
            }
            check(out.len())?;
            Some(Value::new_str(out))
        }
        // `String#sum(bits = 16)` — byte checksum: the sum of all bytes,
        // truncated to the low `bits` bits when bits > 0.
        (Value::Str(a), "sum", rest @ ([] | [Value::Int(_)])) => {
            let total: i64 = a.borrow().iter().map(|b| *b as i64).sum();
            let bits = match rest {
                [Value::Int(n)] => *n,
                _ => 16,
            };
            let v = if bits > 0 && bits < 63 {
                total & ((1i64 << bits) - 1)
            } else {
                total
            };
            Some(Value::Int(v))
        }
        // `String#tr!` — destructive sibling of `tr`. Runs the
        // same translation logic but mutates the receiver in
        // place, returning self on change and nil when the
        // result matches the input. Forwards parse errors
        // (reversed range, set too large) as ArgumentError.
        (Value::Str(a), "tr!", [Value::Str(from), Value::Str(to)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
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
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
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
        // Variadic `end_with?` — true if the string ends with ANY of the
        // suffix arguments (CRuby accepts only Strings here, no Regexp;
        // non-String args are ignored, mirroring the start_with? arm's
        // leniency). The single-String fast path above is the common case.
        (Value::Str(a), "end_with?", suffixes) => {
            let src = a.to_string_lossy();
            let any = suffixes.iter().any(|p| match p {
                Value::Str(b) => src.ends_with(&*b.to_string_lossy()),
                _ => false,
            });
            Some(Value::Bool(any))
        }
        // Variadic `start_with?` — true if ANY argument matches at the
        // start: a String is a literal prefix; a Regexp must match at
        // index 0 (`"Hello".start_with?(/[A-Z]/)`). Non-String/Regexp
        // args are ignored (CRuby raises TypeError; rare). The single-
        // String fast path above handles the common case.
        (Value::Str(a), "start_with?", prefixes) => {
            let src = a.to_string_lossy();
            let mut any = false;
            for p in prefixes {
                let hit = match p {
                    Value::Str(b) => src.starts_with(&*b.to_string_lossy()),
                    #[cfg(feature = "regex")]
                    Value::Regex(re) => match re.captures_owned(&src) {
                        Ok(c) => c.is_some_and(|c| c.m_start == 0),
                        // Deferred build failure — raise RegexpError
                        // at first use (the fuzz repro's exact path:
                        // `"x".start_with?(/[a-#b c dz]/)`).
                        Err(e @ crate::regex_engine::RegexOpError::Build(_)) => {
                            return Err(e.to_ruby_error(re.as_str()));
                        }
                        // Fancy match-time error: pre-existing
                        // "no match" swallow.
                        Err(crate::regex_engine::RegexOpError::Match(_)) => false,
                    },
                    _ => false,
                };
                if hit {
                    any = true;
                    break;
                }
            }
            Some(Value::Bool(any))
        }
        // `String#delete_prefix` / `delete_suffix` — return a copy
        // with the affix stripped (unchanged copy if absent). CRuby.
        // Discovery: P3 Jekyll spike — `page.rb#relative_path` does
        // `path.delete_prefix("/")`.
        (Value::Str(a), "delete_prefix", [Value::Str(pre)]) => {
            let s = a.to_string_lossy();
            let out = pre.with_str_lossy(|p| match s.strip_prefix(p) {
                Some(r) => r.to_string(),
                None => s.clone(),
            });
            Some(Value::new_str(out))
        }
        (Value::Str(a), "delete_suffix", [Value::Str(suf)]) => {
            let s = a.to_string_lossy();
            let out = suf.with_str_lossy(|p| match s.strip_suffix(p) {
                Some(r) => r.to_string(),
                None => s.clone(),
            });
            Some(Value::new_str(out))
        }
        // Bang variants mutate in place and return self when changed,
        // nil when the affix was absent (CRuby).
        (Value::Str(a), "delete_prefix!", [Value::Str(pre)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let s = a.to_string_lossy();
            match pre.with_str_lossy(|p| s.strip_prefix(p).map(|r| r.to_string())) {
                Some(r) => {
                    check(r.len())?;
                    *a.borrow_mut() = r.into_bytes();
                    Some(Value::Str(a.clone()))
                }
                None => Some(Value::Nil),
            }
        }
        (Value::Str(a), "delete_suffix!", [Value::Str(suf)]) => {
            if a.frozen.get() {
                return Err(RubyError::FrozenError {
                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(a)),
                });
            }
            let s = a.to_string_lossy();
            match suf.with_str_lossy(|p| s.strip_suffix(p).map(|r| r.to_string())) {
                Some(r) => {
                    check(r.len())?;
                    *a.borrow_mut() = r.into_bytes();
                    Some(Value::Str(a.clone()))
                }
                None => Some(Value::Nil),
            }
        }
        // `String#to_i(base = 10)` / `#hex` / `#oct` — CRuby's
        // famously lenient parse (leading ASCII whitespace, optional
        // sign, `_` between digits, stop at the first invalid char,
        // 0 on no digits) via the shared `str2int` scanner. Radix 0
        // auto-detects from a `0x`/`0o`/`0b`/`0d` prefix (bare
        // leading `0` → octal); explicit radices consume ONLY a
        // matching prefix. `hex` ≡ `to_i(16)`; `oct` is prefix-
        // driven with default 8 (str2int's negative-base form).
        //
        // Values past i64 range return `Ok(None)` here — this
        // stateless table can't allocate the heap-slot-backed
        // `Value::BigInt`, so the dispatch chain falls through to
        // `Vm::str_to_int_promote` (string_collection_call & the
        // do_call_block / super hooks), which re-runs the same
        // scanner WITH heap access. The fast path (fits-i64) stays
        // allocation-free right here.
        (Value::Str(a), "to_i", []) => {
            match crate::vm::str2int::lenient(&a.borrow(), 10)? {
                crate::vm::str2int::ParsedInt::Small(n) => Some(Value::Int(n)),
                #[cfg(feature = "bignum")]
                crate::vm::str2int::ParsedInt::Big(_) => return Ok(None),
            }
        }
        (Value::Str(a), "to_i", [Value::Int(radix)]) => {
            // CRuby's `string.c` rejects a NEGATIVE base up front
            // (raw value in the message); a positive out-of-range
            // base is validated LAZILY inside the scan — after the
            // whitespace/sign bail — so `"  ".to_i(99)` is 0 while
            // `"z".to_i(99)` raises. See `str2int::parse_int_radix`.
            if *radix < 0 {
                return Err(RubyError::ArgumentError {
                    msg: format!("invalid radix {radix}"),
                });
            }
            match crate::vm::str2int::lenient(&a.borrow(), *radix)? {
                crate::vm::str2int::ParsedInt::Small(n) => Some(Value::Int(n)),
                #[cfg(feature = "bignum")]
                crate::vm::str2int::ParsedInt::Big(_) => return Ok(None),
            }
        }
        (Value::Str(a), "hex", []) => {
            match crate::vm::str2int::lenient(&a.borrow(), 16)? {
                crate::vm::str2int::ParsedInt::Small(n) => Some(Value::Int(n)),
                #[cfg(feature = "bignum")]
                crate::vm::str2int::ParsedInt::Big(_) => return Ok(None),
            }
        }
        (Value::Str(a), "oct", []) => {
            match crate::vm::str2int::lenient(&a.borrow(), -8)? {
                crate::vm::str2int::ParsedInt::Small(n) => Some(Value::Int(n)),
                #[cfg(feature = "bignum")]
                crate::vm::str2int::ParsedInt::Big(_) => return Ok(None),
            }
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
            // CRuby raises ArgumentError on negative repeat
            // count; rubyrs previously used `(*n).max(0) as usize`
            // and silently returned "". Same pattern fixed for
            // Array#take/#drop in PR #340 / cycle 14.
            if *n < 0 {
                return Err(RubyError::ArgumentError {
                    msg: "negative argument".to_string(),
                });
            }
            // wasm32 saturation guard — sibling pattern from
            // PR #316/#323/#330's each_slice/each_cons family.
            // On 32-bit `usize`, `*n as usize` would truncate
            // large positive i64s; `try_from` saturates instead.
            let n = usize::try_from(*n).unwrap_or(usize::MAX);
            // Two guards before `repeat`: (1) `checked_mul` for
            // usize overflow, (2) `> isize::MAX` for the Vec
            // capacity ceiling (`Vec::repeat` panics with
            // "capacity overflow" beyond that). Both raise the
            // same ArgumentError "argument too big" wording CRuby
            // uses. Without this, `"abc" * (2**62)` panics the
            // host VM when `max_value_bytes` is None (no cap).
            let new_len = a.borrow().len().checked_mul(n).filter(|&n| n <= isize::MAX as usize).ok_or_else(|| {
                RubyError::ArgumentError { msg: "argument too big".to_string() }
            })?;
            check(new_len)?;
            Some(with_tag(Value::new_str_bytes(a.borrow().repeat(n)), a.encoding.get()))
        }
        (Value::Str(a), "<", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() < *b.borrow())),
        (Value::Str(a), "<=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() <= *b.borrow())),
        (Value::Str(a), ">", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() > *b.borrow())),
        (Value::Str(a), "<=>", [Value::Str(b)]) => {
            let ord = a.borrow().cmp(&*b.borrow());
            // E1 slice 2: CRuby breaks byte-equal ties by encoding
            // index when the strings aren't compatible (non-ASCII
            // bytes, different encodings): "é" <=> "é".b is 1.
            // CRuby's indices: ASCII-8BIT=0, UTF-8=1, US-ASCII=2 —
            // mirrored here.
            let ord = if ord == std::cmp::Ordering::Equal
                && a.encoding.get() != b.encoding.get()
                && !a.content.borrow().iter().all(|&x| x < 0x80)
            {
                let idx = |t: crate::value::EncodingTag| match t {
                    crate::value::EncodingTag::Binary => 0u8,
                    crate::value::EncodingTag::Utf8 => 1,
                    crate::value::EncodingTag::UsAscii => 2,
                    crate::value::EncodingTag::Other(n) => 3u8.saturating_add(n),
                };
                idx(a.encoding.get()).cmp(&idx(b.encoding.get()))
            } else {
                ord
            };
            Some(Value::Int(ord as i64))
        }
        // `casecmp` / `casecmp?` — ASCII case-insensitive compare (CRuby
        // folds only A-Z↔a-z, not full Unicode). `casecmp` returns
        // -1/0/1, `casecmp?` a Bool; both return nil for a non-String
        // argument. (batchfile and other rouge lexers use casecmp.)
        (Value::Str(a), "casecmp", [Value::Str(b)]) => {
            let aa = a.to_string_lossy().to_ascii_lowercase();
            let bb = b.to_string_lossy().to_ascii_lowercase();
            Some(Value::Int(aa.cmp(&bb) as i64))
        }
        (Value::Str(a), "casecmp?", [Value::Str(b)]) => {
            let aa = a.to_string_lossy().to_ascii_lowercase();
            let bb = b.to_string_lossy().to_ascii_lowercase();
            Some(Value::Bool(aa == bb))
        }
        (Value::Str(_), "casecmp" | "casecmp?", [_]) => Some(Value::Nil),
        (Value::Str(a), ">=", [Value::Str(b)]) => Some(Value::Bool(*a.borrow() >= *b.borrow())),
        // Regex#match? mirror — same semantics either side.
        #[cfg(feature = "regex")]
        (Value::Regex(re), "match?", [Value::Str(s)]) => {
            if let Some(m) = is_match_at_char_pos(s, 0, re) {
                return Ok(Some(Value::Bool(
                    m.map_err(|e| e.to_ruby_error(re.as_str()))?,
                )));
            }
            let matched = if matches!(s.encoding.get(), crate::value::EncodingTag::Binary) {
                match re.is_match_bytes(&s.content.borrow()) {
                    Some(m) => m,
                    None => s.with_str_lossy(|s| re.is_match_from(s))
                        .map_err(|e| e.to_ruby_error(re.as_str()))?,
                }
            } else {
                s.with_str_lossy(|s| re.is_match_from(s))
                    .map_err(|e| e.to_ruby_error(re.as_str()))?
            };
            Some(Value::Bool(matched))
        }
        // `Regexp#match?(nil)` → false (CRuby treats nil as "no
        // match" rather than raising). rack's request IP filter does
        // `trusted_proxies.match?(ip)` where `ip` can be nil for some
        // forwarded entries (spec_request "deals with proxies").
        #[cfg(feature = "regex")]
        (Value::Regex(_), "match?", [Value::Nil]) => Some(Value::Bool(false)),
        // `Regexp#match?(str, pos)` — start the match attempt at
        // character offset `pos` (negative counts from the end). No
        // `$~` update. Searches the suffix from `pos`; out-of-range pos
        // is no match. (rack's request parser probes with a position.)
        #[cfg(feature = "regex")]
        (Value::Regex(re), "match?", [Value::Str(s), Value::Int(pos)]) => {
            if let Some(m) = is_match_at_char_pos(s, *pos, re) {
                return Ok(Some(Value::Bool(
                    m.map_err(|e| e.to_ruby_error(re.as_str()))?,
                )));
            }
            let lossy = s.to_string_lossy();
            let char_len = lossy.chars().count() as i64;
            let cpos = if *pos < 0 { char_len + *pos } else { *pos };
            let matched = if cpos < 0 || cpos > char_len {
                false
            } else {
                let byte_off = lossy
                    .char_indices()
                    .nth(cpos as usize)
                    .map(|(b, _)| b)
                    .unwrap_or(lossy.len());
                re.is_match_from(&lossy[byte_off..])
                    .map_err(|e| e.to_ruby_error(re.as_str()))?
            };
            Some(Value::Bool(matched))
        }
        // Regex#source — the raw pattern string.
        #[cfg(feature = "regex")]
        (Value::Regex(re), "source", []) => Some(Value::new_str(re.as_str().to_string())),
        // `Regexp#options` — the Ruby flag bitmask
        // (IGNORECASE=1 | EXTENDED=2 | MULTILINE=4). `0` for a
        // flagless regexp. Flag THREADING from `/.../imx` literals
        // lands in a follow-up; today every compiled regexp
        // carries flags=0, so this returns 0 (correct for the
        // flagless common case + `Regexp.new(str)`).
        #[cfg(feature = "regex")]
        (Value::Regex(re), "options", []) => Some(Value::Int(re.options() as i64)),
        // `Regexp#casefold?` — true when the IGNORECASE flag is set
        // (i.e. the regexp was built with `/i` or `Regexp::IGNORECASE`).
        // Note this reflects only the literal's own flag, not inline
        // `(?i:...)` groups (CRuby agrees: `/(?i:a)/.casefold?` is false).
        #[cfg(feature = "regex")]
        (Value::Regex(re), "casefold?", []) => {
            Some(Value::Bool(re.options() & crate::regex_engine::RB_IGNORECASE != 0))
        }
        #[cfg(feature = "regex")]
        (Value::Regex(re), "to_s", []) => Some(Value::new_str(re.to_s_string())),
        #[cfg(feature = "regex")]
        (Value::Regex(re), "inspect", []) => Some(Value::new_str(re.inspect_string())),
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
            let mut out = String::new();
            out.push('"');
            match s.encoding.get() {
                crate::value::EncodingTag::Binary => {
                    crate::heap::inspect_escape_bytes_into(&s.content.borrow(), &mut out);
                }
                #[cfg(feature = "_encoding_full")]
                crate::value::EncodingTag::Other(idx) => {
                    let b = s.content.borrow();
                    match crate::encoding_full::char_chunks(idx, &b) {
                        Some(chunks) => crate::heap::inspect_escape_chunks_into(&chunks, &mut out),
                        None => crate::heap::inspect_escape_bytes_into(&b, &mut out),
                    }
                }
                #[cfg(not(feature = "_encoding_full"))]
                crate::value::EncodingTag::Other(_) => {
                    crate::heap::inspect_escape_bytes_into(&s.content.borrow(), &mut out);
                }
                _ => {
                    let b = s.content.borrow();
                    if std::str::from_utf8(&b).is_ok() {
                        drop(b);
                        crate::heap::inspect_escape_into(&s.to_string_lossy(), &mut out);
                    } else {
                        crate::heap::inspect_escape_utf8_bytes_into(&b, &mut out);
                    }
                }
            }
            out.push('"');
            Some(Value::new_str(out))
        }
        _ => None,
    })
}

impl Vm {
    /// Copy the source string's `str_ivars` side-table entry (if
    /// any) onto a freshly dup/cloned string Value — CRuby copies
    /// instance variables on BOTH `String#dup` and `String#clone`
    /// (generic object copy), and rubyrs keeps String ivars in the
    /// Rc-keyed side-table rather than on RStr. Values are cloned
    /// shallowly (children shared with the source — CRuby ditto).
    /// Gated on the set-once `any_str_ivars` flag so the common
    /// no-string-ivars program pays one false branch. Called by the
    /// canonical dup/clone arms below AND the walk-bucket
    /// `String#dup` fast arm (vm/dispatch.rs) so the two paths
    /// can't drift.
    pub(crate) fn str_ivars_copy_on_dup(&mut self, src: &Rc<RStr>, dst: &Value) {
        if !self.any_str_ivars {
            return;
        }
        let src_key = Rc::as_ptr(src) as usize;
        let Some((_, ivars)) = self.str_ivars.get(&src_key) else {
            return;
        };
        if ivars.is_empty() {
            return;
        }
        let ivars = ivars.clone();
        if let Value::Str(ns) = dst {
            let dst_key = Rc::as_ptr(ns) as usize;
            self.str_ivars.insert(dst_key, (ns.clone(), ivars));
        }
    }

    /// The heap-capable half of the Str→Integer family: re-runs the
    /// same `str2int` scan as `string_call`'s stateless `to_i` /
    /// `hex` / `oct` arms, but can lift a past-i64 result into a
    /// `Value::BigInt` via `bigint_to_value` (demote-on-fit +
    /// maybe_gc + alloc-cap). `string_call` returns `Ok(None)` for
    /// those inputs precisely so the dispatch chain reaches this —
    /// hooked from `string_collection_call` (the `do_call` route),
    /// `do_call_block`'s post-primitive fallback, and the
    /// str-singleton `super` arm in `lookup.rs`. Small values are
    /// answered correctly here too, so the helper has no ordering
    /// dependency on `string_call` having run first.
    #[cfg(feature = "bignum")]
    pub(crate) fn str_to_int_promote(
        &mut self,
        s: &Rc<RStr>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, Trap> {
        use crate::vm::str2int::{self, ParsedInt};
        let parsed = match (name, args) {
            ("to_i", []) => str2int::lenient(&s.borrow(), 10),
            ("to_i", [Value::Int(radix)]) => {
                // Mirror string_call's arm: negative base rejected
                // up front (CRuby `string.c`), positive validated
                // lazily inside the scan.
                if *radix < 0 {
                    return Err(self.trap(RubyError::ArgumentError {
                        msg: format!("invalid radix {radix}"),
                    }));
                }
                str2int::lenient(&s.borrow(), *radix)
            }
            ("hex", []) => str2int::lenient(&s.borrow(), 16),
            ("oct", []) => str2int::lenient(&s.borrow(), -8),
            _ => return Ok(None),
        };
        let parsed = match parsed {
            Ok(p) => p,
            Err(e) => return Err(self.trap(e)),
        };
        Ok(Some(match parsed {
            ParsedInt::Small(n) => Value::Int(n),
            ParsedInt::Big(b) => self.bigint_to_value(b)?,
        }))
    }

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
        // Str→Integer promote hook (see `str_to_int_promote`): the
        // stateless `string_call` table already served every
        // fits-i64 parse; only past-i64 inputs (BigInt results)
        // reach here. Bignum-gated — without the feature the
        // stateless arms wrap (documented no-bignum contract) and
        // never fall through.
        #[cfg(feature = "bignum")]
        if let Some(v) = self.str_to_int_promote(&s, name, args)? {
            return Ok(Some(v));
        }
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
                if name == "clone" && args.is_empty() {
                    // `clone` copies content AND keeps the frozen
                    // bit (dup resets it); the encoding tag travels
                    // on both. (CRuby's `clone(freeze:)` kwarg is
                    // not routed — same gap as Object#clone above.)
                    let copy = s.content.borrow().clone();
                    let v = Value::new_str_bytes(copy);
                    if let Value::Str(ref ns) = v {
                        ns.frozen.set(s.frozen.get());
                        ns.encoding.set(s.encoding.get());
                    }
                    self.str_ivars_copy_on_dup(&s, &v);
                    return Ok(Some(v));
                }
                if name == "dup" && args.is_empty() {
                    // Fresh Rc, fresh RefCell, NOT frozen — `dup`
                    // copies content but resets the frozen bit.
                    // The encoding TAG travels (CRuby: `.b.dup`
                    // stays BINARY).
                    let copy = s.content.borrow().clone();
                    let v = Value::new_str_bytes(copy);
                    if let Value::Str(ref ns) = v {
                        ns.encoding.set(s.encoding.get());
                    }
                    self.str_ivars_copy_on_dup(&s, &v);
                    return Ok(Some(v));
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
                    let v = Value::new_str_bytes(copy);
                    if let Value::Str(ref ns) = v {
                        ns.encoding.set(s.encoding.get());
                    }
                    return Ok(Some(v));
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
                        ns.encoding.set(s.encoding.get());
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
                            msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(&s)),
                        }))
                    } else {
                        Ok(())
                    }
                };
                if name == "<<" && args.len() == 1 {
                    check_unfrozen(self)?;
                    match &args[0] {
                        Value::Str(other) => {
                            // E1 slice 2: compatibility check; the
                            // receiver's tag UPGRADES to the result
                            // encoding (CRuby: `"abc" << bin` turns
                            // the receiver BINARY).
                            let tag = crate::value::enc_compat(
                                s.encoding.get(), &s.content.borrow(),
                                other.encoding.get(), &other.content.borrow(),
                            )
                            .ok_or_else(|| self.trap(RubyError::HostException {
                                class_name: "Encoding::CompatibilityError".to_string(),
                                message: format!(
                                    "incompatible character encodings: {} and {}",
                                    s.encoding.get().display(),
                                    other.encoding.get().display()
                                ),
                            }))?;
                            let to_push = other.borrow().clone();
                            s.borrow_mut().extend_from_slice(&to_push);
                            s.encoding.set(tag);
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
                                // Same compat + receiver-tag-upgrade
                                // rule as `<<` above, applied per arg.
                                let tag = crate::value::enc_compat(
                                    s.encoding.get(), &s.content.borrow(),
                                    o.encoding.get(), &o.content.borrow(),
                                )
                                .ok_or_else(|| self.trap(RubyError::HostException {
                                    class_name: "Encoding::CompatibilityError".to_string(),
                                    message: format!(
                                        "incompatible character encodings: {} and {}",
                                        s.encoding.get().display(),
                                        o.encoding.get().display()
                                    ),
                                }))?;
                                let to_push = o.borrow().clone();
                                s.borrow_mut().extend_from_slice(&to_push);
                                s.encoding.set(tag);
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
                if name == "clear" && args.is_empty() {
                    // `String#clear` — empty the buffer IN PLACE, return
                    // self, keep the encoding tag. FrozenError-aware.
                    // Surfaced by net/protocol's `rbuf_flush` (`@rbuf.clear`).
                    check_unfrozen(self)?;
                    s.borrow_mut().clear();
                    return Ok(Some(Value::Str(s)));
                }
                // `String#initialize` — the default ctor body, reached by
                // `String.new(str)` / a String-subclass `.new` with no
                // custom `initialize` (and by `super(str)` from one).
                // Copies the optional source string's bytes + encoding;
                // no arg → empties self. (A trailing kwargs Hash —
                // `String.new("x", encoding: …)` — is accepted and
                // ignored; the transcoding form isn't modelled.)
                if name == "initialize" {
                    check_unfrozen(self)?;
                    let positional = args
                        .first()
                        .filter(|a| !matches!(a, Value::Hash(_)));
                    match positional {
                        None => {
                            s.borrow_mut().clear();
                        }
                        Some(Value::Str(o)) => {
                            let nc = o.borrow().clone();
                            *s.borrow_mut() = nc;
                            s.encoding.set(o.encoding.get());
                        }
                        Some(other) => {
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "no implicit conversion of {} into String",
                                    other.type_name()
                                ),
                            }));
                        }
                    }
                    return Ok(Some(Value::Str(s)));
                }
                if name == "replace" && args.len() == 1 {
                    check_unfrozen(self)?;
                    match &args[0] {
                        Value::Str(o) => {
                            let new_content = o.borrow().clone();
                            *s.borrow_mut() = new_content;
                            // CRuby `replace` adopts the source's
                            // encoding too — `buf.replace(binary_str)`
                            // makes `buf` ASCII-8BIT. The 2-arg
                            // `IO#read(n, buf)` form relies on this so
                            // the output buffer comes back binary
                            // (rack's multipart parser reads into a
                            // reused outbuf).
                            s.encoding.set(o.encoding.get());
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
                // nil if no match. CRuby ALSO accepts a String
                // arg, interpreted as a regex pattern
                // (`Regexp.new(arg)` then match); we handle that
                // by compiling the String into a CompiledRegex on
                // the fly before falling into the regex branch.
                // StringScanner's non-slicing search hook (see
                // stdlib_vendor/strscan.rb). `recv.__strscan_search(re,
                // byte_pos)` searches `re` in `recv` from `byte_pos`
                // WITHOUT copying the tail — the slice `@str[@pos..]`
                // is what makes scan_until O(n²) on a big binary buffer
                // (rack multipart). BINARY only; for char==byte the
                // scanner's `@pos` is already a byte offset. Returns the
                // absolute match start (Int), nil (no match), or false
                // (no byte engine → scanner falls back to slicing).
                #[cfg(feature = "regex")]
                if name == "__strscan_search"
                    && let [Value::Regex(re), Value::Int(pos)] = args
                {
                    let re = re.clone();
                    let start = (*pos).max(0) as usize;
                    return Ok(Some(self.do_strscan_search_binary(&re, &s, start)?));
                }
                // StringScanner's non-slicing ANCHORED match hook (the
                // `match_at_pos` path: scan/check/skip/match?). Mirrors
                // __strscan_search but requires the match to start AT
                // `byte_pos`; avoids the O(remaining) `@str[@pos..]` copy
                // that made kramdown's per-position scan loop O(n²).
                #[cfg(feature = "regex")]
                if name == "__strscan_match_at" && args.len() == 2 {
                    if let [Value::Regex(re), Value::Int(pos)] = args {
                        let re = re.clone();
                        let start = (*pos).max(0) as usize;
                        return Ok(Some(self.do_strscan_match_at_binary(&re, &s, start)?));
                    }
                    // Non-Regexp arg (e.g. csv's `scan("x")` probe): signal
                    // "fall back to the Ruby slice path" with `false`, so the
                    // scanner needn't pre-check `regex.is_a?(Regexp)` on every
                    // hot call. The slice path then raises CRuby's TypeError.
                    return Ok(Some(Value::Bool(false)));
                }
                #[cfg(feature = "regex")]
                if name == "match" && (1..=2).contains(&args.len()) {
                    // `str.match(pattern[, pos])` — shared runner handles
                    // String→Regex coercion, the optional char-index pos,
                    // the BINARY byte path, and sets `$~`. (The block form
                    // `str.match(re) { |m| … }` is routed in
                    // collection_call_block, which calls the same runner.)
                    return Ok(Some(self.string_match_run(&s, args)?));
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
                // `String#[](regexp, name)` — String/Symbol second arg
                // is a NAMED capture reference (CRuby), resolved to its
                // group index so str_bracket_regex's Integer path runs.
                // Unknown name → IndexError, validated against the
                // pattern before the match.
                #[cfg(feature = "regex")]
                if (name == "[]" || name == "slice") && args.len() == 2
                    && let Value::Regex(re) = &args[0]
                    && matches!(&args[1], Value::Str(_) | Value::Sym(_))
                {
                    let gname = match &args[1] {
                        Value::Str(rs) => rs.to_string_lossy(),
                        Value::Sym(sid) => self.interner.resolve(*sid).to_string(),
                        _ => unreachable!(),
                    };
                    let idx = match re
                        .capture_name_index(&gname)
                        .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?
                    {
                        Some(i) => i as i64,
                        None => {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("undefined group name reference: {gname}"),
                            }));
                        }
                    };
                    return Ok(Some(self.str_bracket_regex(&s, re, idx)?));
                }
                // `String#[](substr)` / `slice(substr)` — the SUBSTRING-
                // SEARCH form: returns a new String equal to `substr`
                // when the receiver contains it, else nil. rubocop
                // 1.88's Style/MagicCommentFormat probes separators
                // with `text[wrong_separator]` on every file that
                // carries a magic comment; without this arm the cop
                // crashed (NoMethodError), which kept the runner's
                // `errors.any?` true and silently blocked EVERY
                // result-cache save.
                if (name == "[]" || name == "slice") && args.len() == 1
                    && let Value::Str(sub) = &args[0]
                {
                    let hay = s.content.borrow();
                    let needle = sub.content.borrow();
                    let found = needle.is_empty()
                        || hay.windows(needle.len()).any(|w| w == &needle[..]);
                    let bytes = needle.clone();
                    drop(needle);
                    drop(hay);
                    return Ok(Some(if found {
                        match String::from_utf8(bytes) {
                            Ok(us) => Value::new_str(us),
                            Err(e) => Value::new_str_bytes(e.into_bytes()),
                        }
                    } else {
                        Value::Nil
                    }));
                }
                // Float→Int coerce on the 1-arg index form. CRuby's
                // `String#[]` treats Float via `to_int` (truncates
                // toward zero); rubyrs's match arm only bound
                // Value::Int(_) so `"hello"[2.5]` previously
                // NoMethodError'd. Re-dispatch with the truncated
                // Int so the existing arms handle the rest.
                if (name == "[]" || name == "slice") && args.len() == 1
                    && let Value::Float(f) = &args[0]
                {
                    let coerced = vec![Value::Int(*f as i64)];
                    return self.string_collection_call(s.clone(), name, &coerced);
                }
                if (name == "[]" || name == "slice") && args.len() == 2 {
                    // Float coerce on either or both positions —
                    // matches CRuby `"hello"[1, 2.5]` / `"hello"[0.5, 3]`.
                    let coerce_pos = |v: &Value| match v {
                        Value::Float(f) => Some(Value::Int(*f as i64)),
                        _ => None,
                    };
                    let a0 = coerce_pos(&args[0]);
                    let a1 = coerce_pos(&args[1]);
                    if a0.is_some() || a1.is_some() {
                        let coerced = vec![
                            a0.unwrap_or_else(|| args[0].clone()),
                            a1.unwrap_or_else(|| args[1].clone()),
                        ];
                        return self.string_collection_call(s.clone(), name, &coerced);
                    }
                }
                // `String#byteslice(range)` — slice by BYTE offsets,
                // PRESERVING the receiver's encoding tag (CRuby keeps
                // the original encoding even when the cut lands inside
                // a multibyte char). The Int / (Int,len) forms are
                // handled in `string_call`; the Range form needs heap
                // access for the Range bounds, so it lives here. A
                // non-Int/Nil endpoint falls through.
                if name == "byteslice"
                    && let [Value::Range(rid)] = args
                {
                    let bytes = s.content.borrow();
                    let blen = bytes.len() as i64;
                    let norm = |i: i64| if i < 0 { blen + i } else { i };
                    let r = self.heap.range(*rid);
                    let excl = r.exclusive;
                    let bi = match &r.begin {
                        Value::Int(a) => *a,
                        Value::Nil => 0,
                        _ => return Ok(None),
                    };
                    let ei = match &r.end {
                        Value::Int(c) => *c,
                        Value::Nil => blen,
                        _ => return Ok(None),
                    };
                    let start = norm(bi);
                    if start < 0 || start > blen {
                        return Ok(Some(Value::Nil));
                    }
                    let mut end = norm(ei);
                    if !excl {
                        end += 1; // inclusive; a Nil end (→ blen) clamps below
                    }
                    let end = end.clamp(start, blen);
                    let slice = bytes[start as usize..end as usize].to_vec();
                    return Ok(Some(with_tag(
                        Value::new_str_bytes(slice),
                        s.encoding.get(),
                    )));
                }
                // BINARY-encoded receiver: index / slice by BYTES and
                // keep the ASCII-8BIT tag. The char path below routes
                // through `to_string_lossy`, which U+FFFD-mangles
                // non-UTF-8 bytes — that corrupted StringIO#read and
                // Zlib over binary streams (gzip bodies). CRuby treats
                // ASCII-8BIT as 1 byte = 1 char. Int / (Int,len) /
                // Range only; other arg shapes fall through.
                if (name == "[]" || name == "slice")
                    && s.encoding.get() == crate::value::EncodingTag::Binary
                {
                    let bytes = s.content.borrow();
                    let blen = bytes.len() as i64;
                    let norm = |i: i64| if i < 0 { blen + i } else { i };
                    match args {
                        [Value::Int(i)] => {
                            let idx = norm(*i);
                            return Ok(Some(if idx < 0 || idx >= blen {
                                Value::Nil
                            } else {
                                Value::new_str_bytes_binary(vec![bytes[idx as usize]])
                            }));
                        }
                        [Value::Int(st), Value::Int(ln)] => {
                            let start = norm(*st);
                            if start < 0 || start > blen || *ln < 0 {
                                return Ok(Some(Value::Nil));
                            }
                            let end = (start + *ln).min(blen);
                            return Ok(Some(Value::new_str_bytes_binary(
                                bytes[start as usize..end as usize].to_vec(),
                            )));
                        }
                        [Value::Range(rid)] => {
                            let r = self.heap.range(*rid);
                            let excl = r.exclusive;
                            let bi = match &r.begin {
                                Value::Int(a) => *a,
                                Value::Nil => 0,
                                _ => return Ok(None),
                            };
                            let ei = match &r.end {
                                Value::Int(c) => *c,
                                Value::Nil => blen,
                                _ => return Ok(None),
                            };
                            let start = norm(bi);
                            if start < 0 || start > blen {
                                return Ok(Some(Value::Nil));
                            }
                            let mut end = norm(ei);
                            if !excl {
                                end += 1;
                            }
                            let end = end.clamp(start, blen);
                            return Ok(Some(Value::new_str_bytes_binary(
                                bytes[start as usize..end as usize].to_vec(),
                            )));
                        }
                        _ => {}
                    }
                }
                // Non-ASCII receiver (valid OR invalid UTF-8): char-index
                // via the CACHED char→byte table while preserving the
                // EXACT bytes. The generic char path below routes through
                // `to_string_lossy().chars().collect()` — an O(n) walk +
                // alloc per call, and for invalid UTF-8 it also expands
                // each invalid byte to a 3-byte U+FFFD, corrupting AND
                // growing the slice (rack reads multipart bodies as
                // UTF-8-tagged binary via `File.read`, and StringIO#read
                // slices them with `@str[pos, len]`). For valid UTF-8 the
                // table's boundaries ARE `char_indices`, so this arm now
                // takes over that case too — O(1) per call once the table
                // is built (invalidated on mutation). Keeps the receiver
                // encoding. Int / (Int,len) / Range only.
                if (name == "[]" || name == "slice")
                    && matches!(
                        args,
                        [Value::Int(_)]
                            | [Value::Int(_), Value::Int(_)]
                            | [Value::Range(_)]
                    )
                    && s.encoding.get() != crate::value::EncodingTag::Binary
                    && !s.content.is_ascii_cached()
                {
                    let starts = s.content.char_starts();
                    let bytes = s.content.borrow();
                    let nchars = (starts.len() - 1) as i64;
                    let tag = s.encoding.get();
                    let norm = |i: i64| if i < 0 { nchars + i } else { i };
                    let mk = |c0: usize, c1: usize| {
                        with_tag(
                            Value::new_str_bytes(
                                bytes[starts[c0] as usize..starts[c1] as usize].to_vec(),
                            ),
                            tag,
                        )
                    };
                    match args {
                        [Value::Int(i)] => {
                            let idx = norm(*i);
                            return Ok(Some(if idx < 0 || idx >= nchars {
                                Value::Nil
                            } else {
                                mk(idx as usize, idx as usize + 1)
                            }));
                        }
                        [Value::Int(st), Value::Int(ln)] => {
                            let start = norm(*st);
                            if start < 0 || start > nchars || *ln < 0 {
                                return Ok(Some(Value::Nil));
                            }
                            let end = (start + *ln).min(nchars);
                            return Ok(Some(mk(start as usize, end as usize)));
                        }
                        [Value::Range(rid)] => {
                            let r = self.heap.range(*rid);
                            let excl = r.exclusive;
                            let bi = match &r.begin {
                                Value::Int(a) => *a,
                                Value::Nil => 0,
                                _ => return Ok(None),
                            };
                            let endless_end = matches!(&r.end, Value::Nil);
                            let ei = match &r.end {
                                Value::Int(c) => *c,
                                Value::Nil => nchars,
                                _ => return Ok(None),
                            };
                            let start = norm(bi);
                            if start < 0 || start > nchars {
                                return Ok(Some(Value::Nil));
                            }
                            let mut end = if endless_end { nchars } else { norm(ei) };
                            if !excl && !endless_end {
                                end += 1;
                            }
                            let end = end.clamp(start, nchars);
                            return Ok(Some(mk(start as usize, end as usize)));
                        }
                        _ => {}
                    }
                }
                // ASCII-only (cached) receiver: character index == byte
                // index, so slice the bytes directly — O(len). The
                // generic char path below does
                // `to_string_lossy().chars().collect()`, which is
                // O(string-length) on EVERY call; rack multipart's
                // `@str[@pos, consumed]` per part then makes the whole
                // parse O(n²). Int / (Int,len) / Range; preserves the
                // receiver's encoding tag.
                if (name == "[]" || name == "slice")
                    && matches!(
                        args,
                        [Value::Int(_)]
                            | [Value::Int(_), Value::Int(_)]
                            | [Value::Range(_)]
                    )
                    && s.encoding.get() != crate::value::EncodingTag::Binary
                    && s.content.is_ascii_cached()
                {
                    let bytes = s.content.borrow();
                    let nchars = bytes.len() as i64;
                    let tag = s.encoding.get();
                    let norm = |i: i64| if i < 0 { nchars + i } else { i };
                    let mk = |c0: usize, c1: usize| {
                        with_tag(Value::new_str_bytes(bytes[c0..c1].to_vec()), tag)
                    };
                    match args {
                        [Value::Int(i)] => {
                            let idx = norm(*i);
                            return Ok(Some(if idx < 0 || idx >= nchars {
                                Value::Nil
                            } else {
                                mk(idx as usize, idx as usize + 1)
                            }));
                        }
                        [Value::Int(st), Value::Int(ln)] => {
                            let start = norm(*st);
                            if start < 0 || start > nchars || *ln < 0 {
                                return Ok(Some(Value::Nil));
                            }
                            let end = (start + *ln).min(nchars);
                            return Ok(Some(mk(start as usize, end as usize)));
                        }
                        [Value::Range(rid)] => {
                            let r = self.heap.range(*rid);
                            let excl = r.exclusive;
                            let bi = match &r.begin {
                                Value::Int(a) => *a,
                                Value::Nil => 0,
                                _ => return Ok(None),
                            };
                            let endless_end = matches!(&r.end, Value::Nil);
                            let ei = match &r.end {
                                Value::Int(c) => *c,
                                Value::Nil => nchars,
                                _ => return Ok(None),
                            };
                            let start = norm(bi);
                            if start < 0 || start > nchars {
                                return Ok(Some(Value::Nil));
                            }
                            let mut end = if endless_end { nchars } else { norm(ei) };
                            if !excl && !endless_end {
                                end += 1;
                            }
                            let end = end.clamp(start, nchars);
                            return Ok(Some(mk(start as usize, end as usize)));
                        }
                        _ => {}
                    }
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
                // `String#index(regexp[, offset])` — like the
                // string-needle primitive forms but pattern-based;
                // sets `$~` like CRuby (rack's multipart parser
                // scans quoted-string escapes with
                // `index(/(["\\])/)` in a slice! loop). Offsets and
                // the result are CHAR positions. The scan runs on
                // the suffix at `offset` — lookBEHIND patterns
                // can't see across that boundary (documented;
                // lookahead is unaffected).
                #[cfg(feature = "regex")]
                if name == "index"
                    && let Some(Value::Regex(re)) = args.first()
                    && (args.len() == 1 || matches!(args.get(1), Some(Value::Int(_))))
                {
                    let off = match args.get(1) {
                        Some(Value::Int(o)) => *o,
                        _ => 0,
                    };
                    let sa = s.to_string_lossy();
                    let char_len = sa.chars().count() as i64;
                    let start_char = if off < 0 { char_len + off } else { off };
                    if !(0..=char_len).contains(&start_char) {
                        return Ok(Some(Value::Nil));
                    }
                    let start_byte = sa
                        .char_indices()
                        .nth(start_char as usize)
                        .map(|(b, _)| b)
                        .unwrap_or(sa.len());
                    let owned = re
                        .captures_owned(&sa[start_byte..])
                        .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
                    return Ok(Some(match owned {
                        None => {
                            self.save_match_scope_on_write();
                            self.last_match = None;
                            Value::Nil
                        }
                        Some(oc) => {
                            let abs_start = start_byte + oc.m_start;
                            let abs_end = start_byte + oc.m_end;
                            let result = Value::Int(sa[..abs_start].chars().count() as i64);
                            // Group spans shift by `start_byte` too (the
                            // search ran on the tail from `start_byte`).
                            let group_spans: Vec<Option<(usize, usize)>> = oc.group_spans
                                .iter()
                                .map(|sp| sp.map(|(b, e)| (b + start_byte, e + start_byte)))
                                .collect();
                            let cap_names = re.capture_group_names();
                            self.save_match_scope_on_write();
                            self.last_match = Some(crate::vm::LastMatch {
                                whole: oc.whole,
                                caps: oc.groups,
                                input: sa,
                                m_start: abs_start,
                                m_end: abs_end,
                                named: oc.named,
                                group_spans,
                                cap_names,
                                binary: None,
                            });
                            result
                        }
                    }));
                }
                // `String#slice!` — destructive slice: resolve the
                // byte span the read form would return, cut it out
                // of the receiver, return the removed piece (with
                // the receiver's tag). rack's multipart parser
                // peels Content-Disposition params with
                // `slice!(0, n)` (~60 call sites across its
                // specs). Char-indexed like CRuby's []; the regexp
                // forms share str_bracket_regex so `$~` updates
                // exactly like `String#[regexp]` — including the
                // capture form, which removes the GROUP's span
                // (CRuby: `"hello".slice!(/l(l)o/, 1)` → "helo").
                if name == "slice!" {
                    if args.is_empty() || args.len() > 2 {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: format!(
                                "wrong number of arguments (given {}, expected 1..2)",
                                args.len(),
                            ),
                        }));
                    }
                    if s.frozen.get() {
                        return Err(self.trap(RubyError::FrozenError {
                            msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(&s)),
                        }));
                    }
                    #[cfg(feature = "regex")]
                    if let Value::Regex(re) = &args[0] {
                        // CRuby resolves a String/Symbol second arg as
                        // a NAMED capture reference against the pattern;
                        // an unknown name is IndexError (not TypeError),
                        // resolved before the match runs. A resolved name
                        // becomes its absolute group index, reusing the
                        // Integer-`n` span logic below.
                        let n = match args.get(1) {
                            Some(Value::Int(n)) => *n,
                            Some(Value::Str(gname)) => {
                                let gname = gname.to_string_lossy();
                                match re
                                    .capture_name_index(&gname)
                                    .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?
                                {
                                    Some(idx) => idx as i64,
                                    None => {
                                        return Err(self.trap(RubyError::IndexError {
                                            msg: format!(
                                                "undefined group name reference: {gname}",
                                            ),
                                        }));
                                    }
                                }
                            }
                            Some(Value::Sym(sid)) => {
                                let gname = self.interner.resolve(*sid).to_string();
                                match re
                                    .capture_name_index(&gname)
                                    .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?
                                {
                                    Some(idx) => idx as i64,
                                    None => {
                                        return Err(self.trap(RubyError::IndexError {
                                            msg: format!(
                                                "undefined group name reference: {gname}",
                                            ),
                                        }));
                                    }
                                }
                            }
                            Some(other) => {
                                return Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "no implicit conversion of {} into Integer",
                                        other.type_name(),
                                    ),
                                }));
                            }
                            None => 0,
                        };
                        let bound = s.to_string_lossy();
                        let owned = re
                            .captures_owned(&bound)
                            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
                        let Some(oc) = owned else {
                            self.save_match_scope_on_write();
                            self.last_match = None;
                            return Ok(Some(Value::Nil));
                        };
                        let span = if n == 0 {
                            Some((oc.m_start, oc.m_end))
                        } else if n > 0 && (n as usize) <= oc.group_spans.len() {
                            oc.group_spans[(n as usize) - 1]
                        } else {
                            None
                        };
                        let cap_names = re.capture_group_names();
                        self.save_match_scope_on_write();
                        self.last_match = Some(crate::vm::LastMatch {
                            whole: oc.whole,
                            caps: oc.groups,
                            input: bound,
                            m_start: oc.m_start,
                            m_end: oc.m_end,
                            named: oc.named,
                            group_spans: oc.group_spans,
                            cap_names,
                            binary: None,
                        });
                        return Ok(Some(match span {
                            None => Value::Nil,
                            Some((b0, b1)) => {
                                let removed: Vec<u8> = s.borrow()[b0..b1].to_vec();
                                s.borrow_mut().drain(b0..b1);
                                with_tag(Value::new_str_bytes(removed), s.encoding.get())
                            }
                        }));
                    }
                    // BINARY (ASCII-8BIT) — or any non-UTF-8 byte content —
                    // indexes by BYTE: each byte is one "character" in
                    // CRuby. Building the char→byte map from
                    // `to_string_lossy()` (which expands invalid UTF-8 to
                    // 3-byte U+FFFD) would compute offsets against the lossy
                    // reconstruction and then apply them to the raw bytes —
                    // diverging, and panicking OOB when the lossy form is
                    // longer. rack-session's decrypt does
                    // `data.slice!(-32..-1)` on `Base64.urlsafe_decode64`
                    // output (random encrypted bytes — invalid UTF-8, not
                    // always BINARY-tagged), so gate on actual byte
                    // validity, not just the encoding tag.
                    let binary = matches!(s.encoding.get(), crate::value::EncodingTag::Binary)
                        || std::str::from_utf8(&s.borrow()).is_err();
                    let lossy = if binary { String::new() } else { s.to_string_lossy() };
                    let raw_len = s.borrow().len();
                    let char_bytes: Vec<usize> = if binary {
                        Vec::new()
                    } else {
                        lossy.char_indices().map(|(b, _)| b).collect()
                    };
                    let char_len = if binary { raw_len as i64 } else { char_bytes.len() as i64 };
                    let total = if binary { raw_len } else { lossy.len() };
                    let byte_at = |ci: i64| -> usize {
                        let ci = ci as usize;
                        if binary {
                            ci.min(raw_len)
                        } else if ci >= char_bytes.len() {
                            total
                        } else {
                            char_bytes[ci]
                        }
                    };
                    let span: Option<(usize, usize)> = match (&args[0], args.get(1)) {
                        (Value::Int(i), Some(Value::Int(n))) => {
                            let start = if *i < 0 { char_len + *i } else { *i };
                            if start < 0 || start > char_len || *n < 0 {
                                None
                            } else {
                                let cnt = (*n).min(char_len - start);
                                Some((byte_at(start), byte_at(start + cnt)))
                            }
                        }
                        (Value::Int(i), None) => {
                            let idx = if *i < 0 { char_len + *i } else { *i };
                            if idx < 0 || idx >= char_len {
                                None
                            } else {
                                Some((byte_at(idx), byte_at(idx + 1)))
                            }
                        }
                        (Value::Range(rid), None) => {
                            let r = self.heap.range(*rid);
                            let excl = r.exclusive;
                            let bi = match &r.begin {
                                Value::Int(a) => *a,
                                Value::Nil => 0,
                                _ => return Ok(None),
                            };
                            let ei_opt = match &r.end {
                                Value::Int(c) => Some(*c),
                                Value::Nil => None,
                                _ => return Ok(None),
                            };
                            let start = if bi < 0 { char_len + bi } else { bi };
                            if start < 0 || start > char_len {
                                None
                            } else {
                                let mut end = match ei_opt {
                                    None => char_len,
                                    Some(e) if e < 0 => char_len + e,
                                    Some(e) => e,
                                };
                                if !excl && ei_opt.is_some() { end += 1; }
                                let end = end.clamp(start, char_len);
                                Some((byte_at(start), byte_at(end)))
                            }
                        }
                        (Value::Str(needle), None) => {
                            // `s.slice!(s)` (same Rc) would double-
                            // borrow below — it trivially matches
                            // the whole string.
                            if std::rc::Rc::ptr_eq(&s, needle) {
                                Some((0, s.borrow().len()))
                            } else {
                                let hay = s.borrow();
                                let nb = needle.borrow();
                                if nb.is_empty() {
                                    Some((0, 0))
                                } else {
                                    hay.windows(nb.len())
                                        .position(|w| w == &nb[..])
                                        .map(|p| (p, p + nb.len()))
                                }
                            }
                        }
                        _ => return Ok(None),
                    };
                    return Ok(Some(match span {
                        None => Value::Nil,
                        Some((b0, b1)) => {
                            let removed: Vec<u8> = s.borrow()[b0..b1].to_vec();
                            s.borrow_mut().drain(b0..b1);
                            with_tag(Value::new_str_bytes(removed), s.encoding.get())
                        }
                    }));
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
                // Float→Int coerce on `[]=` index forms — same
                // pattern as the read path above. CRuby treats Float
                // indices via to_int; without this rubyrs raised
                // NoMethodError for `s[2.5] = "x"` / `s[0, 2.5] = "x"`.
                if name == "[]=" && args.len() == 2
                    && let Value::Float(f) = &args[0]
                {
                    let coerced = vec![Value::Int(*f as i64), args[1].clone()];
                    return self.string_collection_call(s.clone(), name, &coerced);
                }
                if name == "[]=" && args.len() == 3 {
                    let coerce_pos = |v: &Value| match v {
                        Value::Float(f) => Some(Value::Int(*f as i64)),
                        _ => None,
                    };
                    let a0 = coerce_pos(&args[0]);
                    let a1 = coerce_pos(&args[1]);
                    if a0.is_some() || a1.is_some() {
                        let coerced = vec![
                            a0.unwrap_or_else(|| args[0].clone()),
                            a1.unwrap_or_else(|| args[1].clone()),
                            args[2].clone(),
                        ];
                        return self.string_collection_call(s.clone(), name, &coerced);
                    }
                }
                // `s[range] = repl` — Range LHS for []=. CRuby resolves
                // the Range into (start, length) and splices like the
                // 3-arg form. begin/end Nil → 0 / len-1. Exclusive
                // bound drops one (for explicit-end ranges only;
                // endless ranges ignore the exclusive flag — matches
                // CRuby + the Array#[]= Range arm landed earlier this
                // session). begin > len OR begin < 0 (after wrap)
                // raises RangeError "<begin>..<end> out of range".
                if name == "[]=" && args.len() == 2
                    && let (Value::Range(rid), Value::Str(repl)) = (&args[0], &args[1])
                {
                    check_unfrozen(self)?;
                    let r = self.heap.range(*rid);
                    let r_begin = r.begin.clone();
                    let r_end = r.end.clone();
                    let r_exclusive = r.exclusive;
                    let chars: Vec<char> = s.to_string_lossy().chars().collect();
                    let len = chars.len() as i64;
                    let begin = match r_begin {
                        Value::Nil => 0,
                        Value::Int(b) => if b < 0 { len + b } else { b },
                        _ => return Ok(None),
                    };
                    let end_idx = match r_end {
                        Value::Nil => len - 1,
                        Value::Int(e) => {
                            let resolved = if e < 0 { len + e } else { e };
                            if r_exclusive { resolved - 1 } else { resolved }
                        }
                        _ => return Ok(None),
                    };
                    if begin < 0 || begin > len {
                        return Err(self.trap(RubyError::RangeError {
                            msg: format!("{}..{} out of range", begin, end_idx),
                        }));
                    }
                    let length = if end_idx < begin { 0 } else { end_idx - begin + 1 };
                    let start = begin as usize;
                    let take = (length as usize).min(chars.len() - start);
                    let mut buf: String = chars[..start].iter().collect();
                    buf.push_str(&repl.to_string_lossy());
                    buf.extend(chars[start + take..].iter());
                    *s.borrow_mut() = buf.into_bytes();
                    return Ok(Some(args[1].clone()));
                }
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
                    // `str[substring] = repl` — replace the FIRST
                    // occurrence of `substring` with `repl`. CRuby
                    // raises `IndexError: string not matched` when the
                    // substring is absent. Discovery: P3 Jekyll spike —
                    // `entry_filter.rb` does `base_dir[site.source] = ""`
                    // to strip the source prefix.
                    if let (Value::Str(needle), Value::Str(repl)) = (&args[0], &args[1]) {
                        let hay = s.to_string_lossy();
                        let pat = needle.to_string_lossy();
                        match hay.find(&pat) {
                            Some(byte_idx) => {
                                let mut buf =
                                    String::with_capacity(hay.len() + repl.borrow().len());
                                buf.push_str(&hay[..byte_idx]);
                                buf.push_str(&repl.to_string_lossy());
                                buf.push_str(&hay[byte_idx + pat.len()..]);
                                *s.borrow_mut() = buf.into_bytes();
                                return Ok(Some(args[1].clone()));
                            }
                            None => {
                                return Err(self.trap(RubyError::IndexError {
                                    msg: "string not matched".to_string(),
                                }));
                            }
                        }
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
                        // Registry encodings: per-character byte
                        // chunks under THAT encoding, each keeping
                        // the receiver's tag (multi-byte aware —
                        // SJIS "日本語".chars is three 2-byte
                        // strings). Broken sequences fall back to
                        // the lossy char route below.
                        #[cfg(feature = "_encoding_full")]
                        if let crate::value::EncodingTag::Other(idx) = s.encoding.get() {
                            let chunks = {
                                let b = s.content.borrow();
                                crate::encoding_full::char_chunks(idx, &b)
                            };
                            if let Some(chunks) = chunks {
                                let tag = s.encoding.get();
                                let elems: Vec<Value> = chunks.into_iter()
                                    .map(|chunk| {
                                        let v = Value::new_str_bytes(chunk);
                                        if let Value::Str(ref ns) = v {
                                            ns.encoding.set(tag);
                                        }
                                        v
                                    })
                                    .collect();
                                self.maybe_gc();
                                let id = self.heap.alloc(HeapObj::Array(elems.into()));
                                return Ok(Some(Value::Array(id)));
                            }
                        }
                        let elems: Vec<Value> = s.to_string_lossy().chars()
                            .map(|c| Value::new_str(c.to_string()))
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    // `String#codepoints` — the integer Unicode code
                    // point of each character. An ASCII-8BIT (BINARY)
                    // subject yields raw byte values (0..255), matching
                    // CRuby. The block form (`each_codepoint`) is routed
                    // in collection_call_block.
                    ("codepoints", []) => {
                        let elems: Vec<Value> = if matches!(s.encoding.get(), crate::value::EncodingTag::Binary) {
                            s.content.borrow().iter().map(|&b| Value::Int(b as i64)).collect()
                        } else {
                            s.to_string_lossy().chars().map(|c| Value::Int(c as i64)).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    // `String#each_codepoint` with NO block → Enumerator
                    // (the block form is handled in collection_call_block).
                    ("each_codepoint", []) => {
                        return self.make_enum_for(Value::Str(s.clone()), "each_codepoint", vec![]).map(Some);
                    }
                    ("partition", [Value::Str(sep)]) => {
                        // Split at the FIRST occurrence → [before, sep, after];
                        // no match → [self, "", ""].
                        let src = s.to_string_lossy();
                        let sep_s = sep.to_string_lossy();
                        let parts = match src.find(sep_s.as_str()) {
                            Some(i) => [
                                src[..i].to_string(),
                                sep_s.to_string(),
                                src[i + sep_s.len()..].to_string(),
                            ],
                            None => [src.to_string(), String::new(), String::new()],
                        };
                        let elems: Vec<Value> = parts.iter().map(Value::new_str).collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    ("rpartition", [Value::Str(sep)]) => {
                        // Split at the LAST occurrence; no match →
                        // ["", "", self] (CRuby's rpartition miss shape).
                        let src = s.to_string_lossy();
                        let sep_s = sep.to_string_lossy();
                        let parts = match src.rfind(sep_s.as_str()) {
                            Some(i) => [
                                src[..i].to_string(),
                                sep_s.to_string(),
                                src[i + sep_s.len()..].to_string(),
                            ],
                            None => [String::new(), String::new(), src.to_string()],
                        };
                        let elems: Vec<Value> = parts.iter().map(Value::new_str).collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    #[cfg(feature = "regex")]
                    ("partition", [Value::Regex(re)]) => {
                        let src = s.to_string_lossy();
                        // A deferred BUILD failure raises RegexpError
                        // (first use of a lazily-compiled regexp); a
                        // fancy MATCH-time error keeps the pre-existing
                        // "no match" swallow (is_match-style trade-off).
                        let found = match re.captures_owned(&src) {
                            Ok(c) => c,
                            Err(e @ crate::regex_engine::RegexOpError::Build(_)) => {
                                return Err(self.trap(e.to_ruby_error(re.as_str())));
                            }
                            Err(crate::regex_engine::RegexOpError::Match(_)) => None,
                        };
                        let parts = match found {
                            Some(c) => [
                                src[..c.m_start].to_string(),
                                src[c.m_start..c.m_end].to_string(),
                                src[c.m_end..].to_string(),
                            ],
                            None => [src.to_string(), String::new(), String::new()],
                        };
                        let elems: Vec<Value> = parts.iter().map(Value::new_str).collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    #[cfg(feature = "regex")]
                    ("rpartition", [Value::Regex(re)]) => {
                        let src = s.to_string_lossy();
                        // Last match → take the final element of all matches.
                        // Same Build-raise / Match-swallow split as
                        // `partition` above.
                        let found = match re.captures_iter_owned(&src) {
                            Ok(v) => v.into_iter().last(),
                            Err(e @ crate::regex_engine::RegexOpError::Build(_)) => {
                                return Err(self.trap(e.to_ruby_error(re.as_str())));
                            }
                            Err(crate::regex_engine::RegexOpError::Match(_)) => None,
                        };
                        let parts = match found {
                            Some(c) => [
                                src[..c.m_start].to_string(),
                                src[c.m_start..c.m_end].to_string(),
                                src[c.m_end..].to_string(),
                            ],
                            None => [String::new(), String::new(), src.to_string()],
                        };
                        let elems: Vec<Value> = parts.iter().map(Value::new_str).collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    ("insert", [Value::Int(idx), Value::Str(ins)]) => {
                        check_unfrozen(self)?;
                        // Char-indexed. A non-negative index inserts BEFORE
                        // that char; a negative index counts from the end and
                        // inserts AFTER (so -1 appends), matching CRuby.
                        let src = s.to_string_lossy();
                        let chars: Vec<char> = src.chars().collect();
                        let n = chars.len() as i64;
                        let pos = if *idx < 0 { n + idx + 1 } else { *idx };
                        if pos < 0 || pos > n {
                            return Err(self.trap(RubyError::IndexError {
                                msg: format!("index {} out of string", idx),
                            }));
                        }
                        let pos = pos as usize;
                        let mut out = String::new();
                        out.extend(chars[..pos].iter());
                        out.push_str(&ins.to_string_lossy());
                        out.extend(chars[pos..].iter());
                        *s.borrow_mut() = out.into_bytes();
                        Some(Value::Str(s.clone()))
                    }
                    ("delete", [Value::Str(set)]) => {
                        // Delete chars matching the tr-style set (ranges
                        // `a-z`, leading `^` negation) — same parser tr uses.
                        let src = s.to_string_lossy();
                        let set_s = set.to_string_lossy();
                        let (set_chars, negated) = match parse_tr_set(&set_s, true) {
                            Ok(t) => t,
                            Err(msg) => return Err(self.trap(RubyError::ArgumentError {
                                msg: msg.to_string(),
                            })),
                        };
                        let setref: std::collections::HashSet<char> = set_chars.into_iter().collect();
                        let out: String = src.chars()
                            .filter(|c| setref.contains(c) == negated)
                            .collect();
                        Some(Value::new_str(out))
                    }
                    ("delete!", [Value::Str(set)]) => {
                        // Destructive `delete` — remove matching chars IN
                        // PLACE; return self if anything changed, nil if
                        // not (CRuby). FrozenError-aware. Surfaced by
                        // stdlib uri/generic.rb's `query=` (`x.delete!`).
                        check_unfrozen(self)?;
                        let src = s.to_string_lossy();
                        let set_s = set.to_string_lossy();
                        let (set_chars, negated) = match parse_tr_set(&set_s, true) {
                            Ok(t) => t,
                            Err(msg) => return Err(self.trap(RubyError::ArgumentError {
                                msg: msg.to_string(),
                            })),
                        };
                        let setref: std::collections::HashSet<char> = set_chars.into_iter().collect();
                        let out: String = src.chars()
                            .filter(|c| setref.contains(c) == negated)
                            .collect();
                        if out == src {
                            Some(Value::Nil)
                        } else {
                            let enc = s.encoding.get();
                            *s.borrow_mut() = out.into_bytes();
                            s.encoding.set(enc);
                            Some(Value::Str(s.clone()))
                        }
                    }
                    ("lines", []) | ("lines", [Value::Str(_)]) => {
                        // `String#lines` — split into lines KEEPING the
                        // separator ("\n" by default). `text.lines`.
                        let src = s.to_string_lossy();
                        let sep = match args.first() {
                            Some(Value::Str(sp)) => sp.to_string_lossy(),
                            _ => "\n".to_string(),
                        };
                        let elems: Vec<Value> = split_lines_keep_sep(&src, &sep)
                            .into_iter()
                            .map(Value::new_str)
                            .collect();
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    ("each_line", []) | ("each_line", [Value::Str(_)]) => {
                        // No-block `each_line` → Enumerator (`s.each_line
                        // .to_a` == `s.lines`); the block form lives in
                        // collection_call_block (iter.rs).
                        return self
                            .make_enum_for(Value::Str(s.clone()), "each_line", args.to_vec())
                            .map(Some);
                    }
                    ("getbyte", [Value::Int(idx)]) => {
                        // Raw byte at a BYTE index (negative counts from
                        // the end); nil when out of range. CRuby's
                        // `String#getbyte`.
                        let bytes = s.borrow();
                        let len = bytes.len() as i64;
                        let i = if *idx < 0 { *idx + len } else { *idx };
                        if i < 0 || i >= len {
                            Some(Value::Nil)
                        } else {
                            Some(Value::Int(bytes[i as usize] as i64))
                        }
                    }
                    ("ord", []) => {
                        // CRuby `String#ord`: the Integer codepoint of
                        // the FIRST character in the receiver's
                        // encoding; ArgumentError on an empty string.
                        // A BINARY string has byte-sized "characters",
                        // so its first byte IS the codepoint (the
                        // common_logger `c.ord` over a `[^[:print:]]`
                        // match can see a high byte). Other encodings
                        // decode the first UTF-8 scalar.
                        let bytes = s.content.borrow();
                        if bytes.is_empty() {
                            return Err(self.trap(RubyError::ArgumentError {
                                msg: "empty string".to_string(),
                            }));
                        }
                        if matches!(s.encoding.get(), crate::value::EncodingTag::Binary) {
                            Some(Value::Int(bytes[0] as i64))
                        } else {
                            drop(bytes);
                            match s.to_string_lossy().chars().next() {
                                Some(c) => Some(Value::Int(c as i64)),
                                None => Some(Value::Int(s.content.borrow()[0] as i64)),
                            }
                        }
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
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    ("split", [Value::Str(sep)]) => {
                        // Byte-faithful path for binary / invalid-UTF-8
                        // receivers with a non-empty, non-AWK sep —
                        // preserves bytes + tag (the `pair.split("=")`
                        // QueryParser shape). Empty / `" "` seps fall
                        // through to the char path below.
                        if wants_byte_faithful(&s) {
                            let sep_b = sep.content.borrow();
                            if !sep_b.is_empty() && &sep_b[..] != b" " {
                                let elems = byte_split_values(
                                    &s.content.borrow(), &sep_b, 0, s.encoding.get());
                                self.maybe_gc();
                                let id = self.heap.alloc(HeapObj::Array(elems.into()));
                                return Ok(Some(Value::Array(id)));
                            }
                        }
                        let sep_s = sep.to_string_lossy();
                        let src = s.to_string_lossy();
                        let elems: Vec<Value> = if sep_s.is_empty() {
                            // CRuby: empty-sep split returns each character.
                            src.chars().map(|c| Value::new_str(c.to_string())).collect()
                        } else if sep_s == " " {
                            // CRuby AWK-style special case: a literal " "
                            // (single space) splits on runs of any whitespace
                            // AND strips leading + trailing empty tokens.
                            // Equivalent to the no-arg `split` form.
                            src.split_whitespace().map(Value::new_str).collect()
                        } else {
                            // CRuby's `split` with no (or zero) limit drops
                            // trailing empty fields: `"a,,".split(",")` =>
                            // ["a"]. Rust's `str::split` keeps them, so trim.
                            let mut parts: Vec<&str> = src.split(sep_s.as_str()).collect();
                            while parts.last() == Some(&"") {
                                parts.pop();
                            }
                            parts.into_iter().map(Value::new_str).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    // `split(regex)` / `split(regex, limit)` —
                    // dual-engine via `CompiledRegex::split_matches`.
                    // Walks the eager match list and emits the
                    // pre-match chunk between consecutive matches,
                    // with capture groups (if any) inserted in
                    // their CRuby positions between the surrounding
                    // chunks.
                    //
                    // Limit semantics:
                    //   absent / 0  : drop trailing empties
                    //   N > 0       : limit bounds the number of
                    //                 CHUNKS / matches processed
                    //                 (so we emit at most N-1
                    //                 matches before the unsplit
                    //                 remainder), NOT the final
                    //                 array length. Captured
                    //                 groups from processed
                    //                 matches are still emitted
                    //                 between chunks, so the
                    //                 result can have more than
                    //                 N elements (CRuby docs:
                    //                 "captured groups will be
                    //                 returned as well, but are
                    //                 not counted towards the
                    //                 limit"). For non-capturing
                    //                 patterns this collapses to
                    //                 "at most N fields".
                    //   N < 0       : split fully, keep trailing
                    //                 empties (and the post-tail
                    //                 empty if the last match ended
                    //                 at end-of-string)
                    // Code-review #357 round 4 — clarified the
                    // capture-group + limit interaction.
                    //
                    // Discovered by TRY_RUNS pass-14 — sinatra-4's
                    // `cleaned_caller` (sinatra/base.rb:1913) does
                    // `line.split(/:(?=\d|in )/, 3)`. Layer #17
                    // unlocked the lookahead pattern's compilation;
                    // this arm makes the split actually run.
                    // (TRY_RUNS pass-14 layer #18.)
                    #[cfg(feature = "regex")]
                    ("split", [Value::Regex(re)]) => {
                        // Byte-faithful for binary / invalid-UTF-8
                        // receivers (preserves bytes + tag); lossless
                        // fast path for valid UTF-8. See
                        // `regex_split_values`.
                        let elems = regex_split_values(&s, re, 0)
                            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    #[cfg(feature = "regex")]
                    ("split", [Value::Regex(re), Value::Int(limit)]) => {
                        let elems = regex_split_values(&s, re, *limit)
                            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
                        Some(Value::Array(id))
                    }
                    ("split", [Value::Str(sep), Value::Int(limit)]) => {
                        // Byte-faithful path (binary / invalid-UTF-8 +
                        // non-empty, non-AWK sep) — the QueryParser
                        // `pair.split("=", 2)` shape.
                        if wants_byte_faithful(&s) {
                            let sep_b = sep.content.borrow();
                            if !sep_b.is_empty() && &sep_b[..] != b" " {
                                let elems = byte_split_values(
                                    &s.content.borrow(), &sep_b, *limit, s.encoding.get());
                                self.maybe_gc();
                                let id = self.heap.alloc(HeapObj::Array(elems.into()));
                                return Ok(Some(Value::Array(id)));
                            }
                        }
                        // `split(sep, limit)` — CRuby semantics:
                        //   limit > 0  : at most `limit` fields; the last
                        //                holds the unsplit remainder; no
                        //                trailing-empty removal.
                        //   limit == 0 : like 1-arg `split` (drop trailing
                        //                empty fields).
                        //   limit < 0  : split fully, keep trailing empties.
                        let sep_s = sep.to_string_lossy();
                        let src = s.to_string_lossy();
                        let limit = *limit;
                        let elems: Vec<Value> = if sep_s.is_empty() {
                            // Empty sep splits into characters; a positive
                            // limit keeps the first `limit-1` chars and
                            // joins the rest into the final field.
                            let cs: Vec<char> = src.chars().collect();
                            if limit > 0 && cs.len() as i64 > limit {
                                let n = (limit - 1) as usize;
                                let mut v: Vec<Value> = cs[..n]
                                    .iter()
                                    .map(|c| Value::new_str(c.to_string()))
                                    .collect();
                                let rest: String = cs[n..].iter().collect();
                                v.push(Value::new_str(rest));
                                v
                            } else {
                                let mut v: Vec<Value> =
                                    cs.iter().map(|c| Value::new_str(c.to_string())).collect();
                                // CRuby quirk: empty separator + negative
                                // limit appends a trailing "" for a
                                // non-empty string (`"abc".split("",-1)` =>
                                // ["a","b","c",""]); empty string => [].
                                if limit < 0 && !cs.is_empty() {
                                    v.push(Value::new_str(String::new()));
                                }
                                v
                            }
                        } else if sep_s == " " {
                            // CRuby AWK-style special case (mirror of the
                            // 1-arg arm above): a literal " " splits on
                            // runs of any whitespace and strips the leading
                            // empty token. Limit interacts as follows:
                            //   limit == 0  : drop trailing empty(s) (same
                            //                 shape as 1-arg "split(\" \")").
                            //   limit < 0   : keep ONE trailing empty if the
                            //                 source ended in whitespace
                            //                 ("  a  b  c  ".split(" ", -1)
                            //                 → ["a","b","c",""]).
                            //   limit > 0   : skip leading WS, then take the
                            //                 first `limit-1` WS-delimited
                            //                 tokens; the last field is the
                            //                 unsplit remainder (including
                            //                 any trailing whitespace).
                            let trimmed_start = src.trim_start();
                            // CRuby quirk: a NON-empty all-whitespace input
                            // with `limit != 0` returns `[""]`, not `[]`.
                            // (`limit == 0` drops trailing empties so even
                            // that single "" gets removed → `[]`.) An empty
                            // source string returns `[]` regardless of limit.
                            let only_ws_nonempty = !src.is_empty() && trimmed_start.is_empty();
                            if limit > 0 {
                                if only_ws_nonempty {
                                    vec![Value::new_str(String::new())]
                                } else {
                                    let mut out: Vec<String> = Vec::with_capacity(limit as usize);
                                    let mut remainder = trimmed_start;
                                    while (out.len() as i64) < limit - 1 {
                                        match remainder.find(char::is_whitespace) {
                                            Some(idx) => {
                                                out.push(remainder[..idx].to_string());
                                                // Skip the WS run at idx so
                                                // the next token starts on
                                                // a non-WS char.
                                                remainder = remainder[idx..].trim_start();
                                                if remainder.is_empty() { break; }
                                            }
                                            None => break,
                                        }
                                    }
                                    if !remainder.is_empty() || !out.is_empty() {
                                        out.push(remainder.to_string());
                                    }
                                    out.into_iter().map(Value::new_str).collect()
                                }
                            } else if limit < 0 {
                                if only_ws_nonempty {
                                    vec![Value::new_str(String::new())]
                                } else {
                                    let mut parts: Vec<Value> = trimmed_start
                                        .split_whitespace()
                                        .map(Value::new_str)
                                        .collect();
                                    // Trailing whitespace at end of source
                                    // => one final "" field. CRuby quirk:
                                    // the sentinel is independent of how
                                    // many whitespace chars trailed.
                                    if !src.is_empty()
                                        && src.chars().last().is_some_and(char::is_whitespace)
                                        && !parts.is_empty()
                                    {
                                        parts.push(Value::new_str(String::new()));
                                    }
                                    parts
                                }
                            } else {
                                // limit == 0 → same as 1-arg form (always
                                // drops trailing empties, including the
                                // all-WS-input collapses-to-[] case).
                                src.split_whitespace().map(Value::new_str).collect()
                            }
                        } else if limit > 0 {
                            src.splitn(limit as usize, sep_s.as_str())
                                .map(Value::new_str)
                                .collect()
                        } else if limit < 0 {
                            src.split(sep_s.as_str()).map(Value::new_str).collect()
                        } else {
                            // limit == 0: drop trailing empty fields.
                            let mut parts: Vec<&str> = src.split(sep_s.as_str()).collect();
                            while parts.last() == Some(&"") {
                                parts.pop();
                            }
                            parts.into_iter().map(Value::new_str).collect()
                        };
                        self.maybe_gc();
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
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
                        let (prepared, p_overrides) = self.sprintf_prepare_args(&fmt_str, fmt_args)?;
                        let out = ruby_sprintf(&fmt_str, &prepared, &self.heap, &self.interner, self.max_value_bytes, &p_overrides)
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
                    // `sub`/`gsub` with a Hash replacement: each match
                    // is looked up (as a String) in the hash and replaced
                    // with the mapped value (`to_s`), or "" when the key
                    // is absent — CRuby's table-driven escape form. This
                    // is what rouge's HTML formatter uses:
                    // `value.gsub(ESCAPE_REGEX, TABLE_FOR_ESCAPE_HTML)`.
                    // Needs heap access to read the Hash, so it lives here
                    // rather than the stateless `string_call`. `sub`
                    // replaces the first match, `gsub` all.
                    #[cfg(feature = "regex")]
                    // `gsub` with ONLY a pattern (no replacement, no
                    // block) → an Enumerator (Ruby 2.6+). Driving it with
                    // a block re-invokes gsub with that block, so the
                    // result is the substituted String —
                    // `"aaa".gsub(/a/).with_index { |m,i| i.to_s }` →
                    // "012", and `"hello".gsub(/l/).count` → 2. Works for
                    // Regex AND String patterns (the block form for a
                    // string literal is handled in collection_call_block).
                    // `sub` has no enumerator form — it needs a
                    // replacement arg.
                    ("gsub", [Value::Regex(_) | Value::Str(_)]) => {
                        return self
                            .make_enum_for(Value::Str(s.clone()), name, vec![args[0].clone()])
                            .map(Some);
                    }
                    // `sub` with a lone pattern (no replacement, no block)
                    // is an arity error in CRuby — sub has no Enumerator
                    // form (only the first match is replaced).
                    #[cfg(feature = "regex")]
                    ("sub", [Value::Regex(_) | Value::Str(_)]) => {
                        return Err(self.trap(RubyError::ArgumentError {
                            msg: "wrong number of arguments (given 1, expected 2)".into(),
                        }));
                    }
                    #[cfg(feature = "regex")]
                    ("sub" | "gsub" | "sub!" | "gsub!", [Value::Regex(re), Value::Hash(hid)]) => {
                        let native = re
                            .as_native()
                            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?
                            .ok_or_else(|| self.trap(RubyError::RuntimeError {
                                msg: format!(
                                    "regex op 'String#{name}' with a Hash replacement is not yet supported on patterns requiring the fancy-regex engine (pattern: /{}/)",
                                    re.as_str(),
                                ),
                            }))?;
                        let pairs = self.heap.hash(*hid).to_vec();
                        let mut table: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        for (k, v) in &pairs {
                            if let Value::Str(ks) = k {
                                let vs = match v {
                                    Value::Str(vstr) => vstr.to_string_lossy(),
                                    other => other.to_display(&self.heap, &self.interner),
                                };
                                table.insert(ks.to_string_lossy(), vs);
                            }
                        }
                        let s_owned = s.to_string_lossy();
                        let single = name == "sub" || name == "sub!";
                        // Cow::Borrowed when no match — lets the bang
                        // variants return nil on no-substitution (CRuby's
                        // contract is "nil iff no match", not "iff
                        // unchanged"). uri's `_encode_uri_component`
                        // (rack set_cookie_header) drives `gsub!(re, table)`.
                        let replaced = if single {
                            native.replace(&s_owned, |caps: &regex::Captures| {
                                table.get(&caps[0]).cloned().unwrap_or_default()
                            })
                        } else {
                            native.replace_all(&s_owned, |caps: &regex::Captures| {
                                table.get(&caps[0]).cloned().unwrap_or_default()
                            })
                        };
                        if name.ends_with('!') {
                            if s.frozen.get() {
                                return Err(self.trap(RubyError::FrozenError {
                                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(&s)),
                                }));
                            }
                            match replaced {
                                std::borrow::Cow::Borrowed(_) => Some(Value::Nil),
                                std::borrow::Cow::Owned(new_str) => {
                                    *s.borrow_mut() = new_str.into_bytes();
                                    Some(Value::Str(s.clone()))
                                }
                            }
                        } else {
                            Some(Value::new_str(replaced.into_owned()))
                        }
                    }
                    // String-pattern Hash form: the literal pattern is the
                    // only possible match key, so resolve its replacement
                    // once and do a plain substring replace.
                    ("sub" | "gsub" | "sub!" | "gsub!", [Value::Str(pat), Value::Hash(hid)]) => {
                        let pairs = self.heap.hash(*hid).to_vec();
                        let pat_s = pat.to_string_lossy();
                        let mut repl = String::new();
                        for (k, v) in &pairs {
                            if let Value::Str(ks) = k
                                && ks.to_string_lossy() == pat_s
                            {
                                repl = match v {
                                    Value::Str(vstr) => vstr.to_string_lossy(),
                                    other => other.to_display(&self.heap, &self.interner),
                                };
                                break;
                            }
                        }
                        let s_owned = s.to_string_lossy();
                        let single = name == "sub" || name == "sub!";
                        let matched = !pat_s.is_empty() && s_owned.contains(pat_s.as_str());
                        let out = if single {
                            s_owned.replacen(pat_s.as_str(), &repl, 1)
                        } else {
                            s_owned.replace(pat_s.as_str(), &repl)
                        };
                        if name.ends_with('!') {
                            if s.frozen.get() {
                                return Err(self.trap(RubyError::FrozenError {
                                    msg: format!("can't modify frozen String: {}", crate::heap::rstr_inspect(&s)),
                                }));
                            }
                            if !matched {
                                Some(Value::Nil)
                            } else {
                                *s.borrow_mut() = out.into_bytes();
                                Some(Value::Str(s.clone()))
                            }
                        } else {
                            Some(Value::new_str(out))
                        }
                    }
                    // `s.each_char` with no block → Enumerator (the block
                    // form is in collection_call_block). `s.each_char.to_a`
                    // == `s.chars`.
                    ("each_byte", []) => {
                        // No-block → Enumerator (`s.each_byte.to_a`); the
                        // block form lives in collection_call_block.
                        return self.make_enum_for(Value::Str(s.clone()), "each_byte", vec![]).map(Some);
                    }
                    ("each_char", []) => {
                        return self.make_enum_for(Value::Str(s.clone()), "each_char", vec![]).map(Some);
                    }
                    #[cfg(feature = "regex")]
                    ("scan", [Value::Regex(re)]) => {
                        // Dual-engine (regex + fancy-regex). Iterate into
                        // OwnedCaptures first so both the returned Array and
                        // `$~` final-state come from the same match data.
                        // (binary input degrades to lossy UTF-8; the engines
                        // only match UTF-8.)
                        let s_owned = s.to_string_lossy();
                        let has_groups = re
                            .captures_len()
                            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?
                            > 1;
                        let matches = re
                            .captures_iter_owned(&s_owned)
                            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
                        // CRuby updates `$~` for no-block scan too: final
                        // match on success, nil on no match. Do this only
                        // after the match walk succeeds so a fancy-regex
                        // runtime error doesn't clobber the caller's `$~`.
                        self.save_match_scope_on_write();
                        self.last_match = matches
                            .last()
                            .map(|oc| self.last_match_from_owned_captures(re, &s_owned, oc));
                        // GC rooting: under STRESS_GC=1 each per-match
                        // sub-Array alloc'd in the has_groups branch is
                        // unreachable until the wrapping result Array is
                        // built — pin each push so it survives subsequent
                        // maybe_gc's. The no-groups branch alloc's only
                        // Strings (Rc-based, not heap-managed by ObjId),
                        // so no pin is needed there.
                        let mut g = PinGuard::new(self);
                        let mut out: Vec<Value> = Vec::with_capacity(matches.len());
                        if has_groups {
                            for caps in &matches {
                                let mut group_vec: Vec<Value> = Vec::with_capacity(caps.groups.len());
                                for grp in &caps.groups {
                                    group_vec.push(
                                        grp.as_ref()
                                            .map(|t| Value::new_str(t.clone()))
                                            .unwrap_or(Value::Nil),
                                    );
                                }
                                g.vm.maybe_gc();
                                g.vm.check_alloc()?;
                                let gid = g.vm.heap.alloc(HeapObj::Array(group_vec.into()));
                                let v = Value::Array(gid);
                                g.pin(v.clone());
                                out.push(v);
                            }
                        } else {
                            for caps in &matches {
                                out.push(Value::new_str(caps.whole.clone()));
                            }
                        }
                        g.vm.maybe_gc();
                        g.vm.check_alloc()?;
                        let id = g.vm.heap.alloc(HeapObj::Array(out.into()));
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
                        let id = self.heap.alloc(HeapObj::Array(parts.into()));
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
                        let id = self.heap.alloc(HeapObj::Array(elems.into()));
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
                        let id = self.heap.alloc(HeapObj::Array(result.into()));
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
                    // `intern` is a CRuby alias of `to_sym`; kramdown's
                    // `configurable.rb` calls `name.intern`.
                    ("to_sym", []) | ("intern", []) => {
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
        re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
        n: i64,
    ) -> Result<Value, Trap> {
        // Dual-engine via `captures_owned` (the same normalized
        // owned-captures path `Regexp#match` uses) — lookaround /
        // backref patterns route through fancy-regex transparently.
        // rack's multipart parser slices MIME headers with
        // `/Content-Disposition:(.*)(?=...)/` (~70 of its specs
        // died on the old native-only trap here). A fancy-engine
        // match-time error (backtracking blow-up) still traps.
        let bound = s.to_string_lossy();
        let owned = re
            .captures_owned(&bound)
            .map_err(|e| self.trap(e.to_ruby_error(re.as_str())))?;
        let oc = match owned {
            None => {
                self.save_match_scope_on_write();
                self.last_match = None;
                return Ok(Value::Nil);
            }
            Some(oc) => oc,
        };
        let picked = if n == 0 {
            Some(oc.whole.clone())
        } else if n > 0 && (n as usize) <= oc.groups.len() {
            oc.groups[(n as usize) - 1].clone()
        } else {
            None
        };
        let cap_names = re.capture_group_names();
        self.save_match_scope_on_write();
        self.last_match = Some(crate::vm::LastMatch {
            whole: oc.whole,
            caps: oc.groups,
            input: bound,
            m_start: oc.m_start,
            m_end: oc.m_end,
            named: oc.named,
            group_spans: oc.group_spans,
            cap_names,
            binary: None,
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

/// First index of `needle` in `hay`, or `None`. Tiny, used by the
/// byte-faithful string-split path.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Byte-faithful `String#split(literal_sep, limit)` for binary /
/// invalid-UTF-8 receivers with a NON-empty separator (the `" "`
/// AWK form and empty-sep form stay on the char path). Splits the raw
/// bytes on the raw separator bytes — preserving every byte and the
/// receiver's encoding — so rack's QueryParser `pair.split("=", 2)`
/// on a `_method=\xBF` (BINARY) pair keeps the invalid byte instead of
/// U+FFFD-mangling it. CRuby limit semantics: `>0` caps the field
/// count (last field is the unsplit remainder), `0` drops trailing
/// empties, `<0` keeps them.
fn byte_split_values(
    bytes: &[u8],
    sep: &[u8],
    limit: i64,
    enc: crate::value::EncodingTag,
) -> Vec<Value> {
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut start = 0usize;
    if limit > 0 {
        while (chunks.len() as i64) + 1 < limit {
            match find_subslice(&bytes[start..], sep) {
                Some(pos) => {
                    chunks.push(bytes[start..start + pos].to_vec());
                    start += pos + sep.len();
                }
                None => break,
            }
        }
        chunks.push(bytes[start..].to_vec());
    } else {
        while let Some(pos) = find_subslice(&bytes[start..], sep) {
            chunks.push(bytes[start..start + pos].to_vec());
            start += pos + sep.len();
        }
        chunks.push(bytes[start..].to_vec());
        if limit == 0 {
            while chunks.last().map(|c| c.is_empty()).unwrap_or(false) {
                chunks.pop();
            }
        }
    }
    chunks
        .into_iter()
        .map(|c| with_tag(Value::new_str_bytes_binary(c), enc))
        .collect()
}

/// True when a String#split should take the byte-faithful path: the
/// receiver isn't valid UTF-8 for its tag (BINARY, or a UTF-8 tag with
/// invalid bytes). Valid UTF-8 (incl. ASCII) keeps the char path.
/// True when `String#split` must operate byte-faithfully rather than via
/// a lossy UTF-8 view: an ASCII-8BIT (BINARY) receiver, or a receiver
/// whose bytes aren't valid UTF-8. The lossy path would turn every
/// invalid byte into a 3-byte U+FFFD, corrupting and growing the chunks.
/// (`sub`/`gsub` gate on ASCII-8BIT only — CRuby raises on a
/// UTF-8-tagged-but-invalid receiver there rather than byte-replacing.)
fn wants_byte_faithful(s: &crate::value::RStr) -> bool {
    use crate::value::EncodingTag;
    s.encoding.get() == EncodingTag::Binary || !s.content.is_utf8_cached()
}

/// Predicate-match fast path for the `match?` family: on a KNOWN-
/// valid-UTF-8 (non-BINARY) receiver, resolve the char-index `pos`
/// in O(1) (ASCII identity / cached `char_starts`) and run
/// `is_match_from` on a BORROWED view of the content — no subject
/// copy, no per-call chars walk. Returns `None` when the receiver is
/// BINARY or not valid UTF-8 (caller falls back to its lossy path);
/// `Some(Ok(false))` for an out-of-range `pos` (CRuby: no match);
/// `Some(Err(..))` when the deferred engine build failed (caller
/// raises RegexpError).
#[cfg(feature = "regex")]
fn is_match_at_char_pos(
    s: &crate::value::RStr,
    pos: i64,
    re: &crate::regex_engine::CompiledRegex,
) -> Option<Result<bool, crate::regex_engine::RegexOpError>> {
    if s.encoding.get() == crate::value::EncodingTag::Binary || !s.content.is_utf8_cached() {
        return None;
    }
    let byte_off = if pos == 0 {
        0
    } else if s.content.is_ascii_cached() {
        let char_len = s.content.borrow().len() as i64;
        let cpos = if pos < 0 { char_len + pos } else { pos };
        if cpos < 0 || cpos > char_len {
            return Some(Ok(false));
        }
        cpos as usize
    } else {
        let starts = s.content.char_starts();
        let char_len = (starts.len() - 1) as i64;
        let cpos = if pos < 0 { char_len + pos } else { pos };
        if cpos < 0 || cpos > char_len {
            return Some(Ok(false));
        }
        starts[cpos as usize] as usize
    };
    let bytes = s.content.borrow();
    debug_assert!(std::str::from_utf8(&bytes).is_ok());
    // SAFETY: `is_utf8_cached` above (every content mutation goes
    // through `borrow_mut`, which resets the cache); `byte_off` is a
    // char-boundary offset (ASCII identity or `char_starts` entry).
    let view = unsafe { std::str::from_utf8_unchecked(&bytes) };
    Some(re.is_match_from(&view[byte_off..]))
}

#[cfg(feature = "regex")]
/// `String#split(regex)` that PRESERVES the receiver's bytes and
/// encoding. Valid UTF-8 receivers take the lossless fast path (chunks
/// tagged UTF-8 — the common case, unchanged). Binary / invalid-UTF-8
/// receivers are split via a Latin-1 round-trip: each byte ⇆ a
/// U+00..FF char, so the regex still matches ASCII separators at the
/// right positions and every chunk re-encodes to its EXACT original
/// bytes, then re-tagged with the receiver's encoding. Without this,
/// `with_str_lossy` U+FFFD-mangled high bytes and dropped the tag — so
/// rack's MethodOverride `_method=\xBF` lost its invalid byte and the
/// subsequent `.upcase` no longer raised ArgumentError.
fn regex_split_values(
    s: &std::rc::Rc<crate::value::RStr>,
    re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
    limit: i64,
) -> Result<Vec<Value>, crate::regex_engine::RegexOpError> {
    use crate::value::EncodingTag;
    let enc = s.encoding.get();
    {
        let b = s.content.borrow();
        if std::str::from_utf8(&b).is_ok() && enc != EncodingTag::Binary {
            let src = String::from_utf8_lossy(&b);
            return regex_split_into_values(re, &src, limit);
        }
    }
    // Latin-1 decode (1 byte → 1 char) so the split is byte-faithful.
    let latin1: String = s.content.borrow().iter().map(|&byte| byte as char).collect();
    let raw = regex_split_into_values(re, &latin1, limit)?;
    Ok(raw
        .into_iter()
        .map(|v| match v {
            Value::Str(cs) => {
                // The chunk is a Latin-1 substring; recover its bytes
                // from the chars (each is U+00..FF) and re-tag with the
                // receiver's encoding.
                let chunk = String::from_utf8_lossy(&cs.content.borrow()).into_owned();
                let bytes: Vec<u8> = chunk.chars().map(|c| c as u32 as u8).collect();
                with_tag(Value::new_str_bytes_binary(bytes), enc)
            }
            other => other,
        })
        .collect())
}

#[cfg(feature = "regex")]
/// `String#split(regex[, limit])` shared core. Walks the
/// `CompiledRegex::split_matches` output and emits a
/// `Vec<Value>` matching CRuby's `split` semantics. Each
/// element is either a `Value::Str` (chunk between matches or
/// participating capture group) or a `Value::Nil` (capture
/// group that didn't participate in the match — e.g. an `|`
/// alternative arm that wasn't taken). Code-review #357 round 1
/// corrected the doc — the previous claim of `Vec<Value::Str>`
/// missed the `Nil` capture-group case.
///
///   - Each pre-match chunk is pushed as its own element.
///   - For each match, any captured groups are pushed after
///     the chunk preceding the match (CRuby's "split keeps
///     parenthesised groups" rule).
///   - After all matches, the post-tail chunk is pushed.
///
/// Limit handling:
///   - `limit == 0`: drop trailing empty fields (including
///     the post-tail empty if the last match ended at EOS).
///   - `limit > 0`: `limit` bounds the number of CHUNKS /
///     matches processed (we emit at most `limit - 1` matches
///     before the unsplit remainder), NOT the final array
///     length. Captured groups from processed matches are
///     still emitted between the surrounding chunks, so the
///     result can have MORE than `limit` elements when the
///     pattern has captures (per CRuby docs: "captured groups
///     will be returned as well, but are not counted towards
///     the limit"). For non-capturing patterns this collapses
///     to "at most `limit` fields". No captures are emitted
///     from the truncating match itself — the remainder is
///     pushed verbatim. Code-review #357 round 4 clarified
///     this contract.
///   - `limit < 0`: emit all fields, keep trailing empties.
///
/// Zero-width matches (e.g. lookaround patterns like the
/// sinatra `/:(?=\d|in )/`) are accepted; the underlying
/// engines' iter cursors handle zero-width loop avoidance
/// internally, so we don't force an extra char step here.
///
/// (TRY_RUNS pass-14 layer #18.)
fn regex_split_into_values(
    re: &std::rc::Rc<crate::regex_engine::CompiledRegex>,
    src: &str,
    limit: i64,
) -> Result<Vec<Value>, crate::regex_engine::RegexOpError> {
    use crate::regex_engine::SplitMatch;
    // CRuby parity: empty source returns `[]` regardless of
    // limit. (`"".split(/,/)` => `[]`.)
    if src.is_empty() {
        return Ok(Vec::new());
    }
    let limit_pos = limit > 0;
    // Saturating i64 → usize for the per-chunk cap. `limit as
    // usize` would wrap on 32-bit targets (wasm32-wasip1) when
    // `limit` exceeds u32::MAX, producing surprise early
    // truncation. Clamp to usize::MAX instead — semantically
    // "effectively unlimited", which is what oversized
    // positive limits should mean. Code-review #357 round 1.
    let max_chunks_before_tail = if limit_pos {
        usize::try_from(limit - 1).unwrap_or(usize::MAX)
    } else {
        usize::MAX
    };
    // Bounded eager collection: when `limit_pos`, we only need
    // `max_chunks_before_tail` matches (the truncating tail
    // consumes the rest verbatim). `Some(n)` short-circuits
    // the engine walk so e.g. `huge.split(/,/, 2)` finds one
    // match and bails. `None` collects all (negative or zero
    // limit). Code-review #357 round 1.
    //
    // +1 compensation for the at-0 zero-width skip below: the
    // walker discards a leading zero-width match at byte 0
    // (CRuby convention — `"abc".split(//)` is `["a","b","c"]`
    // not `["", "a", "b", "c"]`). If we passed the exact bound
    // here, `"abc".split(//, 2)` would collect only the (0,0)
    // match, skip it, and emit nothing — falling through to the
    // truncating tail and producing `["abc"]` instead of the
    // CRuby-correct `["a", "bc"]`. The +1 ensures we collect
    // enough matches to survive at most one skip. Code-review
    // #357 post-review-pass.
    let collection_bound = if limit_pos {
        Some(max_chunks_before_tail.saturating_add(1))
    } else {
        None
    };
    let matches: Vec<SplitMatch> = re.split_matches(src, collection_bound)?;
    let mut out: Vec<Value> = Vec::new();
    let mut last_end: usize = 0;
    let mut chunks_emitted: usize = 0;

    for m in matches.iter() {
        if limit_pos && chunks_emitted >= max_chunks_before_tail {
            break;
        }
        let (s_start, s_end) = m.range;
        // CRuby parity: a zero-width match at byte 0 is
        // skipped (it would otherwise emit an empty leading
        // chunk that confuses lookahead patterns). Subsequent
        // zero-width matches are kept — they're how
        // `/:(?=\d|in )/` produces the expected fragments.
        if s_start == s_end && s_start == last_end && s_start == 0 {
            continue;
        }
        out.push(Value::new_str(src[last_end..s_start].to_string()));
        chunks_emitted += 1;
        // CRuby's "groups included" rule: each captured group
        // is pushed between the surrounding chunks. None for
        // unmatched groups.
        for g in m.groups.iter() {
            match g {
                Some((gs, ge)) => out.push(Value::new_str(src[*gs..*ge].to_string())),
                None => out.push(Value::Nil),
            }
        }
        // Advance past the match. For zero-width matches
        // (s_end == s_start) we DON'T force a char step here:
        // the underlying engines' `captures_iter` /
        // `find_iter` already advance their internal cursor
        // past a zero-width hit, so subsequent matches won't
        // re-fire at the same position. Forcing a step here
        // would over-advance and consume one character between
        // chunks — e.g. `"$10 $20".split(/(?<=\$)/)` would
        // drop the "1" because the zero-width match after the
        // first "$" would skip it. CRuby's behaviour: the
        // chunk between consecutive zero-width matches is the
        // literal slice src[m1.end..m2.start].
        last_end = s_end;
    }
    // Tail.
    if limit_pos && chunks_emitted >= max_chunks_before_tail {
        // Truncating field: unsplit remainder verbatim.
        out.push(Value::new_str(src[last_end..].to_string()));
    } else {
        out.push(Value::new_str(src[last_end..].to_string()));
        // Drop trailing empties when limit is 0 (the default
        // / explicit-0 case).
        if !limit_pos && limit == 0 {
            while matches!(out.last(), Some(Value::Str(s)) if s.with_str_lossy(|x| x.is_empty())) {
                out.pop();
            }
        }
    }
    Ok(out)
}

/// Translate Ruby's `\0` / `\1` / … / `\k<name>` backref syntax
/// in a String#gsub replacement template into the `regex` crate's
/// `${0}` / `${1}` / `${name}` convention. Doubled backslash
/// (`\\`) escapes a literal backslash. `\&` is the entire match
/// (CRuby alias for `\0`); `\'` (post-match) / `\`` (pre-match)
/// are NOT supported in our subset — they'd need MatchData state
/// we don't currently carry.
///
/// Numbered and named refs use the *brace* form (`${1}`, not
/// `$1`): the bare `$1` form makes the regex crate greedily read
/// a following alnum into the group name — `'\1X'` becomes `$1X`,
/// parsed as a group literally named `1X` (never exists → empty),
/// silently dropping the capture. `${1}X` is unambiguous.
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
                    out.push_str("${");
                    out.push(n);
                    out.push('}');
                }
                Some(&'&') => {
                    chars.next();
                    out.push_str("${0}");
                }
                // `\k<name>` / `\k'name'` — named backref → `${name}`.
                Some(&'k') => {
                    chars.next(); // consume `k`
                    let close = match chars.peek() {
                        Some('<') => Some('>'),
                        Some('\'') => Some('\''),
                        _ => None,
                    };
                    if let Some(close) = close {
                        chars.next(); // consume the opening delimiter
                        out.push_str("${");
                        for nc in chars.by_ref() {
                            if nc == close {
                                break;
                            }
                            out.push(nc);
                        }
                        out.push('}');
                    } else {
                        // Bare `\k…` with no delimiter — not a CRuby
                        // backref form; keep the literal `\k`.
                        out.push('\\');
                        out.push('k');
                    }
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

/// Standard Base64 alphabet (RFC 4648), used by the `m` pack/unpack
/// directive and `require "base64"`.
const BASE64_TBL: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Raw Base64 of `input` (no line breaks). Pads with `=` to a
/// multiple of 4.
fn base64_raw(input: &[u8]) -> String {
    let mut s = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        s.push(BASE64_TBL[(b0 >> 2) as usize] as char);
        s.push(BASE64_TBL[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        s.push(if chunk.len() > 1 {
            BASE64_TBL[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            BASE64_TBL[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    s
}

/// `Array#pack("m")` encoder. `rfc2045` (the default `m` / `m*` / `mN`
/// form) inserts a newline every 60 output chars AND appends a
/// trailing newline (CRuby); `m0` (rfc2045 = false) is plain RFC 4648
/// with no breaks. Empty input → "" in both modes (matches CRuby).
pub(crate) fn base64_encode(input: &[u8], rfc2045: bool) -> String {
    let raw = base64_raw(input);
    if !rfc2045 || raw.is_empty() {
        return raw;
    }
    let mut out = String::with_capacity(raw.len() + raw.len() / 60 + 1);
    let mut i = 0;
    while i < raw.len() {
        let end = (i + 60).min(raw.len());
        out.push_str(&raw[i..end]);
        out.push('\n');
        i = end;
    }
    out
}

/// `String#unpack("m")` decoder — tolerant Base64: skips any
/// non-alphabet byte (whitespace / newlines, as RFC 2045 mandates)
/// and stops at the first `=` padding.
pub(crate) fn base64_decode(input: &[u8]) -> Vec<u8> {
    let mut rev = [255u8; 256];
    for (i, &c) in BASE64_TBL.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in input {
        if c == b'=' {
            break;
        }
        let v = rev[c as usize];
        if v == 255 {
            continue;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// `String#unpack("m0")` — STRICT Base64 (RFC 4648). Unlike the
/// tolerant `m`/`base64_decode`, this rejects (returns `None`):
///   - a total length that isn't a multiple of 4,
///   - any non-alphabet byte (whitespace, newlines, `-`/`_`, ...),
///   - `=` padding anywhere but the trailing 1-2 chars of the last group,
///   - non-canonical encodings whose pre-padding leftover bits aren't 0
///     (e.g. `"YW=="`, where 4 stray bits would be dropped).
///
/// Matches CRuby's `m0` exactly — the base64 stdlib gem's
/// `strict_decode64` / `urlsafe_decode64` rely on this strictness.
pub(crate) fn base64_decode_strict(input: &[u8]) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(4) {
        return None;
    }
    let mut rev = [255u8; 256];
    for (i, &c) in BASE64_TBL.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let groups = input.len() / 4;
    for g in 0..groups {
        let chunk = &input[g * 4..g * 4 + 4];
        let is_last = g == groups - 1;
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        let mut j = 0;
        while j < 4 {
            let c = chunk[j];
            if c == b'=' {
                pad = 4 - j;
                // The rest of the group must all be padding...
                if chunk[j..].iter().any(|&x| x != b'=') {
                    return None;
                }
                // ...and padding only appears in the final group.
                if !is_last {
                    return None;
                }
                break;
            }
            let v = rev[c as usize];
            if v == 255 {
                return None;
            }
            vals[j] = v;
            j += 1;
        }
        match pad {
            0 => {
                out.push((vals[0] << 2) | (vals[1] >> 4));
                out.push((vals[1] << 4) | (vals[2] >> 2));
                out.push((vals[2] << 6) | vals[3]);
            }
            1 => {
                // 3 data chars → 2 bytes; the low 2 bits of char 3 are dropped.
                if vals[2] & 0x03 != 0 {
                    return None;
                }
                out.push((vals[0] << 2) | (vals[1] >> 4));
                out.push((vals[1] << 4) | (vals[2] >> 2));
            }
            2 => {
                // 2 data chars → 1 byte; the low 4 bits of char 2 are dropped.
                if vals[1] & 0x0f != 0 {
                    return None;
                }
                out.push((vals[0] << 2) | (vals[1] >> 4));
            }
            // pad of 3 or 4 (e.g. "====") is never valid.
            _ => return None,
        }
    }
    Some(out)
}

/// Subset of CRuby's `String#unpack` — see the per-directive
/// table in the call-site comment. Returns Err with a CRuby-
/// ish message on unsupported directives or malformed input.
/// Decode one UTF-8 scalar from the front of `bytes`, returning
/// `(codepoint, byte_len)`. Used by `unpack("U")`. Returns `None` on a
/// truncated or invalid lead/continuation byte (the caller raises, as CRuby
/// does for malformed UTF-8). Faithful for valid UTF-8 (the real-world case);
/// overlong/surrogate rejection is not modeled.
fn decode_utf8_char(bytes: &[u8]) -> Option<(u32, usize)> {
    let b0 = *bytes.first()?;
    let (len, mut cp) = match b0 {
        0x00..=0x7F => return Some((b0 as u32, 1)),
        0xC0..=0xDF => (2, (b0 & 0x1F) as u32),
        0xE0..=0xEF => (3, (b0 & 0x0F) as u32),
        0xF0..=0xF7 => (4, (b0 & 0x07) as u32),
        _ => return None,
    };
    if bytes.len() < len {
        return None;
    }
    for &b in &bytes[1..len] {
        if b & 0xC0 != 0x80 {
            return None;
        }
        cp = (cp << 6) | (b & 0x3F) as u32;
    }
    Some((cp, len))
}

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
            'D' | 'd' | 'E' | 'G' => {
                // 64-bit IEEE double. D/d = native-endian, E = little-endian,
                // G = big-endian (CRuby). prism's `Serialize#load_double`
                // reads `unpack1("D")` for every Float literal in the parsed
                // source, so this is on the RuboCop/parser_prism hot path.
                let take = if n == usize::MAX { (input.len() - i) / 8 } else { n };
                for _ in 0..take {
                    if i + 8 > input.len() { out.push(Value::Nil); break; }
                    let b = [input[i], input[i+1], input[i+2], input[i+3],
                             input[i+4], input[i+5], input[i+6], input[i+7]];
                    i += 8;
                    let v: f64 = match dir {
                        'E' => f64::from_le_bytes(b),
                        'G' => f64::from_be_bytes(b),
                        _   => f64::from_ne_bytes(b), // D / d (native)
                    };
                    out.push(Value::Float(v));
                }
            }
            'F' | 'f' | 'e' | 'g' => {
                // 32-bit IEEE float. F/f = native-endian, e = little-endian,
                // g = big-endian (CRuby). Widened to Ruby Float (f64).
                let take = if n == usize::MAX { (input.len() - i) / 4 } else { n };
                for _ in 0..take {
                    if i + 4 > input.len() { out.push(Value::Nil); break; }
                    let b = [input[i], input[i+1], input[i+2], input[i+3]];
                    i += 4;
                    let v: f32 = match dir {
                        'e' => f32::from_le_bytes(b),
                        'g' => f32::from_be_bytes(b),
                        _   => f32::from_ne_bytes(b), // F / f (native)
                    };
                    out.push(Value::Float(v as f64));
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
            'm' => {
                // Base64. `m0` (count 0) is STRICT RFC 4648 — rejects
                // whitespace / bad padding / non-canonical input with
                // ArgumentError "invalid base64" (what base64's
                // strict_decode64 / urlsafe_decode64 rely on). Plain
                // `m` (or nonzero count) is the tolerant RFC 2045 form
                // (skips whitespace, stops at padding) — rack's
                // basic-auth reader does `credentials.unpack1('m')`.
                // Either way decodes the REST of the input.
                let decoded = if n == 0 {
                    base64_decode_strict(&input[i..])
                        .ok_or_else(|| "invalid base64".to_string())?
                } else {
                    base64_decode(&input[i..])
                };
                i = input.len();
                out.push(Value::new_str_bytes(decoded));
            }
            'U' => {
                // UTF-8 → Unicode codepoints. Count = number of chars
                // (`*` = all remaining). tzinfo/builder-style readers.
                let limit = if n == usize::MAX { usize::MAX } else { n };
                let mut produced = 0usize;
                while produced < limit && i < input.len() {
                    let (cp, len) = decode_utf8_char(&input[i..])
                        .ok_or_else(|| "malformed UTF-8 character in unpack".to_string())?;
                    out.push(Value::Int(cp as i64));
                    i += len;
                    produced += 1;
                }
            }
            'x' => {
                // Skip forward n bytes (no output). `*` skips to the end.
                // CRuby raises "x outside of string" past the end.
                let skip = if n == usize::MAX { input.len().saturating_sub(i) } else { n };
                if i + skip > input.len() {
                    return Err("x outside of string".to_string());
                }
                i += skip;
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
            'D' | 'd' | 'E' | 'G' => {
                // 64-bit IEEE double (D/d native, E little-endian, G big-
                // endian). Integers coerce to Float, matching CRuby.
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Float(0.0));
                    vi += 1;
                    let f = match v {
                        Value::Float(f) => f,
                        Value::Int(n) => n as f64,
                        _ => return Err("pack: expected Float for D/d/E/G".into()),
                    };
                    let b: [u8; 8] = match dir {
                        'E' => f.to_le_bytes(),
                        'G' => f.to_be_bytes(),
                        _   => f.to_ne_bytes(),
                    };
                    out.extend_from_slice(&b);
                }
            }
            'F' | 'f' | 'e' | 'g' => {
                // 32-bit IEEE float (F/f native, e little-endian, g big-
                // endian). Integers and Floats both coerce to f32.
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Float(0.0));
                    vi += 1;
                    let f = match v {
                        Value::Float(f) => f as f32,
                        Value::Int(n) => n as f32,
                        _ => return Err("pack: expected Float for F/f/e/g".into()),
                    };
                    let b: [u8; 4] = match dir {
                        'e' => f.to_le_bytes(),
                        'g' => f.to_be_bytes(),
                        _   => f.to_ne_bytes(),
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
            'm' => {
                // Base64-encode the next String. `m0` → plain RFC 4648
                // (no breaks); `m` / `m*` / `mN` → RFC 2045 (newline
                // every 60 chars + trailing newline, CRuby default).
                // rack's basic-auth test builds the header with
                // `["user:pass"].pack("m*")`.
                let v = values.get(vi).cloned().unwrap_or_else(|| Value::new_str(""));
                vi += 1;
                let bytes: Vec<u8> = match v {
                    Value::Str(s) => s.borrow().clone(),
                    _ => return Err("pack: expected String for m".into()),
                };
                out.extend_from_slice(base64_encode(&bytes, n != 0).as_bytes());
            }
            'U' => {
                // Unicode codepoints → UTF-8 bytes. builder's XChar does
                // `[item].pack('U')` / `seq.pack('U*')`.
                let take = if n == usize::MAX { values.len() - vi } else { n };
                for _ in 0..take {
                    let v = values.get(vi).cloned().unwrap_or(Value::Int(0));
                    vi += 1;
                    let cp = match v {
                        Value::Int(n) => n,
                        _ => return Err("pack: expected Integer for U".into()),
                    };
                    let ch = u32::try_from(cp)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| "pack: invalid codepoint for U".to_string())?;
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
            }
            'x' => {
                // Null padding (consumes no value). `*` emits nothing.
                let take = if n == usize::MAX { 0 } else { n };
                out.extend(std::iter::repeat_n(0u8, take));
            }
            ' ' | '\t' | '\n' => {}
            _ => return Err(format!("unsupported pack/unpack directive '{}'", dir)),
        }
    }
    Ok(out)
}
