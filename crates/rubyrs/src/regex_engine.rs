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
pub enum CompiledRegex {
    /// Linear-time `regex` engine — preferred. Most Ruby
    /// patterns land here.
    Native(regex::Regex),
    /// fancy-regex backtracking engine — fallback for patterns
    /// the linear engine rejects (lookaround, backrefs).
    Fancy(fancy_regex::Regex),
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
    match regex::Regex::new(pattern) {
        Ok(re) => Ok(CompiledRegex::Native(re)),
        Err(native_err) => match &native_err {
            // Syntax error → try fancy-regex. The wider syntax
            // surface (lookaround, backrefs) is exactly what
            // fancy-regex exists for.
            regex::Error::Syntax(_) => match fancy_regex::Regex::new(pattern) {
                Ok(re) => Ok(CompiledRegex::Fancy(re)),
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
    /// Original source pattern. Used by `Regexp#source`,
    /// `Regexp#to_s`, `Regexp#inspect`, and trap formatting.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            CompiledRegex::Native(r) => r.as_str(),
            CompiledRegex::Fancy(r) => r.as_str(),
        }
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
        match self {
            CompiledRegex::Native(r) => Some(r),
            CompiledRegex::Fancy(_) => None,
        }
    }

    /// Engine label for diagnostic output. Currently consumed
    /// only by the `Debug` impl below; trap messages at the
    /// dispatch sites hard-code `"fancy-regex engine"` for the
    /// fancy-arm RuntimeError so the cost of stringifying isn't
    /// paid on the happy path. If a future migration adds a
    /// third engine or routes traps through a builder, this
    /// helper is the right place to centralise the label.
    pub(crate) fn engine_name(&self) -> &'static str {
        match self {
            CompiledRegex::Native(_) => "regex",
            CompiledRegex::Fancy(_) => "fancy-regex",
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
        match self {
            CompiledRegex::Native(r) => r.is_match(haystack),
            CompiledRegex::Fancy(r) => r.is_match(haystack).unwrap_or(false),
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
        match self {
            CompiledRegex::Native(r) => r.replace(haystack, replacement),
            CompiledRegex::Fancy(r) => r.replace(haystack, replacement),
        }
    }

    /// `String#gsub` — replace all. Same Cow discipline as
    /// `replace`.
    pub(crate) fn replace_all<'h>(&self, haystack: &'h str, replacement: &str) -> std::borrow::Cow<'h, str> {
        match self {
            CompiledRegex::Native(r) => r.replace_all(haystack, replacement),
            CompiledRegex::Fancy(r) => r.replace_all(haystack, replacement),
        }
    }

    /// Collect all match positions + per-group spans, eagerly,
    /// in engine-agnostic owned form. Used by
    /// `String#split(regex[, limit])` so the split walker can
    /// operate independently of which engine produced the
    /// matches (`regex::Captures` and `fancy_regex::Captures`
    /// are distinct lifetime-bound types).
    ///
    /// fancy-regex's `captures_iter` yields `Result<Captures>`;
    /// we stop iteration on the first error (same swallow
    /// rationale as `is_match`'s wrapper — adversarial
    /// recursion-limit hits would otherwise need to plumb
    /// Result through every split call site). For the
    /// lookahead-only patterns that motivated layer #17
    /// (sinatra's `cleaned_caller`) this never fires.
    pub(crate) fn split_matches(&self, haystack: &str) -> Vec<SplitMatch> {
        let mut out: Vec<SplitMatch> = Vec::new();
        match self {
            CompiledRegex::Native(r) => {
                for caps in r.captures_iter(haystack) {
                    if let Some(m0) = caps.get(0) {
                        let range = (m0.start(), m0.end());
                        let groups: Vec<Option<(usize, usize)>> = (1..caps.len())
                            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                            .collect();
                        out.push(SplitMatch { range, groups });
                    }
                }
            }
            CompiledRegex::Fancy(r) => {
                for caps_res in r.captures_iter(haystack) {
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
