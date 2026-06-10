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
            Block::Heading { level, raw, spans } => {
                out.push_str(&pad);
                if opts.auto_ids {
                    let id = generate_id(raw, used_ids);
                    out.push_str(&format!("<h{level} id=\"{id}\">"));
                } else {
                    out.push_str(&format!("<h{level}>"));
                }
                convert_spans(out, spans);
                out.push_str(&format!("</h{level}>\n"));
            }
            Block::Para(spans) => {
                out.push_str(&pad);
                out.push_str("<p>");
                convert_spans(out, spans);
                out.push_str("</p>\n");
            }
            Block::List { ordered, items } => {
                let tag = if *ordered { "ol" } else { "ul" };
                out.push_str(&format!("{pad}<{tag}>\n"));
                let item_pad = " ".repeat(indent + 2);
                for item in items {
                    out.push_str(&item_pad);
                    out.push_str("<li>");
                    convert_spans(out, item);
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

fn convert_spans(out: &mut String, spans: &[Span]) {
    for span in spans {
        match span {
            Span::Text(t) => escape_text(out, t),
            Span::Em(inner) => {
                out.push_str("<em>");
                convert_spans(out, inner);
                out.push_str("</em>");
            }
            Span::Strong(inner) => {
                out.push_str("<strong>");
                convert_spans(out, inner);
                out.push_str("</strong>");
            }
            Span::Code(code) => {
                out.push_str("<code>");
                escape_text(out, code);
                out.push_str("</code>");
            }
            Span::Link { spans, href } => {
                out.push_str("<a href=\"");
                escape_attr(out, href);
                out.push_str("\">");
                convert_spans(out, spans);
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

/// kramdown `Converter::Base#generate_id` + `#basic_generate_id`:
/// strip leading non-ASCII-letters, delete everything outside
/// `[a-zA-Z0-9 -]`, spaces → hyphens, downcase; empty → "section";
/// duplicates get `-1`, `-2`, … suffixes.
fn generate_id(raw: &str, used_ids: &mut HashMap<String, u32>) -> String {
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
