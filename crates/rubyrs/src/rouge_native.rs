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
//! Host fns:
//!   - `__rubyrs_rouge_native_table(json) → Integer | nil`
//!     compile + cache a carmine `LexerTable`; nil = decline (the shim
//!     caches the decline so the lexer stays pure-Ruby).
//!   - `__rubyrs_rouge_native_lex_html(id, source) → String | nil`
//!     one-shot lex + rouge-HTML formatting for inputs that hit no
//!     callback rule; nil = a callback rule matched (the shim then runs
//!     the SESSION protocol, or falls back to pure rouge).
//!   - Session protocol (the v2 VM-callback bridge — the engine pauses on
//!     a callback rule, Ruby executes the ORIGINAL block and replays its
//!     DSL effects, the engine resumes). All session traffic is strings,
//!     so the host fns never need heap allocation on the VM side:
//!     `__rubyrs_rouge_native_lex_start(id, source) → Integer`;
//!     `__rubyrs_rouge_native_lex_run(sid) → String` — JSON
//!     `{"t":"done","html":…}` / `{"t":"cb","s":state,"i":rule,
//!     "g":[whole, group1|null, …]}` / `{"t":"err"}`;
//!     `__rubyrs_rouge_native_lex_apply(sid, ops_json) → true | nil` with
//!     ops `[["t",qualname,value],["push",state|null],["pop",n],
//!     ["goto",state]]`;
//!     `__rubyrs_rouge_native_lex_abort(sid) → nil` (free the session).

#![cfg(feature = "_rouge_native")]

use std::cell::RefCell;

use crate::error::{RubyError, Trap};
use crate::value::Value;

/// The Ruby shim injected after `require "rouge"` (see the hook in
/// `vm/kernel.rs::require_ruby`).
pub(crate) const SHIM: &str = include_str!("rouge_native_shim.rb");

/// A paused/running native lex session.
struct Session {
    lexer: carmine::Lexer<'static>,
    table: &'static carmine::LexerTable,
    text: String,
    /// The pending callback's match groups (`groups[0]` = whole match),
    /// served lazily to the Ruby block via `__rubyrs_rouge_native_group`.
    groups: Vec<Option<String>>,
    /// DSL effects streamed in by the `op_*` host fns, applied on
    /// `lex_resume`.
    ops: Vec<carmine::CallbackOp>,
}

