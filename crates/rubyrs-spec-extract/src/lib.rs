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
//!
//! ## What v0.1 DOES do beyond the `should ==` rewrite
//!
//! - **Strips `require_relative` lines** from the output. The
//!   micro-runner has no loader, so a stray `require_relative
//!   '../../spec_helper'` would raise NoMethodError at file scope
//!   and the runner's `<file-level>` synthetic example would fail
//!   the whole file. Stripping is a line-level filter
//!   (`^\s*require_relative\b.*$`) applied after the AST rewrite
//!   — independent of parse state so even partially-invalid
//!   source still gets the cleanup.

use ruby_prism::{Node, Visit};

/// Recognise `expr.should == val` and rewrite to
/// `assert_eq(expr, val)`, then strip `require_relative` lines
/// the micro-runner can't load. Everything else passes through.
///
/// Returns the rewritten source. `extract(s) == s` only when
/// nothing matched AND there were no `require_relative` lines.
///
/// Parse errors are NOT surfaced here — callers that care
/// should use [`parse_errors`] alongside this fn. The CLI does
/// (`main.rs`); golden tests don't need to.
pub fn extract(source: &str) -> String {
    let parsed = ruby_prism::parse(source.as_bytes());
    let root = parsed.node();
    let mut collector = SubstitutionCollector {
        source,
        substitutions: Vec::new(),
    };
    collector.visit(&root);
    let rewritten = apply_substitutions(source, collector.substitutions);
    strip_require_relative(&rewritten)
}

/// Returns the parse-error messages reported by `ruby_prism` for
/// `source`. Empty Vec means the source parsed cleanly. The
/// extractor itself runs to completion regardless (best-effort
/// rewrite of any valid sub-trees), so this is purely for
/// diagnostic output — the CLI prints these to stderr.
pub fn parse_errors(source: &str) -> Vec<String> {
    let parsed = ruby_prism::parse(source.as_bytes());
    parsed
        .errors()
        .map(|d| d.message().to_owned())
        .collect()
}

/// Drop lines whose first non-whitespace token is `require_relative`.
/// The micro-runner has no loader; leaving these in fails the spec
/// file at `<file-level>`. Cheap line-level filter so the cleanup
/// runs even when prism reports parse errors and the AST is
/// partially-broken.
fn strip_require_relative(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("require_relative")
            && trimmed[16..]
                .chars()
                .next()
                .map(|c| c.is_whitespace() || c == '(')
                .unwrap_or(true)
        {
            continue;
        }
        out.push_str(line);
    }
    out
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
        // Try each recogniser in turn; first one that matches
        // consumes the subtree (we return without recursing).
        // Order matters when patterns could overlap — `raise`
        // is also a method name so `try_lambda_raise` runs
        // before the generic predicate-matcher recogniser.
        if let Some(sub) = try_should_eq(self.source, node)
            .or_else(|| try_should_not_eq(self.source, node))
            .or_else(|| try_lambda_raise(self.source, node))
            .or_else(|| try_predicate_matcher(self.source, node))
        {
            self.substitutions.push(sub);
            // Don't recurse — we've consumed the whole subtree.
            // Recursing would re-visit the inner `.should` call
            // and trigger spurious nested rewrites.
            return;
        }
        // No pattern matched — keep visiting children so
        // patterns nested in arguments / blocks still fire.
        ruby_prism::visit_call_node(self, node);
    }
}

/// `lhs.should == rhs` → `assert_eq(lhs, rhs)` (v0.1).
fn try_should_eq(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    let rhs = match_eq_against(node, "should")?;
    let lhs = node.receiver()?.as_call_node()?.receiver()?;
    Some(Substitution {
        start: node.location().start_offset(),
        end: node.location().end_offset(),
        replacement: format!("assert_eq({}, {})", slice(source, &lhs), slice(source, &rhs)),
    })
}

/// `lhs.should_not == rhs` → `assert_neq(lhs, rhs)` (v0.2).
fn try_should_not_eq(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    let rhs = match_eq_against(node, "should_not")?;
    let lhs = node.receiver()?.as_call_node()?.receiver()?;
    Some(Substitution {
        start: node.location().start_offset(),
        end: node.location().end_offset(),
        replacement: format!("assert_neq({}, {})", slice(source, &lhs), slice(source, &rhs)),
    })
}

