//! The lexer engine — rouge `RegexLexer` semantics over a [`LexerTable`].
//!
//! Two driving styles share one core:
//!
//! - **One-shot**: [`Lexer::lex`] runs to completion, routing rules rouge
//!   defines with match-dependent blocks through a [`Callback`]
//!   implementation ([`NoCallbacks`] declines, surfacing
//!   [`Error::CallbackRequired`]).
//! - **Session** (for embedders bridging to a live Ruby rouge lexer):
//!   [`Lexer::begin`] + [`Lexer::run`] until [`RunStep::Done`]; when
//!   [`RunStep::Callback`] pauses the lex, execute the original Ruby block
//!   out-of-band, replay its DSL effects via [`Lexer::apply_callback_ops`],
//!   and call [`Lexer::run`] again.

use crate::Error;
use crate::table::{Action, Kind, LexerTable, NextState, Rule, TokenId};

/// Handler for rules rouge defines with match-dependent Ruby blocks
/// (`kind: "callback"` in the table), used by the one-shot [`Lexer::lex`].
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
        Err(Error::CallbackRequired {
            state: state.to_string(),
            rule: rule_index,
        })
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
        self.lexer.table.token_id(qualname)
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

/// One replayed DSL effect from an out-of-band callback execution, for
/// [`Lexer::apply_callback_ops`]. Token values are explicit strings (the
/// Ruby side captured them from the live match).
#[derive(Debug)]
pub enum CallbackOp {
    /// rouge `token Tok, val` / each `groups` element.
    Token { qualname: String, value: String },
    /// rouge `push :state`; `None` = bare `push` (re-push current).
    Push(Option<String>),
    /// rouge `pop! n`.
    Pop(usize),
    /// rouge `goto :state`.
    Goto(String),
}

/// What [`Lexer::run`] paused on.
#[derive(Debug)]
pub enum RunStep {
    /// End of input — collect with [`Lexer::take_tokens`].
    Done,
    /// A callback rule matched. Execute the original block out-of-band,
    /// then [`Lexer::apply_callback_ops`] and [`Lexer::run`] again.
    Callback {
        /// State name the rule lives in.
        state: String,
        /// Index of the rule within that state (mixin entries count).
        rule: usize,
        /// `groups[0]` is the whole match; `groups[i]` capture i.
        groups: Vec<Option<String>>,
    },
}

/// rouge permits at most this many consecutive zero-width matches before
/// declaring the rule failed (`RegexLexer::MAX_NULL_SCANS`).
const MAX_NULL_SCANS: u32 = 5;

/// A pending (paused) callback-rule match.
struct Pending {
    size: usize,
}

/// What `step` found at the current position.
enum StepHit {
    /// A non-callback rule ran; advance to this position.
    Advance(usize),
    /// A callback rule matched (recorded in `self.pending`).
    NeedCallback {
        state: u32,
        rule: usize,
        groups: Vec<Option<String>>,
    },
}

/// A lexer run over a [`LexerTable`]. Holds the state stack, position and
/// the consolidated token output; reusable across inputs.
pub struct Lexer<'t> {
    table: &'t LexerTable,
    stack: Vec<u32>,
    /// Native lexer instance state for IR rules (rouge rule procs
    /// read/write a tiny ivar vocabulary — see crate::ir).
    ivars: crate::ir::Ivars,
    pos: usize,
    null_steps: u32,
    pending: Option<Pending>,
    toks: Vec<(TokenId, String)>,
    /// Reusable group-span buffer for `CRegex::captures_at` —
    /// avoids a Vec allocation per rule probe (rules are probed
    /// in order until one matches, so this is the hottest alloc
    /// site in the step loop). Taken (`std::mem::take`) on a hit
    /// and dropped with the borrow; misses just reuse it.
    caps_buf: Vec<Option<(usize, usize)>>,
}

