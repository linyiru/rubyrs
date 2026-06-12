//! The rule-table data model and its JSON loader.
//!
//! The JSON format mirrors what `tools/extract.rb` records from a rouge
//! lexer's `state_definitions`:
//!
//! ```json
//! {
//!   "lexer": "python",
//!   "states": {
//!     "root": [
//!       {"kind": "tok", "re": "\\n+", "opts": 4, "tok": "Text", "next": null},
//!       {"kind": "actions", "re": "(def)(\\s+)", "opts": 0,
//!        "actions": [["groups", ["Keyword", "Text"]], ["push", "funcname"]]},
//!       {"kind": "wordlist", "re": "[a-z_]\\w*", "opts": 0,
//!        "sets": [["Keyword", ["def", "return"]]], "default": "Name"},
//!       {"kind": "callback", "re": "..."},
//!       {"kind": "mixin", "state": "other"}
//!     ]
//!   },
//!   "shortnames": {"Keyword": "k", "Text": ""}
//! }
//! ```
//!
//! `opts` carries Ruby's `Regexp#options` bits (1 = `i`, 2 = `x`, 4 = Ruby
//! `m`, which is Rust's `s` / dot-matches-newline). Token names are interned
//! to [`TokenId`]s at load time; the engine merges and formats by id.

use std::collections::HashMap;

use fancy_regex::Regex;

/// Rule regex, linear-first: regex-automata's meta engine when the
/// pattern fits its syntax (no lookaround/backrefs/atomic groups),
/// the fancy backtracker otherwise. The split matters twice over —
/// the meta engine is linear-time AND supports true anchored-at-pos
/// search, while fancy can only scan-from-pos and post-filter
/// (`m0.start == pos`), paying a full O(n) scan on every miss.
pub(crate) enum CRegex {
    Linear(regex_automata::meta::Regex),
    Fancy(Regex),
}

impl CRegex {
    /// Anchored-at-`pos` capture search with full-haystack context
    /// (`^`/`\b` see the real surroundings). Returns group spans;
    /// `out[0]` is the whole match and is guaranteed to start at
    /// `pos`. `out` is a reusable buffer (cleared on entry).
    pub(crate) fn captures_at(
        &self,
        text: &str,
        pos: usize,
        out: &mut Vec<Option<(usize, usize)>>,
    ) -> bool {
        out.clear();
        match self {
            CRegex::Linear(re) => {
                let mut caps = re.create_captures();
                let input = regex_automata::Input::new(text)
                    .anchored(regex_automata::Anchored::Yes)
                    .range(pos..);
                re.captures(input, &mut caps);
                if !caps.is_match() {
                    return false;
                }
                for i in 0..caps.group_len() {
                    out.push(caps.get_group(i).map(|s| (s.start, s.end)));
                }
                true
            }
            CRegex::Fancy(re) => {
                let caps = match re.captures_from_pos(text, pos) {
                    Ok(Some(c)) => c,
                    _ => return false,
                };
                let m0 = caps.get(0).expect("capture 0 always present");
                if m0.start() != pos {
                    return false;
                }
                for i in 0..caps.len() {
                    out.push(caps.get(i).map(|m| (m.start(), m.end())));
                }
                true
            }
        }
    }
}
use serde_json::Value as J;

use crate::Error;

/// Interned token identifier. Index into [`LexerTable::token_names`] /
/// `token_shortnames`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenId(pub u32);

/// What the lexer does when the state stack changes.
#[derive(Debug, Clone)]
pub(crate) enum NextState {
    /// `:pop!` — pop one state.
    Pop,
    /// `:push` — push the current state again.
    PushSelf,
    /// Push the named state.
    Push(u32),
}

/// One recorded DSL action from a traced rouge rule block.
#[derive(Debug)]
pub(crate) enum Action {
    /// `groups Tok1, Tok2, …` — emit capture groups 1..n.
    Groups(Vec<TokenId>),
    /// `token Tok` — emit the whole match.
    Token(TokenId),
    /// `push :state` / bare `push`.
    Push(u32),
    PushSelf,
    /// `pop! n`.
    Pop(usize),
    /// `goto :state` — replace the stack top.
    Goto(u32),
}

