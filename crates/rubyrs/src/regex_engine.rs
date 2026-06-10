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
    /// The engine, built LAZILY on first use. Construction
    /// (`compile_with_flags`) only VALIDATES the pattern via
    /// `regex_syntax` — full engine building (NFA, meta-strategy,
    /// DFA scaffolding; ~30-100KB live memory per pattern) is
    /// deferred until the first operation that actually matches.
    /// Motivation: on the real-Jekyll require chain, 352 regexes
    /// get constructed (top-level `FOO = /.../ ` constants across
    /// jekyll/kramdown/liquid/rouge) but only 39 are ever matched
    /// during a full site build — eager building wasted ~16MB of
    /// RSS (~half the require-phase footprint) plus the build
    /// time of 313 unused patterns.
    ///
    /// Patterns that the linear engine REJECTS at validation
    /// (lookaround/backrefs) still build their fancy-regex
    /// fallback eagerly — the fancy build is cheap (backtracking
    /// program, no DFA) and pre-filling keeps the error point at
    /// Regexp construction, same as before.
    engine: std::cell::OnceCell<Engine>,
    /// The fully-prepared engine pattern (charclass-octal
    /// rewrite + `(?m)` prefix + any inline `(?is)` Ruby-flag
    /// prefix already applied) fed to `regex::Regex::new` at
    /// first use. Empty when `engine` was pre-filled (fancy path).
    engine_pattern: Box<str>,
    /// Ruby flag bitmask (`RB_IGNORECASE | RB_EXTENDED |
    /// RB_MULTILINE`). `Regexp#options` returns this verbatim.
    ruby_flags: u8,
    /// The BARE pattern as written (no inline `(?is)` flag
    /// prefix). `Regexp#source` / `#to_s` / `#inspect` and trap
    /// formatting render this, never the flag-prefixed string fed
    /// to the engine.
    source: Box<str>,
}

/// Validates the pattern and constructs a `CompiledRegex` whose
/// linear engine is built lazily on first use (see the `engine`
/// field doc). Patterns the linear syntax rejects fall back to
/// an EAGER `fancy-regex` build — lookaround/backrefs are
/// exactly what fancy-regex exists for.
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
    let prepared = prepare_pattern(engine_pattern);
    // Validation-only parse: same syntax surface as
    // `regex::Regex::new` (AST parse + HIR translation) but the
    // HIR is dropped immediately — no NFA/DFA is built. A pattern
    // that validates here can only fail the real build on
    // resource limits (`CompiledTooBig`), which `engine()` below
    // surfaces at first use; the Jekyll/kramdown/liquid/rouge
    // corpus has zero such patterns (they all eager-built fine
    // before this change).
    match regex_syntax::Parser::new().parse(&prepared) {
        Ok(_) => Ok(CompiledRegex {
            engine: std::cell::OnceCell::new(),
            engine_pattern: prepared.into(),
            ruby_flags,
            source: bare_source.into(),
        }),
        // Syntax the linear engine rejects (lookaround,
        // backrefs) → eager fancy-regex build, pre-filling the
        // OnceCell. Keeps the construction-time error point for
        // genuinely malformed patterns.
        Err(syntax_err) => match fancy_regex::Regex::new(&prepared) {
            Ok(re) => {
                let cell = std::cell::OnceCell::new();
                let _ = cell.set(Engine::Fancy(re));
                Ok(CompiledRegex {
                    engine: cell,
                    engine_pattern: "".into(),
                    ruby_flags,
                    source: bare_source.into(),
                })
            }
            Err(fancy_err) => {
                // Both engines rejected. Prefer the fancy-regex
                // error message (covers the wider surface) but
                // mention the linear-engine failure too — same
                // shape as the pre-lazy error.
                Err(format!("{} (also rejected by regex: {})", fancy_err, syntax_err))
            }
        },
    }
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

