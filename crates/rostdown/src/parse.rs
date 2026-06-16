//! Block + span parsing into a small element tree. Anything outside the
//! implemented subset returns `Error::Declined(reason)` — the whole
//! document is parsed before any HTML is emitted, so a decline can never
//! leave partial output.

use std::borrow::Cow;

use crate::{Error, Options, typography};

/// The element tree is a *borrowed, flat-arena* AST.
///
/// Borrowed: every text field is a `Cow<'a, str>` that borrows directly
/// from the input `src` whenever the rendered bytes are identical to the
/// source bytes (the common case — prose, code, hrefs), and only
/// materializes an owned `String` when a typography rewrite
/// (`'`/`"`/`-`/`.`) or backslash escape mutates a char. The lifetime
/// `'a` is tied to `src` inside `to_html`, so the AST never outlives the
/// document it borrows from.
///
/// Flat-arena: there are no per-node `Vec`s. Spans, blocks and list items
/// each live in one growable arena owned by [`Ast`], and node lists are
/// *sibling-linked* — a list is the chain reached by following `next`
/// from a start index, and a composite's children are the chain from its
/// `first_child` index. The recursive inline parser interleaves a
/// composite's children with the siblings that follow it, so a single
/// contiguous range can't describe a list; the `next` link can. The
/// arenas are three `Vec`s grown by `push`, so one render allocates a
/// handful of buffers (amortized) instead of ~855 little node `Vec`s.
///
/// Backtracking: the inline parser speculatively recurses on an emphasis
/// open and reverts (`parse_spans_until` → `None`) when no close is
/// found. Each speculative recursion records `ast.spans.len()` first and
/// `truncate`s back to it on revert, so abandoned nodes never leak into
/// the arena or the output.
#[derive(Debug, Default)]
pub(crate) struct Ast<'a> {
    pub(crate) blocks: Vec<BlockNode<'a>>,
    pub(crate) spans: Vec<SpanNode<'a>>,
    pub(crate) items: Vec<ItemNode<'a>>,
}

impl<'a> Ast<'a> {
    fn new() -> Self {
        Ast::default()
    }

    /// Pre-size the arenas from the source length so the hot top-level
    /// parse rarely re-grows (each regrow is a realloc + memcpy of the
    /// whole arena — the dominant cost once per-node `Vec`s are gone).
    /// Heuristics from the bench corpus: spans dominate (~1 node / 12 B
    /// of source — prose plus markup splits), blocks ~1 / 40 B, items
    /// far rarer. Generous but bounded; a tiny doc still costs three
    /// small `Vec`s, an over-estimate just over-reserves once.
    fn with_capacity_for(src_len: usize) -> Self {
        Ast {
            blocks: Vec::with_capacity(src_len / 40 + 8),
            spans: Vec::with_capacity(src_len / 12 + 16),
            items: Vec::with_capacity(src_len / 256 + 4),
        }
    }

    /// Push a span node and return its index. Not linked yet — the
    /// caller (a [`Chain`]) sets `next`.
    #[inline]
    fn push_span(&mut self, kind: SpanKind<'a>) -> u32 {
        let idx = self.spans.len() as u32;
        self.spans.push(SpanNode { kind, next: None });
        idx
    }

    #[inline]
    fn push_block(&mut self, kind: BlockKind<'a>) -> u32 {
        let idx = self.blocks.len() as u32;
        self.blocks.push(BlockNode { kind, next: None });
        idx
    }

    #[inline]
    fn push_item(&mut self, item: ItemNode<'a>) -> u32 {
        let idx = self.items.len() as u32;
        self.items.push(item);
        idx
    }
}

/// One block in the flat arena plus its sibling link. `next == None`
/// terminates a block list.
#[derive(Debug)]
pub(crate) struct BlockNode<'a> {
    pub(crate) kind: BlockKind<'a>,
    pub(crate) next: Option<u32>,
}

#[derive(Debug)]
pub(crate) enum BlockKind<'a> {
    /// A run of blank lines between blocks (renders as one `\n`).
    Blank,
    /// `raw` is the unparsed heading text — kramdown CORE derives
    /// `auto_ids` slugs from it. `span_text` is the parsed-tree text
    /// (typography applied, link text included, markup gone) — the GFM
    /// parser's `generate_gfm_header_id` input. `spans` is the index of
    /// the first child span (or `None` for an empty heading).
    Heading {
        level: u8,
        raw: Cow<'a, str>,
        span_text: Cow<'a, str>,
        spans: Option<u32>,
    },
    /// Paragraph: index of the first child span (`None` ⇒ empty).
    Para(Option<u32>),
    /// Tight list (no blank lines inside) — `items` is the index of the
    /// first [`ItemNode`]. Items are span runs plus an optional trailing
    /// nested child list; lazy continuations join the item's spans with a
    /// literal newline (kramdown's verbatim line joining).
    List {
        ordered: bool,
        items: Option<u32>,
    },
    /// Blockquote: index of the first child block (`None` ⇒ empty).
    Quote(Option<u32>),
    Code {
        lang: Option<Cow<'a, str>>,
        text: Cow<'a, str>,
    },
    Hr,
}

/// One tight-list item in the flat arena. `spans` is the first child
/// span; `child` is the first item of an optional trailing nested list;
/// `next` links to the following sibling item.
#[derive(Debug)]
pub(crate) struct ItemNode<'a> {
    pub(crate) spans: Option<u32>,
    /// Trailing nested list: `(ordered, first_item_index)`. Tight items
    /// carry at most text-then-one-child in our subset; anything richer
    /// (blank lines, content after the child) declines first.
    pub(crate) child: Option<(bool, Option<u32>)>,
    pub(crate) next: Option<u32>,
    /// Lifetime tie: an `ItemNode` with all-`None` index fields would
    /// otherwise be `'static`. Keeps `'a` bound to `src`.
    _marker: std::marker::PhantomData<&'a str>,
}

/// One span in the flat arena plus its sibling link. `next == None`
/// terminates a span chain (a span run, or a composite's children).
#[derive(Debug)]
pub(crate) struct SpanNode<'a> {
    pub(crate) kind: SpanKind<'a>,
    pub(crate) next: Option<u32>,
}

#[derive(Debug)]
pub(crate) enum SpanKind<'a> {
    /// Raw text (typography + escaping applied at conversion). Borrows a
    /// pristine `src` slice unless a rewrite forced materialization.
    Text(Cow<'a, str>),
    /// Emphasis: index of the first child span.
    Em(Option<u32>),
    /// Strong: index of the first child span.
    Strong(Option<u32>),
    Code(Cow<'a, str>),
    Link {
        /// Index of the first child span.
        spans: Option<u32>,
        href: Cow<'a, str>,
    },
}

/// Builds a sibling-linked chain in one of the arenas: tracks the first
/// and last index pushed so each new node's predecessor gets its `next`
/// patched. `first()` returns the chain head for storing in a parent.
///
/// `Link` is the per-arena patch operation — given a node index, set its
/// `next` field — so one `Chain` type serves spans, blocks and items.
struct Chain {
    first: Option<u32>,
    last: Option<u32>,
}

impl Chain {
    #[inline]
    fn new() -> Self {
        Chain { first: None, last: None }
    }

    /// Append `idx` to the chain, patching the previous tail's `next`
    /// via `set_next`. The pushed node's own `next` must already be
    /// `None` (the arena `push_*` helpers guarantee this).
    #[inline]
    fn link(&mut self, idx: u32, set_next: impl FnOnce(u32, u32)) {
        match self.last {
            None => self.first = Some(idx),
            Some(prev) => set_next(prev, idx),
        }
        self.last = Some(idx);
    }

    #[inline]
    fn first(&self) -> Option<u32> {
        self.first
    }
}

fn declined(what: &'static str) -> Error {
    Error::Declined(what)
}

/// Split `src` on `\n` into line slices — byte-identical to
/// `src.split('\n').collect()` (a trailing `\n` yields a final empty
/// element) but with a tight byte scan instead of std's char-pattern
/// searcher.
fn split_lines(src: &str) -> Vec<&str> {
    let bytes = src.as_bytes();
    // Heuristic capacity (~32 B/line) so the Vec rarely regrows, without
    // a second pass to count newlines exactly.
    let mut out = Vec::with_capacity(src.len() / 32 + 8);
    let mut start = 0;
    // SWAR memchr1 finds each `\n` a word at a time instead of scanning
    // byte-by-byte (line splitting was the top parse self-time).
    while let Some(off) = crate::scan::memchr1(&bytes[start..], b'\n') {
        let nl = start + off;
        out.push(&src[start..nl]);
        start = nl + 1;
    }
    out.push(&src[start..]);
    out
}

/// Parse `src` into a flat-arena [`Ast`]. The returned `root` is the
/// index of the first top-level block (`None` for an empty document);
/// the converter walks the arenas from there.
pub(crate) fn parse<'a>(src: &'a str, opts: &Options) -> Result<(Ast<'a>, Option<u32>), Error> {
    let lines: Vec<&'a str> = split_lines(src);
    // A trailing "\n" yields one empty last element — drop it so it
    // doesn't read as a blank line.
    let lines = match lines.last() {
        Some(&"") => &lines[..lines.len() - 1],
        _ => &lines[..],
    };
    let mut ast = Ast::with_capacity_for(src.len());
    let root = parse_blocks(&mut ast, src, lines, opts)?;
    Ok((ast, root))
}

