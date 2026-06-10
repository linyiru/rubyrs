# rostdown

A [kramdown](https://kramdown.gettalong.org/)-compatible Markdown renderer
in Rust. *Kram is German for stuff; Rost is German for rust.*

rostdown targets **byte-identical HTML** with Ruby kramdown (GFM input
flavor, Jekyll's default options) for a growing subset of the language.
Anything outside the implemented subset is a clean **decline**
([`Error::Declined`]) rather than a guess — embedders fall back to Ruby
kramdown for that document, so output is never silently wrong.

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

The golden corpus under `tests/corpus` is vendored from kramdown's own
test suite (MIT, © Thomas Leitner and contributors); the test runner
reports an implemented-directory dashboard so coverage growth is
measurable.
