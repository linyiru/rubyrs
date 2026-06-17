//! Jekyll bench-post fixtures: the post shapes used by the
//! `jekyll-1k` benchmark site, with expected HTML captured from CRuby
//! kramdown 2.5.2 + kramdown-parser-gfm 1.1.0 under Jekyll's default
//! options (`input: GFM`, `auto_ids`, `entity_output: as_char`, smart
//! quotes, `hard_wrap: false`) and `syntax_highlighter: nil` — i.e.
//! rostdown's `NoHighlight` path.

use rostdown::{NoHighlight, Options, to_html};

fn render(src: &str) -> String {
    to_html(src, &Options::jekyll(), &mut NoHighlight).expect("bench subset must not decline")
}

/// The exact body of bench post 112 (python rotation).
#[test]
fn bench_post_python() {
    let src = "## Heading 112\n\
               \n\
               Some **markdown** body with a [link](https://example.com) and `code`.\n\
               \n\
               - item one\n\
               - item two\n\
               \n\
               > quote 112\n\
               \n\
               ```python\n\
               def calc1(n):\n    \
                   return [k * 2 for k in range(n)]  # post 1\n\
               ```\n";
    let expected = "<h2 id=\"heading-112\">Heading 112</h2>\n\
                    \n\
                    <p>Some <strong>markdown</strong> body with a <a href=\"https://example.com\">link</a> and <code>code</code>.</p>\n\
                    \n\
                    <ul>\n  \
                      <li>item one</li>\n  \
                      <li>item two</li>\n\
                    </ul>\n\
                    \n\
                    <blockquote>\n  \
                      <p>quote 112</p>\n\
                    </blockquote>\n\
                    \n\
                    <pre><code class=\"language-python\">def calc1(n):\n    \
                    return [k * 2 for k in range(n)]  # post 1\n\
                    </code></pre>\n";
    assert_eq!(render(src), expected);
}

/// kramdown smart typography under `entity_output: as_char`.
#[test]
fn typography() {
    assert_eq!(
        render("It's \"quoted\" -- and... done\n"),
        "<p>It\u{2019}s \u{201C}quoted\u{201D} \u{2013} and\u{2026} done</p>\n"
    );
    assert_eq!(render("em --- dash\n"), "<p>em \u{2014} dash</p>\n");
}

/// Heading ids — kramdown-parser-gfm's `generate_gfm_header_id`, NOT
/// the core converter algorithm. Expectations captured from CRuby
/// (kramdown 2.5.2 + kramdown-parser-gfm 1.1.0): leading digits kept,
/// `_` kept, typography applied then deleted (spaces around `--` give
/// TWO hyphens), link text included, duplicates suffixed.
#[test]
fn heading_ids() {
    assert_eq!(
        render("## A B!\n\n## A B!\n\n### 9 lives\n"),
        "<h2 id=\"a-b\">A B!</h2>\n\n<h2 id=\"a-b-1\">A B!</h2>\n\n<h3 id=\"9-lives\">9 lives</h3>\n"
    );
    let html = render(
        "## It's a -- b\n\n## See [link text](https://x.com) here\n\n## with `code span`\n\n## under_score\n",
    );
    let ids: Vec<&str> = html
        .split("id=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "its-a--b",
            "see-link-text-here",
            "with-code-span",
            "under_score"
        ]
    );
}

/// Out-of-subset constructs decline; they must never render wrong.
#[test]
fn declines() {
    for src in [
        "| a | b |\n| c |\n",        // ragged table (column counts differ)
        "para\n====\n",              // setext heading
        "text with line  \nbreak\n", // hard break
        "{:toc}\n",                  // IAL / extension
        "&copy; entity\n",           // entity reference
    ] {
        assert!(
            to_html(src, &Options::jekyll(), &mut NoHighlight).is_err(),
            "expected decline for {src:?}"
        );
    }
}

/// A rouge-shaped highlighter: codespans get Jekyll's class attr and
/// no-lang fences resolve to `default_lang` (kramdown's
/// `syntax_highlighter_opts[:default_lang]`), exactly like the real
/// kramdown+rouge pipeline.
struct FakeRouge;

impl rostdown::CodeHighlighter for FakeRouge {
    fn highlight(&mut self, lang: &str, code: &str) -> Option<String> {
        Some(format!("<HL {lang}:{}>", code.len()))
    }
    fn codespan_class(&self) -> Option<&str> {
        Some("language-plaintext highlighter-rouge")
    }
    fn default_lang(&self) -> Option<&str> {
        Some("plaintext")
    }
}

#[test]
fn rouge_mode_codespan_and_default_lang() {
    let html = to_html(
        "Some `code` here.\n\n```\nplain\n```\n\n```python\nx\n```\n",
        &Options::jekyll(),
        &mut FakeRouge,
    )
    .unwrap();
    assert_eq!(
        html,
        "<p>Some <code class=\"language-plaintext highlighter-rouge\">code</code> here.</p>\n\
         \n\
         <div class=\"language-plaintext highlighter-rouge\"><HL plaintext:6></div>\n\
         \n\
         <div class=\"language-python highlighter-rouge\"><HL python:2></div>\n"
    );
}