/// Byte offset of slice `s` within its backing string `src`. `s` MUST be
/// a sub-slice of `src` (true for every line / span slice we derive from
/// the input). Safe pointer arithmetic — no dereference — used to
/// reconstruct a contiguous `src` range from two of its sub-slices so a
/// multi-line paragraph / fenced-code body can be borrowed as one slice
/// instead of joined into a fresh `String`.
#[inline]
fn offset_in(src: &str, s: &str) -> usize {
    s.as_ptr() as usize - src.as_ptr() as usize
}

/// ASCII whitespace per `char::is_whitespace` (note: includes VT `0x0B`,
/// which `u8::is_ascii_whitespace` omits).
#[inline]
fn ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// `line.trim().is_empty()` with an ASCII fast path — most lines decide on
/// the first byte (a prose letter ⇒ not blank). Falls back to the precise
/// Unicode-aware check only when a non-ASCII byte is reached.
#[inline]
fn is_blank(line: &str) -> bool {
    for (i, &b) in line.as_bytes().iter().enumerate() {
        if b >= 0x80 {
            return line[i..].trim_start().is_empty();
        }
        if !ascii_ws(b) {
            return false;
        }
    }
    true
}

/// `str::trim_start` with an ASCII fast path (non-ASCII boundary ⇒ defer
/// to the Unicode-aware trim, which may strip more, e.g. NBSP).
#[inline]
fn trim_start_ws(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] >= 0x80 {
            return s[i..].trim_start();
        }
        if !ascii_ws(b[i]) {
            break;
        }
        i += 1;
    }
    &s[i..]
}

/// `str::trim` (both ends) with an ASCII fast path; defers to the precise
/// Unicode trim when a trim boundary lands on a non-ASCII byte.
#[inline]
fn trim_ws(s: &str) -> &str {
    let b = s.as_bytes();
    let mut start = 0;
    while start < b.len() && b[start] < 0x80 && ascii_ws(b[start]) {
        start += 1;
    }
    let mut end = b.len();
    while end > start && b[end - 1] < 0x80 && ascii_ws(b[end - 1]) {
        end -= 1;
    }
    if (start < b.len() && b[start] >= 0x80) || (end > start && b[end - 1] >= 0x80) {
        return s.trim(); // non-ASCII at a boundary: be precise
    }
    &s[start..end]
}

/// `str::trim_end` with an ASCII fast path (non-ASCII boundary ⇒ defer to
/// the Unicode-aware trim, which may strip more).
#[inline]
fn trim_end_ws(s: &str) -> &str {
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 && b[end - 1] < 0x80 && ascii_ws(b[end - 1]) {
        end -= 1;
    }
    if end > 0 && b[end - 1] >= 0x80 {
        return s.trim_end();
    }
    &s[..end]
}