impl<'t> Lexer<'t> {
    pub fn new(table: &'t LexerTable) -> Self {
        Lexer {
            table,
            stack: Vec::new(),
            ivars: crate::ir::Ivars::new(),
            pos: 0,
            null_steps: 0,
            pending: None,
            toks: Vec::new(),
            caps_buf: Vec::new(),
        }
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
    /// Execute a compiled rule block (Conditional Action IR). `groups`
    /// is the full capture snapshot with `groups[0]` = whole match.
    fn run_ir_ops(
        &mut self,
        ops: &[crate::ir::IrOp],
        groups: &[Option<String>],
    ) -> Result<(), Error> {
        use crate::ir::{EvalVal, IrOp, IvarVal, eval_cond, eval_expr};
        for op in ops {
            match op {
                IrOp::Token { token, value } => match value {
                    None => {
                        if let Some(Some(w)) = groups.first() {
                            let w = w.clone();
                            self.emit(*token, &w);
                        }
                    }
                    Some(e) => {
                        // nil token value emits nothing (rouge's
                        // yield_token guard). Non-string values can't
                        // be produced for token positions by the
                        // compiler.
                        if let Some(EvalVal::Str(sv)) = eval_expr(e, groups) {
                            self.emit(*token, &sv);
                        }
                    }
                },
                IrOp::Groups(toks) => {
                    for (i, t) in toks.iter().enumerate() {
                        if let Some(Some(v)) = groups.get(i + 1) {
                            let v = v.clone();
                            self.emit(*t, &v);
                        }
                    }
                }
                IrOp::Push(Some(st)) => self.stack.push(*st),
                IrOp::Push(None) => {
                    let top = *self.stack.last().ok_or(Error::EmptyStack)?;
                    self.stack.push(top);
                }
                IrOp::Pop(n) => {
                    for _ in 0..*n {
                        self.stack.pop().ok_or(Error::EmptyStack)?;
                    }
                }
                IrOp::Goto(st) => {
                    *self.stack.last_mut().ok_or(Error::EmptyStack)? = *st;
                }
                IrOp::IvarSet(name, e) => {
                    let v = eval_expr(e, groups)
                        .map(EvalVal::into_ivar)
                        .unwrap_or(IvarVal::Nil);
                    self.ivars.insert(name.clone(), v);
                }
                IrOp::ListPush(name, exprs) => {
                    let mut tuple = Vec::with_capacity(exprs.len());
                    for e in exprs {
                        tuple.push(
                            eval_expr(e, groups)
                                .map(EvalVal::into_ivar)
                                .unwrap_or(IvarVal::Nil),
                        );
                    }
                    // An untouched ivar starts as an empty list — the
                    // observed rouge initializers (`start { @q = [] }`)
                    // are equivalent.
                    match self
                        .ivars
                        .entry(name.clone())
                        .or_insert_with(|| IvarVal::List(Vec::new()))
                    {
                        IvarVal::List(items) => items.push(tuple),
                        other => *other = IvarVal::List(vec![tuple]),
                    }
                }
                IrOp::If {
                    cond,
                    then_ops,
                    else_ops,
                } => {
                    let current = *self.stack.last().ok_or(Error::EmptyStack)?;
                    let branch = if eval_cond(cond, groups, &self.ivars, current) {
                        then_ops
                    } else {
                        else_ops
                    };
                    self.run_ir_ops(branch, groups)?;
                }
            }
        }
        Ok(())
    }

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

    /// Try one state's rules at `self.pos` (rouge `RegexLexer#step`).
    /// Non-callback rules execute fully; a callback rule records a
    /// `Pending` and surfaces `NeedCallback` without executing.
    fn step(&mut self, state: u32, text: &str) -> Result<Option<StepHit>, Error> {
        let pos = self.pos;
        let n_rules = self.table.states[state as usize].len();
        for ri in 0..n_rules {
            // mixin recursion first (no regex on those entries).
            if let Kind::Mixin(other) = self.table.states[state as usize][ri].kind {
                if let Some(hit) = self.step(other, text)? {
                    return Ok(Some(hit));
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
            // a StringScanner positioned mid-string). Spans buffer is
            // reused across rules (cleared inside captures_at).
            if !re.captures_at(text, pos, &mut self.caps_buf) {
                continue;
            }
            let spans = std::mem::take(&mut self.caps_buf);
            let (m0s, m0e) = spans[0].expect("capture 0 always present");
            debug_assert_eq!(m0s, pos);
            let size = m0e - pos;

            match &self.table.states[state as usize][ri].kind {
                Kind::Tok { tok, next } => {
                    let tok = *tok;
                    let next = next.clone();
                    self.emit(tok, &text[m0s..m0e]);
                    self.apply_next(&next)?;
                }
                Kind::Wordlist { sets, default } => {
                    let word = &text[m0s..m0e];
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
                    let groups: Vec<Option<String>> = spans[1..]
                        .iter()
                        .map(|s| s.map(|(a, b)| text[a..b].to_string()))
                        .collect();
                    let whole = text[m0s..m0e].to_string();
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
                Kind::Ir(_) => {
                    // `self.table` is an independent `&'t` borrow, so
                    // re-reading the ops through it leaves `self` free
                    // to mutate (emit / stack / ivars).
                    let table = self.table;
                    let Kind::Ir(ops) = &table.states[state as usize][ri].kind else {
                        unreachable!()
                    };
                    let groups: Vec<Option<String>> = spans
                        .iter()
                        .map(|s| s.map(|(a, b)| text[a..b].to_string()))
                        .collect();
                    self.run_ir_ops(ops, &groups)?;
                }
                Kind::Callback => {
                    let groups: Vec<Option<String>> = spans
                        .iter()
                        .map(|s| s.map(|(a, b)| text[a..b].to_string()))
                        .collect();
                    self.pending = Some(Pending { size });
                    self.caps_buf = spans;
                    return Ok(Some(StepHit::NeedCallback {
                        state,
                        rule: ri,
                        groups,
                    }));
                }
                Kind::Mixin(_) => unreachable!("handled above"),
            }

            // Give the span buffer back for the next probe — `take`
            // left an empty Vec behind, which would re-allocate on
            // every subsequent hit.
            self.caps_buf = spans;
            return Ok(Some(StepHit::Advance(self.bump_null_guard(size, pos)?)));
        }
        Ok(None)
    }

    /// rouge's null-scan accounting after a successful rule. Returns the
    /// new position, or `Err`-free `pos` sentinel handling is done by the
    /// caller via `Option` — here a guard overflow is reported as the
    /// SAME position with `null_steps` saturated; the caller treats the
    /// overflow as a failed step (Error-token fallback), matching rouge's
    /// `return false` after MAX_NULL_SCANS.
    fn bump_null_guard(&mut self, size: usize, pos: usize) -> Result<usize, Error> {
        if size == 0 {
            self.null_steps += 1;
        } else {
            self.null_steps = 0;
        }
        Ok(pos + size)
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

    /// Reset for a fresh input (session style). Pair with [`Lexer::run`].
    pub fn begin(&mut self) {
        self.stack.clear();
        self.stack.push(self.table.root);
        self.pos = 0;
        self.null_steps = 0;
        self.pending = None;
        self.toks.clear();
    }

    /// Drive the lex from the current position until end of input or a
    /// callback rule pauses it.
    pub fn run(&mut self, text: &str) -> Result<RunStep, Error> {
        if self.pending.is_some() {
            return Err(Error::Table(
                "run() called with a pending callback — apply_callback_ops first".into(),
            ));
        }
        while self.pos < text.len() {
            if self.null_steps > MAX_NULL_SCANS {
                // rouge: the over-limit step "fails" → Error + one char.
                self.null_steps = 0;
                self.error_getch(text);
                continue;
            }
            let top = *self.stack.last().ok_or(Error::EmptyStack)?;
            match self.step(top, text)? {
                Some(StepHit::Advance(np)) => self.pos = np,
                Some(StepHit::NeedCallback {
                    state,
                    rule,
                    groups,
                }) => {
                    return Ok(RunStep::Callback {
                        state: self.table.state_names[state as usize].clone(),
                        rule,
                        groups,
                    });
                }
                None => self.error_getch(text),
            }
        }
        Ok(RunStep::Done)
    }

    fn error_getch(&mut self, text: &str) {
        let ch_len = text[self.pos..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
        let err_tok = self.table.tok_error;
        let piece = text[self.pos..self.pos + ch_len].to_string();
        self.emit(err_tok, &piece);
        self.pos += ch_len;
    }

    /// Replay the DSL effects of an out-of-band callback execution and
    /// consume the pending match. Unknown token names / states error —
    /// the embedder should abort the session and fall back.
    pub fn apply_callback_ops(&mut self, ops: &[CallbackOp]) -> Result<(), Error> {
        let pending = self.pending.take().ok_or(Error::EmptyStack)?;
        for op in ops {
            match op {
                CallbackOp::Token { qualname, value } => {
                    let tok = self
                        .table
                        .token_id(qualname)
                        .ok_or_else(|| Error::Table(format!("unknown token {qualname:?}")))?;
                    self.emit(tok, value);
                }
                CallbackOp::Push(None) => {
                    let top = *self.stack.last().ok_or(Error::EmptyStack)?;
                    self.stack.push(top);
                }
                CallbackOp::Push(Some(name)) => {
                    let id = self.state_id(name)?;
                    self.stack.push(id);
                }
                CallbackOp::Pop(n) => {
                    for _ in 0..*n {
                        self.stack.pop().ok_or(Error::EmptyStack)?;
                    }
                }
                CallbackOp::Goto(name) => {
                    let id = self.state_id(name)?;
                    *self.stack.last_mut().ok_or(Error::EmptyStack)? = id;
                }
            }
        }
        let pos = self.pos;
        self.pos = self.bump_null_guard(pending.size, pos)?;
        Ok(())
    }

    /// Take the consolidated tokens accumulated since [`Lexer::begin`].
    pub fn take_tokens(&mut self) -> Vec<(TokenId, String)> {
        std::mem::take(&mut self.toks)
    }

    /// Lex `text` from a fresh `[:root]` stack, returning the consolidated
    /// `(token, value)` stream (rouge `Lexer#lex` semantics, including the
    /// `Error`-token-plus-one-char fallback when no rule matches).
    /// Callback rules are routed through `cb` immediately.
    pub fn lex(
        &mut self,
        text: &str,
        cb: &mut dyn Callback,
    ) -> Result<Vec<(TokenId, String)>, Error> {
        self.begin();
        loop {
            match self.run(text)? {
                RunStep::Done => return Ok(self.take_tokens()),
                RunStep::Callback {
                    state,
                    rule,
                    groups,
                } => {
                    let group_refs: Vec<Option<&str>> =
                        groups.iter().map(|g| g.as_deref()).collect();
                    {
                        let mut ops = EngineOps { lexer: self };
                        cb.invoke(&mut ops, &state, rule, &group_refs)?;
                    }
                    // The trait mutated us directly; consume the pending
                    // match (advance + null guard) with no extra ops.
                    self.apply_callback_ops(&[])?;
                }
            }
        }
    }
}