/// Shared shape-match for `lhs.RECV_NAME == rhs`: confirms the
/// outer call is `==`, the receiver is a no-arg call with the
/// requested name (`should` or `should_not`), and there's
/// exactly one RHS arg. Returns the RHS node on match.
fn match_eq_against<'pr>(
    node: &ruby_prism::CallNode<'pr>,
    recv_name: &str,
) -> Option<Node<'pr>> {
    if cid_to_string(node.name()) != "==" {
        return None;
    }
    let recv_call = node.receiver()?.as_call_node()?;
    if cid_to_string(recv_call.name()) != recv_name {
        return None;
    }
    if recv_call.arguments().is_some() {
        return None;
    }
    let args = node.arguments()?;
    let arg_list: Vec<_> = args.arguments().iter().collect();
    if arg_list.len() != 1 {
        return None;
    }
    Some(arg_list.into_iter().next().unwrap())
}

/// `-> { BODY }.should.raise(CLASS)` →
/// `assert_raises("CLASS") do BODY end` (v0.2).
///
/// Class name is the source text of the outer call's first
/// argument — covers `ArgumentError`, `Math::DomainError`, etc.
/// Lambda body comes from `LambdaNode::body()`'s location;
/// `body()` returns Some for any non-empty lambda, and an
/// empty lambda (`-> {}.should.raise(X)`) produces an empty
/// `do ... end` block which is still valid Ruby.
fn try_lambda_raise(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    if cid_to_string(node.name()) != "raise" {
        return None;
    }
    let should_call = node.receiver()?.as_call_node()?;
    if cid_to_string(should_call.name()) != "should" {
        return None;
    }
    if should_call.arguments().is_some() {
        return None;
    }
    let lambda_node = should_call.receiver()?;
    let lambda = lambda_node.as_lambda_node()?;
    let args = node.arguments()?;
    let arg_list: Vec<_> = args.arguments().iter().collect();
    if arg_list.len() != 1 {
        return None;
    }
    let class_text = slice(source, &arg_list[0]);
    let body_text = lambda
        .body()
        .map(|b| slice(source, &b))
        .unwrap_or_default();
    Some(Substitution {
        start: node.location().start_offset(),
        end: node.location().end_offset(),
        replacement: format!(
            "assert_raises(\"{class_text}\") do\n      {body_text}\n    end"
        ),
    })
}

/// `lhs.should.NAME(args)` → `assert(lhs.NAME(args))`
/// `lhs.should_not.NAME(args)` → `assert(!lhs.NAME(args))`
///
/// `NAME` is any method other than `==` / `raise` — those are
/// handled by dedicated recognisers above. Common upstream
/// shapes: `should.empty?`, `should.equal?(other)`,
/// `should.instance_of?(Class)`.
fn try_predicate_matcher(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    let outer_name = cid_to_string(node.name());
    // `==` and `raise` are caught by their dedicated
    // recognisers; bail so we don't also match here.
    if outer_name == "==" || outer_name == "raise" {
        return None;
    }
    let recv_call = node.receiver()?.as_call_node()?;
    let negate = match cid_to_string(recv_call.name()).as_str() {
        "should" => false,
        "should_not" => true,
        _ => return None,
    };
    if recv_call.arguments().is_some() {
        return None;
    }
    let lhs = recv_call.receiver()?;

    let lhs_text = slice(source, &lhs);
    // Build `.NAME(args)` from the outer call: take the source
    // from the start of the message (the method name) through
    // the end of the outer call. This preserves any args
    // syntax (parens, no parens, block) verbatim.
    let outer_loc = node.location();
    let message_loc = node.message_loc()?;
    let suffix_start = message_loc.start_offset();
    let suffix_end = outer_loc.end_offset();
    let suffix_text = source.get(suffix_start..suffix_end)?;

    let inner = format!("{lhs_text}.{suffix_text}");
    let replacement = if negate {
        format!("assert(!{inner})")
    } else {
        format!("assert({inner})")
    };
    Some(Substitution {
        start: outer_loc.start_offset(),
        end: outer_loc.end_offset(),
        replacement,
    })
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
    subs.sort_by_key(|s| std::cmp::Reverse(s.start));
    let mut out = source.to_string();
    for sub in subs {
        out.replace_range(sub.start..sub.end, &sub.replacement);
    }
    out
}