fn parse_blocks<'a>(
    ast: &mut Ast<'a>,
    src: &'a str,
    lines: &[&'a str],
    opts: &Options,
) -> Result<Option<u32>, Error> {
    let mut chain = Chain::new();
    // Push `kind` into the block arena and link it as the next sibling.
    macro_rules! emit_block {
        ($kind:expr) => {{
            let idx = ast.push_block($kind);
            chain.link(idx, |p, n| ast.blocks[p as usize].next = Some(n));
        }};
    }
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_blank(line) {
            // Collapse a blank run into one Blank between blocks.
            while i < lines.len() && is_blank(lines[i]) {
                i += 1;
            }
            emit_block!(BlockKind::Blank);
            continue;
        }

        decline_block_scan(line)?;

        // ATX heading.
        if let Some(rest) = line.strip_prefix('#') {
            let mut level = 1u8;
            let mut rest = rest;
            while let Some(r) = rest.strip_prefix('#') {
                level += 1;
                rest = r;
            }
            if level <= 6
                && let Some(text) = rest.strip_prefix(' ')
            {
                // kramdown strips optional trailing hashes.
                let text = trim_end_ws(text);
                let text = trim_end_ws(text.trim_end_matches('#'));
                let spans = parse_spans(ast, text)?;
                // span_text is the concatenation of child-span texts.
                // The overwhelmingly common heading is plain prose — a
                // single borrowed `Text` span — so reuse that slice
                // instead of allocating; only mixed-markup headings
                // concatenate into an owned `String`.
                let span_text: Cow<'a, str> = match spans {
                    Some(first)
                        if ast.spans[first as usize].next.is_none()
                            && matches!(ast.spans[first as usize].kind, SpanKind::Text(_)) =>
                    {
                        match &ast.spans[first as usize].kind {
                            SpanKind::Text(t) => t.clone(),
                            _ => unreachable!(),
                        }
                    }
                    _ => {
                        let mut s = String::new();
                        spans_raw_text(ast, spans, &mut s);
                        Cow::Owned(s)
                    }
                };
                // GFM slugs we can't reproduce exactly (Unicode word
                // classes outside our supported set, empty results)
                // decline rather than risk a wrong id. The converter
                // builds the real slug later, so the parser only needs
                // the validity bit — a non-allocating check.
                if opts.gfm && opts.auto_ids && !crate::html::gfm_slug_ok(&span_text) {
                    return Err(declined("heading-gfm-slug"));
                }
                emit_block!(BlockKind::Heading {
                    level,
                    // `text` is a trimmed sub-slice of the heading line —
                    // borrow it directly.
                    raw: Cow::Borrowed(text),
                    span_text,
                    spans,
                });
                i += 1;
                continue;
            }
            return Err(declined("atx-heading-shape"));
        }

        // Horizontal rule: 3+ of the same marker, only that marker +
        // spaces on the line.
        if is_hr(line) {
            emit_block!(BlockKind::Hr);
            i += 1;
            continue;
        }

        // Fenced code block.
        let fence = if opts.gfm && line.starts_with("```") {
            Some("```")
        } else if line.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(fence) = fence {
            let info = line[fence.len()..].trim();
            if info.contains('`') || info.contains('{') {
                return Err(declined("fence-info"));
            }
            let lang = if info.is_empty() {
                None
            } else {
                // The info-string's first word is a sub-slice of the line.
                Some(Cow::Borrowed(info.split_whitespace().next().unwrap_or("")))
            };
            i += 1;
            // Track the first/last CONTENT line so the body — every
            // content line plus its trailing `\n` — can be borrowed as
            // one contiguous `src` slice. Consecutive lines came from
            // `split_lines(src)` separated by exactly one `\n`, and a
            // closing fence always follows the last content line (else
            // we decline), so the `\n` after the last content line is
            // present in `src`. We scan with scalars (no per-fence
            // `Vec<&str>`); the rare non-contiguous case re-walks the
            // content-line range `lines[body_start..body_end]`.
            let body_start = i;
            let mut first_body: Option<&'a str> = None;
            let mut prev_body: Option<&'a str> = None;
            let mut last_body: &'a str = "";
            let mut contiguous = true;
            let mut closed = false;
            while i < lines.len() {
                let l = lines[i];
                if trim_end_ws(l) == fence {
                    closed = true;
                    break;
                }
                if let Some(prev) = prev_body
                    && offset_in(src, l) != offset_in(src, prev) + prev.len() + 1
                {
                    // De-prefixed lines (fence inside a blockquote / list):
                    // not contiguous in `src`, so we must join an owned body.
                    contiguous = false;
                }
                if first_body.is_none() {
                    first_body = Some(l);
                }
                prev_body = Some(l);
                last_body = l;
                i += 1;
            }
            let body_end = i; // exclusive: content lines are [body_start, body_end)
            if closed {
                i += 1; // consume the closing fence line
            }
            if !closed {
                return Err(declined("unclosed-fence"));
            }
            let text = match first_body {
                None => Cow::Borrowed(""),
                Some(first) if contiguous => {
                    // body = src[first.start ..= the `\n` after last] —
                    // each content line plus one trailing newline. The
                    // closing fence guarantees that final `\n` exists.
                    let start = offset_in(src, first);
                    let end = offset_in(src, last_body) + last_body.len() + 1;
                    Cow::Borrowed(&src[start..end])
                }
                Some(_) => {
                    // Non-contiguous: join each content line + `\n`.
                    let mut body = String::with_capacity(64);
                    for l in &lines[body_start..body_end] {
                        body.push_str(l);
                        body.push('\n');
                    }
                    Cow::Owned(body)
                }
            };
            emit_block!(BlockKind::Code { lang, text });
            continue;
        }

        // Blockquote: collect `>`-prefixed lines (plus lazy
        // continuations) and recurse.
        if line.starts_with('>') {
            // The de-`>`-prefixed lines are sub-slices of `src`, so the
            // recursive parse borrows straight from `src` (no owned
            // intermediate `String`s, no deep `into_owned` of the
            // subtree). The `inner` Vec holds only the slice pointers;
            // the data stays in `src`, which outlives the whole AST.
            let mut inner: Vec<&'a str> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if let Some(rest) = l.strip_prefix('>') {
                    inner.push(rest.strip_prefix(' ').unwrap_or(rest));
                    i += 1;
                } else if !is_blank(l) && !inner.is_empty() {
                    // Lazy continuation of the quoted paragraph.
                    inner.push(l);
                    i += 1;
                } else {
                    break;
                }
            }
            // A quote body never starts/ends with Blank markers — drop
            // the leading/trailing `Blank` nodes from the recursed chain
            // by walking past leading Blanks and stopping the chain
            // before trailing ones, rather than mutating a Vec.
            let mut head = parse_blocks(ast, src, &inner, opts)?;
            // Drop leading Blanks.
            while let Some(h) = head
                && matches!(ast.blocks[h as usize].kind, BlockKind::Blank)
            {
                head = ast.blocks[h as usize].next;
            }
            // Cut the chain before any trailing run of Blanks: walk to the
            // last non-Blank node and clear its `next`.
            if let Some(h) = head {
                let mut cur = h;
                let mut last_keep = h;
                loop {
                    if !matches!(ast.blocks[cur as usize].kind, BlockKind::Blank) {
                        last_keep = cur;
                    }
                    match ast.blocks[cur as usize].next {
                        Some(n) => cur = n,
                        None => break,
                    }
                }
                ast.blocks[last_keep as usize].next = None;
            }
            emit_block!(BlockKind::Quote(head));
            continue;
        }

        // Lists (tight only — blank lines inside / loose shapes
        // decline). Nesting via marker-width indentation is
        // supported for UNORDERED parents (`- a` over `  - b`, the
        // form real posts use): the 2-space-stripped tail of an
        // item parses as continuation text plus at most one child
        // list. Ordered parents keep the conservative decline
        // (their content column is digits+2, not a fixed strip
        // width).
        if let Some(ordered) = list_marker(line) {
            let items = parse_list_items(ast, lines, &mut i, ordered)?;
            emit_block!(BlockKind::List { ordered, items });
            continue;
        }

        // kramdown recognizes most block openers behind 1–3 leading
        // spaces (OPT_SPACE); our dispatcher only sees them at column
        // 0, so an indented opener must decline, not become a paragraph.
        if opt_space_opener(line, opts) {
            return Err(declined("opt-space-block"));
        }

        // Paragraph: gather lines. What ends a paragraph differs by
        // flavor: core kramdown's PARAGRAPH_END is only blank lines
        // (plus IAL/EOB/HTML/deflist starts, all declined); GFM's
        // `paragraph_end` quirk (Jekyll's default) adds LIST_START,
        // ATX_HEADER_START, BLOCKQUOTE_START and FENCED_CODEBLOCK_START
        // — but NOT horizontal rules. Opener-looking lines that don't
        // end the paragraph are literal paragraph text in kramdown.
        // Lines are kept VERBATIM (kramdown preserves interior trailing
        // spaces); only the first line loses its OPT_SPACE indent and
        // the final line is right-stripped.
        // The joined paragraph text is USUALLY a contiguous `src` slice:
        // the first line stripped of its OPT_SPACE indent (a sub-slice),
        // the interior lines verbatim, the `\n` separators that
        // `split_lines` consumed (still present in `src` between the line
        // slices), and the final line right-stripped. When the lines DO
        // abut in `src` (the top-level case) we borrow one slice and
        // never build a `String`. When they DON'T — blockquote / list
        // bodies hand us de-prefixed slices with `> `/indent gaps between
        // them — we fall back to a joined owned `String`.
        let para_start = i;
        let mut first_line: Option<&'a str> = None;
        let mut prev_line: &'a str = "";
        let mut last_line: &'a str = "";
        // True while every line so far abuts the previous one in `src`
        // with exactly one `\n` between — the borrow precondition.
        let mut contiguous = true;
        let mut first = true;
        while i < lines.len() {
            let l = lines[i];
            if is_blank(l) {
                break;
            }
            if opts.gfm
                && !first
                && (l.starts_with('#')
                    || l.starts_with('>')
                    || list_marker(l).is_some()
                    || l.starts_with("```")
                    || l.starts_with("~~~"))
            {
                break;
            }
            // A swallowed opener-looking line renders as literal text in
            // kramdown; our spans handle `#`/`>` fine, but hr runs would
            // mis-render (`***` → emphasis decline already; `___`/`---`
            // runs likewise) — anything else opener-shaped is rare
            // enough to decline rather than risk divergence.
            if !first && !opts.gfm && (l.starts_with('>') || list_marker(l).is_some()) {
                return Err(declined("core-paragraph-swallow"));
            }
            if !first && opt_space_opener(l, opts) {
                return Err(declined("opt-space-block"));
            }
            // Setext underlines would silently turn this paragraph into
            // a heading — out of subset.
            if i + 1 < lines.len() {
                let next = lines[i + 1];
                let t = trim_end_ws(next).as_bytes();
                if !t.is_empty() && (t.iter().all(|&b| b == b'=') || t.iter().all(|&b| b == b'-')) {
                    return Err(declined("setext-heading"));
                }
            }
            decline_block_scan(l)?;
            if first {
                // First line: drop OPT_SPACE indent (a sub-slice move).
                first_line = Some(l.trim_start_matches(' '));
            } else {
                // Interior line endings carry hard-break semantics; the
                // just-completed line (`prev_line`) is now interior.
                decline_eol(prev_line)?;
                // Contiguity: this line must abut the previous one in
                // `src` with exactly one `\n` between (true at top level;
                // false once `>`/indent prefixes were stripped).
                if offset_in(src, l) != offset_in(src, prev_line) + prev_line.len() + 1 {
                    contiguous = false;
                }
            }
            prev_line = l;
            last_line = l;
            first = false;
            i += 1;
        }
        let first_line = first_line.expect("paragraph has at least one line");
        // Final paragraph line: kramdown right-strips it (trailing spaces
        // there do NOT produce a hard break).
        let last_stripped = last_line.trim_end_matches([' ', '\t']);
        if contiguous {
            // Reconstruct the contiguous span from the (indent-stripped)
            // first line through the (right-stripped) last line — all
            // verbatim `src`, so the spans borrow directly.
            let start = offset_in(src, first_line);
            let end = offset_in(src, last_stripped) + last_stripped.len();
            let spans = parse_spans(ast, &src[start..end])?;
            emit_block!(BlockKind::Para(spans));
        } else {
            // Gaps between de-prefixed lines (blockquote / list bodies):
            // join `lines[para_start..i]` into an owned `String`
            // (kramdown's verbatim line-join with a single `\n`) — first
            // line indent-stripped, last line right-stripped — parse over
            // it, then push the spans into the arena with their text
            // deep-owned so they don't borrow the temp.
            let para = &lines[para_start..i];
            let last = para.len() - 1; // >= 1: a 1-line para is contiguous
            let mut joined = String::new();
            for (k, &l) in para.iter().enumerate() {
                let segment = if k == 0 {
                    first_line // indent-stripped first line
                } else if k == last {
                    last_stripped // right-stripped final line
                } else {
                    l // interior line, verbatim
                };
                joined.push_str(segment);
                if k != last {
                    joined.push('\n');
                }
            }
            let spans = parse_spans_owned(ast, &joined)?;
            emit_block!(BlockKind::Para(spans));
        }
    }
    Ok(chain.first())
}

