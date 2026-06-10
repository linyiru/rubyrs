//! rostdown — a kramdown-compatible Markdown renderer.
//!
//! Produces byte-identical HTML to Ruby kramdown (GFM input flavor,
//! Jekyll's default options) for an explicitly-bounded subset of the
//! language. Constructs outside the subset return [`Error::Declined`]
//! instead of approximated output — embedders fall back to Ruby kramdown
//! for that document, so rendering is never silently wrong.
//!
//! ```
//! use rostdown::{Options, NoHighlight, to_html};
//! let html = to_html("## Hi\n\nSome *text*.\n", &Options::jekyll(), &mut NoHighlight).unwrap();
//! assert_eq!(html, "<h2 id=\"hi\">Hi</h2>\n\n<p>Some <em>text</em>.</p>\n");
//! ```

mod html;
mod parse;
mod typography;

/// Rendering options. [`Options::jekyll`] mirrors Jekyll's kramdown
/// defaults (`input: GFM`, `auto_ids`, `entity_output: as_char`,
/// `smart_quotes: lsquo,rsquo,ldquo,rdquo`, `hard_wrap: false`).
#[derive(Debug, Clone)]
pub struct Options {
    /// GFM input flavor: backtick code fences (kramdown core only has
    /// `~~~`).
    pub gfm: bool,
    /// Generate `id` attributes on headings with kramdown's slug rules.
    pub auto_ids: bool,
}

impl Options {
    /// Jekyll's kramdown defaults.
    pub fn jekyll() -> Self {
        Options {
            gfm: true,
            auto_ids: true,
        }
    }

    /// kramdown core defaults (the vendored test corpus flavor).
    pub fn core() -> Self {
        Options {
            gfm: false,
            auto_ids: false,
        }
    }
}

/// Pluggable code-block highlighter — the seam where a rouge-compatible
/// engine (e.g. the `carmine` crate) slots in. Return `None` to decline
/// a language; the block then renders as plain
/// `<pre><code class="language-…">`.
pub trait CodeHighlighter {
    /// Produce the highlighter's inner HTML for `code` (kramdown wraps
    /// it in `<div class="language-… highlighter-…">`).
    fn highlight(&mut self, lang: &str, code: &str) -> Option<String>;
    /// The highlighter's name for the wrapper class (rouge → "rouge").
    fn name(&self) -> &str {
        "rouge"
    }
    /// `class` attribute for inline code spans. kramdown with an active
    /// rouge highlighter (Jekyll's setup: `default_lang: plaintext`,
    /// `guess_lang: true`) renders every codespan as
    /// `<code class="language-plaintext highlighter-rouge">`; the
    /// escaping is byte-identical to the plain path, only the attribute
    /// differs. `None` (the default) renders a bare `<code>`.
    fn codespan_class(&self) -> Option<&str> {
        None
    }
}

/// No highlighting: every block renders as plain `<pre><code>`.
pub struct NoHighlight;

impl CodeHighlighter for NoHighlight {
    fn highlight(&mut self, _lang: &str, _code: &str) -> Option<String> {
        None
    }
}

/// Why rostdown refused to render a document.
#[derive(Debug)]
pub enum Error {
    /// The input uses a construct outside the implemented subset; the
    /// payload names it (for diagnostics / coverage dashboards).
    Declined(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Declined(what) => write!(f, "declined: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// Render `src` to kramdown-compatible HTML.
pub fn to_html(
    src: &str,
    opts: &Options,
    highlighter: &mut dyn CodeHighlighter,
) -> Result<String, Error> {
    let blocks = parse::parse(src, opts)?;
    Ok(html::convert(&blocks, opts, highlighter))
}
