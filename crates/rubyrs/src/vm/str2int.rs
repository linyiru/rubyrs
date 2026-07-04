//! Shared string→Integer scanner — the ONE implementation behind
//! every entry point that folds a digit string into a Ruby Integer:
//!
//!   - `String#to_i(base = 10)`   (lenient, `string.rs`)
//!   - `String#hex` / `String#oct` (lenient, `string.rs`)
//!   - `Kernel#Integer(str, base)` (strict, `kernel.rs`)
//!   - `sprintf`'s `%d`/`%x`/`%o`/`%b` String-arg coercion (strict,
//!     `sprintf.rs`)
//!
//! History: each of those sites used to carry its own private fold
//! with i64 `wrapping_mul`/`wrapping_add` — silent data corruption
//! past i64 range (`"18446744073709551616".to_i` → `0`,
//! `"123…890".to_i` → a wrapped negative). The excuse was that
//! `string_call` is a stateless free function with no VM access to
//! allocate a `Value::BigInt` (heap-slot-backed, `ObjId`). The fix
//! keeps the stateless fast path — this scanner returns a
//! [`ParsedInt`] that is `Small(i64)` in the overwhelmingly common
//! case — and lets each VM-context caller lift `Big` results through
//! `Vm::bigint_to_value` (demote-on-fit + `maybe_gc` + alloc-cap).
//!
//! ## CRuby semantics (probed against 3.4.1 — see the diff fixtures)
//!
//! `base` follows CRuby's internal convention:
//!   - `0`      → auto-detect: `0x/0X→16`, `0b/0B→2`, `0o/0O→8`,
//!     `0d/0D→10`, bare leading `0` → 8 (the `0` stays a digit),
//!     else 10.
//!   - `2..=36` → explicit; a prefix is consumed ONLY when it
//!     matches the base (`"0xff".to_i(16)` → 255, but
//!     `"0b10".to_i(16)` → 0xb10 = 2832 — `b` is just a hex digit).
//!   - `< 0`    → auto-detect with default `|base|` (`-1` → 10).
//!     Only `Kernel#Integer` accepts negative bases; `"042"` with
//!     base `-16` is octal 34 because ANY prefix (incl. bare `0`)
//!     overrides the default.
//!
//! Whitespace: ASCII-only `[ \t\n\v\f\r]` — CRuby's `rb_isspace`.
//! Unicode spaces (NBSP…) are NOT skipped: `" 42".to_i` → 0.
//!
//! Underscores: valid only BETWEEN two digits (of the effective
//! base). Lenient mode stops at an ill-placed `_` (`"1__0".to_i` →
//! 1, `"1_".to_i` → 1, `"_1".to_i` → 0, `"0x_10".to_i(16)` → 0);
//! strict mode hard-fails (`Integer("1__0")` raises).
//!
//! Strict mode additionally requires: at least one digit, and
//! nothing but trailing ASCII whitespace after the digits
//! (`Integer("42abc")` / `Integer("08")` — `8` is no octal digit —
//! both raise).
//!
//! Overflow: digits fold in a u64 magnitude with CHECKED arithmetic;
//! on overflow the fold continues into a `num_bigint::BigUint`
//! (chunked — up to a u64's worth of digits folded natively between
//! bigint ops), so results past i64 range come back as exact
//! `ParsedInt::Big`. The i64::MIN edge (`"-9223372036854775808"`)
//! stays `Small`. With the `bignum` feature OFF the fold falls back
//! to the historical two's-complement wrapping (bit-identical to the
//! old per-site `String#to_i` / `Integer()` folds), matching the
//! feature's documented contract ("arithmetic falls back to
//! wrapping"). One INTENTIONAL no-bignum behavior change rides
//! along: sprintf's `%d`-family String coercion previously used
//! `parse::<i64>().unwrap_or(0)` (overflow → `"0"`, not a wrap);
//! routing it through this fold means no-bignum overflow strings
//! now render the wrapped value — consistent with the rest of the
//! no-bignum family instead of the old silent `"0"`.

use crate::error::RubyError;

#[cfg(feature = "bignum")]
use num_bigint::{BigInt, BigUint};

