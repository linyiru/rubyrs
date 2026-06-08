//! Compiled-regex engine wrapper. rubyrs uses `regex` (linear-time,
//! ReDoS-immune) as the primary backend; when `regex::Regex::new`
//! rejects a pattern as unsupported syntax — typically Ruby's
//! lookaround `(?=...)` / `(?!...)` constructs — we fall back to
//! `fancy-regex` for that single pattern. fancy-regex itself
//! delegates simple subpatterns to `regex`, so the cold path is
//! still mostly the linear engine; only the unsupported parts go
//! through fancy-regex's backtracking NFA.
//!
//! Discovery: TRY_RUNS pass-13 — sinatra-4's `cleaned_caller`
//! (sinatra/base.rb:1913) splits on `/:(?=\d|in )/`, which the
//! linear engine can't compile. (Layer #17.)
//!
//! API design: this PR lands the COMPILE fallback plus
//! dual-engine impls for the simple-shape ops (`is_match`,
//! `replace`, `replace_all` — see below). The capture-bearing
//! ops (`captures`, `captures_iter`, `find_iter`, `captures_len`)
//! are NOT dual-engine yet because `regex::Captures` and
//! `fancy_regex::Captures` are distinct types with different
//! lifetimes; call sites that need them consult `as_native()`
//! and raise `RubyError::RuntimeError` on the fancy arm
//! (rubyrs doesn't model `NotImplementedError` as its own
//! `RubyError` variant — `RuntimeError` with a clear "not yet
//! supported" message is the closest fit until that's added).
//! Migrating each capture-bearing operation to a unified
//! owned-captures shape is incremental follow-up work tracked
//! layer-by-layer.

#![cfg(feature = "regex")]

use std::fmt;

/// Compiled regex. Variant chosen at construction time based on
/// whether the linear engine accepted the pattern.
///
/// The enum itself is `pub` because `Value::Regex(Rc<CompiledRegex>)`
/// is reachable from the embedder-visible `Value` type — leaving
/// it `pub(crate)` would trip `private_interfaces`. The variants
/// are technically public too (Rust doesn't allow per-variant
/// visibility on a `pub enum`) and the inherent methods are
/// deliberately `pub(crate)` — they're for in-crate dispatch,
/// not the embedder API. Treat `CompiledRegex` as a fully
/// opaque token: embedders that pattern-match on `Value::Regex(_)`
/// should match the outer variant and pass the inner Rc through
/// without introspecting it. Future engine swaps may change the
/// variants and methods without notice.
///
/// Marked `#[non_exhaustive]` so adding a third backend later
/// (e.g. an Onigmo-shaped engine for tighter CRuby parity) is
/// a non-breaking change — exhaustive matches on `CompiledRegex`
/// from outside this crate are required to include a `_` arm,
/// so an added variant won't break their compile. (Code-review
/// #353 round 4 — first round caught the doc story; this
/// rounds locks it in at the type level.)
#[non_exhaustive]
pub enum Engine {
    /// Linear-time `regex` engine — preferred. Most Ruby
    /// patterns land here.
    Native(regex::Regex),
    /// fancy-regex backtracking engine — fallback for patterns
    /// the linear engine rejects (lookaround, backrefs).
    Fancy(fancy_regex::Regex),
}

/// Ruby `Regexp` option bits — match CRuby's
/// `Regexp::IGNORECASE` / `EXTENDED` / `MULTILINE` constant
/// values. Carried on `CompiledRegex` so `Regexp#options` /
/// `#to_s` / `#inspect` render the flag set, AND folded into the
/// regex-cache key so `/foo/` and `/foo/i` don't collide. Note
/// the Ruby `/m` flag is "dot matches newline" (engine `(?s)`),
/// NOT multi-line `^`/`$`.
pub(crate) const RB_IGNORECASE: u8 = 1;
pub(crate) const RB_EXTENDED: u8 = 2;
pub(crate) const RB_MULTILINE: u8 = 4;

