//! C ABI over `rostdown` for the `kramdown-rostdown` PoC gem.
//!
//! A 1:1 port of rubyrs' in-VM `__rubyrs_kd_*` host fns (see
//! `crates/rubyrs/src/kramdown_native.rs`), exposed as `extern "C"` so
//! the Ruby `ffi` gem can drive it on stock CRuby. Same two-pass
//! scan/supply protocol so code highlighting stays byte-identical:
//! Ruby's Rouge produces each block's inner HTML, the host splices it
//! into kramdown's wrapper markup.
//!
//!   rd_scan(src, gfm, auto_ids, codespan_hl, default_plaintext)
//!     -> i64   session id, or -1 if rostdown declined the document.
//!   rd_block_count(sid) -> i64                  (-1 = bad sid)
//!   rd_block_lang(sid, i) -> *mut c_char        (NUL-term; caller frees)
//!   rd_block_code(sid, i) -> *mut c_char
//!   rd_supply(sid, i, html_ptr, html_len)       (html_ptr NULL = plain)
//!   rd_render(sid) -> *mut c_char               (NULL = declined; frees sid)
//!   rd_abort(sid)                               (free without rendering)
//!   rd_string_free(ptr)
//!
//! All strings cross as bytes; no Ruby-heap interaction. SESSIONS is
//! thread-local — CRuby calls under the GVL on one thread, matching the
//! single-threaded Vm assumption of the original.

use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::slice;

use rostdown::{CodeHighlighter, Options};

/// A document between the scan and render passes.
struct Session {
    src: String,
    opts: Options,
    /// `(lang, code)` per fenced block, in document order.
    blocks: Vec<(String, String)>,
    /// Highlighted HTML per block (inner `None` = plain `<pre><code>`).
    supplied: Vec<Option<String>>,
    /// Render code spans as `<code class="language-plaintext
    /// highlighter-rouge">` (Jekyll's rouge setup) vs bare `<code>`.
    codespan_hl: bool,
    /// `syntax_highlighter_opts[:default_lang] == "plaintext"` — routes
    /// no-language fences onto the highlighted path.
    default_plaintext: bool,
}

thread_local! {
    /// (slots, free list). Slot reuse mirrors the rubyrs host.
    static SESSIONS: RefCell<(Vec<Option<Session>>, Vec<usize>)> =
        const { RefCell::new((Vec::new(), Vec::new())) };
}

/// Highlighter flags shared by the record and replay passes — both
/// passes must agree on these or block enumeration desyncs.
#[derive(Clone, Copy)]
struct HlCfg {
    codespan_hl: bool,
    default_plaintext: bool,
}

impl HlCfg {
    fn codespan_class(&self) -> Option<&'static str> {
        self.codespan_hl
            .then_some("language-plaintext highlighter-rouge")
    }
    fn default_lang(&self) -> Option<&'static str> {
        self.default_plaintext.then_some("plaintext")
    }
}

/// Pass 1: record every block, emit a placeholder (pass-1 HTML is
/// discarded).
struct Recorder {
    cfg: HlCfg,
    blocks: Vec<(String, String)>,
}

impl CodeHighlighter for Recorder {
    fn highlight(&mut self, lang: &str, code: &str) -> Option<String> {
        self.blocks.push((lang.to_string(), code.to_string()));
        Some(String::new())
    }
    fn codespan_class(&self) -> Option<&str> {
        self.cfg.codespan_class()
    }
    fn default_lang(&self) -> Option<&str> {
        self.cfg.default_lang()
    }
}

/// Pass 2: replay the Ruby-supplied HTML in document order.
struct Supplied<'a> {
    cfg: HlCfg,
    htmls: &'a [Option<String>],
    cursor: usize,
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
        self.cfg.codespan_class()
    }
    fn default_lang(&self) -> Option<&str> {
        self.cfg.default_lang()
    }
}

