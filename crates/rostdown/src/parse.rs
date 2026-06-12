//! Block + span parsing into a small element tree. Anything outside the
//! implemented subset returns `Error::Declined(reason)` — the whole
//! document is parsed before any HTML is emitted, so a decline can never
//! leave partial output.

use crate::{Error, Options, typography};

#[derive(Debug)]
pub(crate) enum Block {
    /// A run of blank lines between blocks (renders as one `\n`).
    Blank,
    /// `raw` is the unparsed heading text — kramdown CORE derives
    /// `auto_ids` slugs from it. `span_text` is the parsed-tree text
    /// (typography applied, link text included, markup gone) — the GFM
    /// parser's `generate_gfm_header_id` input.
    Heading {
        level: u8,
        raw: String,
        span_text: String,
        spans: Vec<Span>,
    },
    Para(Vec<Span>),
    /// Tight list (no blank lines inside) — items are span runs
    /// plus an optional trailing nested child list. Lazy
    /// continuations join the item's spans with a literal newline
    /// (kramdown's verbatim line joining).
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    Quote(Vec<Block>),
    Code {
        lang: Option<String>,
        text: String,
    },
    Hr,
}

#[derive(Debug)]
pub(crate) struct ListItem {
    pub(crate) spans: Vec<Span>,
    /// Trailing nested list: `(ordered, items)`. Tight items carry
    /// at most text-then-one-child in our subset; anything richer
    /// (blank lines, content after the child) declines first.
    pub(crate) child: Option<(bool, Vec<ListItem>)>,
}

#[derive(Debug)]
pub(crate) enum Span {
    /// Raw text (typography + escaping applied at conversion).
    Text(String),
    Em(Vec<Span>),
    Strong(Vec<Span>),
    Code(String),
    Link {
        spans: Vec<Span>,
        href: String,
    },
}

fn declined(what: &'static str) -> Error {
    Error::Declined(what)
}

pub(crate) fn parse(src: &str, opts: &Options) -> Result<Vec<Block>, Error> {
    let lines: Vec<&str> = src.split('\n').collect();
    // A trailing "\n" yields one empty last element — drop it so it
    // doesn't read as a blank line.
    let lines = match lines.last() {
        Some(&"") => &lines[..lines.len() - 1],
        _ => &lines[..],
    };
    parse_blocks(lines, opts)
}

fn parse_blocks(lines: &[&str], opts: &Options) -> Result<Vec<Block>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            // Collapse a blank run into one Blank between blocks.
            while i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }
            out.push(Block::Blank);
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
                let text = text.trim_end();
                let text = text.trim_end_matches('#').trim_end();
                let spans = parse_spans(text)?;
                let mut span_text = String::new();
                spans_raw_text(&spans, &mut span_text);
                // GFM slugs we can't reproduce exactly (Unicode word
                // classes outside our supported set, empty results)
                // decline rather than risk a wrong id.
                if opts.gfm && opts.auto_ids && crate::html::gfm_slug(&span_text).is_none() {
                    return Err(declined("heading-gfm-slug"));
                }
                out.push(Block::Heading {
                    level,
                    raw: text.to_string(),
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
            out.push(Block::Hr);
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
                Some(info.split_whitespace().next().unwrap_or("").to_string())
            };
            let mut body = String::new();
            i += 1;
            let mut closed = false;
            while i < lines.len() {
                let l = lines[i];
                if l.trim_end() == fence {
                    closed = true;
                    i += 1;
                    break;
                }
                body.push_str(l);
                body.push('\n');
                i += 1;
            }
            if !closed {
                return Err(declined("unclosed-fence"));
            }
            out.push(Block::Code { lang, text: body });
            continue;
        }

        // Blockquote: collect `>`-prefixed lines (plus lazy
        // continuations) and recurse.
        if line.starts_with('>') {
            let mut inner: Vec<String> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if let Some(rest) = l.strip_prefix('>') {
                    inner.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                    i += 1;
                } else if !l.trim().is_empty() && !inner.is_empty() {
                    // Lazy continuation of the quoted paragraph.
                    inner.push(l.to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            let inner_refs: Vec<&str> = inner.iter().map(String::as_str).collect();
            let mut blocks = parse_blocks(&inner_refs, opts)?;
            // A quote body never starts/ends with Blank markers.
            while matches!(blocks.first(), Some(Block::Blank)) {
                blocks.remove(0);
            }
            while matches!(blocks.last(), Some(Block::Blank)) {
                blocks.pop();
            }
            out.push(Block::Quote(blocks));
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
            let items = parse_list_items(lines, &mut i, ordered)?;
            out.push(Block::List { ordered, items });
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
        let mut text = String::new();
        let mut first = true;
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() {
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
                let t = next.trim_end();
                if !t.is_empty() && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-')) {
                    return Err(declined("setext-heading"));
                }
            }
            decline_block_scan(l)?;
            if !first {
                // Interior line endings carry hard-break semantics.
                decline_eol(&text)?;
                text.push('\n');
            }
            text.push_str(if first { l.trim_start_matches(' ') } else { l });
            first = false;
            i += 1;
        }
        // Final paragraph line: kramdown right-strips it (trailing
        // spaces there do NOT produce a hard break).
        let text = text.trim_end_matches([' ', '\t']);
        out.push(Block::Para(parse_spans(text)?));
    }
    Ok(out)
}