#[derive(Debug)]
pub(crate) enum Kind {
    /// Declarative `rule re, Token, next_state`.
    Tok { tok: TokenId, next: Vec<NextState> },
    /// A rule block statically traced to a fixed action list.
    Actions(Vec<Action>),
    /// Classify the match by membership in word sets (the universal
    /// identifier-classification idiom in rouge lexers).
    Wordlist {
        sets: Vec<(TokenId, HashMap<String, ()>)>,
        default: TokenId,
    },
    /// A rule block AST-compiled to the Conditional Action IR
    /// (Track C) — executed natively, no callback round trip.
    Ir(Vec<crate::ir::IrOp>),
    /// A match-dependent rule block — delegated to [`crate::Callback`].
    Callback,
    /// Try another state's rules in place (`mixin :state`).
    Mixin(u32),
}

pub(crate) struct Rule {
    /// `None` for `mixin` entries.
    pub(crate) re: Option<CRegex>,
    /// The rule's regex begins with `^` — rouge pre-checks
    /// beginning-of-line before trying it.
    pub(crate) bol: bool,
    pub(crate) kind: Kind,
}

/// A compiled rouge lexer rule table, ready to drive [`crate::Lexer`].
pub struct LexerTable {
    pub(crate) states: Vec<Vec<Rule>>,
    pub(crate) state_names: Vec<String>,
    pub(crate) root: u32,
    pub(crate) token_names: Vec<String>,
    pub(crate) token_shortnames: Vec<String>,
    pub(crate) token_ids: HashMap<String, TokenId>,
    /// Token ids referenced by the RULES themselves (tok / actions /
    /// wordlist) — as opposed to the full registry, which also interns
    /// every shortname so out-of-band callbacks can emit any token.
    pub(crate) rule_token_ids: Vec<TokenId>,
    /// `Token::Tokens::Text` — rendered bare (no span) by the HTML
    /// formatter, exactly like rouge.
    pub(crate) tok_text: TokenId,
    /// `Token::Tokens::Error` — emitted when no rule matches.
    pub(crate) tok_error: TokenId,
}

impl LexerTable {
    /// The qualified rouge token name (`"Literal.String.Doc"`) for an id.
    pub fn token_name(&self, id: TokenId) -> &str {
        &self.token_names[id.0 as usize]
    }

    /// The rouge CSS shortname (`"sd"`) for an id; empty for `Text`.
    pub fn token_shortname(&self, id: TokenId) -> &str {
        &self.token_shortnames[id.0 as usize]
    }

    /// The `Text` token id (bare rendering in HTML).
    pub fn text_token(&self) -> TokenId {
        self.tok_text
    }

    /// Iterate every token qualname registered in this table (the full
    /// shortname registry — includes tokens only out-of-band callbacks
    /// could emit).
    pub fn token_names(&self) -> impl Iterator<Item = &str> {
        self.token_names.iter().map(String::as_str)
    }

    /// Look up a token id by qualified name.
    pub fn token_id(&self, qualname: &str) -> Option<TokenId> {
        self.token_ids.get(qualname).copied()
    }

    /// True when one of the table's RULES (tok / actions / wordlist) can
    /// emit `qualname`. Lets embedders apply policy (e.g. decline tables
    /// whose rules emit `Escape`, whose rouge-side handling depends on
    /// formatter options) without tripping on registry-only tokens.
    pub fn rule_emits(&self, qualname: &str) -> bool {
        match self.token_id(qualname) {
            Some(id) => self.rule_token_ids.contains(&id),
            None => false,
        }
    }

    /// Parse a rule table from the JSON produced by `tools/extract.rb`.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let v: J = serde_json::from_str(json).map_err(|e| Error::Table(e.to_string()))?;
        Builder::default().build(&v)
    }
}