/// Parse `joined` (a temporary `String` that does NOT outlive the AST)
/// into the main arena, deep-owning every text `Cow` so no span borrows
/// the temporary. Used for the rare owned-text paragraph fallback
/// (blockquote / list bodies whose de-prefixed source lines don't abut
/// contiguously). Parses into a scratch [`Ast`], then copies the chain
/// in, remapping indices and `into_owned`-ing the Cows.
fn parse_spans_owned<'a>(ast: &mut Ast<'a>, joined: &str) -> Result<Option<u32>, Error> {
    let mut scratch = Ast::new();
    let head = parse_spans(&mut scratch, joined)?;
    Ok(copy_spans_owned(ast, &scratch, head))
}

/// Copy the span chain starting at `head` from `scratch` into `dst`,
/// making every text `Cow` owned (`'static`) so it no longer borrows
/// `scratch`'s backing text. Returns the head index in `dst`.
fn copy_spans_owned<'a>(dst: &mut Ast<'a>, scratch: &Ast<'_>, head: Option<u32>) -> Option<u32> {
    let mut chain = Chain::new();
    let mut cur = head;
    while let Some(idx) = cur {
        let node = &scratch.spans[idx as usize];
        let kind = match &node.kind {
            SpanKind::Text(t) => SpanKind::Text(Cow::Owned(t.clone().into_owned())),
            SpanKind::Code(t) => SpanKind::Code(Cow::Owned(t.clone().into_owned())),
            SpanKind::Em(inner) => SpanKind::Em(copy_spans_owned(dst, scratch, *inner)),
            SpanKind::Strong(inner) => SpanKind::Strong(copy_spans_owned(dst, scratch, *inner)),
            SpanKind::Link { spans, href } => SpanKind::Link {
                spans: copy_spans_owned(dst, scratch, *spans),
                href: Cow::Owned(href.clone().into_owned()),
            },
        };
        let new_idx = dst.push_span(kind);
        chain.link(new_idx, |p, n| dst.spans[p as usize].next = Some(n));
        cur = node.next;
    }
    chain.first()
}

/// Constructs we recognize well enough to refuse: kramdown features
/// outside the subset whose silent mis-parse would corrupt output.
fn decline_block_scan(line: &str) -> Result<(), Error> {
    if line.as_bytes().starts_with(b"    ") || line.as_bytes().first() == Some(&b'\t') {
        return Err(declined("indented-code"));
    }
    let t = trim_start_ws(line);
    // kramdown starts a table on lines containing an unescaped `|`.
    // Scan bytes (no UTF-8 decode): `\` and `|` are ASCII and a
    // multibyte char's bytes are all >= 0x80, so each just resets `esc`
    // — byte-identical to the per-char escape toggle. Most lines have no
    // `|` at all, so a single tight `contains` skips the escape loop.
    let tb = t.as_bytes();
    if crate::scan::memchr1(tb, b'|').is_some() {
        let mut esc = false;
        for &b in tb {
            match b {
                b'\\' => esc = !esc,
                b'|' if !esc => return Err(declined("table")),
                _ => esc = false,
            }
        }
    }
    if t.starts_with("{:") || t.starts_with("{::") {
        return Err(declined("ald-ial-extension"));
    }
    if t.starts_with("[^") {
        return Err(declined("footnote"));
    }
    if t.starts_with("*[") {
        return Err(declined("abbreviation"));
    }
    if t.starts_with("$$") {
        return Err(declined("math"));
    }
    if t == "^" {
        return Err(declined("eob-marker"));
    }
    if t.starts_with(": ") || t == ":" {
        return Err(declined("definition-list"));
    }
    // Link definitions `[id]: url`.
    if t.starts_with('[')
        && let Some(close) = t.find(']')
        && t[close + 1..].starts_with(':')
    {
        return Err(declined("link-definition"));
    }
    // Raw HTML blocks (a line opening with a tag).
    let bytes = t.as_bytes();
    if bytes.first() == Some(&b'<')
        && bytes
            .get(1)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/' || *c == b'!' || *c == b'?')
    {
        return Err(declined("html-block"));
    }
    Ok(())
}

/// Block openers kramdown accepts behind 1–3 leading spaces
/// (OPT_SPACE). Fences are column-0 in kramdown, so declining the
/// indented form is conservative-safe.
fn opt_space_opener(line: &str, opts: &Options) -> bool {
    let n = line.len() - line.trim_start_matches(' ').len();
    if !(1..=3).contains(&n) {
        return false;
    }
    let s = &line[n..];
    s.starts_with('#')
        || s.starts_with('>')
        || list_marker(s).is_some()
        || (opts.gfm && s.starts_with("```"))
        || s.starts_with("~~~")
}

/// kramdown hard-break semantics live in interior paragraph line
/// endings: 2+ trailing spaces (or a trailing backslash) emit
/// `<br />`. Out of subset — decline rather than silently drop the
/// break. Called with the just-completed interior line (the one whose
/// line ending is about to become a `\n` join).
fn decline_eol(last: &str) -> Result<(), Error> {
    let stripped = last.trim_end_matches(' ');
    if last.len() - stripped.len() >= 2 {
        return Err(declined("hard-break"));
    }
    if stripped.ends_with('\\') {
        return Err(declined("eol-backslash"));
    }
    if stripped.ends_with('\t') {
        return Err(declined("eol-tab"));
    }
    Ok(())
}

fn is_hr(line: &str) -> bool {
    // An HR is one marker char (`-`/`*`/`_`) repeated >=3, plus spaces/
    // tabs — so the first non-space char fixes the only possible marker.
    // For prose (first char a letter) this bails on byte one, instead of
    // the old three full `chars()` scans (one per candidate marker).
    let t = trim_ws(line).as_bytes();
    let marker = match t.first() {
        Some(&c @ (b'-' | b'*' | b'_')) => c,
        _ => return false,
    };
    let mut count = 0usize;
    for &b in t {
        if b == marker {
            count += 1;
        } else if b != b' ' && b != b'\t' {
            return false;
        }
    }
    count >= 3
}

