//! `blusher` CRuby extension (magnus): the carmine engine bound to MRI,
//! building the token array DIRECTLY as Ruby objects — no JSON round trip,
//! no per-token re-parse. This is what brings the gem to PARITY with rouge;
//! the FFI/JSON bootstrap was a net loss. carmine's raw lexing speedup does
//! not survive the Ruby-object marshaling at the boundary (token allocation
//! dominates and rouge pays it too); see benchmark/bench.rb and the README.
//!
//! `Blusher::Engine.lex(table_json, input, qualname_map)` returns:
//!   - an Array of `[Rouge::Token, value]` pairs on success,
//!   - `nil` when a callback rule blocks native lexing (caller → rouge),
//!   - raises RuntimeError on a bad table.
//!
//! `qualname_map` is rouge's qualname→Token Hash (`Blusher::Shim::QUALNAME`).
//! Resolving the Token in Rust — via a `hash[name]` aref — lets us skip
//! building an intermediate `[name, value]` array that the Ruby side would
//! then have to re-map: we fold the old Ruby `to_rouge` pass into this one.
//! Unknown names fall back to the `"Error"` entry of the same map (rouge's
//! `Rouge::Token::Tokens::Error`, whose qualname is `"Error"`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use carmine::{Lexer, LexerTable, NoCallbacks};
use magnus::r_hash::ForEach;
use magnus::{function, prelude::*, value::Value, Error, RArray, RHash, RString, Ruby};

thread_local! {
    // Parsed-and-compiled tables, keyed by lexer tag. Building a LexerTable
    // (JSON parse + regex compile) is ~0.5ms and INPUT-INDEPENDENT, so re-doing
    // it per call dwarfs the actual lex on small files. A lexer's table is
    // immutable for the process lifetime, so caching it makes repeat lexes of a
    // language pay that cost once. Thread-local (no lock); each Ruby thread that
    // touches the engine builds its own — fine, tables are cheap to hold.
    static TABLE_CACHE: RefCell<HashMap<String, Rc<LexerTable>>> = RefCell::new(HashMap::new());
}

/// Cached parse of `table_json` under `tag`. First call for a tag parses;
/// subsequent calls reuse the compiled table.
fn cached_table(ruby: &Ruby, tag: &str, table_json: &str) -> Result<Rc<LexerTable>, Error> {
    if let Some(t) = TABLE_CACHE.with(|c| c.borrow().get(tag).cloned()) {
        return Ok(t);
    }
    let table = LexerTable::from_json(table_json)
        .map_err(|e| Error::new(ruby.exception_runtime_error(), format!("blusher table: {e}")))?;
    let rc = Rc::new(table);
    TABLE_CACHE.with(|c| c.borrow_mut().insert(tag.to_string(), rc.clone()));
    Ok(rc)
}

/// Append `val` to `out` with rouge's HTML escaping: `& < >` become entities,
/// `\r` is dropped, everything else verbatim (mirrors `TABLE_FOR_ESCAPE_HTML`).
fn escape_html_into(val: &str, out: &mut String) {
    for ch in val.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => {}
            c => out.push(c),
        }
    }
}

/// Fused lex + `Rouge::Formatters::HTML` formatting, entirely in Rust, returning
/// a SINGLE Ruby String — the whole point: real rouge use produces HTML, and a
/// token stream is only an intermediate. Doing both here means O(1) Ruby
/// allocations instead of one String + one Array per token, so carmine's raw
/// lexing speed finally survives to the Ruby boundary.
///
/// Byte-faithful to the DEFAULT path (escape disabled): `filter_escapes`
/// rewrites the `Escape` token to `Error`; the bare `Text` token (shortname "")
/// emits its escaped value with no span; every other token emits
/// `<span class="SHORTNAME">escaped</span>`. The caller only routes here for an
/// exact `Rouge::Formatters::HTML` instance with escape disabled and no block,
/// so subclasses (HTMLInline/Debug/Pygments/…) keep rouge's own output.
///
/// `shortname` is rouge's qualname→shortname Hash; it is read into a Rust map
/// ONCE per call (≈1k entries) so the per-token lookups never touch Ruby.
/// Returns `nil` to decline (callback rule reached → caller falls back).
fn format_html(
    ruby: &Ruby,
    tag: String,
    table_json: String,
    input: String,
    shortname: RHash,
) -> Result<Value, Error> {
    let table = cached_table(ruby, &tag, &table_json)?;

    let mut sn: HashMap<String, String> = HashMap::new();
    shortname.foreach(|k: String, v: String| {
        sn.insert(k, v);
        Ok(ForEach::Continue)
    })?;

    let mut lexer = Lexer::new(&table);
    match lexer.lex(&input, &mut NoCallbacks) {
        Ok(toks) => {
            let mut out = String::with_capacity(input.len() * 2);
            for (t, v) in &toks {
                let mut name = table.token_name(*t);
                if name == "Escape" {
                    name = "Error"; // filter_escapes (escape disabled — the default)
                }
                match sn.get(name).map(String::as_str) {
                    // bare Text token — no span wrapper
                    Some("") => escape_html_into(v.as_str(), &mut out),
                    Some(short) => {
                        out.push_str("<span class=\"");
                        out.push_str(short);
                        out.push_str("\">");
                        escape_html_into(v.as_str(), &mut out);
                        out.push_str("</span>");
                    }
                    // unknown token name: rouge would raise; decline to be safe.
                    None => return Ok(ruby.qnil().as_value()),
                }
            }
            Ok(RString::new(&out).as_value())
        }
        Err(carmine::Error::CallbackRequired { .. }) => Ok(ruby.qnil().as_value()),
        Err(e) => Err(Error::new(ruby.exception_runtime_error(), format!("blusher format: {e}"))),
    }
}

fn lex(
    ruby: &Ruby,
    tag: String,
    table_json: String,
    input: String,
    qualname: RHash,
) -> Result<Value, Error> {
    let table = cached_table(ruby, &tag, &table_json)?;

    // Fallback token for names rouge doesn't know — resolved once from the map.
    let error_tok: Value = qualname.get("Error").unwrap_or_else(|| ruby.qnil().as_value());

    let mut lexer = Lexer::new(&table);
    match lexer.lex(&input, &mut NoCallbacks) {
        Ok(toks) => {
            let out = RArray::with_capacity(toks.len());
            for (t, v) in &toks {
                let name = table.token_name(*t);
                let tok: Value = match qualname.get(name) {
                    Some(found) => found,
                    None => error_tok,
                };
                let pair = RArray::with_capacity(2);
                pair.push(tok)?;
                pair.push(v.as_str())?;
                out.push(pair)?;
            }
            Ok(out.as_value())
        }
        // Callback rule reachable — decline (the shim falls back to rouge).
        Err(carmine::Error::CallbackRequired { .. }) => Ok(ruby.qnil().as_value()),
        Err(e) => Err(Error::new(ruby.exception_runtime_error(), format!("blusher lex: {e}"))),
    }
}

#[magnus::init(name = "blusher")]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let engine = ruby.define_module("Blusher")?.define_module("Engine")?;
    engine.define_singleton_method("lex", function!(lex, 4))?;
    engine.define_singleton_method("format_html", function!(format_html, 4))?;
    Ok(())
}