/// Constructs we recognize well enough to refuse: kramdown features
/// outside the subset whose silent mis-parse would corrupt output.
fn decline_block_scan(line: &str) -> Result<(), Error> {
    let t = line.trim_start();
    if line.starts_with("    ") || line.starts_with('\t') {
        return Err(declined("indented-code"));
    }
    // kramdown starts a table on lines containing an unescaped `|`.
    let mut esc = false;
    for ch in t.chars() {
        match ch {
            '\\' => esc = !esc,
            '|' if !esc => return Err(declined("table")),
            _ => esc = false,
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
/// break. Called with the paragraph text gathered SO FAR, i.e. the
/// just-completed line is interior, not final.
fn decline_eol(text_so_far: &str) -> Result<(), Error> {
    let last = text_so_far.rsplit('\n').next().unwrap_or(text_so_far);
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
    let t = line.trim();
    for marker in ['-', '*', '_'] {
        let count = t.chars().filter(|c| *c == marker).count();
        if count >= 3 && t.chars().all(|c| c == marker || c == ' ' || c == '\t') {
            return true;
        }
    }
    false
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
fn parse_list_items(
    lines: &[&str],
    i: &mut usize,
    ordered: bool,
) -> Result<Vec<ListItem>, Error> {
    let mut items: Vec<ListItem> = Vec::new();
    while *i < lines.len() {
        let l = lines[*i];
        if l.trim().is_empty() {
            // Blank: list ends here if followed by a non-item; a
            // following same-level item would make the list LOOSE.
            let mut j = *i;
            while j < lines.len() && lines[j].trim().is_empty() {
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
            if content.trim_end() != content {
                return Err(declined("list-trailing-ws"));
            }
            items.push(ListItem { spans: parse_spans(content)?, child: None });
            *i += 1;
            // Marker-indented tail block (>= 2 spaces): strip the
            // content column and attach to THIS item. Ordered
            // parents decline (content column != 2). Tabs decline.
            let mut tail: Vec<&str> = Vec::new();
            while *i < lines.len()
                && !lines[*i].trim().is_empty()
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
                let mut extra: Vec<Span> = Vec::new();
                let mut j = 0usize;
                // Leading non-marker lines: lazy continuations of
                // this item (kramdown joins them verbatim with a
                // newline, indentation stripped).
                while j < tail.len() && list_marker(tail[j]).is_none() {
                    let cont = tail[j];
                    decline_block_scan(cont)?;
                    if cont.trim_end() != cont || cont.starts_with(' ') || cont.starts_with('\t') {
                        return Err(declined("list-continuation-ws"));
                    }
                    extra.push(Span::Text("\n".to_string()));
                    extra.extend(parse_spans(cont)?);
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
                    let child_items = parse_list_items(&tail, &mut k, child_ordered)?;
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
                let item = items.last_mut().expect("just pushed");
                item.spans.extend(extra);
                item.child = child;
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
            if l.trim_end() != l {
                return Err(declined("list-continuation-ws"));
            }
            match items.last_mut() {
                Some(item) => {
                    if item.child.is_some() {
                        // Column-0 text after a nested child would
                        // join the PARENT item in kramdown — out of
                        // our emit shape.
                        return Err(declined("list-after-child"));
                    }
                    item.spans.push(Span::Text("\n".to_string()));
                    item.spans.extend(parse_spans(l)?);
                    *i += 1;
                }
                None => break,
            }
        }
    }
    Ok(items)
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
/// invocation (kramdown's `stop_re` + its acceptance conditions).
struct Stop<'a> {
    delim: &'a str,
    type_char: u8,
    elem: Elem,
}

/// Ruby `/\s/` is ASCII-only — `char::is_whitespace` would also match
/// U+00A0 etc. and silently diverge.
fn ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

pub(crate) fn parse_spans(text: &str) -> Result<Vec<Span>, Error> {
    let (spans, _) = parse_spans_until(text, None, false, false, None)?;
    Ok(spans)
}

/// Recursive-descent span parser mirroring kramdown's `parse_spans` +
/// `parse_emphasis`: scans `text`, optionally watching for an emphasis
/// `stop` delimiter. Returns the spans plus `Some(pos)` where the
/// accepted close begins, or `None` if the text ran out (the caller
/// then reverts to literal delimiters, like kramdown's `revert_pos`).
fn parse_spans_until(
    text: &str,
    stop: Option<&Stop<'_>>,
    in_em: bool,
    in_strong: bool,
    parent: Option<Elem>,
) -> Result<(Vec<Span>, Option<usize>), Error> {
    let mut out = Vec::new();
    let mut buf = String::new();
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
            && text[i..].starts_with(stop.delim)
        {
            let content_nonempty = !out.is_empty() || !buf.is_empty();
            let prev_ok = prev.is_some_and(|c| !ruby_space(c));
            // An em close can't sit on a clean strong delimiter
            // (`**` not followed by a third `*`) — that position
            // belongs to a nested strong.
            let em_ok = stop.elem != Elem::Em || run_len(bytes, i, stop.type_char) != 2;
            // `_` closes don't bind into a following word.
            let underscore_ok = stop.type_char != b'_'
                || !text[i + stop.delim.len()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            if content_nonempty && prev_ok && em_ok && underscore_ok {
                flush(&mut out, &mut buf);
                return Ok((out, Some(i)));
            }
        }
        let c = bytes[i];
        match c {
            b'\\' if i + 1 < bytes.len() => {
                let next = bytes[i + 1] as char;
                // kramdown's exact ESCAPED_CHARS set; anything else
                // keeps the backslash literally.
                if "\\.*_+`<>()[]{}#!:|\"'$=-".contains(next) {
                    buf.push(next);
                    prev = Some(next);
                    i += 2;
                } else {
                    buf.push('\\');
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
                    buf.push('`');
                    prev = Some('`');
                    i += 1;
                    continue;
                }
                // No closing delimiter: kramdown resets and emits the
                // backticks as literal text.
                let Some(close_rel) = rest.find(delim) else {
                    for _ in 0..open {
                        buf.push('`');
                    }
                    prev = Some('`');
                    i += open;
                    continue;
                };
                flush(&mut out, &mut buf);
                // kramdown trims one leading and one trailing space —
                // independently — for multi-backtick delimiters only.
                let mut inner = &rest[..close_rel];
                if open > 1 {
                    inner = inner.strip_prefix(' ').unwrap_or(inner);
                    inner = inner.strip_suffix(' ').unwrap_or(inner);
                }
                out.push(Span::Code(inner.to_string()));
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
                    for _ in 0..take {
                        buf.push('_');
                    }
                    prev = Some('_');
                    i += take;
                    continue;
                }
                let elem = if take == 2 { Elem::Strong } else { Elem::Em };
                let same_type = (elem == Elem::Em && in_em) || (elem == Elem::Strong && in_strong);
                let opens_on_space = text[i + take..].chars().next().is_some_and(ruby_space);
                if same_type || opens_on_space {
                    for _ in 0..take {
                        buf.push(c as char);
                    }
                    prev = Some(c as char);
                    i += take;
                    continue;
                }
                let delim_buf = (c as char).to_string().repeat(take);
                let attempt = Stop {
                    delim: &delim_buf,
                    type_char: c,
                    elem,
                };
                let (inner, close) = parse_spans_until(
                    &text[i + take..],
                    Some(&attempt),
                    in_em || elem == Elem::Em,
                    in_strong || elem == Elem::Strong,
                    Some(elem),
                )?;
                if let Some(close) = close {
                    flush(&mut out, &mut buf);
                    out.push(if take == 2 {
                        Span::Strong(inner)
                    } else {
                        Span::Em(inner)
                    });
                    prev = Some(c as char);
                    i += take + close + take;
                    continue;
                }
                // Unclosed strong retries from pos+1 as a single-char
                // em, unless the immediate parent is an em.
                if elem == Elem::Strong && parent != Some(Elem::Em) {
                    let delim1 = (c as char).to_string();
                    let retry = Stop {
                        delim: &delim1,
                        type_char: c,
                        elem: Elem::Em,
                    };
                    let (inner, close) = parse_spans_until(
                        &text[i + 1..],
                        Some(&retry),
                        true,
                        in_strong,
                        Some(Elem::Em),
                    )?;
                    if let Some(close) = close {
                        flush(&mut out, &mut buf);
                        out.push(Span::Em(inner));
                        prev = Some(c as char);
                        i += 1 + close + 1;
                        continue;
                    }
                }
                // No close anywhere: kramdown reverts and emits the
                // delimiter run as literal text.
                for _ in 0..take {
                    buf.push(c as char);
                }
                prev = Some(c as char);
                i += take;
            }
            b'[' => {
                flush(&mut out, &mut buf);
                let rest = &text[i..];
                let Some((spans, href, len)) = parse_link(rest, in_em, in_strong)? else {
                    return Err(declined("bracket-not-link"));
                };
                out.push(Span::Link { spans, href });
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
                buf.push('<');
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
                buf.push('&');
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
                buf.push(q);
                prev = Some(q);
                i += 1;
            }
            b'-' => {
                let run = run_len(bytes, i, c);
                let sym = match run {
                    1 => '-',
                    2 => typography::NDASH,
                    3 => typography::MDASH,
                    _ => return Err(declined("dash-run")),
                };
                buf.push(sym);
                prev = Some(sym);
                i += run;
            }
            b'.' => {
                let run = run_len(bytes, i, c);
                match run {
                    1 | 2 => {
                        for _ in 0..run {
                            buf.push('.');
                        }
                        prev = Some('.');
                    }
                    3 => {
                        buf.push(typography::HELLIP);
                        prev = Some(typography::HELLIP);
                    }
                    _ => return Err(declined("ellipsis-run")),
                }
                i += run;
            }
            _ => {
                let ch = text[i..].chars().next().expect("in-bounds char");
                buf.push(ch);
                prev = Some(ch);
                i += ch.len_utf8();
            }
        }
    }
    flush(&mut out, &mut buf);
    Ok((out, None))
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

fn flush(out: &mut Vec<Span>, buf: &mut String) {
    if !buf.is_empty() {
        out.push(Span::Text(std::mem::take(buf)));
    }
}

/// Parsed-tree text the way kramdown-parser-gfm's `update_raw_text`
/// collects it: text and codespan values verbatim (typography already
/// applied in our Text spans), other elements contribute their
/// children's text (so link TEXT counts, the href doesn't).
pub(crate) fn spans_raw_text(spans: &[Span], out: &mut String) {
    for span in spans {
        match span {
            Span::Text(t) | Span::Code(t) => out.push_str(t),
            Span::Em(inner) | Span::Strong(inner) | Span::Link { spans: inner, .. } => {
                spans_raw_text(inner, out);
            }
        }
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
fn parse_link(
    rest: &str,
    in_em: bool,
    in_strong: bool,
) -> Result<Option<(Vec<Span>, String, usize)>, Error> {
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
    let (spans, _) = parse_spans_until(&rest[1..close], None, in_em, in_strong, Some(Elem::Link))?;
    Ok(Some((spans, href.to_string(), close + 2 + paren_rel + 1)))
}
