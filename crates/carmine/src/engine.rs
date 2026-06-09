//! The lexer engine — rouge `RegexLexer` semantics over a [`LexerTable`].

use crate::table::{Action, Kind, LexerTable, NextState, Rule, TokenId};
use crate::Error;

/// Handler for rules rouge defines with match-dependent Ruby blocks
/// (`kind: "callback"` in the table). An embedder bridging back to a live
/// rouge lexer implements this by invoking the original block and replaying
/// its DSL calls onto [`EngineOps`].
pub trait Callback {
    /// Called when a callback rule's regex matched. `groups[0]` is the
    /// whole match; `groups[i]` the i-th capture (None when unmatched).
    /// Return `Ok(())` after driving `ops`, or `Err` to abort the lex.
    fn invoke(
        &mut self,
        ops: &mut EngineOps<'_, '_>,
        state: &str,
        rule_index: usize,
        groups: &[Option<&str>],
    ) -> Result<(), Error>;
}

/// The default callback handler: declines every callback rule, making
/// [`Lexer::lex`] return [`Error::CallbackRequired`] so the caller can fall
/// back to running rouge itself.
pub struct NoCallbacks;

impl Callback for NoCallbacks {
    fn invoke(
        &mut self,
        _ops: &mut EngineOps<'_, '_>,
        state: &str,
        rule_index: usize,
        _groups: &[Option<&str>],
    ) -> Result<(), Error> {
        Err(Error::CallbackRequired { state: state.to_string(), rule: rule_index })
    }
}

/// The DSL surface a [`Callback`] can drive — mirrors rouge's
/// `token` / `groups` / `push` / `pop!` / `goto`.
pub struct EngineOps<'lx, 't> {
    lexer: &'lx mut Lexer<'t>,
}

impl EngineOps<'_, '_> {
    /// Emit a token (rouge `token Tok, val`). Empty values are skipped,
    /// matching rouge's `yield_token`.
    pub fn token(&mut self, tok: TokenId, val: &str) {
        self.lexer.emit(tok, val);
    }

    /// Look up a token id by qualified name (`"Keyword"`,
    /// `"Literal.String"`). Returns `None` for names absent from the table.
    pub fn token_id(&self, qualname: &str) -> Option<TokenId> {
        self.lexer
            .table
            .token_names
            .iter()
            .position(|n| n == qualname)
            .map(|i| TokenId(i as u32))
    }

    /// rouge `push :state` / bare `push`.
    pub fn push(&mut self, state: Option<&str>) -> Result<(), Error> {
        match state {
            None => {
                let top = *self.lexer.stack.last().ok_or(Error::EmptyStack)?;
                self.lexer.stack.push(top);
            }
            Some(name) => {
                let id = self.lexer.state_id(name)?;
                self.lexer.stack.push(id);
            }
        }
        Ok(())
    }

    /// rouge `pop!`.
    pub fn pop(&mut self, n: usize) -> Result<(), Error> {
        for _ in 0..n {
            self.lexer.stack.pop().ok_or(Error::EmptyStack)?;
        }
        Ok(())
    }

    /// rouge `goto :state` — replace the stack top.
    pub fn goto(&mut self, state: &str) -> Result<(), Error> {
        let id = self.lexer.state_id(state)?;
        *self.lexer.stack.last_mut().ok_or(Error::EmptyStack)? = id;
        Ok(())
    }
}

/// rouge permits at most this many consecutive zero-width matches before
/// declaring the rule failed (`RegexLexer::MAX_NULL_SCANS`).
const MAX_NULL_SCANS: u32 = 5;

/// A lexer run over a [`LexerTable`]. Holds the state stack and the
/// consolidated token output; reusable across inputs.
pub struct Lexer<'t> {
    table: &'t LexerTable,
    stack: Vec<u32>,
    null_steps: u32,
    toks: Vec<(TokenId, String)>,
}

impl<'t> Lexer<'t> {
    pub fn new(table: &'t LexerTable) -> Self {
        Lexer { table, stack: Vec::new(), null_steps: 0, toks: Vec::new() }
    }

    fn state_id(&self, name: &str) -> Result<u32, Error> {
        self.table
            .state_names
            .iter()
            .position(|n| n == name)
            .map(|i| i as u32)
            .ok_or_else(|| Error::UnknownState(name.to_string()))
    }

    /// Emit with rouge's consolidation: consecutive same-token chunks
    /// merge; nil/empty values are skipped (`yield_token` + the merge
    /// loop in `Lexer#continue_lex`).
    fn emit(&mut self, tok: TokenId, val: &str) {
        if val.is_empty() {
            return;
        }
        if let Some((last_t, last_v)) = self.toks.last_mut()
            && *last_t == tok
        {
            last_v.push_str(val);
            return;
        }
        self.toks.push((tok, val.to_string()));
    }

