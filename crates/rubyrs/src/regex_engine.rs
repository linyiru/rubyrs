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
//! API design: this PR lands the COMPILE fallback only. Existing
//! match-time call sites (`is_match`, `captures`, `replace`,
//! `find_iter`, etc.) keep using the linear `regex::Regex`
//! directly via `as_native()`; the fancy arm of each call site
//! raises a clear NotImplementedError naming the unsupported
//! operation. That keeps the diff to a manageable size while
//! unblocking the surface that motivated the change (regex
//! compilation no longer fails on lookaround patterns).
//! Migrating each operation to the dual-engine dispatcher is
//! incremental follow-up work tracked layer-by-layer.

#![cfg(feature = "regex")]

use std::fmt;

/// Compiled regex. Variant chosen at construction time based on
/// whether the linear engine accepted the pattern.
///
/// Public because `Value::Regex(Rc<CompiledRegex>)` is exposed
/// through the embedder-visible `Value` enum, but the variants
/// and helper methods stay `pub(crate)` — host code doesn't
/// need to introspect the engine.
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
    /// migration strategy: existing call sites use this
    /// accessor and surface a NotImplementedError on the fancy
    /// arm; new operations are written to dispatch through the
    /// enum from the start.
    pub(crate) fn as_native(&self) -> Option<&regex::Regex> {
        match self {
            CompiledRegex::Native(r) => Some(r),
            CompiledRegex::Fancy(_) => None,
        }
    }

    /// Engine label for error messages. Used by the
    /// "operation not supported on fancy-regex pattern" trap
    /// path so the user sees which engine produced the regex.
    pub(crate) fn engine_name(&self) -> &'static str {
        match self {
            CompiledRegex::Native(_) => "regex",
            CompiledRegex::Fancy(_) => "fancy-regex",
        }
    }

    /// True iff the haystack contains a match. Both engines
    /// support this directly; fancy-regex's `is_match` is
    /// fallible (recursion limit) — collapse the error to
    /// `false` so this stays a plain `bool`.
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