/// Result of a fold: an i64 when the value fits (the common case,
/// allocation-free), otherwise the exact big integer for a
/// VM-context caller to lift via `Vm::bigint_to_value`.
pub(crate) enum ParsedInt {
    Small(i64),
    #[cfg(feature = "bignum")]
    Big(BigInt),
}

/// Digit value of an ASCII byte (`0-9a-zA-Z` → 0..=35); anything
/// else maps to 99 so a plain `< radix` test rejects it.
#[inline]
fn digit_val(b: u8) -> u32 {
    match b {
        b'0'..=b'9' => (b - b'0') as u32,
        b'a'..=b'z' => (b - b'a') as u32 + 10,
        b'A'..=b'Z' => (b - b'A') as u32 + 10,
        _ => 99,
    }
}

/// CRuby's `rb_isspace` — ASCII whitespace ONLY. Deliberately not
/// Rust's Unicode `char::is_whitespace` (which would also skip NBSP
/// etc.; CRuby does not: `" 42".to_i` → 0).
#[inline]
fn is_ruby_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// Magnitude accumulator. `bignum`: u64 with checked ops, promoting
/// to a chunk-folded BigUint on overflow. Non-`bignum`: u64 with
/// wrapping ops — the low 64 bits are two's-complement-identical to
/// the historical signed `wrapping_mul`/`wrapping_add` fold.
///
/// Complexity note (future work): the chunked left-to-right fold is
/// O(n²) in the digit count — each `mag * scale + chunk` flush is
/// O(limbs) and there are n/~19 flushes — where CRuby's
/// `rb_cstr_parse_inum` uses divide-and-conquer multiplication for
/// huge inputs. Measured: exact and fine through ~100k digits
/// (~17 ms); at 1M digits ~1.7 s vs CRuby's ~0.02 s. If a workload
/// ever feeds million-digit strings to `to_i`, port a
/// split-in-half recursive combine (or num-bigint's radix parse,
/// which shares the same shape but is a constant factor better).
#[cfg(feature = "bignum")]
enum Acc {
    Small(u64),
    /// `mag` holds the promoted prefix; `chunk`/`scale` batch up to
    /// a u64's worth of trailing digits (`chunk < scale` invariant,
    /// `scale = radix^k`) so each bigint mul/add covers ~k digits
    /// instead of one.
    Big { mag: BigUint, chunk: u64, scale: u64 },
}

#[cfg(feature = "bignum")]
impl Acc {
    fn new() -> Self {
        Acc::Small(0)
    }

    #[inline]
    fn push(&mut self, radix: u64, d: u64) {
        match self {
            Acc::Small(m) => match m.checked_mul(radix).and_then(|x| x.checked_add(d)) {
                Some(nm) => *m = nm,
                None => {
                    *self = Acc::Big {
                        mag: BigUint::from(*m),
                        chunk: d,
                        scale: radix,
                    };
                }
            },
            Acc::Big { mag, chunk, scale } => match scale.checked_mul(radix) {
                // `chunk < scale` ⇒ `chunk*radix + d < scale*radix ≤ u64::MAX`
                // — the chunk fold can't overflow while the scale fits.
                Some(ns) => {
                    *chunk = *chunk * radix + d;
                    *scale = ns;
                }
                None => {
                    *mag = std::mem::take(mag) * *scale + *chunk;
                    *chunk = d;
                    *scale = radix;
                }
            },
        }
    }

    fn finish(self, neg: bool) -> ParsedInt {
        match self {
            Acc::Small(m) => small_to_parsed(m, neg),
            Acc::Big { mag, chunk, scale } => {
                // Promotion only happens past u64::MAX, so a `Big`
                // result is always genuinely out of i64 range for
                // both signs — no demote check needed here (and
                // `Vm::bigint_to_value` re-checks anyway).
                let mag = mag * scale + chunk;
                let b = BigInt::from(mag);
                ParsedInt::Big(if neg { -b } else { b })
            }
        }
    }
}

#[cfg(not(feature = "bignum"))]
struct Acc(u64);

#[cfg(not(feature = "bignum"))]
impl Acc {
    fn new() -> Self {
        Acc(0)
    }