/// Rewrite the Perl-style shorthand classes `\s \d \w \h` (and their
/// negations) to explicit ASCII classes. Ruby/Onigmo defines them as
/// ASCII-only — `\s` = `[ \t\r\n\f\v]`, `\d` = `[0-9]`, `\w` =
/// `[0-9A-Za-z_]`, `\h` = `[0-9A-Fa-f]` — while the Rust engines
/// default them to Unicode (`\s` matches U+00A0, `\d` matches
/// arabic-indic digits, `\w` matches every Unicode letter). Passing
/// them through verbatim silently OVER-matched: discovered by the
/// front-matter differential, where `---\s*\n` with a stray NBSP
/// after the fence matched on rubyrs but not on CRuby. `\h`/`\H`
/// are Onigmo-only spellings the Rust engines reject outright, so
/// rewriting also makes those patterns work at all.
///
/// `\b` / `\B` are deliberately NOT touched: Onigmo's word BOUNDARY
/// is Unicode-aware (an asymmetry with its ASCII `\w` — verified on
/// CRuby 3.4: `"café" =~ /caf\b/` → nil, `/café\b/` → 0), which is
/// exactly the Rust default.
///
/// Inside a character class the shorthands expand to a NESTED class
/// (`[\s]` → `[[ \t\r\n\f\x0B]]`, `[\S]` → `[[^ \t\r\n\f\x0B]]` —
/// regex-crate set notation), which composes correctly under both a
/// positive and a negated outer class. Nesting (rather than splicing
/// the member characters inline) also avoids manufacturing RANGES
/// out of thin air: CRuby rejects a shorthand as a range endpoint
/// (`/[\d-x]/` → SyntaxError "unmatched range specifier"); inline
/// splicing would have silently turned that into the range `9-x`,
/// while the nested form `[[0-9]-x]` reads the `-` as a literal.
/// (Accepting-with-literal-dash is still WIDER than CRuby's outright
/// rejection, but no real program carries a pattern its own runtime
/// can't parse.) POSIX bracket expressions `[[:alpha:]]` are
/// skipped verbatim so their inner `]` doesn't confuse the class
/// tracker. (POSIX classes themselves have a separate
/// Ruby-Unicode-vs-Rust-ASCII divergence — out of scope here,
/// tracked separately.)
fn rewrite_ascii_shorthand_classes(pat: &str) -> std::borrow::Cow<'_, str> {
    // Two rewrite triggers: backslash shorthands and POSIX bracket
    // expressions. `[[:alnum:]]` has no backslash at all — gating on
    // '\\' alone silently skipped the POSIX translation (caught by
    // the regex_posix_unicode_classes fixture's scan row, whose
    // pattern was the only one without a \A anchor).
    if !pat.contains('\\') && !pat.contains("[:") {
        return std::borrow::Cow::Borrowed(pat);
    }
    const SPACE: &str = " \\t\\r\\n\\f\\x0B";
    const DIGIT: &str = "0-9";
    const WORD: &str = "0-9A-Za-z_";
    const HEX: &str = "0-9A-Fa-f";
    let chars: Vec<char> = pat.chars().collect();
    let mut out = String::with_capacity(pat.len() + 16);
    let mut in_class = false;
    let mut at_class_start = false;
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            let body = match n {
                's' | 'S' => Some(SPACE),
                'd' | 'D' => Some(DIGIT),
                'w' | 'W' => Some(WORD),
                'h' | 'H' => Some(HEX),
                _ => None,
            };
            if let Some(body) = body {
                // Same spelling in or out of a class: outside it
                // IS the class; inside it nests as a set-notation
                // union member, correct under any outer polarity
                // (see doc).
                let neg = if n.is_ascii_uppercase() { "^" } else { "" };
                out.push_str(&format!("[{neg}{body}]"));
                i += 2;
                changed = true;
                at_class_start = false;
                continue;
            }
            // Any other escape pair: copy verbatim.
            out.push(c);
            out.push(n);
            i += 2;
            at_class_start = false;
            continue;
        }
        if in_class && c == '[' && chars.get(i + 1) == Some(&':') {
            // POSIX bracket expression. Onigmo's POSIX classes are
            // UNICODE-aware on UTF-8 strings (the mirror image of
            // the \s\d\w situation: there RUBY is the ASCII side) —
            // CRuby's [[:alpha:]] matches é/日/Ⅷ while Rust's
            // [[:alpha:]] is ASCII-only. Translate each name to the
            // Unicode property set CRuby ground-truth probing
            // produced (probe chars per class are in the
            // regex_posix_unicode_classes fixture):
            //   alpha  → \p{Alphabetic}            (é 日 Ⅷ ʰ, not ́ )
            //   digit  → \p{Nd}                    (٣ matches)
            //   upper/lower → \p{Upper/Lowercase}  (Ⅷ / ʰ match)
            //   space  → \p{White_Space}           (NBSP, NEL)
            //   blank  → tab + \p{Zs}
            //   word   → Alphabetic+M+Nd+Pc+Join_Control (= Rust's
            //            Unicode \w; spelled out so this pass's own
            //            ASCII \w rewrite can't interfere)
            //   punct  → P + Sm + Sc + Sk           (NOT So: © is
            //            graph-only in Onigmo)
            //   cntrl  → \p{Cc}                    (includes NEL)
            //   graph  → not(White_Space|Cc|Cn|Cs)  (Cf like the
            //            soft hyphen DOES match, mirroring Onigmo)
            //   print  → graph ∪ \p{Zs}
            //   xdigit / ascii → kept verbatim (Onigmo is ASCII-only
            //            for these two — fullwidth ｆ is NOT xdigit)
            // `[[:^name:]]` negation maps to a nested [^...] class.
            let mut j = i + 2;
            let neg = chars.get(i + 2) == Some(&'^');
            let name_start = if neg { i + 3 } else { i + 2 };
            while j + 1 < chars.len() && !(chars[j] == ':' && chars[j + 1] == ']') {
                j += 1;
            }
            if j + 1 < chars.len() {
                let name: String = chars[name_start..j].iter().collect();
                let body: Option<&str> = match name.as_str() {
                    "alpha" => Some(r"\p{Alphabetic}"),
                    "alnum" => Some(r"\p{Alphabetic}\p{Nd}"),
                    "digit" => Some(r"\p{Nd}"),
                    "upper" => Some(r"\p{Uppercase}"),
                    "lower" => Some(r"\p{Lowercase}"),
                    "space" => Some(r"\p{White_Space}"),
                    "blank" => Some(r"	\p{Zs}"),
                    "word" => Some(r"\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\p{Join_Control}"),
                    "punct" => Some(r"\p{P}\p{Sm}\p{Sc}\p{Sk}"),
                    "cntrl" => Some(r"\p{Cc}"),
                    // graph/print need a negated base — handled below.
                    _ => None,
                };
                let translated: Option<String> = match name.as_str() {
                    "graph" => Some(if neg {
                        r"[\p{White_Space}\p{Cc}\p{Cn}\p{Cs}]".to_string()
                    } else {
                        r"[^\p{White_Space}\p{Cc}\p{Cn}\p{Cs}]".to_string()
                    }),
                    "print" => Some(if neg {
                        // not(graph ∪ Zs) = White_Space|Cc|Cn|Cs minus Zs
                        // — expressible as a difference set.
                        r"[[\p{White_Space}\p{Cc}\p{Cn}\p{Cs}]--\p{Zs}]".to_string()
                    } else {
                        r"[[^\p{White_Space}\p{Cc}\p{Cn}\p{Cs}]\p{Zs}]".to_string()
                    }),
                    _ => body.map(|b| format!("[{}{b}]", if neg { "^" } else { "" })),
                };
                if let Some(t) = translated {
                    out.push_str(&t);
                    changed = true;
                } else {
                    // xdigit / ascii (already ASCII in Onigmo) or an
                    // unknown name — copy verbatim; the Rust parser
                    // gives unknown names a construction-time error,
                    // same as CRuby.
                    for &cc in &chars[i..=j + 1] {
                        out.push(cc);
                    }
                }
                i = j + 2;
                at_class_start = false;
                continue;
            }
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

/// Pattern preparation shared by validation and the (deferred)
/// engine build: charclass-octal rewrite, then the `(?m)` prefix.
///
/// Ruby's `^` / `$` are ALWAYS line anchors (they match at every
/// line boundary, not just the string ends — `\A` / `\z` / `\Z`
/// are the string anchors). The regex / fancy-regex crates default
/// `^` / `$` to string-only anchoring, switching to line anchors
/// only under engine `(?m)` (multi-line). So every Ruby pattern
/// gets a `(?m)` engine prefix. This is ORTHOGONAL to Ruby's `/m`
/// literal flag, which means dot-matches-newline → engine `(?s)`
/// (applied separately in `apply_ruby_flags`). The bare `source`
/// stored on `CompiledRegex` is untouched, so `#source` / `#inspect`
/// never see this prefix. Discovery: P3 Jekyll spike — jekyll's
/// `YAML_FRONT_MATTER_REGEXP` (`\A(...)^((---|\.\.\.)\s*$...)/m`)
/// relies on `^`/`$` matching the front-matter delimiter lines.
fn prepare_pattern(pattern: &str) -> String {
    let pattern = rewrite_charclass_octal_escapes(pattern);
    let pattern = rewrite_ascii_shorthand_classes(&pattern);
    format!("(?m){pattern}")
}

impl CompiledRegex {
    /// The engine, building it on first access. The pattern was
    /// already validated by `regex_syntax` at construction, so
    /// `regex::Regex::new` can only fail here on resource limits
    /// (`CompiledTooBig` — the linear engine's guard against
    /// pathological pattern sizes). That failure panics; the
    /// `Runtime::eval` boundary catches unwinding panics and
    /// converts them to a RuntimeError trap with the message
    /// preserved, so Ruby code sees a raise at the first match
    /// rather than a process abort. (Pre-lazy behaviour trapped at
    /// Regexp construction instead — acceptable shift: CRuby has
    /// no size limit at all, and the corpus has zero such
    /// patterns.)
    fn engine(&self) -> &Engine {
        self.engine.get_or_init(|| match regex::Regex::new(&self.engine_pattern) {
            Ok(re) => Engine::Native(re),
            Err(e) => panic!("regex build failed at first use for /{}/: {}", self.source, e),
        })
    }

    /// True once the engine has been built (first match) — or
    /// immediately for the eager fancy-regex path. `RUBYRS_REGEX_STATS=1`
    /// uses this to report how many cached regexes were ever used.
    pub(crate) fn is_built(&self) -> bool {
        self.engine.get().is_some()
    }

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
        match self.engine() {
            Engine::Native(r) => Some(r),
            Engine::Fancy(_) => None,
        }
    }

    /// Number of capture groups + 1 (group 0 = whole match), across
    /// both engines.
    pub(crate) fn captures_len(&self) -> usize {
        match self.engine() {
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
        match self.engine() {
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
        // Reflection only (Debug impl) — must NOT force the lazy
        // build. An unbuilt cell is always the native engine:
        // the fancy fallback is pre-filled at construction.
        match self.engine.get() {
            Some(Engine::Native(_)) | None => "regex",
            Some(Engine::Fancy(_)) => "fancy-regex",
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
        match self.engine() {
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
        match self.engine() {
            Engine::Native(r) => r.replace(haystack, replacement),
            Engine::Fancy(r) => r.replace(haystack, replacement),
        }
    }

    /// `String#gsub` — replace all. Same Cow discipline as
    /// `replace`.
    pub(crate) fn replace_all<'h>(&self, haystack: &'h str, replacement: &str) -> std::borrow::Cow<'h, str> {
        match self.engine() {
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
        match self.engine() {
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
        match self.engine() {
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
        match self.engine() {
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