#[derive(Default)]
struct Builder {
    token_ids: HashMap<String, TokenId>,
    token_names: Vec<String>,
    state_ids: HashMap<String, u32>,
    state_names: Vec<String>,
}

impl crate::ir::IrInterner for Builder {
    fn ir_tok(&mut self, name: &str) -> TokenId {
        self.tok(name)
    }
    fn ir_state(&mut self, name: &str) -> u32 {
        self.state(name)
    }
}

impl Builder {
    fn tok(&mut self, name: &str) -> TokenId {
        if let Some(id) = self.token_ids.get(name) {
            return *id;
        }
        let id = TokenId(self.token_names.len() as u32);
        self.token_ids.insert(name.to_string(), id);
        self.token_names.push(name.to_string());
        id
    }

    fn state(&mut self, name: &str) -> u32 {
        if let Some(id) = self.state_ids.get(name) {
            return *id;
        }
        let id = self.state_names.len() as u32;
        self.state_ids.insert(name.to_string(), id);
        self.state_names.push(name.to_string());
        id
    }

    fn build(mut self, v: &J) -> Result<LexerTable, Error> {
        let states_obj = v
            .get("states")
            .and_then(J::as_object)
            .ok_or_else(|| Error::Table("missing \"states\" object".into()))?;

        // Pre-register state ids in document order so mixin/push targets
        // resolve regardless of declaration order.
        for name in states_obj.keys() {
            self.state(name);
        }

        let tok_text = self.tok("Text");
        let tok_error = self.tok("Error");

        let mut states: Vec<Vec<Rule>> = Vec::with_capacity(states_obj.len());
        for (name, rules_v) in states_obj {
            let rules_arr = rules_v
                .as_array()
                .ok_or_else(|| Error::Table(format!("state {name:?} is not an array")))?;
            let mut rules = Vec::with_capacity(rules_arr.len());
            for r in rules_arr {
                rules.push(self.rule(name, r)?);
            }
            states.push(rules);
        }

        let root = *self
            .state_ids
            .get("root")
            .ok_or_else(|| Error::Table("no \"root\" state".into()))?;

        // Everything interned so far came from the rules (plus the
        // Text/Error pre-registrations) — snapshot it for `rule_emits`.
        let rule_token_ids: Vec<TokenId> =
            (0..self.token_names.len() as u32).map(TokenId).collect();

        let shortnames_obj = v
            .get("shortnames")
            .and_then(J::as_object)
            .ok_or_else(|| Error::Table("missing \"shortnames\" object".into()))?;
        // Intern the FULL shortname registry so out-of-band callbacks can
        // emit tokens no rule references.
        for name in shortnames_obj.keys() {
            self.tok(name);
        }
        let mut token_shortnames = vec![String::new(); self.token_names.len()];
        for (i, name) in self.token_names.iter().enumerate() {
            match shortnames_obj.get(name).and_then(J::as_str) {
                Some(s) => token_shortnames[i] = s.to_string(),
                // `Text` legitimately has no shortname; anything else
                // missing would panic in rouge's formatter too — fail
                // at load instead of mid-render.
                None if name == "Text" || name == "Error" => {}
                None => {
                    return Err(Error::Table(format!("no shortname for token {name:?}")));
                }
            }
        }

        Ok(LexerTable {
            states,
            state_names: self.state_names,
            root,
            token_names: self.token_names,
            token_shortnames,
            token_ids: self.token_ids,
            rule_token_ids,
            tok_text,
            tok_error,
        })
    }