/// A compiled Ruby regexp: the chosen linear-or-backtracking
/// `Engine` plus the Ruby-level metadata the `Regexp` reflection
/// methods need. `Value::Regex(Rc<CompiledRegex>)` is the single
/// shared shape across every dispatch site, so keeping the inner
/// type a struct (rather than the bare engine enum) lets the
/// Ruby flag bitmask + the bare source travel with it without
/// touching any `Value::Regex(_)` match arm.
pub struct CompiledRegex {
    engine: Engine,
    /// Ruby flag bitmask (`RB_IGNORECASE | RB_EXTENDED |
    /// RB_MULTILINE`). `Regexp#options` returns this verbatim.
    ruby_flags: u8,
    /// The BARE pattern as written (no inline `(?is)` flag
    /// prefix). `Regexp#source` / `#to_s` / `#inspect` and trap
    /// formatting render this, never the flag-prefixed string fed
    /// to the engine.
    source: Box<str>,
}

/// Builds either engine. Tries `regex` first; falls back to
/// `fancy-regex` ONLY when the linear engine's error is a
/// genuine syntax problem. Resource-limit failures
/// (`regex::Error::CompiledTooBig`) are NOT bypassed — they're
/// the linear engine's safety guard against pathological
/// pattern sizes, and routing such patterns into the
/// backtracking engine would defeat the guard. The
/// CompiledTooBig error surfaces as-is for the caller to trap.
///
/// If fancy-regex also rejects, the combined error mentions
/// both engines' messages so a pattern that's malformed (not
/// just lookaround-shaped) gives a useful trap.
pub(crate) fn compile(pattern: &str) -> Result<CompiledRegex, String> {
    // Flagless path: the engine pattern IS the bare source.
    compile_with_flags(pattern, 0, pattern)
}

/// Compile with Ruby flags already applied to `engine_pattern`
/// (an inline `(?is)` prefix prepended by `apply_ruby_flags`),
/// while retaining the BARE `source` (no prefix) for `#source` /
/// `#inspect` and the raw `ruby_flags` bitmask for `#options`.
pub(crate) fn compile_with_flags(
    engine_pattern: &str,
    ruby_flags: u8,
    bare_source: &str,
) -> Result<CompiledRegex, String> {
    let engine = build_engine(engine_pattern)?;
    Ok(CompiledRegex { engine, ruby_flags, source: bare_source.into() })
}

/// Rewrite octal escapes (`\NNN`) that appear INSIDE a character
/// class to the equivalent `\x{..}` hex escape, which both the
/// `regex` and `fancy-regex` engines accept. CRuby/Onigmo treat
/// `\2` inside `[...]` as the octal character U+0002 (backreferences
/// are only meaningful outside a class), but the Rust engines reject
/// the bare-octal-in-class form. Octal escapes OUTSIDE a class are
/// left untouched (there `\2` is a backreference, handled by
/// fancy-regex). No-op (borrowed) for the common case with no
/// such escape.
///
/// Discovery: P3 Jekyll spike — kramdown's IAL parser builds
/// `/...=("|')((?:\\\}|\\\2|[^}\2])*?)\2/` at load time; the
/// `[^}\2]` class tripped both engines.
fn rewrite_charclass_octal_escapes(pat: &str) -> std::borrow::Cow<'_, str> {
    if !pat.contains('[') {
        return std::borrow::Cow::Borrowed(pat);
    }
    let chars: Vec<char> = pat.chars().collect();
    let mut out = String::with_capacity(pat.len());
    let mut in_class = false;
    // True while still at the start of a class (`[` / `[^`), where a
    // `]` is a literal member rather than the class terminator.
    let mut at_class_start = false;
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            if in_class && ('0'..='7').contains(&n) {
                // Collect up to 3 octal digits.
                let mut j = i + 1;
                let mut val: u32 = 0;
                let mut cnt = 0;
                while j < chars.len() && cnt < 3 && ('0'..='7').contains(&chars[j]) {
                    val = val * 8 + (chars[j] as u32 - '0' as u32);
                    j += 1;
                    cnt += 1;
                }
                out.push_str(&format!("\\x{{{:x}}}", val));
                i = j;
                changed = true;
                at_class_start = false;
                continue;
            }
            // Any other escape pair: copy verbatim (handles `\[`,
            // `\]`, `\\`, `\2` backref outside a class, etc.).
            out.push(c);
            out.push(n);
            i += 2;
            at_class_start = false;
            continue;
        }
        if !in_class {
            if c == '[' {
                in_class = true;
                at_class_start = true;
            }
        } else if c == '^' && at_class_start {
            // Negation right after `[` — still at the start.
        } else if c == ']' && !at_class_start {
            in_class = false;
            at_class_start = false;
        } else {
            at_class_start = false;
        }
        out.push(c);
        i += 1;
    }
    if changed {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(pat)
    }
}