fn with_session<R>(sid: i64, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
    if sid < 0 {
        return None;
    }
    SESSIONS.with(|s| {
        let (slots, _) = &mut *s.borrow_mut();
        slots.get_mut(sid as usize).and_then(Option::as_mut).map(f)
    })
}

fn free_session(sid: i64) -> Option<Session> {
    if sid < 0 {
        return None;
    }
    SESSIONS.with(|s| {
        let (slots, free) = &mut *s.borrow_mut();
        let taken = slots.get_mut(sid as usize).and_then(Option::take);
        if taken.is_some() {
            free.push(sid as usize);
        }
        taken
    })
}

/// `String` -> heap C string. Returns null on interior NUL (markdown
/// never contains one; the Ruby side treats null as decline+fallback).
fn into_cstr(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `ptr`/`len` must describe a valid initialized byte range (or len 0).
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        (len == 0).then_some(&[][..])
    } else {
        Some(unsafe { slice::from_raw_parts(ptr, len) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_scan(
    src_ptr: *const u8,
    src_len: usize,
    gfm: bool,
    auto_ids: bool,
    codespan_hl: bool,
    default_plaintext: bool,
) -> i64 {
    let Some(raw) = (unsafe { bytes(src_ptr, src_len) }) else {
        return -1;
    };
    let Ok(src) = std::str::from_utf8(raw) else {
        return -1;
    };
    let opts = Options { gfm, auto_ids };
    let cfg = HlCfg {
        codespan_hl,
        default_plaintext,
    };
    let mut rec = Recorder {
        cfg,
        blocks: Vec::new(),
    };
    if rostdown::to_html(src, &opts, &mut rec).is_err() {
        return -1; // declined → Ruby falls back to pure kramdown.
    }
    let n = rec.blocks.len();
    let session = Session {
        src: src.to_string(),
        opts,
        blocks: rec.blocks,
        supplied: vec![None; n],
        codespan_hl,
        default_plaintext,
    };
    SESSIONS.with(|s| {
        let (slots, free) = &mut *s.borrow_mut();
        match free.pop() {
            Some(i) => {
                slots[i] = Some(session);
                i as i64
            }
            None => {
                slots.push(Some(session));
                (slots.len() - 1) as i64
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_block_count(sid: i64) -> i64 {
    with_session(sid, |s| s.blocks.len() as i64).unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_block_lang(sid: i64, i: i64) -> *mut c_char {
    match with_session(sid, |s| s.blocks.get(i as usize).map(|(l, _)| l.clone())) {
        Some(Some(lang)) => into_cstr(lang),
        _ => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_block_code(sid: i64, i: i64) -> *mut c_char {
    match with_session(sid, |s| s.blocks.get(i as usize).map(|(_, c)| c.clone())) {
        Some(Some(code)) => into_cstr(code),
        _ => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_supply(sid: i64, i: i64, html_ptr: *const u8, html_len: usize) {
    let html = if html_ptr.is_null() {
        None
    } else {
        unsafe { bytes(html_ptr, html_len) }
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(str::to_string)
    };
    with_session(sid, |s| {
        if let Some(slot) = s.supplied.get_mut(i as usize) {
            *slot = html;
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_render(sid: i64) -> *mut c_char {
    let Some(session) = free_session(sid) else {
        return std::ptr::null_mut();
    };
    let cfg = HlCfg {
        codespan_hl: session.codespan_hl,
        default_plaintext: session.default_plaintext,
    };
    let mut supplied = Supplied {
        cfg,
        htmls: &session.supplied,
        cursor: 0,
        desynced: false,
    };
    match rostdown::to_html(&session.src, &session.opts, &mut supplied) {
        Ok(html) if !supplied.desynced => into_cstr(html),
        _ => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rd_abort(sid: i64) {
    let _ = free_session(sid);
}

/// # Safety
/// `ptr` must be a pointer previously returned by one of the `rd_*`
/// string-returning fns and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rd_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}