thread_local! {
    /// Compiled lexer tables, indexed by the id handed back to Ruby.
    /// `Box::leak`'d so sessions can borrow them as `'static` — tables
    /// are per-lexer-class, bounded, and live for the process anyway.
    /// Thread-local because Vm itself is single-threaded; tables are
    /// engine-only state with no GC interaction.
    static TABLES: RefCell<Vec<&'static carmine::LexerTable>> = const { RefCell::new(Vec::new()) };
    /// Active sessions (slot reuse via the free list).
    static SESSIONS: RefCell<(Vec<Option<Session>>, Vec<usize>)> =
        const { RefCell::new((Vec::new(), Vec::new())) };
    /// Lazily-compiled STATIC tables (`rouge_tables/*.json`, extracted
    /// at development time by `tools/dump_rouge_static_tables.rb`).
    /// `None` in a slot = compile declined, cached so we don't retry.
    static STATIC_TABLES: RefCell<[Option<Option<&'static carmine::LexerTable>>; 3]> =
        const { RefCell::new([None, None, None]) };
}

/// Pre-extracted tables for the languages the jekyll workload rotates.
/// Index must match the `STATIC_TABLES` slot layout. The kramdown
/// shim's version gate (`STATIC_HL_ROUGE_VERSION`) pins these to the
/// rouge release they were extracted from.
const STATIC_TABLE_SRC: [(&str, &str); 3] = [
    ("python", include_str!("rouge_tables/python.json")),
    ("ruby", include_str!("rouge_tables/ruby.json")),
    ("bash", include_str!("rouge_tables/bash.json")),
];

thread_local! {
    /// Lexer-file gate for lazy rouge loading: while raised,
    /// `Kernel::load` of `…/rouge/lexers/*.rb` is skipped (rouge.rb's
    /// eager `load_lexers` walk becomes a no-op) and the shim loads
    /// lexer files on demand, lowering the gate around each real load.
    /// Raised ONLY by the kramdown shim after its rouge-version check.
    static LEXER_GATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Queried by the `Kernel::load` builtin (vm/kernel.rs).
pub(crate) fn lexer_gate_active() -> bool {
    LEXER_GATE.with(std::cell::Cell::get)
}

fn static_table_for(lang: &str) -> Option<&'static carmine::LexerTable> {
    let idx = STATIC_TABLE_SRC
        .iter()
        .position(|(name, _)| *name == lang)?;
    STATIC_TABLES.with(|t| {
        let mut slots = t.borrow_mut();
        if slots[idx].is_none() {
            let compiled = carmine::LexerTable::from_json(STATIC_TABLE_SRC[idx].1)
                .ok()
                .filter(|table| !table.rule_emits("Escape"))
                .map(|table| &*Box::leak(Box::new(table)));
            slots[idx] = Some(compiled);
        }
        slots[idx].unwrap_or(None)
    })
}

fn arg_err(msg: &str) -> Trap {
    Trap {
        err: RubyError::ArgumentError {
            msg: msg.to_string(),
        },
        backtrace: vec![],
    }
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
                if table.rule_emits("Escape") {
                    return Ok(Value::Nil);
                }
                let table: &'static carmine::LexerTable = Box::leak(Box::new(table));
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
        let Some(table) = table_for(id) else {
            return Err(arg_err("rouge_native: bad table id"));
        };
        let mut lexer = carmine::Lexer::new(table);
        match lexer.lex(&source, &mut carmine::NoCallbacks) {
            Ok(toks) => Ok(Value::new_str(carmine::html::format(table, &toks))),
            // A match-dependent callback rule fired — the shim escalates
            // this CALL to the session protocol (or pure rouge).
            Err(carmine::Error::CallbackRequired { .. }) => Ok(Value::Nil),
            // Anything else (broken table at runtime) also declines
            // rather than aborting the build.
            Err(_) => Ok(Value::Nil),
        }
    });

    rt.register_fn("__rubyrs_rouge_native_lex_start", |args| {
        let (id, source) = match args {
            [Value::Int(id), Value::Str(s)] => (*id, s.to_string_lossy()),
            _ => return Err(arg_err("__rubyrs_rouge_native_lex_start(id, source)")),
        };
        let Some(table) = table_for(id) else {
            return Err(arg_err("rouge_native: bad table id"));
        };
        let mut lexer = carmine::Lexer::new(table);
        lexer.begin();
        let session = Session {
            lexer,
            table,
            text: source,
            groups: Vec::new(),
            ops: Vec::new(),
        };
        let sid = SESSIONS.with(|s| {
            let (slots, free) = &mut *s.borrow_mut();
            match free.pop() {
                Some(i) => {
                    slots[i] = Some(session);
                    i
                }
                None => {
                    slots.push(Some(session));
                    slots.len() - 1
                }
            }
        });
        Ok(Value::Int(sid as i64))
    });

    // Session protocol — JSON-free for per-callback speed: `lex_run`
    // returns a tiny status string; the shim pulls match groups lazily
    // (`group`), streams DSL effects through the `op_*` fns into a
    // host-side buffer, and `lex_resume` applies them + continues. Any
    // protocol misuse frees the session and returns nil/"E".
    rt.register_fn("__rubyrs_rouge_native_lex_run", |args| {
        let sid = match args {
            [Value::Int(sid)] => *sid,
            _ => return Err(arg_err("__rubyrs_rouge_native_lex_run(sid)")),
        };
        let reply = with_session(sid, |session| match session.lexer.run(&session.text) {
            Ok(carmine::RunStep::Done) => {
                let toks = session.lexer.take_tokens();
                let html = carmine::html::format(session.table, &toks);
                SessionReply::Done(html)
            }
            Ok(carmine::RunStep::Callback {
                state,
                rule,
                groups,
            }) => {
                session.groups = groups;
                SessionReply::Callback { state, rule }
            }
            Err(_) => SessionReply::Errored,
        });
        Ok(Value::new_str(reply))
    });

    rt.register_fn("__rubyrs_rouge_native_group", |args| {
        let (sid, i) = match args {
            [Value::Int(sid), Value::Int(i)] => (*sid, *i),
            _ => return Err(arg_err("__rubyrs_rouge_native_group(sid, i)")),
        };
        let v = SESSIONS.with(|s| {
            let (slots, _) = &mut *s.borrow_mut();
            let idx = usize::try_from(sid).ok()?;
            let session = slots.get_mut(idx)?.as_mut()?;
            usize::try_from(i)
                .ok()
                .and_then(|gi| session.groups.get(gi))
                .cloned()
        });
        Ok(match v {
            Some(Some(g)) => Value::new_str(g),
            _ => Value::Nil,
        })
    });

    rt.register_fn("__rubyrs_rouge_native_op_token", |args| {
        let (sid, qual, val) = match args {
            [Value::Int(sid), Value::Str(q), Value::Str(v)] => {
                (*sid, q.to_string_lossy(), v.to_string_lossy())
            }
            _ => return Err(arg_err("__rubyrs_rouge_native_op_token(sid, qual, val)")),
        };
        push_op(
            sid,
            carmine::CallbackOp::Token {
                qualname: qual,
                value: val,
            },
        );
        Ok(Value::Nil)
    });

    rt.register_fn("__rubyrs_rouge_native_op_push", |args| {
        let (sid, st) = match args {
            [Value::Int(sid), Value::Str(s)] => (*sid, Some(s.to_string_lossy())),
            [Value::Int(sid), Value::Nil] => (*sid, None),
            _ => return Err(arg_err("__rubyrs_rouge_native_op_push(sid, state|nil)")),
        };
        push_op(sid, carmine::CallbackOp::Push(st));
        Ok(Value::Nil)
    });

    rt.register_fn("__rubyrs_rouge_native_op_pop", |args| {
        let (sid, n) = match args {
            [Value::Int(sid), Value::Int(n)] => (*sid, (*n).max(0) as usize),
            _ => return Err(arg_err("__rubyrs_rouge_native_op_pop(sid, n)")),
        };
        push_op(sid, carmine::CallbackOp::Pop(n));
        Ok(Value::Nil)
    });

    rt.register_fn("__rubyrs_rouge_native_op_goto", |args| {
        let (sid, st) = match args {
            [Value::Int(sid), Value::Str(s)] => (*sid, s.to_string_lossy()),
            _ => return Err(arg_err("__rubyrs_rouge_native_op_goto(sid, state)")),
        };
        push_op(sid, carmine::CallbackOp::Goto(st));
        Ok(Value::Nil)
    });

    rt.register_fn("__rubyrs_rouge_native_lex_resume", |args| {
        let sid = match args {
            [Value::Int(sid)] => *sid,
            _ => return Err(arg_err("__rubyrs_rouge_native_lex_resume(sid)")),
        };
        let ok = SESSIONS.with(|s| {
            let (slots, free) = &mut *s.borrow_mut();
            let Ok(idx) = usize::try_from(sid) else {
                return false;
            };
            let Some(slot) = slots.get_mut(idx) else {
                return false;
            };
            let Some(session) = slot.as_mut() else {
                return false;
            };
            let ops = std::mem::take(&mut session.ops);
            match session.lexer.apply_callback_ops(&ops) {
                Ok(()) => true,
                Err(_) => {
                    *slot = None;
                    free.push(idx);
                    false
                }
            }
        });
        Ok(if ok { Value::Bool(true) } else { Value::Nil })
    });

    rt.register_fn("__rubyrs_rouge_native_lex_abort", |args| {
        if let [Value::Int(sid)] = args {
            free_session(*sid);
        }
        Ok(Value::Nil)
    });

    // Static fast path for the kramdown accelerator: highlight a fenced
    // block from a PRE-EXTRACTED table without rouge being loaded at
    // all. Returns the COMPLETE block HTML (the HTMLLegacy/HTMLPygments
    // wrapper `<div class="highlight"><pre class="highlight"><code>…`
    // is fixed for Jekyll's whitelisted options, so the host emits it
    // directly). nil = no table for the language, the table declined,
    // or a callback rule fired — the shim then requires rouge lazily
    // and takes the dynamic path. The caller (kramdown shim) gates this
    // behind a rouge-version check, so a site with a different rouge
    // never sees a stale table.
    // Raise/lower the lazy-lexer gate (see LEXER_GATE). The shim
    // lowers it around demand loads and for the load-everything
    // fallback; anything unexpected leaves it in the safe state the
    // caller set.
    rt.register_fn("__rubyrs_rouge_native_lexer_gate", |args| {
        let on = match args {
            [Value::Bool(b)] => *b,
            _ => return Err(arg_err("__rubyrs_rouge_native_lexer_gate(bool)")),
        };
        LEXER_GATE.with(|g| g.set(on));
        Ok(Value::Nil)
    });

    rt.register_fn("__rubyrs_rouge_native_static_lex", |args| {
        let (lang, source) = match args {
            [Value::Str(l), Value::Str(s)] => (l.to_string_lossy(), s.to_string_lossy()),
            _ => return Err(arg_err("__rubyrs_rouge_native_static_lex(lang, source)")),
        };
        let Some(table) = static_table_for(&lang) else {
            return Ok(Value::Nil);
        };
        let mut lexer = carmine::Lexer::new(table);
        match lexer.lex(&source, &mut carmine::NoCallbacks) {
            Ok(toks) => {
                let inner = carmine::html::format(table, &toks);
                let mut out = String::with_capacity(inner.len() + 64);
                out.push_str("<div class=\"highlight\"><pre class=\"highlight\"><code>");
                out.push_str(&inner);
                out.push_str("</code></pre></div>");
                Ok(Value::new_str(out))
            }
            // Callback rule (or any engine surprise): decline — the
            // shim escalates to the rouge-backed dynamic path.
            Err(_) => Ok(Value::Nil),
        }
    });
}

