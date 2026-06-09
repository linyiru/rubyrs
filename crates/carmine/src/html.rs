//! The rouge-compatible HTML formatter (`Rouge::Formatters::HTML`):
//! escape `&` `<` `>` only, render the exact `Text` token bare, and wrap
//! everything else in `<span class="SHORTNAME">`.

use crate::table::{LexerTable, TokenId};

fn escape_into(out: &mut String, v: &str) {
    if !v.contains(['&', '<', '>']) {
        out.push_str(v);
        return;
    }
    for ch in v.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// Format a token stream produced by [`crate::Lexer::lex`] as rouge-
/// compatible HTML.
pub fn format(table: &LexerTable, tokens: &[(TokenId, String)]) -> String {
    let mut out = String::new();
    write(table, tokens, &mut out);
    out
}

/// Like [`format`], appending into an existing buffer.
pub fn write(table: &LexerTable, tokens: &[(TokenId, String)], out: &mut String) {
    for (tok, val) in tokens {
        if *tok == table.text_token() {
            escape_into(out, val);
        } else {
            out.push_str("<span class=\"");
            out.push_str(table.token_shortname(*tok));
            out.push_str("\">");
            escape_into(out, val);
            out.push_str("</span>");
        }
    }
}