    #[inline]
    fn push(&mut self, radix: u64, d: u64) {
        // Two's-complement wrap — bit-identical to the historical
        // per-site `i64::wrapping_mul`/`wrapping_add` folds this
        // module replaced (the no-bignum contract keeps wrapping).
        self.0 = self.0.wrapping_mul(radix).wrapping_add(d);
    }

    fn finish(self, neg: bool) -> ParsedInt {
        let n = self.0 as i64;
        ParsedInt::Small(if neg { n.wrapping_neg() } else { n })
    }
}

#[cfg(feature = "bignum")]
fn small_to_parsed(m: u64, neg: bool) -> ParsedInt {
    const I64_MAX: u64 = i64::MAX as u64;
    if neg {
        if m <= I64_MAX {
            ParsedInt::Small(-(m as i64))
        } else if m == I64_MAX + 1 {
            // `"-9223372036854775808"` is exactly i64::MIN — must
            // stay a Small Int, not promote.
            ParsedInt::Small(i64::MIN)
        } else {
            ParsedInt::Big(-BigInt::from(m))
        }
    } else if m <= I64_MAX {
        ParsedInt::Small(m as i64)
    } else {
        ParsedInt::Big(BigInt::from(m))
    }
}

/// Core scanner. `base` uses the CRuby-internal convention described
/// in the module docs.
///
/// Radix validation is LAZY, faithful to `bignum.c`'s
/// `rb_int_parse_cstr` order (fuzz-verified against CRuby 3.4):
///
///   1. If the input is non-empty: skip ASCII whitespace, consume
///      one sign; if that exhausts the input, bail BEFORE the radix
///      is even looked at (`"  ".to_i(99)` → 0,
///      `Integer("+", -37)` → invalid VALUE — never invalid radix).
///      A from-the-start EMPTY string skips this block entirely and
///      does reach validation (`"".to_i(99)` raises invalid radix).
///   2. Resolve the base: auto (`base <= 0`) via prefix / bare-0 /
///      default; explicit `{2,8,10,16}` consume a matching prefix.
///   3. Validate the RESOLVED base — a prefix-resolved base is
///      always valid, so `Integer("0x10", -99)` parses while
///      `Integer("10", -99)` raises `invalid radix 99` (the message
///      shows the resolved, i.e. negated, value).
///   4. Empty remainder → bail (`"0x".to_i(16)` → 0).
///   5. Digit scan (underscores between digits; strict tail check).
///
/// `Err` = invalid radix (an ArgumentError NEVER suppressed by
/// `Integer(..., exception: false)`). `Ok(None)` = strict-mode
/// invalid value (the caller owns that message so it can
/// inspect-format the receiver). Lenient mode never yields
/// `Ok(None)` — every bail is `Small(0)`.
///
/// Note `String#to_i` rejects NEGATIVE bases up front with the raw
/// value in the message (`"invalid radix -16"`, CRuby's `string.c`
/// check) — that lives at the `to_i` call sites, not here.
pub(crate) fn parse_int_radix(
    bytes: &[u8],
    base: i64,
    strict: bool,
) -> Result<Option<ParsedInt>, RubyError> {
    fn bad(strict: bool) -> Result<Option<ParsedInt>, RubyError> {
        if strict {
            Ok(None)
        } else {
            Ok(Some(ParsedInt::Small(0)))
        }
    }
    let mut i = 0usize;
    let mut neg = false;
    if !bytes.is_empty() {
        while i < bytes.len() && is_ruby_space(bytes[i]) {
            i += 1;
        }
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
            neg = bytes[i] == b'-';
            i += 1;
        }
        // CRuby's ASSERT_LEN bail (step 1 above).
        if i >= bytes.len() {
            return bad(strict);
        }
    }
    // Step 2 — base resolution. `resolved` stays i64 so an absurd
    // caller base (`Integer("x", -99999999999)`) reports itself
    // faithfully in the error message.
    let auto = base <= 0;
    let mut resolved: i64 = if base == 0 || base == -1 {
        10
    } else if base < 0 {
        -base
    } else {
        base
    };
    if i + 1 < bytes.len() && bytes[i] == b'0' {
        let prefix_r: i64 = match bytes[i + 1] {
            b'x' | b'X' => 16,
            b'b' | b'B' => 2,
            b'o' | b'O' => 8,
            b'd' | b'D' => 10,
            _ => 0,
        };
        if auto {
            if prefix_r != 0 {
                resolved = prefix_r;
                i += 2;
            } else {
                // Bare leading `0` in auto mode → octal. The `0`
                // itself stays in the digit stream so strict mode
                // still validates every char (`Integer("08")`
                // raises: `8` is no octal digit) and `"0_1_0"`
                // keeps a digit before its first underscore.
                resolved = 8;
            }
        } else if prefix_r != 0 && prefix_r == resolved {
            // Explicit base consumes ONLY a matching prefix
            // (`"0xff".to_i(16)` → 255; `"0b10".to_i(16)` → 0xb10).
            i += 2;
        }
    }
    // Step 3 — validate the resolved base.
    if !(2..=36).contains(&resolved) {
        return Err(RubyError::ArgumentError {
            msg: format!("invalid radix {resolved}"),
        });
    }
    let radix = resolved as u32;
    // Step 4 — nothing left after the prefix (or an originally
    // empty input that reached this far).
    if i >= bytes.len() {
        return bad(strict);
    }
    // Step 5 — digit fold.
    let digits_start = i;
    let mut saw_digit = false;
    let mut acc = Acc::new();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'_' {
            // `_` is a digit separator ONLY between two digits of
            // the effective base. Ill-placed underscores stop a
            // lenient parse and fail a strict one.
            let prev_digit = i > digits_start && digit_val(bytes[i - 1]) < radix;
            let next_digit = i + 1 < bytes.len() && digit_val(bytes[i + 1]) < radix;
            if prev_digit && next_digit {
                i += 1;
                continue;
            }
            if strict {
                return Ok(None);
            }
            break;
        }
        let d = digit_val(b);
        if d < radix {
            acc.push(radix as u64, d as u64);
            saw_digit = true;
            i += 1;
        } else {
            break;
        }
    }
    if !saw_digit {
        return bad(strict);
    }
    if strict {
        // Strict allows trailing ASCII whitespace, nothing else
        // (`Integer(" 42 ")` OK, `Integer("42abc")` raises, embedded
        // NUL raises).
        while i < bytes.len() && is_ruby_space(bytes[i]) {
            i += 1;
        }
        if i != bytes.len() {
            return Ok(None);
        }
    }
    Ok(Some(acc.finish(neg)))
}

