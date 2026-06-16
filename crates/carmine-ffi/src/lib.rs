//! C ABI over carmine — the foundation for a rouge-compatible Ruby gem
//! (and language-neutral bindings). The interface is deliberately COARSE,
//! matching carmine's "lex-or-decline" contract: a single call lexes an
//! input with a given rule table and returns the token stream, or signals
//! that a callback rule was hit (the caller then falls back to pure rouge).
//!
//! `carmine_lex(table_json, input)` returns a heap-allocated JSON C string:
//!   {"status":"ok","tokens":[["Keyword","def"],["Text"," "], …]}
//!   {"status":"decline"}                  // a callback rule blocks native lexing
//!   {"status":"error","message":"…"}
//! The caller MUST release the returned pointer with `carmine_free`.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use carmine::{Lexer, LexerTable, NoCallbacks};

fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a NUL-terminated C string for the call's
    // duration; we copy it out immediately.
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(str::to_string)
}

fn run(table_json: &str, input: &str) -> serde_json::Value {
    let table = match LexerTable::from_json(table_json) {
        Ok(t) => t,
        Err(e) => return serde_json::json!({"status": "error", "message": format!("table: {e}")}),
    };
    let mut lexer = Lexer::new(&table);
    match lexer.lex(input, &mut NoCallbacks) {
        Ok(toks) => {
            let tokens: Vec<[&str; 2]> = toks
                .iter()
                .map(|(t, v)| [table.token_name(*t), v.as_str()])
                .collect();
            serde_json::json!({"status": "ok", "tokens": tokens})
        }
        // A callback rule was reachable — carmine can't be sure it matches
        // rouge, so the embedder falls back to rouge itself.
        Err(carmine::Error::CallbackRequired { .. }) => serde_json::json!({"status": "decline"}),
        Err(e) => serde_json::json!({"status": "error", "message": format!("{e}")}),
    }
}

/// Lex `input` with the rule table `table_json`. See the module docs for the
/// returned JSON shape. Never panics across the boundary.
///
/// # Safety
/// `table_json` and `input` must be valid NUL-terminated C strings (or null).
#[unsafe(no_mangle)]
pub extern "C" fn carmine_lex(table_json: *const c_char, input: *const c_char) -> *mut c_char {
    let out = match (cstr(table_json), cstr(input)) {
        (Some(t), Some(i)) => std::panic::catch_unwind(|| run(&t, &i).to_string())
            .unwrap_or_else(|_| r#"{"status":"error","message":"panic"}"#.to_string()),
        _ => r#"{"status":"error","message":"null or non-utf8 argument"}"#.to_string(),
    };
    CString::new(out)
        .unwrap_or_else(|_| CString::new("{}").expect("static"))
        .into_raw()
}

/// Free a string returned by [`carmine_lex`].
///
/// # Safety
/// `p` must be a pointer previously returned by `carmine_lex` (or null), and
/// must not be used after this call.
#[unsafe(no_mangle)]
pub extern "C" fn carmine_free(p: *mut c_char) {
    if !p.is_null() {
        // SAFETY: reclaims the CString this library allocated in carmine_lex.
        unsafe { drop(CString::from_raw(p)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(table: &str, input: &str) -> serde_json::Value {
        let t = CString::new(table).unwrap();
        let i = CString::new(input).unwrap();
        let p = carmine_lex(t.as_ptr(), i.as_ptr());
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
        carmine_free(p);
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn ok_decline_and_error() {
        let table = r##"{"lexer":"t","states":{"root":[
            {"kind":"tok","re":"a","opts":0,"tok":"Keyword","next":null},
            {"kind":"callback","re":"b","opts":0}
          ]},"shortnames":{"Keyword":"k"}}"##;
        let ok = call(table, "a");
        assert_eq!(ok["status"], "ok");
        assert_eq!(ok["tokens"][0][0], "Keyword");
        assert_eq!(ok["tokens"][0][1], "a");
        // "b" hits the callback rule → decline (caller falls back to rouge).
        assert_eq!(call(table, "b")["status"], "decline");
        // Malformed table → error, not a crash.
        assert_eq!(call("{not json", "a")["status"], "error");
    }
}
