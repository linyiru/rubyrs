//! Block + span parsing into a small element tree. Anything outside the
//! implemented subset returns `Error::Declined(reason)` — the whole
//! document is parsed before any HTML is emitted, so a decline can never
//! leave partial output.

use crate::{Error, Options, typography};

#[derive(Debug)]
pub(crate) enum Block {
    /// A run of blank lines between blocks (renders as one `\n`).
    Blank,
    /// `raw` is the unparsed heading text — kramdown derives `auto_ids`
    /// slugs from it (markup characters get deleted by the slug rules).
    Heading {
        level: u8,
        raw: String,
        spans: Vec<Span>,
    },
    Para(Vec<Span>),
    /// Tight list (no blank lines inside) — items are span runs.
    List {
        ordered: bool,
        items: Vec<Vec<Span>>,
    },
    Quote(Vec<Block>),
    Code {
        lang: Option<String>,
        text: String,
    },
    Hr,
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
                out.push(Block::Heading {
                    level,
                    raw: text.to_string(),
                    spans: parse_spans(text)?,
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

        // Lists (tight only — a blank line inside, nesting, or
        // multi-paragraph items decline).
        if let Some(ordered) = list_marker(line) {
            let mut items: Vec<Vec<Span>> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    // Blank: list ends here if followed by a non-item;
                    // a following item would make the list LOOSE.
                    let mut j = i;
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
                    // Trailing whitespace carries hard-break semantics.
                    if content.trim_end() != content {
                        return Err(declined("list-trailing-ws"));
                    }
                    items.push(parse_spans(content)?);
                    i += 1;
                } else if l.starts_with("  ") || l.starts_with('\t') {
                    return Err(declined("list-continuation"));
                } else if list_marker(l).is_some() {
                    return Err(declined("mixed-list-markers"));
                } else {
                    // Lazy continuation line appended to the last item.
                    if l.trim_end() != l || l.starts_with(' ') {
                        return Err(declined("list-continuation-ws"));
                    }
                    match items.last_mut() {
                        Some(item) => {
                            item.push(Span::Text("\n".to_string()));
                            item.extend(parse_spans(l)?);
                            i += 1;
                        }
                        None => break,
                    }
                }
            }
            out.push(Block::List { ordered, items });
            continue;
        }

        // kramdown recognizes most block openers behind 1–3 leading
        // spaces (OPT_SPACE); our dispatcher only sees them at column
        // 0, so an indented opener must decline, not become a paragraph.
        if opt_space_opener(line, opts) {
            return Err(declined("opt-space-block"));
        }

        // Paragraph: gather until blank / next block opener. Lines are
        // kept VERBATIM (kramdown preserves interior trailing spaces);
        // only the first line loses its OPT_SPACE indent and the final
        // line is right-stripped.
        let mut text = String::new();
        let mut first = true;
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty()
                || l.starts_with('#')
                || l.starts_with('>')
                || is_hr(l)
                || list_marker(l).is_some()
                || (opts.gfm && l.starts_with("```"))
                || l.starts_with("~~~")
            {
                break;
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

pub(crate) fn parse_spans(text: &str) -> Result<Vec<Span>, Error> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    // Last logical character, across span boundaries — smart-quote
    // open/close classification needs it (kramdown sees the raw source).
    let mut prev: Option<char> = None;
    while i < bytes.len() {
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
                let run = run_len(bytes, i, c);
                if run > 2 {
                    return Err(declined("triple-emphasis"));
                }
                // Intra-word underscores are literal in kramdown.
                if c == b'_' && prev_is_alnum(bytes, i) && next_is_alnum(bytes, i + run) {
                    buf.push('_');
                    if run == 2 {
                        buf.push('_');
                    }
                    prev = Some('_');
                    i += run;
                    continue;
                }
                // Opening delimiter must be followed by non-space.
                let after = bytes.get(i + run).copied();
                if after.is_none() || after == Some(b' ') {
                    // Not an opener: literal.
                    for _ in 0..run {
                        buf.push(c as char);
                    }
                    prev = Some(c as char);
                    i += run;
                    continue;
                }
                flush(&mut out, &mut buf);
                let delim = (c as char).to_string().repeat(run);
                let rest = &text[i + run..];
                let Some(close_rel) = find_emph_close(rest, &delim) else {
                    return Err(declined("unbalanced-emphasis"));
                };
                let inner = parse_spans(&rest[..close_rel])?;
                out.push(if run == 2 {
                    Span::Strong(inner)
                } else {
                    Span::Em(inner)
                });
                prev = Some(c as char);
                i += run + close_rel + run;
            }
            b'[' => {
                flush(&mut out, &mut buf);
                let rest = &text[i..];
                let Some((spans, href, len)) = parse_link(rest)? else {
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
    Ok(out)
}

fn flush(out: &mut Vec<Span>, buf: &mut String) {
    if !buf.is_empty() {
        out.push(Span::Text(std::mem::take(buf)));
    }
}

fn run_len(bytes: &[u8], i: usize, c: u8) -> usize {
    bytes[i..].iter().take_while(|b| **b == c).count()
}

fn prev_is_alnum(bytes: &[u8], i: usize) -> bool {
    i > 0 && (bytes[i - 1].is_ascii_alphanumeric())
}

fn next_is_alnum(bytes: &[u8], i: usize) -> bool {
    bytes.get(i).is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Find the closing delimiter for an emphasis run: same delimiter,
/// preceded by non-space, not part of a longer run.
fn find_emph_close(rest: &str, delim: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(pos) = rest[from..].find(delim) {
        let abs = from + pos;
        let before_ok = abs > 0 && !rest[..abs].ends_with(' ');
        let after = rest[abs + delim.len()..].chars().next();
        let not_longer_run = after != Some(delim.chars().next().expect("non-empty delim"));
        if before_ok && not_longer_run && abs > 0 {
            return Some(abs);
        }
        from = abs + delim.len();
    }
    None
}

/// Parse `[text](href)` at the start of `rest`. Titles, references and
/// nested brackets decline.
#[allow(clippy::type_complexity)]
fn parse_link(rest: &str) -> Result<Option<(Vec<Span>, String, usize)>, Error> {
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
    let spans = parse_spans(&rest[1..close])?;
    Ok(Some((spans, href.to_string(), close + 2 + paren_rel + 1)))
}
