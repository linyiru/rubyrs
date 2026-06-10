//! kramdown smart typography: smart quotes (`lsquo,rsquo,ldquo,rdquo`)
//! plus the typographic symbols `--`/`---`/`...`, rendered with
//! `entity_output: as_char` — i.e. the Unicode characters themselves.
//! Quote contexts we cannot classify with certainty decline instead of
//! guessing.

use crate::Error;

pub(crate) const NDASH: char = '\u{2013}'; // –
pub(crate) const MDASH: char = '\u{2014}'; // —
pub(crate) const HELLIP: char = '\u{2026}'; // …
const LSQUO: char = '\u{2018}'; // ‘
const RSQUO: char = '\u{2019}'; // ’
const LDQUO: char = '\u{201C}'; // “
const RDQUO: char = '\u{201D}'; // ”

/// Context where a quote OPENS: start of text, or after whitespace, an
/// opening bracket, or a dash.
fn opening_context(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{' | NDASH | MDASH),
    }
}

/// Context where a quote CLOSES: directly after a word or closing
/// punctuation.
fn closing_context(prev: Option<char>) -> bool {
    prev.is_some_and(|c| {
        c.is_alphanumeric()
            || matches!(
                c,
                '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '%' | HELLIP | RSQUO | RDQUO
            )
    })
}

pub(crate) fn single_quote(prev: Option<char>, next: Option<char>) -> Result<char, Error> {
    let prev_word = prev.is_some_and(char::is_alphanumeric);
    let next_word = next.is_some_and(char::is_alphanumeric);
    // Apostrophe inside a word: It's, don't.
    if prev_word && next_word {
        return Ok(RSQUO);
    }
    if opening_context(prev) && next.is_some_and(|c| !c.is_whitespace()) {
        // RubyPants special-cases decade abbreviations ('80s → rsquo);
        // don't guess which one a digit context is.
        if next.is_some_and(|c| c.is_ascii_digit()) {
            return Err(Error::Declined("decade-quote"));
        }
        return Ok(LSQUO);
    }
    if closing_context(prev) {
        return Ok(RSQUO);
    }
    Err(Error::Declined("ambiguous-single-quote"))
}

pub(crate) fn double_quote(prev: Option<char>, next: Option<char>) -> Result<char, Error> {
    if opening_context(prev) && next.is_some_and(|c| !c.is_whitespace()) {
        return Ok(LDQUO);
    }
    if closing_context(prev) {
        return Ok(RDQUO);
    }
    Err(Error::Declined("ambiguous-double-quote"))
}