/// Engine selection without the `CompiledRegex` wrapper — shared
/// by `compile` and `compile_with_flags`. Tries `regex` first,
/// falls back to `fancy-regex` only on a genuine syntax error
/// (CompiledTooBig surfaces as-is; see the `compile` doc above).
fn build_engine(pattern: &str) -> Result<Engine, String> {
    let pattern = rewrite_charclass_octal_escapes(pattern);
    // Ruby's `^` / `$` are ALWAYS line anchors (they match at every
    // line boundary, not just the string ends — `\A` / `\z` / `\Z`
    // are the string anchors). The regex / fancy-regex crates default
    // `^` / `$` to string-only anchoring, switching to line anchors
    // only under engine `(?m)` (multi-line). So every Ruby pattern
    // gets a `(?m)` engine prefix. This is ORTHOGONAL to Ruby's `/m`
    // literal flag, which means dot-matches-newline → engine `(?s)`
    // (applied separately in `apply_ruby_flags`). The bare `source`
    // stored on `CompiledRegex` is untouched, so `#source` / `#inspect`
    // never see this prefix. Discovery: P3 Jekyll spike — jekyll's
    // `YAML_FRONT_MATTER_REGEXP` (`\A(...)^((---|\.\.\.)\s*$...)/m`)
    // relies on `^`/`$` matching the front-matter delimiter lines.
    let prefixed = format!("(?m){pattern}");
    let pattern: &str = &prefixed;
    match regex::Regex::new(pattern) {
        Ok(re) => Ok(Engine::Native(re)),
        Err(native_err) => match &native_err {
            // Syntax error → try fancy-regex. The wider syntax
            // surface (lookaround, backrefs) is exactly what
            // fancy-regex exists for.
            regex::Error::Syntax(_) => match fancy_regex::Regex::new(pattern) {
                Ok(re) => Ok(Engine::Fancy(re)),
                Err(fancy_err) => {
                    // Both engines rejected. Prefer the
                    // fancy-regex error message (covers the
                    // wider surface) but mention the native
                    // failure too — useful when a pattern is
                    // genuinely malformed rather than just
                    // lookaround-shaped.
                    Err(format!("{} (also rejected by regex: {})", fancy_err, native_err))
                }
            },
            // CompiledTooBig (or any future non-syntax
            // variant) is a real safety/resource signal from
            // the linear engine. Surface it as-is — don't
            // route around the guard by handing the pattern
            // to fancy-regex's backtracker, which has no
            // equivalent size limit. (Code-review #353 round 1.)
            _ => Err(native_err.to_string()),
        },
    }
}

impl CompiledRegex {
    /// The BARE source pattern as written (NO inline `(?is)` flag
    /// prefix — that only lives in the engine's compiled pattern).
    /// Used by `Regexp#source` / `#to_s` / `#inspect` and trap
    /// formatting. For the flagless path this equals the engine's
    /// own pattern; for the flagged path it's the pre-prefix
    /// source so `#source` never leaks the `(?is)` group.
    pub(crate) fn as_str(&self) -> &str {
        &self.source
    }

