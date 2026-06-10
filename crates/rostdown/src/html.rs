//! HTML conversion, byte-faithful to kramdown's HTML converter for the
//! supported subset: one `"\n"` per blank-run between blocks, child
//! blocks indented by 2, `auto_ids` slugs with kramdown's exact rules,
//! `<pre><code class="language-…">` for plain code blocks and the
//! `<div class="language-… highlighter-…">` wrapper for highlighted ones.

use std::collections::HashMap;

use crate::parse::{Block, Span};
use crate::{CodeHighlighter, Options};

pub(crate) fn convert(blocks: &[Block], opts: &Options, hl: &mut dyn CodeHighlighter) -> String {
    let mut out = String::new();
    let mut used_ids: HashMap<String, u32> = HashMap::new();
    convert_blocks(&mut out, blocks, 0, opts, hl, &mut used_ids);
    out
}

fn convert_blocks(
    out: &mut String,
    blocks: &[Block],
    indent: usize,
    opts: &Options,
    hl: &mut dyn CodeHighlighter,
    used_ids: &mut HashMap<String, u32>,
) {
    let pad = " ".repeat(indent);
    for block in blocks {
        match block {
            // kramdown emits a bare "\n" per blank-run, no indent.
            Block::Blank => out.push('\n'),
            Block::Heading {
                level,
                raw,
                span_text,
                spans,
            } => {
                out.push_str(&pad);
                if opts.auto_ids {
                    // GFM sets ids at parse time with its own slug
                    // algorithm; core uses the converter's. Parse
                    // validated gfm_slug, so the fallback is inert.
                    let base = if opts.gfm {
                        gfm_slug(span_text).unwrap_or_else(|| basic_generate_id(raw))
                    } else {
                        basic_generate_id(raw)
                    };
                    let id = dedup_id(base, used_ids);
                    out.push_str(&format!("<h{level} id=\"{id}\">"));
                } else {
                    out.push_str(&format!("<h{level}>"));
                }
                convert_spans(out, spans, hl.codespan_class());
                out.push_str(&format!("</h{level}>\n"));
            }
            Block::Para(spans) => {
                out.push_str(&pad);
                out.push_str("<p>");
                convert_spans(out, spans, hl.codespan_class());
                out.push_str("</p>\n");
            }
            Block::List { ordered, items } => {
                let tag = if *ordered { "ol" } else { "ul" };
                out.push_str(&format!("{pad}<{tag}>\n"));
                let item_pad = " ".repeat(indent + 2);
                for item in items {
                    out.push_str(&item_pad);
                    out.push_str("<li>");
                    convert_spans(out, item, hl.codespan_class());
                    out.push_str("</li>\n");
                }
                out.push_str(&format!("{pad}</{tag}>\n"));
            }
            Block::Quote(inner) => {
                out.push_str(&format!("{pad}<blockquote>\n"));
                convert_blocks(out, inner, indent + 2, opts, hl, used_ids);
                out.push_str(&format!("{pad}</blockquote>\n"));
            }
            Block::Code { lang, text } => {
                if let Some(lang) = lang
                    && let Some(hl_html) = hl.highlight(lang, text)
                {
                    // kramdown convert_codeblock with a syntax highlighter.
                    out.push_str(&format!(
                        "{pad}<div class=\"language-{lang} highlighter-{}\">{hl_html}{pad}</div>\n",
                        hl.name()
                    ));
                } else {
                    out.push_str(&pad);
                    out.push_str("<pre><code");
                    if let Some(lang) = lang {
                        out.push_str(&format!(" class=\"language-{lang}\""));
                    }
                    out.push('>');
                    let body_start = out.len();
                    escape_text(out, text);
                    // kramdown: chomp, then exactly one trailing newline.
                    if !out[body_start..].ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("</code></pre>\n");
                }
            }
            Block::Hr => out.push_str(&format!("{pad}<hr />\n")),
        }
    }
}