fn push_op(sid: i64, op: carmine::CallbackOp) {
    SESSIONS.with(|s| {
        let (slots, _) = &mut *s.borrow_mut();
        if let Ok(idx) = usize::try_from(sid)
            && let Some(Some(session)) = slots.get_mut(idx)
        {
            session.ops.push(op);
        }
    });
}

fn table_for(id: i64) -> Option<&'static carmine::LexerTable> {
    TABLES.with(|t| {
        usize::try_from(id)
            .ok()
            .and_then(|i| t.borrow().get(i).copied())
    })
}

/// Reply states for `lex_run`, encoded as a tagged string the shim
/// dispatches on the first byte: `"D" + html` (session freed) /
/// `"C{rule}:{state}"` (paused; groups served via `group`) / `"E"`
/// (errored, session freed).
enum SessionReply {
    Done(String),
    Callback { state: String, rule: usize },
    Errored,
}

fn with_session(sid: i64, f: impl FnOnce(&mut Session) -> SessionReply) -> String {
    let Ok(idx) = usize::try_from(sid) else {
        return "E".to_string();
    };
    // Take the session OUT of the slot for the duration of `f` so a
    // re-entrant host call can't alias it; restore unless finished.
    let taken = SESSIONS.with(|s| {
        let (slots, _) = &mut *s.borrow_mut();
        slots.get_mut(idx).and_then(Option::take)
    });
    let Some(mut session) = taken else {
        return "E".to_string();
    };
    let reply = f(&mut session);
    let (restore, out) = match reply {
        SessionReply::Done(html) => (false, format!("D{html}")),
        SessionReply::Callback { state, rule } => (true, format!("C{rule}:{state}")),
        SessionReply::Errored => (false, "E".to_string()),
    };
    SESSIONS.with(|s| {
        let (slots, free) = &mut *s.borrow_mut();
        if restore {
            slots[idx] = Some(session);
        } else {
            free.push(idx);
        }
    });
    out
}

fn free_session(sid: i64) {
    if let Ok(idx) = usize::try_from(sid) {
        SESSIONS.with(|s| {
            let (slots, free) = &mut *s.borrow_mut();
            if let Some(slot) = slots.get_mut(idx)
                && slot.take().is_some()
            {
                free.push(idx);
            }
        });
    }
}
