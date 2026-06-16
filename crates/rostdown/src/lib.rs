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
#[cfg(feature = "arena")]
mod arena;
mod parse;
mod scan;
mod typography;

#[cfg(feature = "arena")]
pub use arena::ScopedAlloc;

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
    /// Language used for fenced blocks WITHOUT an info string —
    /// kramdown's `syntax_highlighter_opts[:default_lang]` (Jekyll:
    /// "plaintext", so even plain fences render highlighted inside
    /// `<div class="language-plaintext highlighter-rouge">`). `None`
    /// (the default) keeps no-lang fences on the `<pre><code>` path.
    fn default_lang(&self) -> Option<&str> {
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
    #[cfg(not(feature = "arena"))]
    {
        let (ast, root) = parse::parse(src, opts)?;
        Ok(html::convert(&ast, root, opts, highlighter, src.len()))
    }
    // With the `arena` feature, run the parse+convert inside a bump
    // scope: the whole `Block`/`Span` tree is pointer-bumped and freed
    // wholesale. The result String is the one thing that must outlive
    // the arena, so on the outermost scope we copy it to the system
    // allocator (allocations at depth 0 forward to System) before
    // resetting. Inert if `ScopedAlloc` isn't the global allocator.
    #[cfg(feature = "arena")]
    {
        let guard = arena::Scope::enter();
        let built = (|| {
            let (ast, root) = parse::parse(src, opts)?;
            Ok(html::convert(&ast, root, opts, highlighter, src.len()))
        })();
        match built {
            Ok(html) => {
                let outermost = arena::leave_no_reset();
                core::mem::forget(guard); // we left manually
                if outermost {
                    let owned = String::from(html.as_str()); // depth 0 → System
                    drop(html); // arena String → dealloc is a no-op
                    arena::reset();
                    Ok(owned)
                } else {
                    // Nested in an outer scope: it owns the arena lifetime.
                    Ok(html)
                }
            }
            Err(e) => {
                drop(guard); // leave + (if outermost) reset
                Err(e)
            }
        }
    }
}

/// Diagnostic: time the parse phase and the convert phase separately,
/// returning `(parse_ns_per_op, convert_ns_per_op)`. Runs on the system
/// allocator (no arena scope), so it isolates compute. Not public API.
#[cfg(feature = "profiling")]
pub fn profile_phases(src: &str, opts: &Options, iters: u32) -> (f64, f64) {
    use std::time::Instant;
    let warm = (iters / 5).max(3);

    // Parse phase: full parse (builds + drops the arena each iter).
    for _ in 0..warm {
        std::hint::black_box(parse::parse(src, opts).ok());
    }
    let t = Instant::now();
    for _ in 0..iters {
        let parsed = parse::parse(src, opts).expect("profile corpus parses");
        std::hint::black_box(&parsed);
    }
    let parse_ns = t.elapsed().as_nanos() as f64 / iters as f64;

    // Convert phase: HTML emission over a fixed pre-parsed tree.
    let (ast, root) = parse::parse(src, opts).expect("profile corpus parses");
    let mut hl = NoHighlight;
    for _ in 0..warm {
        std::hint::black_box(html::convert(&ast, root, opts, &mut hl, src.len()));
    }
    let t = Instant::now();
    for _ in 0..iters {
        let h = html::convert(&ast, root, opts, &mut hl, src.len());
        std::hint::black_box(&h);
    }
    let convert_ns = t.elapsed().as_nanos() as f64 / iters as f64;

    (parse_ns, convert_ns)
}