fn convert_spans(out: &mut String, spans: &[Span], codespan_class: Option<&str>) {
    for span in spans {
        match span {
            Span::Text(t) => escape_text(out, t),
            Span::Em(inner) => {
                out.push_str("<em>");
                convert_spans(out, inner, codespan_class);
                out.push_str("</em>");
            }
            Span::Strong(inner) => {
                out.push_str("<strong>");
                convert_spans(out, inner, codespan_class);
                out.push_str("</strong>");
            }
            Span::Code(code) => {
                match codespan_class {
                    Some(class) => {
                        out.push_str("<code class=\"");
                        out.push_str(class);
                        out.push_str("\">");
                    }
                    None => out.push_str("<code>"),
                }
                escape_text(out, code);
                out.push_str("</code>");
            }
            Span::Link { spans, href } => {
                out.push_str("<a href=\"");
                escape_attr(out, href);
                out.push_str("\">");
                convert_spans(out, spans, codespan_class);
                out.push_str("</a>");
            }
        }
    }
}

/// kramdown `escape_html(…, :text)` — `&`, `<`, `>` only.
fn escape_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// kramdown `escape_html(…, :attribute)` — also escapes `"`.
fn escape_attr(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// kramdown CORE `Converter::Base#basic_generate_id`: strip leading
/// non-ASCII-letters, delete everything outside `[a-zA-Z0-9 -]`,
/// spaces → hyphens, downcase; empty → "section".
fn basic_generate_id(raw: &str) -> String {
    let stripped = raw.trim_start_matches(|c: char| !c.is_ascii_alphabetic());
    let mut id = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '-' => id.push(ch),
            'A'..='Z' => id.push(ch.to_ascii_lowercase()),
            ' ' => id.push('-'),
            _ => {}
        }
    }
    if id.is_empty() {
        id.push_str("section");
    }
    id
}

/// kramdown-parser-gfm `generate_gfm_header_id`: Unicode downcase,
/// delete `[^\p{Word}\- \t]`, then ` `/`\t` → `-` (one hyphen EACH, no
/// collapsing; leading digits are kept, unlike core).
///
/// We reproduce it for: ASCII (where `\p{Word}` is `[A-Za-z0-9_]` and
/// other ASCII gets deleted), the typography characters our parser
/// emits (punctuation classes — non-Word, deleted), and caseless CJK
/// ranges (`\p{Word}`, preserved verbatim). Any other non-ASCII
/// returns `None` and the parser declines the document — Ruby's
/// Unicode word classes and casing can't be safely approximated.
pub(crate) fn gfm_slug(span_text: &str) -> Option<String> {
    let mut id = String::with_capacity(span_text.len());
    for ch in span_text.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '_' | '-' => id.push(ch),
            'A'..='Z' => id.push(ch.to_ascii_lowercase()),
            ' ' | '\t' => id.push('-'),
            c if c.is_ascii() => {} // ASCII punctuation: non-Word, deleted
            // Typography output (smart quotes, dashes, ellipsis).
            '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\u{2013}' | '\u{2014}'
            | '\u{2026}' => {}
            // Caseless \p{Word} ranges passed through verbatim:
            // CJK ideographs, kana, hangul syllables.
            c @ ('\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}') => id.push(c),
            _ => return None,
        }
    }
    if id.is_empty() { None } else { Some(id) }
}

/// Duplicate-id suffixing, shared by both algorithms (kramdown core's
/// `@used_ids` and GFM's `@id_counter` behave identically): first use
/// is bare, repeats get `-1`, `-2`, …
fn dedup_id(id: String, used_ids: &mut HashMap<String, u32>) -> String {
    match used_ids.get_mut(&id) {
        Some(count) => {
            *count += 1;
            format!("{id}-{count}")
        }
        None => {
            used_ids.insert(id.clone(), 0);
            id
        }
    }
}