    fn rule(&mut self, state: &str, r: &J) -> Result<Rule, Error> {
        let kind_s = r
            .get("kind")
            .and_then(J::as_str)
            .ok_or_else(|| Error::Table(format!("rule in {state:?} missing \"kind\"")))?;

        if kind_s == "mixin" {
            let target = r
                .get("state")
                .and_then(J::as_str)
                .ok_or_else(|| Error::Table("mixin missing \"state\"".into()))?;
            return Ok(Rule {
                re: None,
                bol: false,
                kind: Kind::Mixin(self.state(target)),
            });
        }

        let src = r
            .get("re")
            .and_then(J::as_str)
            .ok_or_else(|| Error::Table(format!("rule in {state:?} missing \"re\"")))?;
        let opts = r.get("opts").and_then(J::as_u64).unwrap_or(0);
        let re = compile_ruby_regex(src, opts)?;
        let bol = src.starts_with('^');

        let kind = match kind_s {
            "tok" => {
                let tok = r
                    .get("tok")
                    .and_then(J::as_str)
                    .ok_or_else(|| Error::Table("tok rule missing \"tok\"".into()))?;
                let tok = self.tok(tok);
                let next = self.next_states(r.get("next").unwrap_or(&J::Null))?;
                Kind::Tok { tok, next }
            }
            "actions" => {
                let acts = r
                    .get("actions")
                    .and_then(J::as_array)
                    .ok_or_else(|| Error::Table("actions rule missing \"actions\"".into()))?;
                let mut out = Vec::with_capacity(acts.len());
                for a in acts {
                    out.push(self.action(a)?);
                }
                Kind::Actions(out)
            }
            "wordlist" => {
                let sets_v = r
                    .get("sets")
                    .and_then(J::as_array)
                    .ok_or_else(|| Error::Table("wordlist rule missing \"sets\"".into()))?;
                let mut sets = Vec::with_capacity(sets_v.len());
                for s in sets_v {
                    let pair = s
                        .as_array()
                        .filter(|p| p.len() == 2)
                        .ok_or_else(|| Error::Table("wordlist set is not [token, words]".into()))?;
                    let tok = pair[0]
                        .as_str()
                        .ok_or_else(|| Error::Table("wordlist token not a string".into()))?;
                    let tok = self.tok(tok);
                    let words_v = pair[1]
                        .as_array()
                        .ok_or_else(|| Error::Table("wordlist words not an array".into()))?;
                    let mut words = HashMap::with_capacity(words_v.len());
                    for w in words_v {
                        let w = w
                            .as_str()
                            .ok_or_else(|| Error::Table("wordlist word not a string".into()))?;
                        words.insert(w.to_string(), ());
                    }
                    sets.push((tok, words));
                }
                let default = r
                    .get("default")
                    .and_then(J::as_str)
                    .ok_or_else(|| Error::Table("wordlist rule missing \"default\"".into()))?;
                let default = self.tok(default);
                Kind::Wordlist { sets, default }
            }
            "ir" => {
                let ops = r
                    .get("ops")
                    .and_then(J::as_array)
                    .ok_or_else(|| Error::Table("ir rule missing \"ops\"".into()))?;
                Kind::Ir(crate::ir::parse_ops(ops, self)?)
            }
            "callback" => Kind::Callback,
            other => return Err(Error::Table(format!("unknown rule kind {other:?}"))),
        };
        Ok(Rule {
            re: Some(re),
            bol,
            kind,
        })
    }