/// Lenient fold — `String#to_i(base)` / `#hex` (base 16) / `#oct`
/// (base -8). Garbage / empty → `Small(0)`; `Err` = invalid radix.
pub(crate) fn lenient(bytes: &[u8], base: i64) -> Result<ParsedInt, RubyError> {
    Ok(parse_int_radix(bytes, base, false)?.unwrap_or(ParsedInt::Small(0)))
}

/// Strict fold — `Kernel#Integer(str, base)` and sprintf's `%d`
/// String coercion. `Ok(None)` = "invalid value for Integer()"
/// (the caller owns the message so it can inspect-format the
/// receiver); `Err` = invalid radix (never suppressed).
pub(crate) fn strict(bytes: &[u8], base: i64) -> Result<Option<ParsedInt>, RubyError> {
    parse_int_radix(bytes, base, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(bytes: &[u8], base: i64, strict: bool) -> Option<i64> {
        match parse_int_radix(bytes, base, strict).expect("unexpected invalid radix") {
            Some(ParsedInt::Small(n)) => Some(n),
            #[cfg(feature = "bignum")]
            Some(ParsedInt::Big(_)) => panic!("expected Small for {:?}", bytes),
            None => None,
        }
    }

    #[cfg(feature = "bignum")]
    fn any_str(bytes: &[u8], base: i64, strict: bool) -> Option<String> {
        match parse_int_radix(bytes, base, strict).expect("unexpected invalid radix") {
            Some(ParsedInt::Small(n)) => Some(n.to_string()),
            Some(ParsedInt::Big(b)) => Some(b.to_string()),
            None => None,
        }
    }

    fn radix_err(bytes: &[u8], base: i64, strict: bool) -> String {
        match parse_int_radix(bytes, base, strict) {
            Err(RubyError::ArgumentError { msg }) => msg,
            other => panic!(
                "expected invalid-radix error for {:?} base {base}, got {:?}",
                bytes,
                other.is_ok()
            ),
        }
    }

    /// The lazy validation order (CRuby 3.4 `bignum.c`, fuzz-verified):
    /// emptied-by-scan bails before validation; empty-from-start and
    /// non-empty inputs reach it; prefix-resolved bases skip it.
    #[test]
    fn lazy_radix_validation() {
        // Whitespace-only / sign-only bail BEFORE validation.
        assert_eq!(small(b"  ", 99, false), Some(0));
        assert_eq!(small(b"+", 37, false), Some(0));
        assert_eq!(small(b"+", -37, true), None); // invalid VALUE, not radix
        // Empty-from-start reaches validation.
        assert_eq!(radix_err(b"", 37, false), "invalid radix 37");
        assert_eq!(radix_err(b"", 99, true), "invalid radix 99");
        // Non-empty reaches validation (even for "0"-led strings
        // with a positive base — no prefix arm applies).
        assert_eq!(radix_err(b"z", 37, false), "invalid radix 37");
        assert_eq!(radix_err(b"0x10", 99, true), "invalid radix 99");
        assert_eq!(radix_err(b" + ", 37, false), "invalid radix 37");
        // Negative base: resolved (negated) value in the message...
        assert_eq!(radix_err(b"10", -37, true), "invalid radix 37");
        // ...but a prefix (incl. bare-0 octal) resolves first and
        // skips validation entirely.
        assert_eq!(small(b"00", -37, true), Some(0));
        assert_eq!(small(b"0z", -37, true), None); // octal, invalid VALUE
        assert_eq!(any_str(b"0x10", -99, true).as_deref(), Some("16"));
    }

    #[test]
    fn lenient_basics() {
        assert_eq!(small(b"42", 10, false), Some(42));
        assert_eq!(small(b"  -42abc", 10, false), Some(-42));
        assert_eq!(small(b"+42", 10, false), Some(42));
        assert_eq!(small(b"", 10, false), Some(0));
        assert_eq!(small(b"abc", 10, false), Some(0));
        assert_eq!(small(b"- 42", 10, false), Some(0));
        assert_eq!(small(b"\t\n\x0b\x0c\r 42", 10, false), Some(42));
        // NBSP (UTF-8 c2 a0) is NOT whitespace to CRuby.
        assert_eq!(small(b"\xc2\xa042", 10, false), Some(0));
        // Underscores between digits only.
        assert_eq!(small(b"1_0", 10, false), Some(10));
        assert_eq!(small(b"1__0", 10, false), Some(1));
        assert_eq!(small(b"1_", 10, false), Some(1));
        assert_eq!(small(b"_1", 10, false), Some(0));
    }

    #[test]
    fn lenient_prefix_rules() {
        // Explicit base consumes ONLY a matching prefix.
        assert_eq!(small(b"0x10", 16, false), Some(16));
        assert_eq!(small(b"0X10", 16, false), Some(16));
        assert_eq!(small(b"0x10", 10, false), Some(0));
        assert_eq!(small(b"0b10", 16, false), Some(0xb10));
        assert_eq!(small(b"0x10", 2, false), Some(0));
        assert_eq!(small(b"0b10", 36, false), Some(14292));
        assert_eq!(small(b"0x_10", 16, false), Some(0));
        // Auto (base 0): letter prefixes + bare-0 octal.
        assert_eq!(small(b"0x10", 0, false), Some(16));
        assert_eq!(small(b"010", 0, false), Some(8));
        assert_eq!(small(b"08", 0, false), Some(0));
        assert_eq!(small(b"0_1_0", 0, false), Some(8));
        assert_eq!(small(b"0__10", 0, false), Some(0));
        // Negative base (oct's -8): prefix overrides the default.
        assert_eq!(small(b"10", -8, false), Some(8));
        assert_eq!(small(b"0b10", -8, false), Some(2));
        assert_eq!(small(b"0x10", -8, false), Some(16));
        // `String#to_i` no-arg is base 10 — `0d` honored, `0x` not.
        assert_eq!(small(b"0d19", 10, false), Some(19));
        assert_eq!(small(b"0D19", 10, false), Some(19));
        assert_eq!(small(b"019", 10, false), Some(19));
    }

    #[test]
    fn strict_basics() {
        assert_eq!(small(b"42", 0, true), Some(42));
        assert_eq!(small(b" \t42\n ", 0, true), Some(42));
        assert_eq!(small(b"42abc", 0, true), None);
        assert_eq!(small(b"", 0, true), None);
        assert_eq!(small(b"+", 0, true), None);
        assert_eq!(small(b"4 2", 0, true), None);
        assert_eq!(small(b"42\x00", 0, true), None);
        assert_eq!(small(b"1__0", 0, true), None);
        assert_eq!(small(b"1_", 0, true), None);
        assert_eq!(small(b"_1", 0, true), None);
        assert_eq!(small(b"0x_10", 16, true), None);
        assert_eq!(small(b"08", 0, true), None);
        assert_eq!(small(b"0_1_0", 0, true), Some(8));
        assert_eq!(small(b"-0x10", 0, true), Some(-16));
        assert_eq!(small(b"042", -10, true), Some(34));
        assert_eq!(small(b"10", -16, true), Some(16));
        assert_eq!(small(b"0b10", -16, true), Some(2));
    }

    #[test]
    fn i64_boundaries_stay_small() {
        assert_eq!(small(b"9223372036854775807", 10, false), Some(i64::MAX));
        assert_eq!(small(b"-9223372036854775808", 10, false), Some(i64::MIN));
        assert_eq!(small(b"7fffffffffffffff", 16, false), Some(i64::MAX));
        assert_eq!(small(b"-8000000000000000", 16, false), Some(i64::MIN));
    }

    #[cfg(feature = "bignum")]
    #[test]
    fn promotes_past_i64() {
        assert_eq!(
            any_str(b"9223372036854775808", 10, false).as_deref(),
            Some("9223372036854775808")
        );
        assert_eq!(
            any_str(b"-9223372036854775809", 10, false).as_deref(),
            Some("-9223372036854775809")
        );
        assert_eq!(
            any_str(b"18446744073709551616", 10, true).as_deref(),
            Some("18446744073709551616")
        );
        assert_eq!(
            any_str(b"123456789012345678901234567890", 10, false).as_deref(),
            Some("123456789012345678901234567890")
        );
        assert_eq!(
            any_str(b"1_000_000_000_000_000_000_000", 10, false).as_deref(),
            Some("1000000000000000000000")
        );
        assert_eq!(
            any_str(b"ffffffffffffffffff", 16, false).as_deref(),
            Some("4722366482869645213695")
        );
    }

    /// Property test: for random digit strings across every base,
    /// the chunked fold must agree exactly with num-bigint's own
    /// radix parser (the reference implementation). Deterministic
    /// xorshift so failures reproduce.
    #[cfg(feature = "bignum")]
    #[test]
    fn fold_matches_num_bigint_reference() {
        let mut state: u64 = 0x243F6A8885A308D3; // pi digits, fixed seed
        let mut rng = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        for case in 0..20_000u32 {
            let base = 2 + (rng() % 35) as usize; // 2..=36
            let len = 1 + (rng() % 40) as usize; // 1..=40 digits
            let neg = rng() % 2 == 0;
            let mut s: Vec<u8> = Vec::with_capacity(len + 1);
            if neg {
                s.push(b'-');
            }
            for _ in 0..len {
                s.push(DIGITS[(rng() % base as u64) as usize]);
            }
            let ours = match parse_int_radix(&s, base as i64, true).expect("valid radix") {
                Some(ParsedInt::Small(n)) => num_bigint::BigInt::from(n),
                Some(ParsedInt::Big(b)) => b,
                None => panic!("case {case}: fold failed on valid input {:?}", s),
            };
            let reference = num_bigint::BigInt::parse_bytes(&s, base as u32)
                .expect("reference parse must succeed");
            assert_eq!(
                ours, reference,
                "case {case}: fold mismatch for base {base} input {:?}",
                String::from_utf8_lossy(&s)
            );
            // Small/Big boundary discipline: Small ⇔ fits i64.
            if let Ok(Some(ParsedInt::Big(b))) = parse_int_radix(&s, base as i64, true) {
                assert!(
                    i64::try_from(&b).is_err(),
                    "case {case}: Big result fits i64 — must demote"
                );
            }
        }
    }
}
