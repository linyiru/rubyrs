//! `_kramdown_native` — rostdown-backed accelerator for the kramdown gem
//! under Jekyll.
//!
//! ADR 0019 Rule 6 partition (same shape as `_json_native` /
//! `_rouge_native`): pure-Ruby kramdown stays the spec; this battery is
//! the behaviour-equivalent fast path. After `require
//! "kramdown-parser-gfm"` completes (the moment Jekyll's KramdownParser
//! loads its GFM dependency — Kramdown::JekyllDocument is defined by
//! then), a Ruby shim (`kramdown_native_shim.rb`, injected by the
//! require hook in `vm/kernel.rs`) patches
//! `Kramdown::JekyllDocument#initialize/#to_html`. Documents whose
//! options match Jekyll's defaults render through rostdown —
//! byte-identical to kramdown+GFM — and anything outside rostdown's
//! subset declines per document, falling back to the pure-Ruby parse.
//!
//! Code blocks keep byte-identity BY CONSTRUCTION: the host pauses
//! after a scan pass and the shim highlights each block through the
//! same `Rouge`-formatter path the kramdown rouge plugin uses (which
//! `_rouge_native` accelerates when active); the host then splices the
//! supplied HTML into kramdown's wrapper markup.
//!
//! Host fns (all traffic is strings/ints — no VM heap interaction):
//!   - `__rubyrs_kd_scan(src) → Integer | nil` — parse with a recording
//!     highlighter; nil = rostdown declined (shim falls back). On
//!     success returns a session id; the session holds the source and
//!     the ordered `(lang, code)` list of fenced blocks.
//!   - `__rubyrs_kd_count(sid) → Integer`, `__rubyrs_kd_lang(sid, i) →
//!     String`, `__rubyrs_kd_code(sid, i) → String` — enumerate blocks.
//!   - `__rubyrs_kd_supply(sid, i, html | nil)` — highlighted HTML for
//!     block i (nil = highlighter declined → plain `<pre><code>` path).
//!   - `__rubyrs_kd_render(sid) → String | nil` — second pass replaying
//!     the supplied HTML in document order; frees the session. nil
//!     means an internal inconsistency — the shim falls back to Ruby.
//!   - `__rubyrs_kd_abort(sid) → nil` — free without rendering.

#![cfg(feature = "_kramdown_native")]

use std::cell::RefCell;

use crate::error::{RubyError, Trap};
use crate::value::Value;
use rostdown::{CodeHighlighter, Options};

/// The Ruby shim injected after `require "kramdown-parser-gfm"` (see
/// the hook in `vm/kernel.rs::require_ruby`).
pub(crate) const SHIM: &str = include_str!("kramdown_native_shim.rb");

/// Framework flavor — controls the code-span class and the default
/// fence language, the two places Jekyll's and Bridgetown's kramdown
/// configs render code differently (Bridgetown omits `default_lang`, so
/// its code-span class is `highlighter-rouge`, not Jekyll's
/// `language-plaintext highlighter-rouge`).
#[derive(Clone, Copy, PartialEq)]
enum Flavor {
    Jekyll,
    Bridgetown,
}

impl Flavor {
    fn from_arg(s: &str) -> Self {
        if s == "bridgetown" { Flavor::Bridgetown } else { Flavor::Jekyll }
    }
    fn codespan_class(self) -> &'static str {
        match self {
            Flavor::Jekyll => "language-plaintext highlighter-rouge",
            Flavor::Bridgetown => "highlighter-rouge",
        }
    }
    /// Default fence language for a no-info fence. Jekyll sets
    /// `default_lang: plaintext`; Bridgetown sets none (its no-lang
    /// fences are declined upstream in the shim, so `None` here just
    /// keeps them off the native highlight path).
    fn default_lang(self) -> Option<&'static str> {
        match self {
            Flavor::Jekyll => Some("plaintext"),
            Flavor::Bridgetown => None,
        }
    }
}

