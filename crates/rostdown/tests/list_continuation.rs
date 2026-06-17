//! List-continuation coverage — the construct behind the top real-content
//! decline (`list-continuation`) once tables landed. Expected HTML is
//! byte-for-byte what kramdown 2.5.2 produces under the gem's profile
//! (`Options::jekyll()` ⇒ GFM input, `auto_ids: false`, `hard_wrap:
//! false` — a newline inside an item joins with `\n`, not `<br />`).
//!
//! The `ok` cases pin the continuation shapes rostdown already renders
//! identically (regression guard). The `declines_*` cases document the
//! frontier: each is a continuation kramdown accepts that rostdown
//! conservatively declines today, with the kramdown target noted so the
//! case flips `declined → ok` when support lands. Right-or-declined holds
//! throughout — a continuation is never rendered wrong.

use rostdown::{Error, NoHighlight, Options, to_html};

fn render(src: &str) -> Result<String, Error> {
    to_html(src, &Options::jekyll(), &mut NoHighlight)
}

#[track_caller]
fn ok(src: &str, expected: &str) {
    match render(src) {
        Ok(html) => assert_eq!(html, expected, "input: {src:?}"),
        Err(e) => panic!("expected byte-identical render, declined {src:?}: {e:?}"),
    }
}

#[track_caller]
fn declined(src: &str) {
    match render(src) {
        Ok(html) => panic!("expected decline for {src:?}, got {html:?}"),
        Err(Error::Declined(_)) => {}
    }
}

// ---- Supported: byte-identical to kramdown -------------------------------

#[test]
fn unordered_two_space_continuation() {
    ok(
        "- first line\n  second line\n",
        "<ul>\n  <li>first line\nsecond line</li>\n</ul>\n",
    );
}

#[test]
fn unordered_lazy_column0_continuation() {
    ok(
        "- first\nsecond\n",
        "<ul>\n  <li>first\nsecond</li>\n</ul>\n",
    );
}

#[test]
fn unordered_multiple_continuation_lines() {
    ok("- a\n  b\n  c\n", "<ul>\n  <li>a\nb\nc</li>\n</ul>\n");
}

#[test]
fn continuation_keeps_inline_markup() {
    ok(
        "- see `x`\n  and *y*\n",
        "<ul>\n  <li>see <code>x</code>\nand <em>y</em></li>\n</ul>\n",
    );
}

#[test]
fn ordered_tight_single_line() {
    ok(
        "1. a\n2. b\n3. c\n",
        "<ol>\n  <li>a</li>\n  <li>b</li>\n  <li>c</li>\n</ol>\n",
    );
}

#[test]
fn ordered_lazy_column0_continuation() {
    ok(
        "1. first\nsecond\n",
        "<ol>\n  <li>first\nsecond</li>\n</ol>\n",
    );
}

#[test]
fn ordered_indented_continuation() {
    // The common numbered-list-with-wrapped-text shape: the continuation
    // is indented to the marker's content column (digits + ". ").
    ok(
        "1. first line\n   second line\n2. next\n",
        "<ol>\n  <li>first line\nsecond line</li>\n  <li>next</li>\n</ol>\n",
    );
}

// ---- Frontier: kramdown accepts, rostdown declines (never wrong) ----------

#[test]
fn declines_loose_lists() {
    // Blank line between items ⇒ loose list (`<li><p>…</p></li>`).
    declined("- a\n\n- b\n");
    declined("1. a\n\n2. b\n");
}

#[test]
fn declines_irregular_indentation() {
    // 1-space indent (kramdown reads it as a same-level item).
    declined("- a\n b\n");
    // Tab indentation.
    declined("- a\n\tb\n");
    // Trailing whitespace on an item line carries a hard break.
    declined("- a  \n  b\n");
}