    /// Ruby flag bitmask (`RB_IGNORECASE | RB_EXTENDED |
    /// RB_MULTILINE`) — what `Regexp#options` returns. `0` for a
    /// flagless regexp.
    pub(crate) fn options(&self) -> u8 {
        self.ruby_flags
    }

    /// `Regexp#to_s` rendering: `(?<on>-<off>:source)`. CRuby
    /// orders the flag letters `m, i, x` (where `m` is dotall),
    /// puts SET flags before the `-` and UNSET after, and drops
    /// the `-` entirely when no flag is unset. Flagless → the
    /// familiar `(?-mix:source)`.
    pub(crate) fn to_s_string(&self) -> String {
        let (on, off) = self.flag_letter_split();
        if off.is_empty() {
            format!("(?{}:{})", on, self.source)
        } else {
            format!("(?{}-{}:{})", on, off, self.source)
        }
    }

    /// `Regexp#inspect` rendering: `/source/<set letters>` in the
    /// same `m, i, x` order. Flagless → `/source/`.
    pub(crate) fn inspect_string(&self) -> String {
        let (on, _) = self.flag_letter_split();
        format!("/{}/{}", self.source, on)
    }

    /// `(set-letters, unset-letters)` in CRuby's `m, i, x` order.
    fn flag_letter_split(&self) -> (String, String) {
        let f = self.ruby_flags;
        let table = [(RB_MULTILINE, 'm'), (RB_IGNORECASE, 'i'), (RB_EXTENDED, 'x')];
        let on: String = table.iter().filter(|(b, _)| f & b != 0).map(|(_, c)| *c).collect();
        let off: String = table.iter().filter(|(b, _)| f & b == 0).map(|(_, c)| *c).collect();
        (on, off)
    }

    /// Borrow the underlying linear-time regex. Returns `None`
    /// for fancy-regex patterns — those call sites must either
    /// add dual-engine handling or raise a Trap. The current
    /// migration strategy: capture-bearing call sites use this
    /// accessor and surface `RubyError::RuntimeError` on the
    /// fancy arm (rubyrs doesn't model `NotImplementedError`
    /// as a `RubyError` variant). New operations are written
    /// to dispatch through the enum from the start.
    pub(crate) fn as_native(&self) -> Option<&regex::Regex> {
        match &self.engine {
            Engine::Native(r) => Some(r),
            Engine::Fancy(_) => None,
        }
    }

    /// Number of capture groups + 1 (group 0 = whole match), across
    /// both engines.
    pub(crate) fn captures_len(&self) -> usize {
        match &self.engine {
            Engine::Native(r) => r.captures_len(),
            Engine::Fancy(r) => r.captures_len(),
        }
    }

    /// Engine-agnostic capture iteration for `String#scan` (no-block).
    /// Returns one entry per non-overlapping match; each entry holds the
    /// capture groups `0..captures_len` as owned Strings (`None` for an
    /// unmatched optional group). Group 0 is the whole match. Returning
    /// owned data keeps the two engines' incompatible `Captures` types
    /// out of the call site and avoids any heap alloc during iteration.
    /// (fancy-regex's iterator is fallible — backtracker recursion can
    /// fail on pathological input; `.flatten()` drops those Errs, the
    /// same error-suppression the dual-engine `is_match` documents.)
    pub(crate) fn scan_captures(&self, text: &str) -> Vec<Vec<Option<String>>> {
        let mut out: Vec<Vec<Option<String>>> = Vec::new();
        match &self.engine {
            Engine::Native(r) => {
                for caps in r.captures_iter(text) {
                    out.push(
                        (0..caps.len())
                            .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                            .collect(),
                    );
                }
            }
            Engine::Fancy(r) => {
                for caps in r.captures_iter(text).flatten() {
                    out.push(
                        (0..caps.len())
                            .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                            .collect(),
                    );
                }
            }
        }
        out
    }

