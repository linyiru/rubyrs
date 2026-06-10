//! `_rouge_native` — carmine-backed accelerator for the rouge gem.
//!
//! ADR 0019 Rule 6 partition (same shape as `_json_native`): the pure-Ruby
//! rouge gem stays the spec; this battery is the behaviour-equivalent fast
//! path. After `require "rouge"` completes, a Ruby shim
//! (`rouge_native_shim.rb`, injected by the require hook in `vm/kernel.rs`)
//! extracts each lexer's rule table at first use — running the same
//! recording-StateDSL logic as carmine's `tools/extract.rb` against the
//! LIVE lexer class — and hands the JSON to the host fns below. Lexing +
//! HTML formatting for supported lexers then run natively in carmine,
//! byte-identical to rouge; anything unsupported declines and the shim
//! falls back to the pure-Ruby gem (per lexer at table build, per call
//! when a match-dependent `callback` rule fires mid-lex).
//!
//! Two host fns:
//!   - `__rubyrs_rouge_native_table(json) → Integer | nil`
//!     compile + cache a carmine `LexerTable`; nil = decline (the shim
//!     caches the decline so the lexer stays pure-Ruby).
//!   - `__rubyrs_rouge_native_lex_html(id, source) → String | nil`
//!     lex + rouge-HTML-format in one shot; nil = a callback rule
//!     matched (the shim re-runs that call through pure rouge).

#![cfg(feature = "_rouge_native")]

use std::cell::RefCell;

use crate::error::{RubyError, Trap};
use crate::value::Value;

/// The Ruby shim injected after `require "rouge"` (see the hook in
/// `vm/kernel.rs::require_ruby`).
pub(crate) const SHIM: &str = include_str!("rouge_native_shim.rb");

thread_local! {
    /// Compiled lexer tables, indexed by the id handed back to Ruby.
    /// Thread-local because Vm itself is single-threaded; tables are
    /// engine-only state with no GC interaction.
    static TABLES: RefCell<Vec<carmine::LexerTable>> = const { RefCell::new(Vec::new()) };
}

fn arg_err(msg: &str) -> Trap {
    Trap { err: RubyError::ArgumentError { msg: msg.to_string() }, backtrace: vec![] }
}

/// Register the `__rubyrs_rouge_native_*` host fns on `rt`. Idempotent.
/// The shim detects registration via `defined?(...)` and stays inert
/// when absent.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_rouge_native_table", |args| {
        let json = match args {
            [Value::Str(s)] => s.to_string_lossy(),
            _ => return Err(arg_err("__rubyrs_rouge_native_table(json_string)")),
        };
        match carmine::LexerTable::from_json(&json) {
            Ok(table) => {
                // Decline tables that can emit `Escape`: rouge's
                // formatter pipeline gives that token special treatment
                // (filter_escapes / raw passthrough) that depends on
                // formatter options. Declining keeps `filter_escapes`
                // an identity for every accepted table, which is what
                // makes the shim's Formatter#format bypass safe.
                if table.token_names().any(|n| n == "Escape") {
                    return Ok(Value::Nil);
                }
                let id = TABLES.with(|t| {
                    let mut t = t.borrow_mut();
                    t.push(table);
                    t.len() - 1
                });
                Ok(Value::Int(id as i64))
            }
            // Any load/compile failure (unsupported regex syntax, table
            // shape) is a per-lexer DECLINE, not an error: the shim
            // caches `false` and the lexer stays pure-Ruby.
            Err(_) => Ok(Value::Nil),
        }
    });

    rt.register_fn("__rubyrs_rouge_native_lex_html", |args| {
        let (id, source) = match args {
            [Value::Int(id), Value::Str(s)] => (*id, s.to_string_lossy()),
            _ => return Err(arg_err("__rubyrs_rouge_native_lex_html(id, source)")),
        };
        TABLES.with(|t| {
            let tables = t.borrow();
            let Some(table) = usize::try_from(id).ok().and_then(|i| tables.get(i)) else {
                return Err(arg_err("rouge_native: bad table id"));
            };
            let mut lexer = carmine::Lexer::new(table);
            match lexer.lex(&source, &mut carmine::NoCallbacks) {
                Ok(toks) => Ok(Value::new_str(carmine::html::format(table, &toks))),
                // A match-dependent callback rule fired — this CALL falls
                // back to pure rouge (the table stays valid for inputs
                // that don't hit the rule).
                Err(carmine::Error::CallbackRequired { .. }) => Ok(Value::Nil),
                // Anything else (broken table at runtime) also declines
                // rather than aborting the build.
                Err(_) => Ok(Value::Nil),
            }
        })
    });
}
