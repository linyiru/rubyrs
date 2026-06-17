//! Tight nested-list golden cases — the construct behind the
//! jekyll-1k corpus's 457-document decline (every post carries a
//! `- a\n  - nested` list). Expected HTML is byte-for-byte what
//! kramdown 2.5.2 (GFM input, auto_ids off) produces — probed on
//! CRuby; see the probe table in the task notes. The official
//! kramdown corpus only exercises loose/multi-paragraph lists
//! (still declined), so these pin the tight-nesting subset.

use rostdown::{Error, NoHighlight, Options, to_html};

fn render(src: &str) -> Result<String, Error> {
    to_html(src, &Options::jekyll(), &mut NoHighlight)
}

#[track_caller]
fn ok(src: &str, expected: &str) {
    match render(src) {
        Ok(html) => assert_eq!(html, expected, "input: {src:?}"),
        Err(e) => panic!("declined {src:?}: {e:?}"),
    }
}

#[track_caller]
fn declined(src: &str) {
    match render(src) {
        Ok(html) => panic!("expected decline for {src:?}, got {html:?}"),
        Err(Error::Declined(_)) => {}
    }
}

#[test]
fn nested_two_levels() {
    ok(
        "- a\n  - b\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn nested_with_tail_item() {
    ok(
        "- a\n  - b\n- c\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n  <li>c</li>\n</ul>\n",
    );
}

#[test]
fn nested_three_levels() {
    ok(
        "- a\n  - b\n    - c\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b\n        <ul>\n          <li>c</li>\n        </ul>\n      </li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn nested_mid_list() {
    ok(
        "- a\n- b\n  - c\n- d\n",
        "<ul>\n  <li>a</li>\n  <li>b\n    <ul>\n      <li>c</li>\n    </ul>\n  </li>\n  <li>d</li>\n</ul>\n",
    );
}

#[test]
fn ordered_child_in_unordered() {
    ok(
        "- a\n  1. b\n",
        "<ul>\n  <li>a\n    <ol>\n      <li>b</li>\n    </ol>\n  </li>\n</ul>\n",
    );
}

#[test]
fn two_children() {
    ok(
        "- a\n  - x\n  - y\n- b\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>x</li>\n      <li>y</li>\n    </ul>\n  </li>\n  <li>b</li>\n</ul>\n",
    );
}

#[test]
fn continuation_then_child() {
    ok(
        "- a\n  cont\n  - b\n",
        "<ul>\n  <li>a\ncont\n    <ul>\n      <li>b</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn child_then_deep_continuation() {
    // The 2-space continuation AFTER a child marker attaches to the
    // deepest open item (kramdown joins it onto `b`).
    ok(
        "- a\n  - b\n  cont\n",
        "<ul>\n  <li>a\n    <ul>\n      <li>b\ncont</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn corpus_post_shape() {
    // The exact list every jekyll-1k post carries.
    ok(
        "- item one 7\n- item two\n  - nested\n",
        "<ul>\n  <li>item one 7</li>\n  <li>item two\n    <ul>\n      <li>nested</li>\n    </ul>\n  </li>\n</ul>\n",
    );
}

#[test]
fn ordered_parent_nested_child() {
    // An ordered parent carries an indented child at its digits+2 content
    // column (kramdown nests it just like an unordered parent).
    ok(
        "1. a\n   1. b\n",
        "<ol>\n  <li>a\n    <ol>\n      <li>b</li>\n    </ol>\n  </li>\n</ol>\n",
    );
}

#[test]
fn conservative_declines_hold() {
    // Blank-separated nesting is LOOSE (`<li><p>` form) — declined.
    declined("- a\n\n  - b\n");
    // Tab indentation — declined.
    declined("- a\n\t- b\n");
    // 1-space indent is a SAME-level item in kramdown — declined.
    declined("- a\n - b\n");
    // Column-0 text after a child would join the parent — declined.
    declined("- a\n  - b\ncont\n");
}