/// A document between the scan and render passes.
struct KdSession {
    src: String,
    flavor: Flavor,
    /// `(lang, code)` per fenced block, in document order.
    blocks: Vec<(String, String)>,
    /// Highlighted HTML per block (inner `None` = plain path).
    supplied: Vec<Option<String>>,
}

thread_local! {
    /// Active sessions (slot reuse via the free list). Thread-local
    /// because Vm is single-threaded; engine-only state, no GC
    /// interaction.
    static SESSIONS: RefCell<(Vec<Option<KdSession>>, Vec<usize>)> =
        const { RefCell::new((Vec::new(), Vec::new())) };
}

fn arg_err(msg: &str) -> Trap {
    Trap {
        err: RubyError::ArgumentError {
            msg: msg.to_string(),
        },
        backtrace: vec![],
    }
}

/// Pass-1 highlighter: records every block, emits a placeholder so the
/// conversion proceeds (the pass-1 HTML is discarded).
struct Recorder {
    blocks: Vec<(String, String)>,
    flavor: Flavor,
}

impl CodeHighlighter for Recorder {
    fn highlight(&mut self, lang: &str, code: &str) -> Option<String> {
        self.blocks.push((lang.to_string(), code.to_string()));
        Some(String::new())
    }
    fn codespan_class(&self) -> Option<&str> {
        Some(self.flavor.codespan_class())
    }
    fn default_lang(&self) -> Option<&str> {
        self.flavor.default_lang()
    }
}

/// Pass-2 highlighter: replays the shim-supplied HTML in document
/// order. `highlight()` call order is deterministic, so a plain cursor
/// pairs pass-2 calls with pass-1 blocks.
struct Supplied<'a> {
    htmls: &'a [Option<String>],
    cursor: usize,
    flavor: Flavor,
    /// Set when pass-2 calls don't line up with pass-1 blocks — the
    /// render is then abandoned (shim falls back to Ruby).
    desynced: bool,
}

impl CodeHighlighter for Supplied<'_> {
    fn highlight(&mut self, _lang: &str, _code: &str) -> Option<String> {
        match self.htmls.get(self.cursor) {
            Some(html) => {
                self.cursor += 1;
                html.clone()
            }
            None => {
                self.desynced = true;
                None
            }
        }
    }
    fn codespan_class(&self) -> Option<&str> {
        Some(self.flavor.codespan_class())
    }
    fn default_lang(&self) -> Option<&str> {
        self.flavor.default_lang()
    }
}

fn with_session<R>(sid: i64, f: impl FnOnce(&mut KdSession) -> R) -> Option<R> {
    SESSIONS.with(|s| {
        let (slots, _) = &mut *s.borrow_mut();
        slots.get_mut(sid as usize).and_then(Option::as_mut).map(f)
    })
}

fn free_session(sid: i64) -> Option<KdSession> {
    SESSIONS.with(|s| {
        let (slots, free) = &mut *s.borrow_mut();
        let taken = slots.get_mut(sid as usize).and_then(Option::take);
        if taken.is_some() {
            free.push(sid as usize);
        }
        taken
    })
}