/// `Some(ordered?)` when the line opens a list item.
/// Collect the items of one (tight) list level starting at
/// `lines[*i]`. Shares the old inline loop's decline rules; the
/// marker-indented tail of an item (stripped by exactly the
/// unordered content column, 2) parses as lazy-continuation text
/// followed by at most one nested child list — which recurses
/// through this same fn, so deeper nesting works and a deeper
/// continuation line attaches to the DEEPEST open item
/// (kramdown's behaviour, probed: `- a` / `  - b` / `    cont`
/// joins `cont` onto b).
fn parse_list_items<'a>(
    ast: &mut Ast<'a>,
    lines: &[&'a str],
    i: &mut usize,
    ordered: bool,
) -> Result<Option<u32>, Error> {
    // The items of THIS level, sibling-linked.
    let mut items = Chain::new();
    // The currently-open item: its index in the arena and a span Chain so
    // lazy continuations can append to its span run after it was pushed.
    // `None` until the first item opens.
    let mut cur_item: Option<u32> = None;
    let mut cur_spans = Chain::new();
    let mut cur_has_child = false;
    while *i < lines.len() {
        let l = lines[*i];
        if is_blank(l) {
            // Blank: list ends here if followed by a non-item; a
            // following same-level item would make the list LOOSE.
            let mut j = *i;
            while j < lines.len() && is_blank(lines[j]) {
                j += 1;
            }
            if j < lines.len() && list_marker(lines[j]) == Some(ordered) {
                return Err(declined("loose-list"));
            }
            break;
        }
        if list_marker(l) == Some(ordered) {
            let content = strip_marker(l, ordered);
            // Item content is block-level in kramdown — tables,
            // EOB markers, IALs etc. inside an item are out of
            // subset, same as at the top level.
            decline_block_scan(content)?;
            // Trailing whitespace carries hard-break semantics.
            if trim_end_ws(content) != content {
                return Err(declined("list-trailing-ws"));
            }
            // Flush the previous item's span tail back into its node
            // before opening the next item.
            if let Some(prev) = cur_item {
                ast.items[prev as usize].spans = cur_spans.first();
            }
            let mut spans = Chain::new();
            let parsed = parse_spans(ast, content)?;
            chain_extend_spans(ast, &mut spans, parsed);
            let item_idx = ast.push_item(ItemNode {
                spans: spans.first(),
                child: None,
                next: None,
                _marker: std::marker::PhantomData,
            });
            items.link(item_idx, |p, n| ast.items[p as usize].next = Some(n));
            cur_item = Some(item_idx);
            cur_spans = spans;
            cur_has_child = false;
            *i += 1;
            // Marker-indented tail block (>= 2 spaces): strip the
            // content column and attach to THIS item. Ordered
            // parents decline (content column != 2). Tabs decline.
            let mut tail: Vec<&str> = Vec::new();
            while *i < lines.len()
                && !is_blank(lines[*i])
                && (lines[*i].starts_with("  ") || lines[*i].starts_with('\t'))
            {
                if lines[*i].starts_with('\t') {
                    return Err(declined("list-tab-indent"));
                }
                if ordered {
                    return Err(declined("list-continuation"));
                }
                tail.push(&lines[*i][2..]);
                *i += 1;
            }
            if !tail.is_empty() {
                let mut j = 0usize;
                // Leading non-marker lines: lazy continuations of
                // this item (kramdown joins them verbatim with a
                // newline, indentation stripped).
                while j < tail.len() && list_marker(tail[j]).is_none() {
                    let cont = tail[j];
                    decline_block_scan(cont)?;
                    if trim_end_ws(cont) != cont || cont.starts_with(' ') || cont.starts_with('\t') {
                        return Err(declined("list-continuation-ws"));
                    }
                    let nl = ast.push_span(SpanKind::Text(Cow::Borrowed("\n")));
                    cur_spans.link(nl, |p, n| ast.spans[p as usize].next = Some(n));
                    let parsed = parse_spans(ast, cont)?;
                    chain_extend_spans(ast, &mut cur_spans, parsed);
                    j += 1;
                }
                let child = if j < tail.len() {
                    // Child list: recurse over the rest of the
                    // stripped tail. Deeper-indented lines inside
                    // recurse again; a trailing stripped non-marker
                    // line is the child's own lazy continuation.
                    let child_ordered = list_marker(tail[j])
                        .expect("loop exit condition");
                    let mut k = j;
                    let child_items = parse_list_items(ast, &tail, &mut k, child_ordered)?;
                    if k < tail.len() {
                        // Content after the child list inside the
                        // same item (blank-separated etc.) — out of
                        // subset.
                        return Err(declined("list-after-child"));
                    }
                    Some((child_ordered, child_items))
                } else {
                    None
                };
                // Persist the (possibly extended) span tail and child.
                let item = cur_item.expect("just pushed");
                ast.items[item as usize].spans = cur_spans.first();
                ast.items[item as usize].child = child;
                cur_has_child = child.is_some();
            }
        } else if l.starts_with(' ') {
            // Sub-2-space indent (1 space): kramdown treats a
            // 1-space marker as a SAME-level item; conservatively
            // decline the whole family.
            return Err(declined("list-continuation"));
        } else if list_marker(l).is_some() {
            return Err(declined("mixed-list-markers"));
        } else {
            // Lazy continuation line appended to the last item.
            decline_block_scan(l)?;
            if trim_end_ws(l) != l {
                return Err(declined("list-continuation-ws"));
            }
            match cur_item {
                Some(item) => {
                    if cur_has_child {
                        // Column-0 text after a nested child would
                        // join the PARENT item in kramdown — out of
                        // our emit shape.
                        return Err(declined("list-after-child"));
                    }
                    let nl = ast.push_span(SpanKind::Text(Cow::Borrowed("\n")));
                    cur_spans.link(nl, |p, n| ast.spans[p as usize].next = Some(n));
                    let parsed = parse_spans(ast, l)?;
                    chain_extend_spans(ast, &mut cur_spans, parsed);
                    ast.items[item as usize].spans = cur_spans.first();
                    *i += 1;
                }
                None => break,
            }
        }
    }
    // Persist the final open item's span tail.
    if let Some(prev) = cur_item {
        ast.items[prev as usize].spans = cur_spans.first();
    }
    Ok(items.first())
}

/// Append an already-built span chain (head index `head`, or `None`) to
/// `chain`, linking its head onto the current tail and advancing the
/// chain's `last` to the appended chain's tail so subsequent links extend
/// past it. Mirrors `Vec<Span>::extend` for the sibling-linked arena.
fn chain_extend_spans(ast: &mut Ast<'_>, chain: &mut Chain, head: Option<u32>) {
    let Some(head) = head else { return };
    chain.link(head, |p, n| ast.spans[p as usize].next = Some(n));
    // Walk to the appended chain's tail and make it the new chain tail.
    let mut tail = head;
    while let Some(n) = ast.spans[tail as usize].next {
        tail = n;
    }
    chain.last = Some(tail);
}

fn list_marker(line: &str) -> Option<bool> {
    let b = line.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'*' | b'+' | b'-') && (b[1] == b' ' || b[1] == b'\t') {
        return Some(false);
    }
    let digits = line.bytes().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && b.len() > digits + 1
        && b[digits] == b'.'
        && (b[digits + 1] == b' ' || b[digits + 1] == b'\t')
    {
        return Some(true);
    }
    None
}

fn strip_marker(line: &str, ordered: bool) -> &str {
    if ordered {
        let digits = line.bytes().take_while(|c| c.is_ascii_digit()).count();
        line[digits + 1..].trim_start_matches([' ', '\t'])
    } else {
        line[1..].trim_start_matches([' ', '\t'])
    }
}

// ---- span parsing -------------------------------------------------------

/// Span element kinds — kramdown blocks same-type nesting (an `em`
/// anywhere inside an `em` stays literal) and gates the strong→em
/// retry on the immediate parent.
#[derive(Clone, Copy, PartialEq)]
enum Elem {
    Em,
    Strong,
    Link,
}

/// The emphasis close being searched for by a `parse_spans_until`
/// invocation (kramdown's `stop_re` + its acceptance conditions). The
/// delimiter is always `delim_len` (1 or 2) copies of `type_char`, so we
/// carry just those two bytes — no heap-allocated delimiter `String`
/// (one per emphasis attempt; emphasis is common in prose).
struct Stop {
    type_char: u8,
    delim_len: usize,
    elem: Elem,
}

impl Stop {
    /// Whether `s` opens with this stop's delimiter run (`delim_len`
    /// copies of `type_char`). Equivalent to the old
    /// `s.starts_with(stop.delim)`.
    #[inline]
    fn matches_at(&self, s: &str) -> bool {
        let b = s.as_bytes();
        b.len() >= self.delim_len && b[..self.delim_len].iter().all(|&x| x == self.type_char)
    }
}

/// Ruby `/\s/` is ASCII-only — `char::is_whitespace` would also match
/// U+00A0 etc. and silently diverge.
fn ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// Accumulates the running `Text` span of `parse_spans_until`. While the
/// output bytes are byte-identical to the source, it tracks just the
/// `[seg_start, seg_end)` byte range of `text` and emits a borrowed
/// `Cow::Borrowed` slice on `flush` — no allocation. The first time a
/// typography rewrite (`'`→`'`, `--`→`–`, `...`→`…`) or a backslash
/// escape changes a char, it copies the pristine range collected so far
/// into an owned `String` and switches to `push`-ing into it.
///
/// SYNERGY with `next_trigger`: the inline parser's `_` arm scans a run
/// of ordinary bytes with `next_trigger`; that run is a pristine `text`
/// slice, so it's recorded as a borrow extent (`push_verbatim`) rather
/// than copied — the whole point of the zero-copy AST.
struct TextRun<'a> {
    text: &'a str,
    seg_start: usize,
    seg_end: usize,
    owned: Option<String>,
}

impl<'a> TextRun<'a> {
    #[inline]
    fn new(text: &'a str) -> Self {
        TextRun { text, seg_start: 0, seg_end: 0, owned: None }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        match &self.owned {
            Some(s) => s.is_empty(),
            None => self.seg_start == self.seg_end,
        }
    }

    /// Begin a fresh segment at `pos` if the accumulator is currently
    /// empty (i.e. right after a `flush`, before any byte of the next
    /// segment has been recorded). A no-op once a segment is in progress.
    #[inline]
    fn restart_if_empty(&mut self, pos: usize) {
        if self.owned.is_none() && self.seg_start == self.seg_end {
            self.seg_start = pos;
            self.seg_end = pos;
        }
    }