    /// Engine label for diagnostic output. Currently consumed
    /// only by the `Debug` impl below; trap messages at the
    /// dispatch sites hard-code `"fancy-regex engine"` for the
    /// fancy-arm RuntimeError so the cost of stringifying isn't
    /// paid on the happy path. If a future migration adds a
    /// third engine or routes traps through a builder, this
    /// helper is the right place to centralise the label.
    pub(crate) fn engine_name(&self) -> &'static str {
        match &self.engine {
            Engine::Native(_) => "regex",
            Engine::Fancy(_) => "fancy-regex",
        }
    }

    /// True iff the haystack contains a match.
    ///
    /// **fancy-regex error suppression — documented limitation.**
    /// `fancy_regex::Regex::is_match` returns `Result<bool>`
    /// because the backtracker can fail at runtime (recursion
    /// limit on pathological inputs). This wrapper collapses
    /// the error to `false` so call sites stay a plain `bool`:
    /// the dispatchers that need this method
    /// (`String#match?`, `Regexp#match?`) operate inside
    /// `with_str_lossy` closures returning bool, and
    /// propagating Result through that closure-shape is
    /// non-trivial without lifting the closure's signature.
    ///
    /// The trade-off: a recursion-limit hit on a fancy-regex
    /// pattern silently reports "no match" rather than raising
    /// `RegexpError`. Only fires on adversarial patterns
    /// (deeply nested backrefs); a follow-up can swap the
    /// wrapper to Result and lift the closure shape if a real
    /// call site needs strict error semantics. Code-review
    /// #353 round 1 flagged this; documenting the trade-off
    /// is the adopted resolution.
    pub(crate) fn is_match(&self, haystack: &str) -> bool {
        match &self.engine {
            Engine::Native(r) => r.is_match(haystack),
            Engine::Fancy(r) => r.is_match(haystack).unwrap_or(false),
        }
    }

    /// `String#sub` — first match. `replacement` is in the
    /// regex crate's `$N` backref form (Ruby's `\N` has
    /// already been translated upstream by
    /// `ruby_backref_to_dollar`). fancy-regex uses the same
    /// `$N` convention. Returns `Cow::Borrowed` when there's
    /// no match — rubyrs's `sub!` path uses that as the
    /// no-match signal.
    pub(crate) fn replace<'h>(&self, haystack: &'h str, replacement: &str) -> std::borrow::Cow<'h, str> {
        match &self.engine {
            Engine::Native(r) => r.replace(haystack, replacement),
            Engine::Fancy(r) => r.replace(haystack, replacement),
        }
    }

    /// `String#gsub` — replace all. Same Cow discipline as
    /// `replace`.
    pub(crate) fn replace_all<'h>(&self, haystack: &'h str, replacement: &str) -> std::borrow::Cow<'h, str> {
        match &self.engine {
            Engine::Native(r) => r.replace_all(haystack, replacement),
            Engine::Fancy(r) => r.replace_all(haystack, replacement),
        }
    }

    /// Collect match positions + per-group spans, eagerly, in
    /// engine-agnostic owned form. Used by
    /// `String#split(regex[, limit])` so the split walker can
    /// operate independently of which engine produced the
    /// matches (`regex::Captures` and `fancy_regex::Captures`
    /// are distinct lifetime-bound types).
    ///
    /// `max_matches` bounds the collection: `Some(n)` stops
    /// after the n-th match (callers pass `Some(limit-1)` for
    /// `split(re, limit)` with positive limit so the engine
    /// walk short-circuits — `"a,b,c,...,z".split(/,/, 2)`
    /// finds exactly one match and bails). `None` collects all.
    /// Pre-existing call sites that want all matches pass
    /// `None`. Code-review #357 round 1.
    ///
    /// fancy-regex's `captures_iter` yields `Result<Captures>`;
    /// we stop iteration on the first error (same swallow
    /// rationale as `is_match`'s wrapper — adversarial
    /// recursion-limit hits would otherwise need to plumb
    /// Result through every split call site). For the
    /// lookahead-only patterns that motivated layer #17
    /// (sinatra's `cleaned_caller`) this never fires.
    pub(crate) fn split_matches(
        &self,
        haystack: &str,
        max_matches: Option<usize>,
    ) -> Vec<SplitMatch> {
        // Sanity cap on the preallocation. `max_matches` can
        // arrive as `usize::MAX` when an oversized Ruby
        // \`limit\` saturates the \`try_from\` conversion at
        // the call site; `Vec::with_capacity(usize::MAX)`
        // would OOM/panic before any matching occurs. 64 is
        // a generous hint for the common case (most splits
        // produce single-digit matches); we'll grow naturally
        // for legitimately large match counts. Code-review
        // #357 round 2.
        const CAP_HINT_MAX: usize = 64;
        let cap = max_matches.unwrap_or(0).min(CAP_HINT_MAX);
        let mut out: Vec<SplitMatch> = Vec::with_capacity(cap);
        // Bound the iterator with `.take(bound)` so the engine
        // stops searching for the next match BEFORE we'd
        // reject it — the previous loop pulled one extra match
        // per call (engine work + ObjectIds + allocation) only
        // to discard it. \`usize::MAX\` is effectively
        // unbounded for the \`None\` case. Code-review #357
        // round 6.
        let bound = max_matches.unwrap_or(usize::MAX);
        match &self.engine {
            Engine::Native(r) => {
                for caps in r.captures_iter(haystack).take(bound) {
                    if let Some(m0) = caps.get(0) {
                        let range = (m0.start(), m0.end());
                        let groups: Vec<Option<(usize, usize)>> = (1..caps.len())
                            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                            .collect();
                        out.push(SplitMatch { range, groups });
                    }
                }
            }
            Engine::Fancy(r) => {
                for caps_res in r.captures_iter(haystack).take(bound) {
                    let caps = match caps_res {
                        Ok(c) => c,
                        Err(_) => break,
                    };
                    if let Some(m0) = caps.get(0) {
                        let range = (m0.start(), m0.end());
                        let groups: Vec<Option<(usize, usize)>> = (1..caps.len())
                            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                            .collect();
                        out.push(SplitMatch { range, groups });
                    }
                }
            }
        }
        out
    }

    /// Single-match capture extraction in engine-agnostic owned
    /// form — the `captures` counterpart to `split_matches`. Used
    /// by `String#match` / `Regexp#match` / `=~` so those ops work
    /// on BOTH the linear `regex` engine AND the fancy-regex
    /// fallback (Mustermann wraps every route in `/\A...\Z/`, and
    /// the `\Z` anchor forces fancy, so route matching depends on
    /// this).
    ///
    /// Returns `Ok(None)` for no-match, `Ok(Some(..))` with the
    /// whole-match span + per-group matched strings + named
    /// captures, or `Err` only when the fancy engine errors at
    /// match time (recursion-limit / backtracking blow-up). The
    /// linear arm never errors. Mirrors `split_matches`' fancy
    /// `Result` handling but surfaces the error to the caller
    /// (match/`=~` want a clean trap, not a silent no-match).
    pub(crate) fn captures_owned(
        &self,
        haystack: &str,
    ) -> Result<Option<OwnedCaptures>, String> {
        match &self.engine {
            Engine::Native(r) => match r.captures(haystack) {
                None => Ok(None),
                Some(caps) => {
                    let m0 = match caps.get(0) {
                        Some(m) => m,
                        None => return Ok(None),
                    };
                    let groups = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                        .collect();
                    let named = r
                        .capture_names()
                        .enumerate()
                        .filter_map(|(i, n)| {
                            n.map(|name| {
                                (name.to_string(), caps.get(i).map(|m| m.as_str().to_string()))
                            })
                        })
                        .collect();
                    Ok(Some(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        named,
                    }))
                }
            },
            Engine::Fancy(r) => match r.captures(haystack) {
                Err(e) => Err(e.to_string()),
                Ok(None) => Ok(None),
                Ok(Some(caps)) => {
                    let m0 = match caps.get(0) {
                        Some(m) => m,
                        None => return Ok(None),
                    };
                    let groups = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                        .collect();
                    let named = r
                        .capture_names()
                        .enumerate()
                        .filter_map(|(i, n)| {
                            n.map(|name| {
                                (name.to_string(), caps.get(i).map(|m| m.as_str().to_string()))
                            })
                        })
                        .collect();
                    Ok(Some(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        named,
                    }))
                }
            },
        }
    }

    /// All non-overlapping matches of this regex in `haystack`, each
    /// as an `OwnedCaptures`. Engine-agnostic (linear OR fancy-regex),
    /// so callers that need per-match group info across both backends
    /// — block-form `gsub` / `sub` — don't branch on the engine. A
    /// fancy-regex match-time error surfaces as `Err`. Discovery: P3
    /// Jekyll spike — kramdown's IAL parser drives a lookahead pattern
    /// (fancy engine) through `gsub { … }`.
    pub(crate) fn captures_iter_owned(
        &self,
        haystack: &str,
    ) -> Result<Vec<OwnedCaptures>, String> {
        let mut out = Vec::new();
        match &self.engine {
            Engine::Native(r) => {
                for caps in r.captures_iter(haystack) {
                    let m0 = match caps.get(0) {
                        Some(m) => m,
                        None => continue,
                    };
                    let groups = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                        .collect();
                    let named = r
                        .capture_names()
                        .enumerate()
                        .filter_map(|(i, n)| {
                            n.map(|name| (name.to_string(), caps.get(i).map(|m| m.as_str().to_string())))
                        })
                        .collect();
                    out.push(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        named,
                    });
                }
            }
            Engine::Fancy(r) => {
                for caps in r.captures_iter(haystack) {
                    let caps = caps.map_err(|e| e.to_string())?;
                    let m0 = match caps.get(0) {
                        Some(m) => m,
                        None => continue,
                    };
                    let groups = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                        .collect();
                    let named = r
                        .capture_names()
                        .enumerate()
                        .filter_map(|(i, n)| {
                            n.map(|name| (name.to_string(), caps.get(i).map(|m| m.as_str().to_string())))
                        })
                        .collect();
                    out.push(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        named,
                    });
                }
            }
        }
        Ok(out)
    }
}

