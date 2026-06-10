//! carmine — a [rouge](https://github.com/rouge-ruby/rouge)-compatible
//! syntax-highlighting engine.
//!
//! carmine executes **rule tables extracted from rouge's lexers** with the
//! same state-machine semantics as rouge's `RegexLexer` (ordered rules,
//! `mixin` recursion, `^`-rule beginning-of-line pre-check, anchored-at-
//! position matching, null-scan guard, `Error`-token fallback, consecutive
//! same-token consolidation) and formats tokens with rouge's HTML span /
//! escape rules — producing byte-identical output for the supported rule
//! kinds.
//!
//! ```no_run
//! use carmine::{LexerTable, Lexer, NoCallbacks, html};
//!
//! let json = std::fs::read_to_string("python.json").unwrap();
//! let table = LexerTable::from_json(&json).unwrap();
//! let mut lexer = Lexer::new(&table);
//! let tokens = lexer.lex("def f(x):\n    return x\n", &mut NoCallbacks).unwrap();
//! let html = html::format(&table, &tokens);
//! ```
//!
//! Rule tables are produced by `tools/extract.rb` (shipped in the source
//! repository), which loads rouge and records each lexer's state
//! definitions through a tracing DSL. Tables derived from rouge are subject
//! to rouge's MIT license (© Jeanine Adkisson and contributors).

mod engine;
pub(crate) mod ir;
mod table;

pub mod html;

pub use engine::{Callback, CallbackOp, EngineOps, Lexer, NoCallbacks, RunStep};
pub use table::{LexerTable, TokenId};

/// Errors surfaced by table loading and lexing.
#[derive(Debug)]
pub enum Error {
    /// The rule-table JSON was malformed or missing required fields.
    Table(String),
    /// A regex in the table failed to compile.
    Regex { pattern: String, message: String },
    /// A rule of kind `callback` matched, and the active [`Callback`]
    /// implementation declined to handle it ([`NoCallbacks`] always
    /// declines). Callers typically fall back to running rouge itself.
    CallbackRequired {
        /// State the rule lives in.
        state: String,
        /// Index of the rule within the state.
        rule: usize,
    },
    /// The state stack was popped while empty (mirrors rouge's
    /// `'empty stack!'` error — indicates a broken rule table).
    EmptyStack,
    /// A rule referenced a state that does not exist in the table.
    UnknownState(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Table(m) => write!(f, "malformed rule table: {m}"),
            Error::Regex { pattern, message } => {
                write!(f, "regex compile failed: /{pattern}/: {message}")
            }
            Error::CallbackRequired { state, rule } => {
                write!(f, "rule {rule} in state {state:?} requires a callback")
            }
            Error::EmptyStack => write!(f, "empty stack!"),
            Error::UnknownState(s) => write!(f, "unknown state: {s:?}"),
        }
    }
}

impl std::error::Error for Error {}