    /// Record verbatim source bytes `text[a..b]` — identical input and
    /// output, so they extend the borrowed segment (or, once owned, get
    /// copied in bulk). `a` must abut the current segment end.
    #[inline]
    fn push_verbatim(&mut self, a: usize, b: usize) {
        self.restart_if_empty(a);
        debug_assert_eq!(a, self.seg_end, "non-contiguous verbatim run");
        if let Some(owned) = &mut self.owned {
            owned.push_str(&self.text[a..b]);
        }
        self.seg_end = b;
    }

    /// Record a single verbatim source byte at `i` (an ASCII trigger
    /// char emitted literally — its guarded arm didn't fire). Same as a
    /// 1-byte verbatim run.
    #[inline]
    fn push_byte(&mut self, i: usize) {
        self.push_verbatim(i, i + 1);
    }

    /// Record a rewritten char `ch` that replaces source `text[a..b]`
    /// (typography / escape). This breaks the borrow: materialize the
    /// pristine prefix once, then push `ch`. `a` must abut the segment.
    #[inline]
    fn push_char(&mut self, ch: char, a: usize, b: usize) {
        self.restart_if_empty(a);
        debug_assert_eq!(a, self.seg_end, "non-contiguous rewrite");
        let owned = self.owned.get_or_insert_with(|| {
            // Copy the verbatim prefix collected so far, then diverge.
            self.text[self.seg_start..self.seg_end].to_owned()
        });
        owned.push(ch);
        self.seg_end = b;
    }

    /// Emit the accumulated `Text` span (if any) into the arena, linking
    /// it onto `chain`, then reset to empty.
    #[inline]
    fn flush(&mut self, ast: &mut Ast<'a>, chain: &mut Chain) {
        match self.owned.take() {
            Some(owned) => {
                if !owned.is_empty() {
                    let idx = ast.push_span(SpanKind::Text(Cow::Owned(owned)));
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                }
            }
            None => {
                if self.seg_start < self.seg_end {
                    let idx = ast.push_span(SpanKind::Text(Cow::Borrowed(
                        &self.text[self.seg_start..self.seg_end],
                    )));
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                }
            }
        }
        // Collapse to empty; the next push restarts at its own position.
        self.seg_start = self.seg_end;
    }
}

pub(crate) fn parse_spans<'a>(ast: &mut Ast<'a>, text: &'a str) -> Result<Option<u32>, Error> {
    let (head, _) = parse_spans_until(ast, text, None, false, false, None)?;
    Ok(head)
}

/// Recursive-descent span parser mirroring kramdown's `parse_spans` +
/// `parse_emphasis`: scans `text`, optionally watching for an emphasis
/// `stop` delimiter. Returns the spans plus `Some(pos)` where the
/// accepted close begins, or `None` if the text ran out (the caller
/// then reverts to literal delimiters, like kramdown's `revert_pos`).
fn parse_spans_until<'a>(
    ast: &mut Ast<'a>,
    text: &'a str,
    stop: Option<&Stop>,
    in_em: bool,
    in_strong: bool,
    parent: Option<Elem>,
) -> Result<(Option<u32>, Option<usize>), Error> {
    // The span run this call produces, sibling-linked in `ast.spans`.
    let mut chain = Chain::new();
    // Borrowing text accumulator (replaces a `String` buf): a run of
    // verbatim bytes is emitted as `Cow::Borrowed`; only a rewrite forces
    // an owned `String`.
    let mut acc = TextRun::new(text);
    let bytes = text.as_bytes();
    let mut i = 0;
    // Last logical character, across span boundaries — smart-quote
    // open/close classification and emphasis-close pre-checks need it
    // (kramdown sees the raw source via pre_match).
    let mut prev: Option<char> = None;
    while i < bytes.len() {
        // Emphasis close? kramdown checks the stop_re before running
        // span parsers, with these acceptance conditions; a rejected
        // candidate falls through to normal parsing (where it may OPEN
        // a nested span of a different type).
        if let Some(stop) = stop
            && stop.matches_at(&text[i..])
        {
            let content_nonempty = chain.first().is_some() || !acc.is_empty();
            let prev_ok = prev.is_some_and(|c| !ruby_space(c));
            // An em close can't sit on a clean strong delimiter
            // (`**` not followed by a third `*`) — that position
            // belongs to a nested strong.
            let em_ok = stop.elem != Elem::Em || run_len(bytes, i, stop.type_char) != 2;
            // `_` closes don't bind into a following word.
            let underscore_ok = stop.type_char != b'_'
                || !text[i + stop.delim_len..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            if content_nonempty && prev_ok && em_ok && underscore_ok {
                acc.flush(ast, &mut chain);
                return Ok((chain.first(), Some(i)));
            }
        }
        let c = bytes[i];
        match c {
            b'\\' if i + 1 < bytes.len() => {
                let next = bytes[i + 1] as char;
                // kramdown's exact ESCAPED_CHARS set; anything else
                // keeps the backslash literally.
                if "\\.*_+`<>()[]{}#!:|\"'$=-".contains(next) {
                    // The escape drops the `\`, so the output (`next`)
                    // differs from the 2 source bytes — a rewrite.
                    acc.push_char(next, i, i + 2);
                    prev = Some(next);
                    i += 2;
                } else {
                    // Lone `\` kept verbatim.
                    acc.push_byte(i);
                    prev = Some('\\');
                    i += 1;
                }
            }
            b'`' => {
                // Code span with N-backtick delimiter.
                let open = run_len(bytes, i, b'`');
                let delim = &text[i..i + open];
                let rest = &text[i + open..];
                // kramdown: a SINGLE backtick surrounded by whitespace
                // (or start of text) is a literal backtick, not a span.
                if open == 1
                    && prev.is_none_or(char::is_whitespace)
                    && rest.chars().next().is_some_and(char::is_whitespace)
                {
                    acc.push_byte(i); // literal backtick, verbatim
                    prev = Some('`');
                    i += 1;
                    continue;
                }
                // No closing delimiter: kramdown resets and emits the
                // backticks as literal text (verbatim source).
                let Some(close_rel) = rest.find(delim) else {
                    acc.push_verbatim(i, i + open);
                    prev = Some('`');
                    i += open;
                    continue;
                };
                acc.flush(ast, &mut chain);
                // kramdown trims one leading and one trailing space —
                // independently — for multi-backtick delimiters only.
                // `inner` stays a sub-slice of `text`, so it borrows.
                let mut inner = &rest[..close_rel];
                if open > 1 {
                    inner = inner.strip_prefix(' ').unwrap_or(inner);
                    inner = inner.strip_suffix(' ').unwrap_or(inner);
                }
                let idx = ast.push_span(SpanKind::Code(Cow::Borrowed(inner)));
                chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                prev = Some('`');
                i += open + close_rel + open;
            }
            b'*' | b'_' => {
                // kramdown EMPHASIS_START takes at most two delimiter
                // chars; a longer run leaves the rest as content.
                let take = run_len(bytes, i, c).min(2);
                // Intra-word underscore bail:
                // pre_match =~ /[[:alpha:]]-?[[:alpha:]]*_*\z/.
                if c == b'_' && underscore_intraword(&text[..i], prev) {
                    acc.push_verbatim(i, i + take); // verbatim `_`(s)
                    prev = Some('_');
                    i += take;
                    continue;
                }
                let elem = if take == 2 { Elem::Strong } else { Elem::Em };
                let same_type = (elem == Elem::Em && in_em) || (elem == Elem::Strong && in_strong);
                let opens_on_space = text[i + take..].chars().next().is_some_and(ruby_space);
                if same_type || opens_on_space {
                    acc.push_verbatim(i, i + take); // literal delimiter run
                    prev = Some(c as char);
                    i += take;
                    continue;
                }
                let attempt = Stop {
                    type_char: c,
                    delim_len: take,
                    elem,
                };
                // BACKTRACK: record the arena length before the speculative
                // recursion so abandoned inner nodes can be reverted.
                let saved = ast.spans.len();
                let (inner, close) = parse_spans_until(
                    ast,
                    &text[i + take..],
                    Some(&attempt),
                    in_em || elem == Elem::Em,
                    in_strong || elem == Elem::Strong,
                    Some(elem),
                )?;
                if let Some(close) = close {
                    acc.flush(ast, &mut chain);
                    let idx = ast.push_span(if take == 2 {
                        SpanKind::Strong(inner)
                    } else {
                        SpanKind::Em(inner)
                    });
                    chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                    prev = Some(c as char);
                    i += take + close + take;
                    continue;
                }
                // Reverted: drop the speculative inner nodes so they don't
                // leak into the arena or the output.
                ast.spans.truncate(saved);
                // Unclosed strong retries from pos+1 as a single-char
                // em, unless the immediate parent is an em.
                if elem == Elem::Strong && parent != Some(Elem::Em) {
                    let retry = Stop {
                        type_char: c,
                        delim_len: 1,
                        elem: Elem::Em,
                    };
                    let saved = ast.spans.len();
                    let (inner, close) = parse_spans_until(
                        ast,
                        &text[i + 1..],
                        Some(&retry),
                        true,
                        in_strong,
                        Some(Elem::Em),
                    )?;
                    if let Some(close) = close {
                        acc.flush(ast, &mut chain);
                        let idx = ast.push_span(SpanKind::Em(inner));
                        chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                        prev = Some(c as char);
                        i += 1 + close + 1;
                        continue;
                    }
                    // Reverted again: drop the retry's speculative nodes.
                    ast.spans.truncate(saved);
                }
                // No close anywhere: kramdown reverts and emits the
                // delimiter run as literal text (verbatim source).
                acc.push_verbatim(i, i + take);
                prev = Some(c as char);
                i += take;
            }
            b'[' => {
                acc.flush(ast, &mut chain);
                let rest = &text[i..];
                let Some((spans, href, len)) = parse_link(ast, rest, in_em, in_strong)? else {
                    return Err(declined("bracket-not-link"));
                };
                let idx = ast.push_span(SpanKind::Link { spans, href });
                chain.link(idx, |p, n| ast.spans[p as usize].next = Some(n));
                prev = Some(')');
                i += len;
            }
            b'!' if bytes.get(i + 1) == Some(&b'[') => {
                return Err(declined("image"));
            }
            b'<' => {
                // Autolinks / inline HTML are out of subset; a bare `<`
                // followed by space/punct is plain text.
                let next = bytes.get(i + 1).copied();
                if next.is_some_and(|c| c.is_ascii_alphabetic() || c == b'/' || c == b'!') {
                    return Err(declined("inline-html-or-autolink"));
                }
                if next == Some(b'<') {
                    // kramdown typography turns `<<`/`>>` into guillemets.
                    return Err(declined("guillemets"));
                }
                acc.push_byte(i); // literal `<`, verbatim
                prev = Some('<');
                i += 1;
            }
            b'>' if bytes.get(i + 1) == Some(&b'>') => {
                return Err(declined("guillemets"));
            }
            b'&' => {
                // Entity references are parsed by kramdown; bare `&` is
                // escaped. Treat `&word;` / `&#…;` as out of subset.
                let rest = &text[i + 1..];
                let semi = rest.find(';');
                if let Some(s) = semi
                    && s <= 8
                    && rest[..s]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '#')
                    && s > 0
                {
                    return Err(declined("entity"));
                }
                acc.push_byte(i); // literal `&`, verbatim
                prev = Some('&');
                i += 1;
            }
            b'~' if bytes.get(i + 1) == Some(&b'~') => {
                return Err(declined("strikethrough"));
            }
            b'{' if bytes.get(i + 1) == Some(&b':') => {
                // Span IALs `{: …}` and extensions `{::comment}` etc.
                return Err(declined("ial-or-extension"));
            }
            b'\'' | b'"' => {
                if run_len(bytes, i, c) > 1 {
                    return Err(declined("quote-run"));
                }
                let next = text[i + 1..].chars().next();
                let q = if c == b'\'' {
                    typography::single_quote(prev, next)?
                } else {
                    typography::double_quote(prev, next)?
                };
                // Smart quote: the emitted char (U+2018..U+201D) always
                // differs from the ASCII source byte — a rewrite.
                acc.push_char(q, i, i + 1);
                prev = Some(q);
                i += 1;
            }
            b'-' => {
                let dash_run = run_len(bytes, i, c);
                let sym = match dash_run {
                    1 => '-',
                    2 => typography::NDASH,
                    3 => typography::MDASH,
                    _ => return Err(declined("dash-run")),
                };
                if dash_run == 1 {
                    // A lone `-` is itself — keep the borrow.
                    acc.push_byte(i);
                } else {
                    // `--`/`---` collapse to a single en/em dash: rewrite.
                    acc.push_char(sym, i, i + dash_run);
                }
                prev = Some(sym);
                i += dash_run;
            }
            b'.' => {
                let dot_run = run_len(bytes, i, c);
                match dot_run {
                    1 | 2 => {
                        // `.`/`..` are themselves — verbatim source.
                        acc.push_verbatim(i, i + dot_run);
                        prev = Some('.');
                    }
                    3 => {
                        // `...` → `…`: rewrite.
                        acc.push_char(typography::HELLIP, i, i + 3);
                        prev = Some(typography::HELLIP);
                    }
                    _ => return Err(declined("ellipsis-run")),
                }
                i += dot_run;
            }
            _ if is_trigger(c) => {
                // A trigger byte whose guarded arm didn't fire (e.g. `!`
                // not before `[`, `>` not before `>`, a trailing `\`).
                // All triggers are ASCII, so this is the whole char,
                // emitted verbatim.
                acc.push_byte(i);
                prev = Some(c as char);
                i += 1;
            }
            _ => {
                // SYNERGY: scan a run of ordinary bytes with the existing
                // `next_trigger` (scalar TRIGGER table, or NEON byteset
                // under `--features simd`); the run is a pristine slice
                // of `text`, so it's recorded as a BORROW extent instead
                // of being copied. Triggers are ASCII so the run never
                // splits a multibyte char, and stop delimiters start with
                // `*`/`_` (triggers), so a run never skips a pending
                // emphasis close.
                let start = i;
                // bytes[i] is non-trigger (this arm); find the next one.
                i = match next_trigger(&bytes[i + 1..]) {
                    Some(off) => i + 1 + off,
                    None => bytes.len(),
                };
                acc.push_verbatim(start, i);
                prev = text[start..i].chars().next_back();
            }
        }
    }
    acc.flush(ast, &mut chain);
    Ok((chain.first(), None))
}