/// One match for `String#split(regex)`: the full match span
/// plus per-capture-group spans. Spans are byte offsets into
/// the haystack. Owned (Vec of tuples) so the dispatch logic
/// doesn't carry the engine-specific `Captures` lifetime.
/// `groups[i]` is the byte span of group i+1 (1-indexed in
/// Ruby semantics); `None` for groups that didn't participate
/// in this match (e.g. an unmatched `|` arm).
#[derive(Debug, Clone)]
pub(crate) struct SplitMatch {
    pub(crate) range: (usize, usize),
    pub(crate) groups: Vec<Option<(usize, usize)>>,
}

/// Owned, engine-agnostic single-match capture data for
/// `String#match` / `Regexp#match` / `=~`. The MatchData
/// materializer wants owned `String`s (independent of the
/// engine's lifetime-bound `Captures`), so this carries matched
/// strings, not just spans like `SplitMatch`.
#[derive(Debug, Clone)]
pub(crate) struct OwnedCaptures {
    /// The whole match (`$~[0]` / `$&`).
    pub(crate) whole: String,
    /// Byte offsets of the whole match in the haystack — used to
    /// derive pre/post-match and the `$~` side-channel.
    pub(crate) m_start: usize,
    pub(crate) m_end: usize,
    /// Groups 1..N — matched string, or `None` for a group that
    /// didn't participate (e.g. an unmatched `|` arm).
    pub(crate) groups: Vec<Option<String>>,
    /// `(name, matched | None)` for each NAMED capture group.
    pub(crate) named: Vec<(String, Option<String>)>,
}

/// Hand-rolled because `regex::Regex` and `fancy_regex::Regex`
/// both implement Debug with different shapes; the lowest-
/// common-denominator is just the source pattern, which is
/// what `Value::Debug` prints for this variant anyway.
impl fmt::Debug for CompiledRegex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledRegex")
            .field("engine", &self.engine_name())
            .field("pattern", &self.as_str())
            .finish()
    }
}
