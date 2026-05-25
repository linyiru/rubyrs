//! Mechanically rewrites ruby/spec files into the
//! `assert_eq` / `assert_raises` shape that rubyrs's micro-runner
//! understands (`crates/rubyrs/spec/`).
//!
//! v0.1 recognises exactly one pattern: `expr.should == val`.
//! That pattern accounts for the bulk of the equality-style
//! `it` blocks in upstream `core/string`, `core/method`, and
//! similar simple files — the same shape PR #48/#52/#55 have
//! been translating by hand.
//!
//! Approach: byte-range substitution. We parse the source with
//! `ruby_prism`, walk only the `CallNode`s we care about, and
//! collect `(start, end, replacement)` substitutions. After the
//! walk we apply them in reverse byte order so earlier offsets
//! stay valid. Everything we don't recognise passes through
//! verbatim — including comments, whitespace, blank lines, and
//! any matcher shapes (`should.raise`, `it_behaves_like`,
//! predicate-style `should.X?`) we haven't taught the extractor
//! about yet. Those land in the output as-is so a human can
//! see what still needs to be hand-translated.
//!
//! ## What v0.1 deliberately does NOT do
//!
//! - `expr.should_not == val` — needs an `assert_neq` helper in
//!   `spec_helper.rb`; pending.
//! - `expr.should.foo?` (predicate matcher) — needs per-predicate
//!   knowledge.
//! - `-> { ... }.should.raise(X)` — could lower to
//!   `assert_raises("X") { ... }` but requires parsing the lambda
//!   and the matcher class name.
//! - `it_behaves_like :shared, ...` — inlining shared examples is
//!   a separate piece of work.
//! - Removing `require_relative` lines — those are no-ops in the
//!   micro-runner but currently pass through harmlessly (each
//!   raises a NoMethodError that the runner catches). A future
//!   pass could strip them; not gated.

use ruby_prism::{Node, Visit};

/// Recognise `expr.should == val` and rewrite to
/// `assert_eq(expr, val)`. Everything else passes through.
///
/// Returns the rewritten source. `extract(s) == s` when nothing
/// matched.
pub fn extract(source: &str) -> String {
    let parsed = ruby_prism::parse(source.as_bytes());
    let root = parsed.node();
    let mut collector = SubstitutionCollector {
        source,
        substitutions: Vec::new(),
    };
    collector.visit(&root);
    apply_substitutions(source, collector.substitutions)
}

/// One byte-range replacement: `source[start..end] = replacement`.
struct Substitution {
    start: usize,
    end: usize,
    replacement: String,
}

struct SubstitutionCollector<'a> {
    source: &'a str,
    substitutions: Vec<Substitution>,
}

impl<'pr> Visit<'pr> for SubstitutionCollector<'_> {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        // Look for `lhs.should == rhs`:
        //   outer: CallNode { name=:==, receiver=should_call, args=[rhs] }
        //   should_call: CallNode { name=:should, receiver=lhs, args=None }
        let name = cid_to_string(node.name());
        if name == "==" {
            if let Some(should_call_node) = node.receiver()
                && let Some(should_call) = should_call_node.as_call_node()
                && cid_to_string(should_call.name()) == "should"
                && should_call.arguments().is_none()
                && let Some(lhs) = should_call.receiver()
                && let Some(args) = node.arguments()
            {
                let arg_list: Vec<_> = args.arguments().iter().collect();
                if arg_list.len() == 1 {
                    let rhs = &arg_list[0];
                    let outer_loc = node.location();
                    let lhs_text = slice(self.source, &lhs);
                    let rhs_text = slice(self.source, rhs);
                    self.substitutions.push(Substitution {
                        start: outer_loc.start_offset(),
                        end: outer_loc.end_offset(),
                        replacement: format!("assert_eq({}, {})", lhs_text, rhs_text),
                    });
                    // Don't recurse — we've consumed the whole
                    // `lhs.should == rhs` subtree. Recursing would
                    // re-visit the inner `.should` call and
                    // possibly trigger spurious nested rewrites if
                    // a future pattern overlaps.
                    return;
                }
            }
        }
        // Default recursion — keep visiting children to find
        // patterns nested inside arguments, blocks, etc.
        ruby_prism::visit_call_node(self, node);
    }
}

fn cid_to_string(id: ruby_prism::ConstantId<'_>) -> String {
    String::from_utf8_lossy(id.as_slice()).into_owned()
}

fn slice(source: &str, node: &Node<'_>) -> String {
    let loc = node.location();
    source[loc.start_offset()..loc.end_offset()].to_string()
}

/// Apply substitutions in reverse byte order so earlier offsets
/// stay valid as later edits rewrite the tail of the string.
fn apply_substitutions(source: &str, mut subs: Vec<Substitution>) -> String {
    subs.sort_by(|a, b| b.start.cmp(&a.start));
    let mut out = source.to_string();
    for sub in subs {
        out.replace_range(sub.start..sub.end, &sub.replacement);
    }
    out
}