/// Register the `__rubyrs_kd_*` host fns on `rt`. Idempotent. The shim
/// detects registration via `defined?(...)` and stays inert when
/// absent.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_kd_scan", |args| {
        let (src, flavor) = match args {
            // `(src)` — legacy single-arg form defaults to Jekyll.
            [Value::Str(s)] => (s.to_string_lossy(), Flavor::Jekyll),
            // `(src, "jekyll" | "bridgetown")`.
            [Value::Str(s), Value::Str(f)] => {
                (s.to_string_lossy(), Flavor::from_arg(&f.to_string_lossy()))
            }
            _ => return Err(arg_err("__rubyrs_kd_scan(src[, flavor])")),
        };
        let mut recorder = Recorder { blocks: Vec::new(), flavor };
        // GFM markdown parsing is flavor-independent (gfm + auto_ids); the
        // flavor only changes code-span/fence emission via the highlighter
        // trait. The shim has verified the document options match.
        if rostdown::to_html(&src, &Options::jekyll(), &mut recorder).is_err() {
            return Ok(Value::Nil);
        }
        let n = recorder.blocks.len();
        let session = KdSession {
            src: src.to_string(),
            flavor,
            blocks: recorder.blocks,
            supplied: vec![None; n],
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

    rt.register_fn("__rubyrs_kd_count", |args| {
        let sid = match args {
            [Value::Int(sid)] => *sid,
            _ => return Err(arg_err("__rubyrs_kd_count(sid)")),
        };
        match with_session(sid, |s| s.blocks.len()) {
            Some(n) => Ok(Value::Int(n as i64)),
            None => Err(arg_err("kd_native: bad session id")),
        }
    });

    rt.register_fn("__rubyrs_kd_lang", |args| {
        let (sid, i) = match args {
            [Value::Int(sid), Value::Int(i)] => (*sid, *i as usize),
            _ => return Err(arg_err("__rubyrs_kd_lang(sid, i)")),
        };
        match with_session(sid, |s| s.blocks.get(i).map(|(lang, _)| lang.clone())) {
            Some(Some(lang)) => Ok(Value::new_str(lang)),
            _ => Err(arg_err("kd_native: bad block index")),
        }
    });

    rt.register_fn("__rubyrs_kd_code", |args| {
        let (sid, i) = match args {
            [Value::Int(sid), Value::Int(i)] => (*sid, *i as usize),
            _ => return Err(arg_err("__rubyrs_kd_code(sid, i)")),
        };
        match with_session(sid, |s| s.blocks.get(i).map(|(_, code)| code.clone())) {
            Some(Some(code)) => Ok(Value::new_str(code)),
            _ => Err(arg_err("kd_native: bad block index")),
        }
    });

    rt.register_fn("__rubyrs_kd_supply", |args| {
        let (sid, i, html) = match args {
            [Value::Int(sid), Value::Int(i), Value::Str(s)] => {
                (*sid, *i as usize, Some(s.to_string_lossy().to_string()))
            }
            [Value::Int(sid), Value::Int(i), Value::Nil] => (*sid, *i as usize, None),
            _ => return Err(arg_err("__rubyrs_kd_supply(sid, i, html_or_nil)")),
        };
        match with_session(sid, |s| {
            if let Some(slot) = s.supplied.get_mut(i) {
                *slot = html;
                true
            } else {
                false
            }
        }) {
            Some(true) => Ok(Value::Nil),
            _ => Err(arg_err("kd_native: bad supply index")),
        }
    });

    rt.register_fn("__rubyrs_kd_render", |args| {
        let sid = match args {
            [Value::Int(sid)] => *sid,
            _ => return Err(arg_err("__rubyrs_kd_render(sid)")),
        };
        let Some(session) = free_session(sid) else {
            return Err(arg_err("kd_native: bad session id"));
        };
        let mut supplied = Supplied {
            htmls: &session.supplied,
            cursor: 0,
            flavor: session.flavor,
            desynced: false,
        };
        match rostdown::to_html(&session.src, &Options::jekyll(), &mut supplied) {
            // Same source as the accepted scan pass, so Ok is the only
            // expected arm; the guards below catch impossible drift
            // (pass-2 disagreeing with pass-1) and decline to Ruby
            // rather than risk wrong output.
            Ok(html) if !supplied.desynced && supplied.cursor == session.supplied.len() => {
                Ok(Value::new_str(html))
            }
            _ => Ok(Value::Nil),
        }
    });

    rt.register_fn("__rubyrs_kd_abort", |args| {
        let sid = match args {
            [Value::Int(sid)] => *sid,
            _ => return Err(arg_err("__rubyrs_kd_abort(sid)")),
        };
        free_session(sid);
        Ok(Value::Nil)
    });
}
