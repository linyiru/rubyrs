# rostdown

A [kramdown](https://kramdown.gettalong.org/)-compatible Markdown renderer
in Rust. *Kram is German for stuff; Rost is German for rust.*

rostdown targets **byte-identical HTML** with Ruby kramdown (GFM input
flavor, Jekyll's default options) for a growing subset of the language.
Anything outside the implemented subset is a clean **decline**
([`Error::Declined`]) rather than a guess — embedders fall back to Ruby
kramdown for that document, so output is never silently wrong.

```rust
use rostdown::{to_html, Options, NoHighlight};

let html = to_html("## Hi\n\nSome *text*.\n", &Options::jekyll(), &mut NoHighlight).unwrap();
assert_eq!(html, "<h2 id=\"hi\">Hi</h2>\n\n<p>Some <em>text</em>.</p>\n");
```

## Supported subset

- Block: ATX headings (with kramdown's `auto_ids` slugs), paragraphs,
  unordered/ordered lists, blockquotes, GFM fenced code blocks,
  horizontal rules.
- Span: emphasis/strong, code spans, inline links, backslash escapes.
- Typography: kramdown's smart quotes and typographic symbols
  (`--`/`---`/`...`), `entity_output: as_char`.
- Code blocks route through a [`CodeHighlighter`] hook (plug in a
  rouge-compatible highlighter such as
  [carmine](https://crates.io/crates/carmine), or none for plain
  `<pre><code>`).

## Performance

rostdown is a **zero-copy, flat-node, byte-scanning** engine: the
`Block`/`Span` tree borrows text straight from the source (`Cow<&str>`)
instead of copying it, the nodes live in flat index arenas (no per-node
`Vec` allocation), and the hot scans use word-at-a-time (SWAR) byte
search. On the **default build — no `unsafe`, no dependencies** — it
renders a prose-heavy corpus *faster than* pulldown-cmark while doing
strictly more (smart typography, heading `id` slugs, decline-checking)
and emitting byte-identical kramdown HTML. The opt-in features below add
further headroom. (The rubyrs `poc/markdown-bench` harness has the
cross-engine numbers.)

## Features (all off by default)

- **`arena`** — exposes [`ScopedAlloc`], a scoped bump allocator an
  embedder installs as its `#[global_allocator]`; `to_html` then
  pointer-bumps its AST and frees it wholesale, for extra throughput.
  This is the crate's only `unsafe` module: off by default, covered by
  Miri (Stacked + Tree Borrows) and AddressSanitizer over the live path,
  and `to_html` pauses the arena around highlighter callbacks so their
  allocations survive the reset.
- **`simd`** — an aarch64 NEON byteset for the inline scanner's
  ordinary-text skip (scalar lookup table otherwise). Adds `unsafe`.
- **`profiling`** — exposes `profile_phases()` for parse-vs-convert
  timing. Diagnostic only.

## Testing

The golden corpus under `tests/corpus` is vendored from kramdown's own
test suite (MIT, © Thomas Leitner and contributors); the test runner
reports an implemented-directory dashboard so coverage growth is
measurable. The contract is *right-or-declined* — a render that mismatches
the expected HTML is a hard failure, never accepted. With `--features
arena` the corpus runs through the live bump-allocator path (the byte-
identity gate for the arena that Miri can't reach).
