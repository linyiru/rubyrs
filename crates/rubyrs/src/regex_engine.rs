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

/// Prepared-pattern length above which a fancy-regex (lookaround /
/// backref / possessive) pattern defers its build to first use instead
/// of eager-building at construction. Below it, eager build is cheap
/// (<0.5ms) and keeps construction-time error reporting; above it the
/// fancy compiler's super-linear cost dominates (the RFC3986 URI grammar
/// at ~1.4KB takes ~12ms), so deferral is the win. Empirically the
/// fancy compile stays sub-millisecond up to a few hundred chars.
const LAZY_FANCY_THRESHOLD: usize = 256;

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
    /// Byte-oriented engine for matching BINARY (ASCII-8BIT) subjects,
    /// built lazily from `engine_pattern` with Unicode disabled
    /// (`(?-u)`), so `\x80`-`\xff` and `.`/`\w` operate on raw bytes —
    /// CRuby's behaviour for an ASCII-8BIT regexp / subject. `Some` is
    /// the built engine; `None` means it couldn't be built (the
    /// pattern needs Unicode, e.g. `\x{1234}`, or it's the fancy path)
    /// and the caller falls back to the UTF-8 engine. Only consulted
    /// for binary-encoded String subjects — UTF-8 matching is
    /// untouched. Motivation: rack Lint's `value.b !~ /[\x80-\xff]/n`
    /// over CGI env values.
    bytes_engine: std::cell::OnceCell<Option<regex::bytes::Regex>>,
    /// Byte-oriented engine ANCHORED at the haystack start (`\A(?:…)`),
    /// for `StringScanner#scan`/`check`/`skip`/`match?` — they match
    /// only AT the current position, never ahead. Without the `\A`
    /// wrapper a miss-at-pos would forward-scan the whole remaining
    /// buffer (turning kramdown's per-position `check` loop O(n²));
    /// the anchor makes a miss fail in O(1). `Some` is the built
    /// engine; `None` means it couldn't be built (Unicode/fancy
    /// pattern → caller falls back to the slice path).
    anchored_bytes_engine: std::cell::OnceCell<Option<regex::bytes::Regex>>,
    /// Fancy engine anchored at the haystack start (`\A(?:…)`), the
    /// backtracking counterpart of `anchored_bytes_engine` for patterns
    /// the linear byte engine can't build (lookaround / backref). Backs
    /// StringScanner's anchored match for those patterns so a per-
    /// position `check` neither copies the tail nor forward-scans with
    /// the slow engine — both of which made kramdown O(n²). `None` ⇒
    /// couldn't build (caller falls back to the Ruby slice path).
    anchored_fancy_engine: std::cell::OnceCell<Option<fancy_regex::Regex>>,
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
    /// Memoized DUPLICATE named-capture groups: `(name, [group
    /// indices])` for any name written on 2+ groups (legal in
    /// Ruby/Oniguruma — `(?<a>X)|(?<a>Y)` — but fancy-regex keeps
    /// every group's value while collapsing the NAME onto one group,
    /// so `m[:a]` would resolve to a non-participating arm). Parsed
    /// LAZILY from `source` on first named-capture access and ONLY
    /// trusted when the parsed group count matches the engine's — see
    /// `duplicate_named_groups`. Empty (the common case) means no dup
    /// names, so the named-capture path is byte-for-byte unchanged.
    dup_named: std::cell::OnceCell<Vec<(String, Vec<usize>)>>,
    /// True when the ORIGINAL pattern began with a `\G` anchor (which
    /// `preprocess_regex_pattern` strips so the linear engine can compile).
    /// `\G` means "match exactly at the search position"; the match-at-pos
    /// paths (`String#match`/`match?`/`=~` with an offset) re-honour it by
    /// anchoring to the sliced tail's start. Set post-construction at the
    /// regex-literal / string-coercion sites (see `set_g_anchored`); the
    /// non-positional scan/gsub paths IGNORE it (they keep the stripped
    /// semantics that `regex_g_anchor.rb` pins). Default false.
    g_anchored: bool,
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
    // Ambiguous `\k<name>` backrefs to duplicated group names (which both
    // engines reject) → numeric `\N`, before any other preparation.
    let engine_pattern = rewrite_dup_named_backrefs(engine_pattern, ruby_flags & RB_EXTENDED != 0);
    let prepared = prepare_pattern(&engine_pattern);
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
            bytes_engine: std::cell::OnceCell::new(),
            anchored_bytes_engine: std::cell::OnceCell::new(),
            anchored_fancy_engine: std::cell::OnceCell::new(),
            engine_pattern: prepared.into(),
            ruby_flags,
            source: bare_source.into(),
            dup_named: std::cell::OnceCell::new(),
            g_anchored: false,
        }),
        // Syntax the linear engine rejects (lookaround, backrefs,
        // possessive quantifiers). For LARGE such patterns, DEFER the
        // fancy-regex build to first use — `fancy_regex::Regex::new` is
        // ~10-100x the native build for grammar-scale patterns (the
        // 1.4KB RFC3986 URI grammar eager-builds in ~12ms vs the native
        // engine's ~0.5ms; uri/rouge alone cost ~40ms of Bridgetown's
        // require phase building patterns that are never matched at
        // load). `engine()` builds native-then-fancy lazily, mirroring
        // the linear path's deferral.
        //
        // SMALL patterns used to keep the EAGER build for its
        // construction-time RegexpError — but "small" is no shield
        // against a pathological build: a ~20-char case-insensitive
        // lookaround over `[\W_]` costs ~6.5ms in fancy-regex's
        // NFA construction (Unicode case-fold expansion), and rubocop's
        // Naming/InclusiveLanguage compiles five such per FILE (~26ms
        // per file, never matched — the cop is disabled). So small
        // patterns now defer too, as long as `Expr::parse_tree`
        // (a cheap syntax-only parse) accepts them — that keeps the
        // construction-time RegexpError for malformed patterns; only
        // a pattern that parses but fails the full NFA build (resource
        // limits) shifts its error to first match, the same tradeoff
        // the linear path and the large-pattern branch already made.
        Err(_) if prepared.len() > LAZY_FANCY_THRESHOLD
            || fancy_regex::Expr::parse_tree(&prepared).is_ok() => {
            if std::env::var_os("RUBYRS_REGEX_STATS").is_some_and(|v| v == "2") {
                eprintln!("[fancy-regex:lazy] /{}/", bare_source);
            }
            Ok(CompiledRegex {
                engine: std::cell::OnceCell::new(),
                bytes_engine: std::cell::OnceCell::new(),
                anchored_bytes_engine: std::cell::OnceCell::new(),
                anchored_fancy_engine: std::cell::OnceCell::new(),
                engine_pattern: prepared.into(),
                ruby_flags,
                source: bare_source.into(),
                dup_named: std::cell::OnceCell::new(),
                g_anchored: false,
            })
        }
        Err(syntax_err) => match fancy_regex::Regex::new(&prepared) {
            Ok(re) => {
                // RUBYRS_REGEX_STATS=2: name every pattern that
                // lands on the backtracking engine — the fancy VM
                // showing up in a profile means one of these is
                // hot (jekyll-1k hunt, 2026-06-12).
                if std::env::var_os("RUBYRS_REGEX_STATS").is_some_and(|v| v == "2") {
                    eprintln!("[fancy-regex] /{}/", bare_source);
                }
                let cell = std::cell::OnceCell::new();
                let _ = cell.set(Engine::Fancy(re));
                // No bytes engine for the fancy path (lookaround /
                // backrefs have no byte-oriented build); binary
                // subjects fall back to the UTF-8 engine.
                let bytes_cell = std::cell::OnceCell::new();
                let _ = bytes_cell.set(None);
                let anchored_bytes_cell = std::cell::OnceCell::new();
                let _ = anchored_bytes_cell.set(None);
                Ok(CompiledRegex {
                    engine: cell,
                    bytes_engine: bytes_cell,
                    anchored_bytes_engine: anchored_bytes_cell,
                    // Keep the prepared pattern (NOT "") so the anchored
                    // fancy engine can be rebuilt for StringScanner's
                    // anchored match. Safe: `engine()` is pre-filled here
                    // so it never reads this; `bytes_engine()` still fails
                    // to build a byte engine from a fancy pattern → None.
                    engine_pattern: prepared.into(),
                    ruby_flags,
                    source: bare_source.into(),
                    dup_named: std::cell::OnceCell::new(),
                    anchored_fancy_engine: std::cell::OnceCell::new(),
                    g_anchored: false,
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
    // `\x20`, not a literal space: under `(?x)` the Rust engines
    // ignore whitespace INSIDE character classes too (Onigmo keeps
    // it), so a literal space spliced into a class silently vanishes
    // from extended-mode patterns. Caught by rouge's ruby lexer:
    // its x-mode `(module)(\s+)(...)` rule stopped matching the
    // space after `module` once \s became a class with a bare
    // space in it.
    const SPACE: &str = "\\x20\\t\\r\\n\\f\\x0B";
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
/// Rewrite `\k<name>` / `\k'name'` PATTERN backreferences to a numeric
/// `\N` backref WHEN `name` is a DUPLICATED capture-group name.
///
/// Ruby/Onigmo allow the same group name on both sides of an alternation
/// (`(?<x>A)|(?<x>B)`) and let `\k<name>` refer to whichever branch
/// matched. fancy-regex accepts the duplicate group *definitions* and
/// numeric backrefs, but rejects a `\k<name>` backref to the ambiguous
/// name ("Invalid group name in back reference"); the linear `regex`
/// engine has no backrefs at all. So each ambiguous `\k<name>` is
/// rewritten to the index of the nearest same-named group opened
/// textually before it — the one in scope within that alternation branch.
///
/// Non-duplicated names are left as `\k<name>` (they compile fine and
/// keep the pattern readable); a pattern with no duplicated names is
/// returned borrowed (zero-copy — the overwhelmingly common case).
/// Name-based capture access (`md[:name]`, `named_captures`) is
/// unaffected: it resolves through the SOURCE parse, which keeps the
/// duplicate names, via `named_capture_index_map` — not the engine view.
///
/// Surfaced by RuboCop's Lint/DuplicateMethods, whose `humanize_scope`
/// uses exactly `/(?:(?<name>.*)::)#<Class:\k<name>>|#<Class:(?<name>.*)>.../`.
fn rewrite_dup_named_backrefs(pat: &str, base_extended: bool) -> std::borrow::Cow<'_, str> {
    // Pass 1: which names are duplicated?
    let (_, names) = parse_capture_groups(pat, base_extended);
    let mut dup: Vec<&str> = Vec::new();
    for (n, _) in &names {
        if names.iter().filter(|(m, _)| m == n).count() >= 2 && !dup.contains(&n.as_str()) {
            dup.push(n);
        }
    }
    if dup.is_empty() {
        return std::borrow::Cow::Borrowed(pat);
    }

    // Pass 2: walk (same group-open rules as `parse_capture_groups`),
    // splicing each `\k<dupname>` backref to `\N`. Copy everything else
    // verbatim into a byte buffer (input is UTF-8; only ASCII is inserted).
    let b = pat.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(pat.len());
    let mut i = 0usize;
    let mut group = 0usize;
    // Capturing groups seen so far, as (index, name?) — used to resolve a
    // backref to the nearest same-named group already opened.
    let mut seen: Vec<(usize, Option<String>)> = Vec::new();
    let mut in_class = false;
    let mut class_start = false;
    let mut ext: Vec<bool> = vec![base_extended];
    let extended = |ext: &[bool]| *ext.last().unwrap_or(&base_extended);
    let read_until = |start: usize, term: u8| -> Option<(String, usize)> {
        let mut j = start;
        while j < b.len() && b[j] != term {
            j += 1;
        }
        if j >= b.len() {
            return None;
        }
        std::str::from_utf8(&b[start..j]).ok().map(|s| (s.to_string(), j))
    };
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            // A `\k<name>`/`\k'name'` named backref outside a class is the
            // only rewrite target; inside `[...]` a backref is literal.
            if !in_class && b.get(i + 1) == Some(&b'k') {
                let close = match b.get(i + 2) {
                    Some(&b'<') => Some(b'>'),
                    Some(&b'\'') => Some(b'\''),
                    _ => None,
                };
                if let Some(term) = close
                    && let Some((name, j)) = read_until(i + 3, term)
                    && dup.contains(&name.as_str())
                    && let Some(&(idx, _)) =
                        seen.iter().rev().find(|(_, n)| n.as_deref() == Some(&name))
                {
                    out.push(b'\\');
                    out.extend_from_slice(idx.to_string().as_bytes());
                    i = j + 1;
                    continue;
                }
            }
            // Verbatim escape: copy `\` + its next byte (backrefs/escapes
            // never open a group). A trailing lone `\` copies just itself.
            out.push(b'\\');
            if let Some(&n) = b.get(i + 1) {
                out.push(n);
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_class {
            if c == b']' && !class_start {
                in_class = false;
            }
            class_start = c == b'^' && class_start;
            out.push(c);
            i += 1;
            continue;
        }
        if c == b'#' && extended(&ext) {
            while i < b.len() && b[i] != b'\n' {
                out.push(b[i]);
                i += 1;
            }
            continue;
        }
        match c {
            b'[' => {
                in_class = true;
                class_start = true;
                out.push(c);
                i += 1;
            }
            b')' => {
                if ext.len() > 1 {
                    ext.pop();
                }
                out.push(c);
                i += 1;
            }
            b'(' if b.get(i + 1) == Some(&b'?') => {
                let after = b.get(i + 2).copied();
                let is_lookbehind = after == Some(b'<')
                    && matches!(b.get(i + 3), Some(&x) if x == b'=' || x == b'!');
                if (after == Some(b'<') && !is_lookbehind) || after == Some(b'\'') {
                    let (name_start, term) =
                        if after == Some(b'<') { (i + 3, b'>') } else { (i + 3, b'\'') };
                    group += 1;
                    ext.push(extended(&ext));
                    match read_until(name_start, term) {
                        Some((name, j)) => {
                            seen.push((group, Some(name)));
                            out.extend_from_slice(&b[i..=j]);
                            i = j + 1;
                        }
                        None => {
                            out.extend_from_slice(&b[i..]);
                            return std::borrow::Cow::Owned(String::from_utf8(out).unwrap());
                        }
                    }
                } else if after == Some(b'P') && b.get(i + 3) == Some(&b'<') {
                    group += 1;
                    ext.push(extended(&ext));
                    match read_until(i + 4, b'>') {
                        Some((name, j)) => {
                            seen.push((group, Some(name)));
                            out.extend_from_slice(&b[i..=j]);
                            i = j + 1;
                        }
                        None => {
                            out.extend_from_slice(&b[i..]);
                            return std::borrow::Cow::Owned(String::from_utf8(out).unwrap());
                        }
                    }
                } else if after == Some(b'#') {
                    let mut j = i + 3;
                    while j < b.len() && b[j] != b')' {
                        if b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    let end = (j + 1).min(b.len());
                    out.extend_from_slice(&b[i..end]);
                    i = end;
                } else if matches!(after, Some(b'=') | Some(b'!') | Some(b'>')) || is_lookbehind {
                    ext.push(extended(&ext));
                    out.extend_from_slice(&b[i..i + 2]);
                    i += 2;
                } else {
                    // Flag group/directive `(?flags:` or `(?flags)` — copy
                    // through the spec, tracking the `x` (extended) flag.
                    let mut j = i + 2;
                    let mut new_ext = extended(&ext);
                    let mut sign = true;
                    while j < b.len() {
                        match b[j] {
                            b'x' => new_ext = sign,
                            b'-' => sign = false,
                            b'i' | b'm' | b's' | b'a' | b'd' | b'u' | b'n' => {}
                            b':' => {
                                ext.push(new_ext);
                                j += 1;
                                break;
                            }
                            b')' => {
                                if let Some(top) = ext.last_mut() {
                                    *top = new_ext;
                                }
                                j += 1;
                                break;
                            }
                            _ => break,
                        }
                        j += 1;
                    }
                    let end = j.min(b.len());
                    out.extend_from_slice(&b[i..end]);
                    i = end;
                }
            }
            b'(' => {
                group += 1;
                ext.push(extended(&ext));
                seen.push((group, None));
                out.push(c);
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    std::borrow::Cow::Owned(String::from_utf8(out).unwrap_or_else(|_| pat.to_string()))
}

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
            // The native engine rejected it: either a large lookaround/
            // backref/possessive pattern whose fancy build was DEFERRED
            // (see `compile_with_flags`'s threshold arm), or a genuinely
            // malformed pattern. Build fancy now; a fancy failure here is
            // the deferred construction error surfacing at first match
            // (the `Runtime::eval` boundary converts the panic to a
            // RegexpError-shaped trap).
            Err(native_err) => match fancy_regex::Regex::new(&self.engine_pattern) {
                Ok(re) => Engine::Fancy(re),
                Err(fancy_err) => panic!(
                    "regex build failed at first use for /{}/: {} (also rejected by regex: {})",
                    self.source, fancy_err, native_err
                ),
            },
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

    /// True when the original pattern led with a `\G` anchor (see the field
    /// doc). The match-at-pos paths anchor on this; scan/gsub ignore it.
    pub(crate) fn g_anchored(&self) -> bool {
        self.g_anchored
    }

    /// Record that the original pattern led with `\G`. Called at the
    /// regex-literal / `String#match`-coercion compile sites (which still
    /// hold the un-stripped source) before the regex is shared via `Rc`.
    pub(crate) fn set_g_anchored(&mut self, v: bool) {
        self.g_anchored = v;
    }

    /// Predicate match over `tail` (already sliced to the search position):
    /// anchored to the start when the pattern led with `\G`, else a forward
    /// search. The `match?`-family arms use this so a `\G` anchor is honoured
    /// without the MatchData materialisation `String#match` needs.
    pub(crate) fn is_match_from(&self, tail: &str) -> bool {
        if self.g_anchored {
            match self.captures_owned_str_anchored(tail) {
                Some(inner) => inner.is_some(),
                None => self.is_match(tail),
            }
        } else {
            self.is_match(tail)
        }
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

    /// Absolute group index (0 = whole match, 1..N = capture groups)
    /// of the named capture group `name`, or `None` if no group has
    /// that name. `capture_names()` yields one `Option<&str>` per
    /// group in index order (group 0 + the unnamed groups are `None`),
    /// so `position` lands on the matching absolute index — exactly the
    /// `n` semantics `str_bracket_regex` / `slice!` already use for the
    /// Integer form. Lets `String#[](regexp, name)` / `#slice!(regexp,
    /// name)` resolve a String/Symbol capture reference against the
    /// PATTERN (independent of any match), so an unknown name is
    /// CRuby's `IndexError: undefined group name reference: <name>`.
    pub(crate) fn capture_name_index(&self, name: &str) -> Option<usize> {
        match self.engine() {
            Engine::Native(r) => r.capture_names().position(|n| n == Some(name)),
            Engine::Fancy(r) => r.capture_names().position(|n| n == Some(name)),
        }
    }

    /// Per-group capture names for groups 1..N, in index order (the
    /// whole-match group 0 is dropped). Entry `i` is `Some(name)` for a
    /// `(?<name>…)` group or `None` for an unnamed `(…)` group. Backs
    /// `MatchData#begin(:name)` / `#offset(:name)` — the materialiser
    /// stores this alongside the positional byte spans so a named index
    /// resolves to its group position.
    pub(crate) fn capture_group_names(&self) -> Vec<Option<String>> {
        let collect = |it: &mut dyn Iterator<Item = Option<&str>>| -> Vec<Option<String>> {
            it.skip(1).map(|n| n.map(|s| s.to_string())).collect()
        };
        match self.engine() {
            Engine::Native(r) => collect(&mut r.capture_names()),
            Engine::Fancy(r) => collect(&mut r.capture_names()),
        }
    }

    /// Named capture-group names in group-index order, deduplicated
    /// keeping the first occurrence — `Regexp#names` semantics
    /// (`/(?<a>.)(?<b>.)(?<a>.)/.names == ["a", "b"]`). `capture_names()`
    /// yields one `Option<&str>` per group (group 0 + unnamed groups are
    /// `None`); we drop the `None`s and collapse duplicates.
    pub(crate) fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |names: &mut dyn Iterator<Item = Option<&str>>| {
            for n in names.flatten() {
                if !out.iter().any(|seen| seen == n) {
                    out.push(n.to_string());
                }
            }
        };
        match self.engine() {
            Engine::Native(r) => push(&mut r.capture_names()),
            Engine::Fancy(r) => push(&mut r.capture_names()),
        }
        out
    }

    /// Names written on 2+ capture groups (`(?<a>X)|(?<a>Y)`), each
    /// paired with ALL its 1-based group indices. Memoized. The
    /// source parse is TRUSTED only when its total group count equals
    /// the engine's (`total_caps` is the engine `captures_len`,
    /// i.e. 1 + group count); on any mismatch — or no duplicate name —
    /// the result is empty and the named-capture path is unchanged.
    /// `build_named_captures` uses this to resolve a collapsed name to
    /// the arm that actually participated.
    fn duplicate_named_groups(&self, total_caps: usize) -> &[(String, Vec<usize>)] {
        self.dup_named.get_or_init(|| {
            let base_extended = self.ruby_flags & RB_EXTENDED != 0;
            let (count, names) = parse_capture_groups(&self.source, base_extended);
            if count + 1 != total_caps {
                return Vec::new();
            }
            let mut grouped: Vec<(String, Vec<usize>)> = Vec::new();
            for (name, idx) in names {
                match grouped.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1.push(idx),
                    None => grouped.push((name, vec![idx])),
                }
            }
            grouped.retain(|(_, idxs)| idxs.len() >= 2);
            grouped
        })
    }

    /// Full `Regexp#named_captures` map: each capture name paired with
    /// ALL its 1-based group indices, in first-appearance order. The
    /// engines collapse duplicate names (only the last `(?<a>…)` keeps
    /// the name), so `capture_group_names()` alone loses the earlier
    /// indices; when the source parse is trusted (group counts agree),
    /// a duplicated name's FULL index list is substituted. mustermann's
    /// `params` reads `regexp.named_captures` to gather every `*` splat
    /// capture into the "splat" array.
    pub(crate) fn named_capture_index_map(&self) -> Vec<(String, Vec<usize>)> {
        // Prefer the SOURCE parse when its group count matches the
        // engine's (the same trust gate `duplicate_named_groups` uses):
        // it preserves both every duplicate index AND CRuby's
        // first-source-appearance ordering, which the engine view loses
        // (the engine keeps only the last `(?<a>…)`).
        let base_extended = self.ruby_flags & RB_EXTENDED != 0;
        let (count, names) = parse_capture_groups(&self.source, base_extended);
        if count + 1 == self.captures_len() {
            let mut out: Vec<(String, Vec<usize>)> = Vec::new();
            for (name, idx) in names {
                match out.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1.push(idx),
                    None => out.push((name, vec![idx])),
                }
            }
            return out;
        }
        // Untrusted parse (mixed named/unnamed renumbering, exotic
        // syntax): fall back to the engine's per-group names.
        let cap_names = self.capture_group_names();
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, slot) in cap_names.iter().enumerate() {
            let Some(nm) = slot else { continue };
            let idx = i + 1;
            match out.iter_mut().find(|(n, _)| n == nm) {
                Some(slot) => slot.1.push(idx),
                None => out.push((nm.clone(), vec![idx])),
            }
        }
        out
    }

    // `scan_captures` (the original no-block `String#scan` capture
    // iterator) was REPLACED by `captures_iter_owned` above: the
    // MatchRange rework needs `$~` built from the same match data as
    // the returned Array, and propagates fancy-regex backtracker
    // errors instead of `.flatten()`-dropping them. Removed rather
    // than kept — its error-suppression semantics are exactly what
    // the replacement deprecates.

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

    /// Lazily build (and cache) the byte-oriented engine for matching
    /// BINARY subjects — Unicode disabled, so the pattern's `\x80`..`\xff`
    /// / `.` / `\w` operate on raw bytes (CRuby ASCII-8BIT semantics).
    /// `None` when it can't be built (the prepared pattern needs Unicode,
    /// e.g. a `\x{NNNN}` > 0xFF escape, or it's the fancy path with an
    /// empty `engine_pattern`); callers then fall back to the UTF-8 engine.
    fn bytes_engine(&self) -> Option<&regex::bytes::Regex> {
        self.bytes_engine
            .get_or_init(|| {
                if self.engine_pattern.is_empty() {
                    return None;
                }
                regex::bytes::RegexBuilder::new(&self.engine_pattern)
                    .unicode(false)
                    .build()
                    .ok()
            })
            .as_ref()
    }

    /// `match?` over a BINARY subject. `None` ⇒ no byte engine (caller
    /// falls back to the UTF-8 path); `Some(bool)` ⇒ the byte-level
    /// match verdict.
    pub(crate) fn is_match_bytes(&self, haystack: &[u8]) -> Option<bool> {
        Some(self.bytes_engine()?.is_match(haystack))
    }

    /// Byte engine anchored at the haystack start (`\A(?:…)`). Built
    /// lazily from `engine_pattern`. The `(?:…)` keeps the pattern's
    /// own group numbering; `\A` forces a match to begin at offset 0
    /// so a miss returns immediately instead of forward-scanning.
    fn anchored_bytes_engine(&self) -> Option<&regex::bytes::Regex> {
        self.anchored_bytes_engine
            .get_or_init(|| {
                if self.engine_pattern.is_empty() {
                    return None;
                }
                regex::bytes::RegexBuilder::new(&format!(r"\A(?:{})", self.engine_pattern))
                    .unicode(false)
                    .build()
                    .ok()
            })
            .as_ref()
    }

    /// Anchored byte-level captures at the haystack start — backs
    /// `StringScanner#scan`/`check`/`skip`/`match?` via
    /// `do_strscan_match_at_binary`. Outer `Option`: `None` ⇒ no byte
    /// engine (caller falls back). Inner: `None` ⇒ no match anchored at
    /// offset 0. Spans are byte offsets (= char offsets for ASCII).
    pub(crate) fn captures_owned_bytes_anchored(
        &self,
        haystack: &[u8],
    ) -> Option<Option<OwnedCaptures>> {
        let re = self.anchored_bytes_engine()?;
        let caps = match re.captures(haystack) {
            Some(c) => c,
            None => return Some(None),
        };
        let m0 = match caps.get(0) {
            Some(m) => m,
            None => return Some(None),
        };
        let lossy = |m: regex::bytes::Match<'_>| String::from_utf8_lossy(m.as_bytes()).into_owned();
        let groups = (1..caps.len()).map(|i| caps.get(i).map(lossy)).collect();
        let group_spans = (1..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect();
        let named = build_named_captures(
            re.capture_names(),
            self.duplicate_named_groups(caps.len()),
            |i| caps.get(i).map(lossy),
        );
        Some(Some(OwnedCaptures {
            whole: lossy(m0),
            m_start: m0.start(),
            m_end: m0.end(),
            groups,
            group_spans,
            named,
        }))
    }

    /// `=~` / `match` over a BINARY subject — byte-level captures. The
    /// OUTER `Option` distinguishes "no byte engine, fall back"
    /// (`None`) from "byte engine ran" (`Some`); the inner `Option` is
    /// the match (`None` ⇒ no match). Spans are BYTE offsets (= char
    /// offsets for ASCII-8BIT). Captured substrings are rendered
    /// lossily into the `String`-shaped `OwnedCaptures` — exact for the
    /// common valid-byte case; a documented divergence for `$~`
    /// pre/post-match over genuinely invalid UTF-8.
    pub(crate) fn captures_owned_bytes(&self, haystack: &[u8]) -> Option<Option<OwnedCaptures>> {
        let re = self.bytes_engine()?;
        let caps = match re.captures(haystack) {
            Some(c) => c,
            None => return Some(None),
        };
        let m0 = match caps.get(0) {
            Some(m) => m,
            None => return Some(None),
        };
        let lossy = |m: regex::bytes::Match<'_>| {
            String::from_utf8_lossy(m.as_bytes()).into_owned()
        };
        let groups = (1..caps.len()).map(|i| caps.get(i).map(lossy)).collect();
        let group_spans = (1..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect();
        let named = build_named_captures(
            re.capture_names(),
            self.duplicate_named_groups(caps.len()),
            |i| caps.get(i).map(lossy),
        );
        Some(Some(OwnedCaptures {
            whole: lossy(m0),
            m_start: m0.start(),
            m_end: m0.end(),
            groups,
            group_spans,
            named,
        }))
    }

    /// Fancy engine anchored at the haystack start (`\A(?:…)`), built
    /// lazily from `engine_pattern`. Counterpart of
    /// `anchored_bytes_engine` for lookaround / backref patterns.
    fn anchored_fancy_engine(&self) -> Option<&fancy_regex::Regex> {
        self.anchored_fancy_engine
            .get_or_init(|| {
                if self.engine_pattern.is_empty() {
                    return None;
                }
                fancy_regex::Regex::new(&format!(r"\A(?:{})", self.engine_pattern)).ok()
            })
            .as_ref()
    }

    /// Anchored str-level captures at the haystack start, via the fancy
    /// engine — backs `do_strscan_match_at_binary`'s fallback when the
    /// pattern has no linear byte engine. Outer `Option`: `None` ⇒ no
    /// fancy engine could be built (caller falls back to the Ruby slice
    /// path). Inner: `None` ⇒ no anchored match. Spans are byte offsets.
    pub(crate) fn captures_owned_str_anchored(
        &self,
        haystack: &str,
    ) -> Option<Option<OwnedCaptures>> {
        let re = self.anchored_fancy_engine()?;
        let caps = match re.captures(haystack) {
            Ok(Some(c)) => c,
            Ok(None) => return Some(None),
            Err(_) => return Some(None),
        };
        let m0 = match caps.get(0) {
            Some(m) => m,
            None => return Some(None),
        };
        let groups = (1..caps.len())
            .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
            .collect();
        let group_spans = (1..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect();
        let named = build_named_captures(
            re.capture_names(),
            self.duplicate_named_groups(caps.len()),
            |i| caps.get(i).map(|m| m.as_str().to_string()),
        );
        Some(Some(OwnedCaptures {
            whole: m0.as_str().to_string(),
            m_start: m0.start(),
            m_end: m0.end(),
            groups,
            group_spans,
            named,
        }))
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

    /// Byte-level `String#sub` for a BINARY subject — preserves the raw
    /// bytes instead of round-tripping through a lossy UTF-8 view (which
    /// turns every invalid byte into a 3-byte U+FFFD, corrupting and
    /// GROWING binary payloads such as rack multipart file bodies).
    /// `None` ⇒ no byte engine, caller falls back to the UTF-8 path.
    /// `replacement` is in the regex crate's `$N` backref form.
    pub(crate) fn replace_bytes(&self, haystack: &[u8], replacement: &[u8]) -> Option<Vec<u8>> {
        Some(self.bytes_engine()?.replace(haystack, replacement).into_owned())
    }

    /// Byte-level `String#gsub` for a BINARY subject — replace all.
    /// Same discipline as `replace_bytes`.
    pub(crate) fn replace_all_bytes(&self, haystack: &[u8], replacement: &[u8]) -> Option<Vec<u8>> {
        Some(self.bytes_engine()?.replace_all(haystack, replacement).into_owned())
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
                    let group_spans = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect();
                    let named = build_named_captures(
                        r.capture_names(),
                        self.duplicate_named_groups(caps.len()),
                        |i| caps.get(i).map(|m| m.as_str().to_string()),
                    );
                    Ok(Some(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        group_spans,
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
                    let group_spans = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect();
                    let named = build_named_captures(
                        r.capture_names(),
                        self.duplicate_named_groups(caps.len()),
                        |i| caps.get(i).map(|m| m.as_str().to_string()),
                    );
                    Ok(Some(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        group_spans,
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
                    let group_spans = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect();
                    let named = build_named_captures(
                        r.capture_names(),
                        self.duplicate_named_groups(caps.len()),
                        |i| caps.get(i).map(|m| m.as_str().to_string()),
                    );
                    out.push(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        group_spans,
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
                    let group_spans = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect();
                    let named = build_named_captures(
                        r.capture_names(),
                        self.duplicate_named_groups(caps.len()),
                        |i| caps.get(i).map(|m| m.as_str().to_string()),
                    );
                    out.push(OwnedCaptures {
                        whole: m0.as_str().to_string(),
                        m_start: m0.start(),
                        m_end: m0.end(),
                        groups,
                        group_spans,
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
    /// Byte spans for groups 1..N (parallel to `groups`) —
    /// `String#slice!(regexp, n)` cuts the receiver at the
    /// capture's span, so spans travel with the strings.
    pub(crate) group_spans: Vec<Option<(usize, usize)>>,
    /// `(name, matched | None)` for each NAMED capture group.
    pub(crate) named: Vec<(String, Option<String>)>,
}

/// Build the `(name, value)` list from a `capture_names()` iterator
/// and a group-value accessor, resolving DUPLICATE group names
/// correctly. Ruby/Oniguruma allows several groups to share a name
/// (e.g. rack's `(?<host>\[(?<address>...)\]|(?<address>...))`); the
/// linear `regex` crate rejects that so such patterns run on
/// fancy-regex, which keeps every group's POSITION/value but the
/// duplicate name resolves to the LAST group that PARTICIPATED — not
/// the textually-last group, which for an alternation is `nil`.
/// So: dedup to one entry per name, a later matched value overrides,
/// and a later `nil` never clobbers an earlier match.
fn build_named_captures<'a>(
    names: impl Iterator<Item = Option<&'a str>>,
    dup_named: &[(String, Vec<usize>)],
    get: impl Fn(usize) -> Option<String>,
) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for (i, n) in names.enumerate() {
        let Some(name) = n else { continue };
        let val = get(i);
        if let Some(slot) = out.iter_mut().find(|(nm, _)| nm == name) {
            if val.is_some() {
                slot.1 = val;
            }
        } else {
            out.push((name.to_string(), val));
        }
    }
    // Augment names the engine collapsed onto a single group: resolve
    // each from ALL its group indices, taking the LAST that
    // participated (CRuby/Oniguruma semantics). Empty unless the
    // source carried a duplicate name AND the parse was trusted, so
    // single-named patterns are untouched.
    for (name, indices) in dup_named {
        let resolved = indices.iter().filter_map(|&idx| get(idx)).last();
        if let Some(slot) = out.iter_mut().find(|(nm, _)| nm == name) {
            slot.1 = resolved;
        } else {
            out.push((name.clone(), resolved));
        }
    }
    out
}

/// Best-effort scan of a regex SOURCE for capturing-group structure:
/// returns `(total capturing groups, [(name, 1-based group index)])`.
/// Group indices follow the standard open-paren order that both the
/// `regex` crate and fancy-regex use. Skipped from the count: char
/// classes (`[...]`, where `(` is literal), escapes (`\(`), inline
/// comments (`(?#...)`), non-capturing / lookaround / atomic / flag
/// groups (`(?:`, `(?=`, `(?!`, `(?<=`, `(?<!`, `(?>`, `(?x-mi:`)).
/// Only plain `(` and the named forms `(?<name>` / `(?'name'` /
/// `(?P<name>` add a group.
///
/// EXTENDED (`/x`) mode is tracked through nested flag groups
/// (`base_extended` is the regexp's own `x` flag) so that an `x`-mode
/// `#`-to-end-of-line comment — which can itself contain parens, as in
/// rack's `# ... (except square brackets) ...` — is skipped rather
/// than miscounted.
///
/// HEURISTIC — the caller verifies `total + 1 == engine group count`
/// and discards the result on any mismatch, so a miscount can only
/// degrade to the unaugmented path, never corrupt a capture.
fn parse_capture_groups(src: &str, base_extended: bool) -> (usize, Vec<(String, usize)>) {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut group = 0usize;
    let mut names: Vec<(String, usize)> = Vec::new();
    let mut in_class = false;
    let mut class_start = false;
    // Stack of the EXTENDED flag for each open group scope; the top is
    // the current mode. One entry pushed per group open, popped on `)`.
    let mut ext: Vec<bool> = vec![base_extended];
    let extended = |ext: &[bool]| *ext.last().unwrap_or(&base_extended);
    let read_until = |start: usize, term: u8| -> Option<(String, usize)> {
        let mut j = start;
        while j < b.len() && b[j] != term {
            j += 1;
        }
        if j >= b.len() {
            return None;
        }
        std::str::from_utf8(&b[start..j]).ok().map(|s| (s.to_string(), j))
    };
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if in_class {
            // First char after `[` (or `[^`) may be a literal `]`.
            if c == b']' && !class_start {
                in_class = false;
            }
            class_start = c == b'^' && class_start;
            i += 1;
            continue;
        }
        // `/x` comment: `#` to end of line (outside a class). The comment
        // body is uninterpreted — its parens do not open groups.
        if c == b'#' && extended(&ext) {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        match c {
            b'[' => {
                in_class = true;
                class_start = true;
                i += 1;
            }
            b')' => {
                if ext.len() > 1 {
                    ext.pop();
                }
                i += 1;
            }
            b'(' if b.get(i + 1) == Some(&b'?') => {
                let after = b.get(i + 2).copied();
                let is_lookbehind =
                    after == Some(b'<') && matches!(b.get(i + 3), Some(&x) if x == b'=' || x == b'!');
                if after == Some(b'<') && !is_lookbehind {
                    group += 1;
                    ext.push(extended(&ext));
                    match read_until(i + 3, b'>') {
                        Some((name, j)) => {
                            names.push((name, group));
                            i = j + 1;
                        }
                        None => return (group, names),
                    }
                } else if after == Some(b'\'') {
                    group += 1;
                    ext.push(extended(&ext));
                    match read_until(i + 3, b'\'') {
                        Some((name, j)) => {
                            names.push((name, group));
                            i = j + 1;
                        }
                        None => return (group, names),
                    }
                } else if after == Some(b'P') && b.get(i + 3) == Some(&b'<') {
                    group += 1;
                    ext.push(extended(&ext));
                    match read_until(i + 4, b'>') {
                        Some((name, j)) => {
                            names.push((name, group));
                            i = j + 1;
                        }
                        None => return (group, names),
                    }
                } else if after == Some(b'#') {
                    // `(?#comment)` — skip to the closing paren.
                    let mut j = i + 3;
                    while j < b.len() && b[j] != b')' {
                        if b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    i = j + 1;
                } else if matches!(after, Some(b'=') | Some(b'!') | Some(b'>'))
                    || is_lookbehind
                {
                    // Lookaround / atomic group — non-capturing, mode
                    // unchanged. Push a scope so its `)` balances.
                    ext.push(extended(&ext));
                    i += 2;
                } else {
                    // Flag group/directive: `(?flags:` / `(?flags-neg:`
                    // (a non-capturing scope) or `(?flags)` / `(?flags-neg)`
                    // (an inline directive for the REST of the current
                    // scope). `(?:` is the flagless scope form. Parse the
                    // flag spec to track `x`.
                    let mut j = i + 2;
                    let mut new_ext = extended(&ext);
                    let mut sign = true; // before the `-`
                    while j < b.len() {
                        match b[j] {
                            b'x' => {
                                new_ext = sign;
                            }
                            b'-' => sign = false,
                            b'i' | b'm' | b's' | b'a' | b'd' | b'u' | b'n' => {}
                            b':' => {
                                // Scope: push the new mode; body parsed normally.
                                ext.push(new_ext);
                                j += 1;
                                break;
                            }
                            b')' => {
                                // Inline directive: applies to the rest of
                                // the CURRENT scope; no new group.
                                if let Some(top) = ext.last_mut() {
                                    *top = new_ext;
                                }
                                j += 1;
                                break;
                            }
                            _ => break, // not a flag spec — bail defensively
                        }
                        j += 1;
                    }
                    i = j;
                }
            }
            b'(' => {
                group += 1;
                ext.push(extended(&ext));
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    (group, names)
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