/// kramdown's intra-word underscore bail:
/// `pre_match =~ /[[:alpha:]]-?[[:alpha:]]*_*\z/`. `pre` is the local
/// slice before the delimiter; at a recursion boundary (`pre` empty)
/// the cross-span `prev` char approximates the lookback.
fn underscore_intraword(pre: &str, prev: Option<char>) -> bool {
    if pre.is_empty() {
        return prev.is_some_and(|c| c.is_alphabetic());
    }
    let s = pre.trim_end_matches('_');
    let s2 = s.trim_end_matches(|c: char| c.is_alphabetic());
    if s2.len() < s.len() {
        return true; // …alpha(_*)\z
    }
    if let Some(before_dash) = s2.strip_suffix('-') {
        // …alpha-\z (the optional hyphen with empty trailing alphas)
        return before_dash
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphabetic());
    }
    false
}

/// Bytes that begin a span-parser match arm (markup delimiters,
/// typography triggers, escapes). Everything else is ordinary text and
/// can be bulk-copied. The set MUST stay in sync with the `match c`
/// arms in `parse_spans_until` — e.g. `~`/`{` are here so a run never
/// swallows a `~~`/`{:` that should decline, and they're all ASCII so a
/// run never splits a multibyte char.
/// 256-entry membership table for the trigger bytes. One indexed load
/// per byte in the inline parser's hot "skip ordinary text" loop, vs a
/// chain of compares for 15 scattered values.
static TRIGGER: [bool; 256] = {
    let mut t = [false; 256];
    let mut i = 0;
    let set = b"\\`*_[!<>&~{'\"-.";
    while i < set.len() {
        t[set[i] as usize] = true;
        i += 1;
    }
    t
};

#[inline]
fn is_trigger(c: u8) -> bool {
    TRIGGER[c as usize]
}

/// Index of the first trigger byte in `hay`, or `None`. Scalar (the
/// `TRIGGER` table) by default; under `--features simd` on aarch64 a NEON
/// byteset scans 16 bytes per iteration. The two paths MUST agree — the
/// `next_trigger_matches_scalar` test pins it.
#[inline]
fn next_trigger(hay: &[u8]) -> Option<usize> {
    #[cfg(all(target_arch = "aarch64", feature = "simd"))]
    {
        // SAFETY: bounded 16-byte loads (guarded by `+ 16 <= len`); NEON
        // is baseline on aarch64.
        unsafe { next_trigger_neon(hay) }
    }
    #[cfg(not(all(target_arch = "aarch64", feature = "simd")))]
    {
        hay.iter().position(|&b| TRIGGER[b as usize])
    }
}