    fn next_states(&mut self, next: &J) -> Result<Vec<NextState>, Error> {
        match next {
            J::Null => Ok(vec![]),
            J::String(s) => Ok(vec![match s.as_str() {
                "pop!" => NextState::Pop,
                "push" => NextState::PushSelf,
                other => NextState::Push(self.state(other)),
            }]),
            J::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for n in a {
                    out.extend(self.next_states(n)?);
                }
                Ok(out)
            }
            other => Err(Error::Table(format!("bad next_state: {other}"))),
        }
    }

    fn action(&mut self, a: &J) -> Result<Action, Error> {
        let arr = a
            .as_array()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::Table("action is not a non-empty array".into()))?;
        let verb = arr[0]
            .as_str()
            .ok_or_else(|| Error::Table("action verb not a string".into()))?;
        let arg = arr.get(1);
        match verb {
            "groups" => {
                let toks = arg
                    .and_then(J::as_array)
                    .ok_or_else(|| Error::Table("groups action missing token list".into()))?;
                let mut out = Vec::with_capacity(toks.len());
                for t in toks {
                    let t = t
                        .as_str()
                        .ok_or_else(|| Error::Table("groups token not a string".into()))?;
                    out.push(self.tok(t));
                }
                Ok(Action::Groups(out))
            }
            "token" => {
                let t = arg
                    .and_then(J::as_str)
                    .ok_or_else(|| Error::Table("token action missing token".into()))?;
                Ok(Action::Token(self.tok(t)))
            }
            "push" => match arg.and_then(J::as_str) {
                Some("__self__") | None => Ok(Action::PushSelf),
                Some(s) => Ok(Action::Push(self.state(s))),
            },
            "pop" => Ok(Action::Pop(arg.and_then(J::as_u64).unwrap_or(1) as usize)),
            "goto" => {
                let s = arg
                    .and_then(J::as_str)
                    .ok_or_else(|| Error::Table("goto action missing state".into()))?;
                Ok(Action::Goto(self.state(s)))
            }
            other => Err(Error::Table(format!("unknown action verb {other:?}"))),
        }
    }
}
/// PORTED from rubyrs's `regex_engine.rs` (keep the two copies in
/// sync — carmine is a standalone published crate and cannot depend
/// on rubyrs). rouge lexer rules are written for Onigmo, so they
/// carry the same two divergences vs the Rust engines that bit the
/// rubyrs side: `\s \d \w \h` are ASCII in Onigmo but Unicode in
/// Rust (silent OVER-match), and POSIX brackets `[[:alpha:]]` are
/// Unicode in Onigmo but ASCII in Rust (silent UNDER-match — every
/// identifier rule in the shipped rouge tables uses
/// `[[:alpha:]_][[:alnum:]_]*`, so non-ASCII identifiers in
/// highlighted code tokenized differently from CRuby+rouge until
/// this rewrite). fancy-regex parses with regex-syntax, so the
/// rewritten property classes are accepted identically.
///
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


/// Compile a Ruby/Onigmo regex source + `Regexp#options` bits into a
/// [`fancy_regex::Regex`].
///
/// Semantics mapping (the classic Ruby-vs-Rust traps):
/// - Ruby's `^` / `$` are ALWAYS line anchors → Rust `m` enabled
///   unconditionally (without it a mid-text `#.*$` comment rule silently
///   fails — caught by a jekyll byte-diff).
/// - rouge matches through StringScanner, whose `^` ALSO matches at the
///   current scan position (ruby-lang bug #7092 — rouge's own source
///   carries an "XXX HACK" comment about it). Rules like
///   `(^[ \t]*)(match|case)…` rely on it mid-line. Since the engine
///   anchors every match at the current position anyway, a `^` in
///   LEADING position (start of pattern, possibly nested in group
///   openers / x-mode whitespace / alternation roots) is equivalent to
///   "true" — strip it. Non-leading `^` keeps line-anchor semantics.
/// - Ruby's `m` option (bit 4) means dot-matches-newline → Rust `s`.
/// - bits: 1 = `i`, 2 = `x`.
/// - Onigmo `{,n}` → `{0,n}`.
fn compile_ruby_regex(src: &str, opts: u64) -> Result<CRegex, Error> {
    let mut flags = String::from("m");
    if opts & 1 != 0 {
        flags.push('i');
    }
    if opts & 2 != 0 {
        flags.push('x');
    }
    if opts & 4 != 0 {
        flags.push('s');
    }
    let extended = opts & 2 != 0;
    let fixed = strip_leading_carets(src, extended).replace("{,", "{0,");
    let fixed = rewrite_ascii_shorthand_classes(&fixed);
    let pat = format!("(?{flags}){fixed}");
    // Linear first; syntax the meta engine rejects (lookaround,
    // backrefs, atomic groups) falls to the fancy backtracker.
    if let Ok(re) = regex_automata::meta::Regex::new(&pat) {
        return Ok(CRegex::Linear(re));
    }
    // Fancy can't anchor through its API, but it SUPPORTS `\G`
    // (anchors at the search start = captures_from_pos's pos), so
    // a miss is O(1) instead of a scan to end-of-text — the
    // engine probes every rule at every position, so unanchored
    // misses dominated the profile (124 samples even after the
    // linear split). `(?:...)` keeps capture numbering intact.
    let anchored = format!("\\G(?:{pat})");
    Regex::new(&anchored)
        .map(CRegex::Fancy)
        .map_err(|e| Error::Regex {
            pattern: src.to_string(),
            message: e.to_string(),
        })
}