    /// Try one state's rules at `pos` (rouge `RegexLexer#step`). Returns
    /// `Ok(Some(new_pos))` when a rule matched, `Ok(None)` otherwise.
    fn step(
        &mut self,
        state: u32,
        text: &str,
        pos: usize,
        cb: &mut dyn Callback,
    ) -> Result<Option<usize>, Error> {
        // Index-based loop: the rules borrow lives only across each probe
        // so the action arms can mutate self.
        let n_rules = self.table.states[state as usize].len();
        for ri in 0..n_rules {
            // mixin recursion first (no regex on those entries).
            if let Kind::Mixin(other) = self.table.states[state as usize][ri].kind {
                if let Some(np) = self.step(other, text, pos, cb)? {
                    return Ok(Some(np));
                }
                continue;
            }
            let rule: &Rule = &self.table.states[state as usize][ri];
            if rule.bol && !(pos == 0 || text.as_bytes()[pos - 1] == b'\n') {
                continue;
            }
            let re = rule.re.as_ref().expect("non-mixin rule has a regex");
            // Anchored-at-pos match with FULL-haystack context so `^`,
            // `\b` and lookbehind see the real surroundings (rouge scans
            // a StringScanner positioned mid-string).
            let caps = match re.captures_from_pos(text, pos) {
                Ok(Some(c)) => c,
                _ => continue,
            };
            let m0 = caps.get(0).expect("capture 0 always present");
            if m0.start() != pos {
                continue;
            }
            let size = m0.end() - pos;

            match &self.table.states[state as usize][ri].kind {
                Kind::Tok { tok, next } => {
                    let tok = *tok;
                    let next = next.clone();
                    self.emit(tok, m0.as_str());
                    self.apply_next(&next)?;
                }
                Kind::Wordlist { sets, default } => {
                    let word = m0.as_str();
                    let tok = sets
                        .iter()
                        .find(|(_, set)| set.contains_key(word))
                        .map(|(t, _)| *t)
                        .unwrap_or(*default);
                    self.emit(tok, word);
                }
                Kind::Actions(_) => {
                    // Re-borrow pattern: capture group strings first, then
                    // walk the actions by index so emits can mutate self.
                    let groups: Vec<Option<String>> = (1..caps.len())
                        .map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                        .collect();
                    let whole = m0.as_str().to_string();
                    let n_acts = match &self.table.states[state as usize][ri].kind {
                        Kind::Actions(a) => a.len(),
                        _ => unreachable!(),
                    };
                    for ai in 0..n_acts {
                        let act = match &self.table.states[state as usize][ri].kind {
                            Kind::Actions(a) => &a[ai],
                            _ => unreachable!(),
                        };
                        match act {
                            Action::Groups(toks) => {
                                let toks = toks.clone();
                                for (i, t) in toks.iter().enumerate() {
                                    if let Some(Some(v)) = groups.get(i) {
                                        let v = v.clone();
                                        self.emit(*t, &v);
                                    }
                                }
                            }
                            Action::Token(t) => {
                                let t = *t;
                                self.emit(t, &whole);
                            }
                            Action::Push(s) => {
                                let s = *s;
                                self.stack.push(s);
                            }
                            Action::PushSelf => {
                                let top = *self.stack.last().ok_or(Error::EmptyStack)?;
                                self.stack.push(top);
                            }
                            Action::Pop(n) => {
                                let n = *n;
                                for _ in 0..n {
                                    self.stack.pop().ok_or(Error::EmptyStack)?;
                                }
                            }
                            Action::Goto(s) => {
                                let s = *s;
                                *self.stack.last_mut().ok_or(Error::EmptyStack)? = s;
                            }
                        }
                    }
                }
                Kind::Callback => {
                    let groups: Vec<Option<&str>> =
                        (0..caps.len()).map(|i| caps.get(i).map(|m| m.as_str())).collect();
                    let state_name = self.table.state_names[state as usize].clone();
                    let mut ops = EngineOps { lexer: self };
                    cb.invoke(&mut ops, &state_name, ri, &groups)?;
                }
                Kind::Mixin(_) => unreachable!("handled above"),
            }

            if size == 0 {
                self.null_steps += 1;
                if self.null_steps > MAX_NULL_SCANS {
                    return Ok(None);
                }
            } else {
                self.null_steps = 0;
            }
            return Ok(Some(pos + size));
        }
        Ok(None)
    }

    fn apply_next(&mut self, next: &[NextState]) -> Result<(), Error> {
        for n in next {
            match n {
                NextState::Pop => {
                    self.stack.pop().ok_or(Error::EmptyStack)?;
                }
                NextState::PushSelf => {
                    let top = *self.stack.last().ok_or(Error::EmptyStack)?;
                    self.stack.push(top);
                }
                NextState::Push(s) => self.stack.push(*s),
            }
        }
        Ok(())
    }

    /// Lex `text` from a fresh `[:root]` stack, returning the consolidated
    /// `(token, value)` stream (rouge `Lexer#lex` semantics, including the
    /// `Error`-token-plus-one-char fallback when no rule matches).
    pub fn lex(
        &mut self,
        text: &str,
        cb: &mut dyn Callback,
    ) -> Result<Vec<(TokenId, String)>, Error> {
        self.stack.clear();
        self.stack.push(self.table.root);
        self.null_steps = 0;
        self.toks.clear();

        let mut pos = 0;
        while pos < text.len() {
            let top = *self.stack.last().ok_or(Error::EmptyStack)?;
            match self.step(top, text, pos, cb)? {
                Some(np) => pos = np,
                None => {
                    let ch_len =
                        text[pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    let err_tok = self.table.tok_error;
                    self.emit(err_tok, &text[pos..pos + ch_len]);
                    pos += ch_len;
                }
            }
        }
        Ok(std::mem::take(&mut self.toks))
    }
}