// NEON byteset (Langdale's nibble-lookup): a byte `b` is in the trigger
// set iff bit `b>>4` is set in LO_NIB[b & 0xF]. HI_NIB[h] = 1<<h selects
// that bit. High nibbles 8..15 (non-ASCII) map to 0 — never a trigger,
// so multibyte UTF-8 is skipped as ordinary run text.
#[cfg(all(target_arch = "aarch64", feature = "simd"))]
const LO_NIB: [u8; 16] = {
    let mut t = [0u8; 16];
    let set = b"\\`*_[!<>&~{'\"-.";
    let mut i = 0;
    while i < set.len() {
        let b = set[i];
        t[(b & 0x0F) as usize] |= 1u8 << (b >> 4);
        i += 1;
    }
    t
};
#[cfg(all(target_arch = "aarch64", feature = "simd"))]
const HI_NIB: [u8; 16] = {
    let mut t = [0u8; 16];
    let mut h = 0;
    while h < 8 {
        t[h] = 1u8 << h;
        h += 1;
    }
    t
};

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
#[target_feature(enable = "neon")]
unsafe fn next_trigger_neon(hay: &[u8]) -> Option<usize> {
    use core::arch::aarch64::*;
    let lo_tbl = unsafe { vld1q_u8(LO_NIB.as_ptr()) };
    let hi_tbl = unsafe { vld1q_u8(HI_NIB.as_ptr()) };
    let mut i = 0;
    while i + 16 <= hay.len() {
        let v = unsafe { vld1q_u8(hay.as_ptr().add(i)) };
        let lo = vqtbl1q_u8(lo_tbl, vandq_u8(v, vdupq_n_u8(0x0F)));
        let hi = vqtbl1q_u8(hi_tbl, vshrq_n_u8(v, 4));
        // 0xFF in lanes where (lo & hi) != 0, i.e. byte is a trigger.
        let m = vtstq_u8(lo, hi);
        // NEON movemask: shift-narrow to 4 bits per lane → one nibble per
        // input byte in a u64; trailing_zeros/4 is the first match index.
        let narrowed = vshrn_n_u16(vreinterpretq_u16_u8(m), 4);
        let mask = vget_lane_u64(vreinterpret_u64_u8(narrowed), 0);
        if mask != 0 {
            return Some(i + (mask.trailing_zeros() as usize >> 2));
        }
        i += 16;
    }
    while i < hay.len() {
        if TRIGGER[hay[i] as usize] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parsed-tree text the way kramdown-parser-gfm's `update_raw_text`
/// collects it: text and codespan values verbatim (typography already
/// applied in our Text spans), other elements contribute their
/// children's text (so link TEXT counts, the href doesn't). Walks the
/// flat arena from the chain head `spans`.
pub(crate) fn spans_raw_text(ast: &Ast<'_>, spans: Option<u32>, out: &mut String) {
    let mut cur = spans;
    while let Some(idx) = cur {
        let node = &ast.spans[idx as usize];
        match &node.kind {
            SpanKind::Text(t) | SpanKind::Code(t) => out.push_str(t),
            SpanKind::Em(inner) | SpanKind::Strong(inner) | SpanKind::Link { spans: inner, .. } => {
                spans_raw_text(ast, *inner, out);
            }
        }
        cur = node.next;
    }
}

fn run_len(bytes: &[u8], i: usize, c: u8) -> usize {
    bytes[i..].iter().take_while(|b| **b == c).count()
}

/// Parse `[text](href)` at the start of `rest`. Titles, references and
/// nested brackets decline. The enclosing-emphasis flags thread through
/// so same-type nesting stays blocked inside link text (kramdown's
/// `@stack` check spans the link boundary).
#[allow(clippy::type_complexity)]
fn parse_link<'a>(
    ast: &mut Ast<'a>,
    rest: &'a str,
    in_em: bool,
    in_strong: bool,
) -> Result<Option<(Option<u32>, Cow<'a, str>, usize)>, Error> {
    let bytes = rest.as_bytes();
    debug_assert_eq!(bytes[0], b'[');
    let mut depth = 1;
    let mut close = None;
    for (idx, b) in bytes.iter().enumerate().skip(1) {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Err(declined("unclosed-bracket"));
    };
    if bytes.get(close + 1) != Some(&b'(') {
        return Ok(None);
    }
    let after = &rest[close + 2..];
    let Some(paren_rel) = after.find(')') else {
        return Err(declined("unclosed-link-paren"));
    };
    let href = &after[..paren_rel];
    if href.contains(' ') || href.contains('"') {
        return Err(declined("link-title-or-space"));
    }
    let (spans, _) =
        parse_spans_until(ast, &rest[1..close], None, in_em, in_strong, Some(Elem::Link))?;
    // `href` is a sub-slice of `rest` (the link source) — borrow it.
    Ok(Some((spans, Cow::Borrowed(href), close + 2 + paren_rel + 1)))
}

#[cfg(test)]
mod byte_opt_tests {
    //! Unit coverage for the byte-scan rewrites (perf work). The golden
    //! corpus gates byte-identity at the document level; these pin the
    //! individual functions on edge cases the corpus may not exercise —
    //! especially the byte-vs-char hazards (escaped/After-multibyte `|`).
    use super::*;

    fn reason(line: &str) -> Option<&'static str> {
        match decline_block_scan(line) {
            Ok(()) => None,
            Err(Error::Declined(r)) => Some(r),
        }
    }

    #[test]
    fn decline_table_pipe_escape_and_multibyte() {
        assert_eq!(reason("plain prose, nothing special"), None);
        assert_eq!(reason("a | b"), Some("table")); // unescaped pipe
        assert_eq!(reason(r"a \| b"), None); // escaped pipe is NOT a table
        assert_eq!(reason(r"a \\| b"), Some("table")); // \\ then | → unescaped
        // byte scan must stay correct around multibyte chars:
        assert_eq!(reason("café | x"), Some("table"));
        assert_eq!(reason(r"café \| x"), None);
        assert_eq!(reason("naïve prose"), None); // multibyte, no pipe
    }

    #[test]
    fn decline_indented_code_and_prefixes() {
        assert_eq!(reason("    four spaces"), Some("indented-code"));
        assert_eq!(reason("\ttab"), Some("indented-code"));
        assert_eq!(reason("   three spaces ok"), None);
        assert_eq!(reason("{:.css}"), Some("ald-ial-extension"));
        assert_eq!(reason("[^1]: footnote"), Some("footnote"));
        assert_eq!(reason("$$ math $$"), Some("math"));
        assert_eq!(reason("<div>"), Some("html-block"));
        assert_eq!(reason("[id]: http://x"), Some("link-definition"));
    }

    #[test]
    fn is_hr_true_cases() {
        for s in ["---", "***", "___", "----", "- - -", "*  *  *", "  ---  ", "-\t-\t-"] {
            assert!(is_hr(s), "{s:?} should be HR");
        }
    }

    #[test]
    fn is_hr_false_cases() {
        for s in ["--", "**", "hello", "-*-", "- - x", "", "- -", "-x-", "a---", "---x"] {
            assert!(!is_hr(s), "{s:?} should NOT be HR");
        }
    }

    #[test]
    fn split_lines_matches_std_split() {
        for s in [
            "", "a", "a\n", "a\nb", "a\nb\n", "\n", "\n\n", "a\n\nb", "café\nx\n",
            "trailing\nnewline\n", "no newline at all",
        ] {
            let std: Vec<&str> = s.split('\n').collect();
            assert_eq!(split_lines(s), std, "split mismatch for {s:?}");
        }
    }

    #[test]
    fn next_trigger_matches_scalar() {
        // Every value 0..=255 (incl. non-ASCII, which must NOT match) at
        // every length — exercises the NEON path under `--features simd`
        // against the scalar `is_trigger` oracle.
        let bytes: Vec<u8> = (0u8..=255).cycle().take(400).collect();
        for len in 0..bytes.len() {
            let hay = &bytes[..len];
            let oracle = hay.iter().position(|&b| is_trigger(b));
            assert_eq!(next_trigger(hay), oracle, "len={len}");
        }
        for pos in 0..40usize {
            let mut h = vec![b'x'; 40];
            h[pos] = b'*';
            assert_eq!(next_trigger(&h), Some(pos), "pos={pos}");
        }
    }
}