/// Remove `^` anchors that sit in LEADING position — i.e. every char
/// before them is "transparent" at match start: group openers (`(`,
/// `(?:`, `(?<name>`, `(?flags:` / `(?flags)`), alternation bars at the
/// top of those groups, and (in x-mode) whitespace and `#` comments.
/// Char classes (`[^…]`) and escapes are respected. This reproduces
/// StringScanner's pos-matching `^` under carmine's anchored-at-pos
/// search (see `compile_ruby_regex`).
fn strip_leading_carets(src: &str, extended: bool) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut leading = true;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if leading {
            match c {
                '^' => {
                    // The pos-anchor quirk: drop it.
                    i += 1;
                    continue;
                }
                '|' => {
                    out.push(c);
                    i += 1;
                    continue; // a new alternation root is leading again
                }
                '(' => {
                    // Copy the group opener; stay leading for `(`,
                    // `(?:`, `(?<name>`, `(?flags:`, `(?flags)`.
                    let rest = &src[i..];
                    let opener_len = group_opener_len(rest);
                    out.push_str(&rest[..opener_len]);
                    i += opener_len;
                    continue;
                }
                _ if extended && c.is_whitespace() => {
                    out.push(c);
                    i += 1;
                    continue;
                }
                _ if extended && c == '#' => {
                    // x-mode comment runs to end of line.
                    while i < bytes.len() && bytes[i] != b'\n' {
                        out.push(bytes[i] as char);
                        i += 1;
                    }
                    continue;
                }
                _ => leading = false,
            }
        }
        // Non-leading copy, tracking escapes and char classes so a later
        // `|` at group top can't be confused with one inside `[...]`.
        match c {
            '\\' if i + 1 < bytes.len() => {
                out.push(c);
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            '[' => {
                // Copy the whole char class verbatim.
                out.push(c);
                i += 1;
                if i < bytes.len() && bytes[i] == b'^' {
                    out.push('^');
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b']' {
                    out.push(']');
                    i += 1;
                }
                while i < bytes.len() && bytes[i] != b']' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        out.push(bytes[i] as char);
                        out.push(bytes[i + 1] as char);
                        i += 2;
                    } else {
                        out.push(bytes[i] as char);
                        i += 1;
                    }
                }
                continue;
            }
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Length of a group opener at the start of `rest` (which begins with
/// `(`): `(`, `(?:`, `(?<name>`, `(?'name'`, `(?flags)` or `(?flags:`.
/// Lookarounds (`(?=`, `(?!`, `(?<=`, `(?<!`) are NOT leading-
/// transparent — return 1 so the caret inside them is preserved.
fn group_opener_len(rest: &str) -> usize {
    let b = rest.as_bytes();
    if b.len() < 2 || b[1] != b'?' {
        return 1; // plain `(`
    }
    if b.len() >= 3 && (b[2] == b'=' || b[2] == b'!') {
        return 1; // lookahead — treat `(` alone, inside is non-leading
    }
    if b.len() >= 3 && b[2] == b'<' {
        if b.len() >= 4 && (b[3] == b'=' || b[3] == b'!') {
            return 1; // lookbehind
        }
        // named group `(?<name>`
        if let Some(end) = rest.find('>') {
            return end + 1;
        }
        return 1;
    }
    if b.len() >= 3 && b[2] == b':' {
        return 3; // `(?:`
    }
    // `(?flags:` or `(?flags)`
    for (j, ch) in rest.char_indices().skip(2) {
        match ch {
            ':' | ')' => return j + 1,
            'a'..='z' | 'A'..='Z' | '-' => continue,
            _ => return 1,
        }
    }
    1
}
