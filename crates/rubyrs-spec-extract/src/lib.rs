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
    // v0.3: lift `before :each do BODY end` into each sibling
    // `it` block's body. Adds insertion subs for the it bodies
    // and a delete sub for the before call itself.
    let mut lifter = BeforeEachLifter {
        source,
        substitutions: Vec::new(),
        consumed_before_ranges: Vec::new(),
    };
    lifter.visit(&root);
    let consumed = lifter.consumed_before_ranges;

    // Drop recogniser substitutions that fall inside a lifter
    // delete range. The bytes they target are about to be wiped
    // by the delete; applying them first would mutate `out` with
    // original offsets, then the delete would use those same
    // original offsets against a now-shifted string and either
    // panic (range past end) or corrupt the output. Recogniser
    // subs INSIDE a before block that's about to be deleted are
    // moot anyway — that's exactly the cluster E "nested args
    // aren't re-rewritten" caveat for the lifted copies.
    let mut all_subs: Vec<Substitution> = collector
        .substitutions
        .into_iter()
        .filter(|s| !consumed.iter().any(|(dstart, dend)| {
            s.start >= *dstart && s.end <= *dend
        }))
        .collect();
    all_subs.extend(lifter.substitutions);

    let rewritten = apply_substitutions(source, all_subs);
    let stripped = strip_require_relative(&rewritten);

    // v0.3: prepend a header listing patterns the extractor saw
    // but didn't rewrite. Skips entries the lifter consumed —
    // those ARE handled, just by a different code path. Reuses
    // the existing parse rather than re-running prism over the
    // source.
    let unhandled = collect_unhandled(source, &root, &consumed);
    if unhandled.is_empty() {
        stripped
    } else {
        insert_skip_header(&stripped, &render_skip_header(&unhandled))
    }
}

/// Insert the skip header after any leading shebang and magic
/// comments (`# encoding:`, `# frozen_string_literal:`, etc.) so
/// Ruby's parser still picks those up. Without this, prepending
/// at byte 0 would push magic comments off line 1-2 — Ruby looks
/// for them ONLY in that position, so they'd silently stop
/// working.
fn insert_skip_header(source: &str, header: &str) -> String {
    let mut byte_cursor = 0usize;
    for line in source.split_inclusive('\n') {
        // `trim()` strips both ends — important because
        // `split_inclusive` keeps the trailing `\n`, so a
        // blank source line comes through as `"\n"` (or
        // `"  \n"`); `trim_start()` alone would leave the
        // newline and `is_empty()` would never fire.
        let trimmed = line.trim();
        // Shebang only on line 1 — but split_inclusive doesn't
        // expose line number directly; the loop just keeps
        // skipping while the line "looks like" something we
        // want to keep above the header. A non-line-1 shebang
        // wouldn't be honoured by Ruby anyway, so skipping is
        // still correct.
        let keep_above_header = trimmed.starts_with("#!")
            || is_magic_comment(trimmed)
            || trimmed.is_empty();
        if !keep_above_header {
            break;
        }
        byte_cursor += line.len();
    }
    let (prefix, rest) = source.split_at(byte_cursor);
    format!("{prefix}{header}{rest}")
}

/// Recognise the comment lines Ruby's parser treats as magic
/// comments. Conservative: only flags forms we're sure about so
/// regular `# ...` comments don't get pushed above the header.
fn is_magic_comment(trimmed: &str) -> bool {
    if !trimmed.starts_with('#') {
        return false;
    }
    let body = trimmed[1..].trim_start();
    // The `-*- encoding: utf-8 -*-` and `coding:` forms can also
    // appear inside a `-*-` wrap; the patterns below match the
    // declarations themselves either way.
    body.starts_with("encoding:")
        || body.starts_with("encoding ")
        || body.starts_with("coding:")
        || body.starts_with("coding ")
        || body.starts_with("frozen_string_literal:")
        || body.starts_with("warn_indent:")
        || body.starts_with("shareable_constant_value:")
        || body.contains("-*- encoding")
        || body.contains("-*- coding")
}

/// Apply just the recogniser pass (no lifter, no
/// require_relative strip, no skip-log header) to a source
/// slice. Runs every recogniser the main collector runs —
/// `should ==`, `should_not ==`, predicate, lambda-raise,
/// `mock_int(literal_int)` — against the input. Used by
/// `BeforeEachLifter` to pre-rewrite a `before :each` body
/// before lifting it into each `it`; the cluster E
/// "args-of-matched-subtree not re-rewritten" limitation
/// doesn't apply because the body is its own parse here.
/// Returns the input verbatim if no recogniser fires.
fn rewrite_recognisers(source: &str) -> String {
    let parsed = ruby_prism::parse(source.as_bytes());
    let root = parsed.node();
    let mut collector = SubstitutionCollector {
        source,
        substitutions: Vec::new(),
    };
    collector.visit(&root);
    apply_substitutions(source, collector.substitutions)
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
            .or_else(|| try_mock_int(self.source, node))
        {
            self.substitutions.push(sub);
            return;
        }
        // No pattern matched. We DELIBERATELY do NOT recurse
        // into the receiver chain — only into arguments and
        // block. Reason: if the outer call (just visited) is
        // unmatched but its receiver chain contains a CallNode
        // that WOULD match a recogniser, rewriting the receiver
        // would orphan the outer call. Source like
        // `arr.should.first.frozen?` would otherwise become
        // `assert(arr.first).frozen?` — the `.frozen?` chains
        // off the assert's return value (nil from __spec_*),
        // raising NoMethodError at run-time. Args / block are
        // independent expressions whose rewrites don't interfere
        // with the outer chain, so they keep the default walk.
        if let Some(args) = node.arguments() {
            ruby_prism::visit_arguments_node(self, &args);
        }
        if let Some(block) = node.block() {
            // `block` may be a BlockNode (do/end / { } body) or
            // a BlockArgumentNode (`&proc`). Dispatch by
            // concrete type since Node doesn't have a generic
            // visit method.
            if let Some(b) = block.as_block_node() {
                ruby_prism::visit_block_node(self, &b);
            } else if let Some(b) = block.as_block_argument_node() {
                ruby_prism::visit_block_argument_node(self, &b);
            }
        }
    }
}

/// `lhs.should == rhs` → `assert_eq(lhs, rhs)` (v0.1).
fn try_should_eq(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    let rhs = match_eq_against(node, b"should")?;
    let lhs = node.receiver()?.as_call_node()?.receiver()?;
    Some(Substitution {
        start: node.location().start_offset(),
        end: node.location().end_offset(),
        replacement: format!("assert_eq({}, {})", slice(source, &lhs), slice(source, &rhs)),
    })
}

/// `lhs.should_not == rhs` → `assert_neq(lhs, rhs)` (v0.2).
fn try_should_not_eq(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    let rhs = match_eq_against(node, b"should_not")?;
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
    recv_name: &[u8],
) -> Option<Node<'pr>> {
    if !name_is(node.name(), b"==") {
        return None;
    }
    let recv_call = node.receiver()?.as_call_node()?;
    if !name_is(recv_call.name(), recv_name) {
        return None;
    }
    if recv_call.arguments().is_some() {
        return None;
    }
    // Allocation-free `== 1` check: take the first arg, then
    // confirm there's no second. Avoids one Vec per CallNode
    // visit which adds up over large spec files (thousands of
    // CallNodes per file in upstream `core/*`).
    let mut args_iter = node.arguments()?.arguments().iter();
    let first = args_iter.next()?;
    if args_iter.next().is_some() {
        return None;
    }
    Some(first)
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
    if !name_is(node.name(), b"raise") {
        return None;
    }
    let should_call = node.receiver()?.as_call_node()?;
    if !name_is(should_call.name(), b"should") {
        return None;
    }
    if should_call.arguments().is_some() {
        return None;
    }
    let lambda_node = should_call.receiver()?;
    let lambda = lambda_node.as_lambda_node()?;
    // Same allocation-free `== 1` check as `match_eq_against`.
    let mut args_iter = node.arguments()?.arguments().iter();
    let class_node = args_iter.next()?;
    if args_iter.next().is_some() {
        return None;
    }
    // Gate: only accept a ConstantReadNode (`ArgumentError`)
    // or ConstantPathNode (`Math::DomainError`). Anything else
    // — a string literal (`raise("FrozenError")`), a method
    // call (`raise(some_var.class)`), a local variable
    // (`raise(error_class)`) — would slice into the class_text
    // literally and produce an `assert_raises("<text>")` that
    // can never match `e.class.to_s`, silently always-failing.
    // Falling through to passthrough lets a human polish.
    if class_node.as_constant_read_node().is_none()
        && class_node.as_constant_path_node().is_none()
    {
        return None;
    }
    let class_text = slice(source, &class_node);
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
    // Gate: predicate methods end in `?` (`empty?`, `frozen?`,
    // `equal?`, `kind_of?`, `instance_of?`, `include?`,
    // `start_with?`, `end_with?`, etc — the entire mspec
    // predicate-matcher convention). Non-`?` method names like
    // `.should.first` aren't part of the mspec matcher set and
    // shouldn't get an `assert(...)` wrap that would mask intent.
    // This also implicitly excludes `==` and `raise` (neither
    // ends in `?`), which are caught by their own recognisers.
    if node.name().as_slice().last() != Some(&b'?') {
        return None;
    }
    let recv_call = node.receiver()?.as_call_node()?;
    let negate = if name_is(recv_call.name(), b"should") {
        false
    } else if name_is(recv_call.name(), b"should_not") {
        true
    } else {
        return None;
    };
    if recv_call.arguments().is_some() {
        return None;
    }
    // Bail when the predicate call has an attached block.
    // Wrapping `lhs.PRED? do ... end` in `assert(...)` would
    // (under Ruby's lower-precedence `do/end` binding) risk
    // attaching the block to `assert` rather than the
    // predicate. Mspec's predicate matchers don't normally
    // take blocks, so passing through is the safe move —
    // a human can finish if they really meant
    // `should.all? { ... }`.
    if node.block().is_some() {
        return None;
    }
    let lhs = recv_call.receiver()?;

    let lhs_text = slice(source, &lhs);
    // Build `.NAME(args)` from the outer call: take the source
    // from the start of the message (the method name) through
    // the end of the outer call. This preserves any positional
    // args syntax (parens or no parens) verbatim. We've
    // already excluded the block case above so the suffix
    // never contains a `{ ... }` or `do ... end` tail.
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

/// `mock_int(N)` → `N` (v0.3). mspec's `mock_int` constructs a
/// fake object that responds to `to_int` with `N`. For
/// rubyrs's micro-runner there's no mock library AND the
/// places upstream uses `mock_int` (e.g. `digits(mock_int(2))`)
/// just want an Integer at the call site — substituting the
/// literal value gets us the same effective test in one line.
///
/// Restricted to a single Integer-literal argument. Anything
/// else (`mock_int(some_var)`, multi-arg, no-arg) falls
/// through so the skip log picks it up.
fn try_mock_int(source: &str, node: &ruby_prism::CallNode<'_>) -> Option<Substitution> {
    if !name_is(node.name(), b"mock_int") {
        return None;
    }
    // mspec's `mock_int` is a top-level helper — always called
    // with no receiver. Bail when a receiver is present so
    // user code like `obj.mock_int(2)` (a method named the same
    // on someone's class) doesn't get its receiver silently
    // stripped.
    if node.receiver().is_some() {
        return None;
    }
    let mut args_iter = node.arguments()?.arguments().iter();
    let arg = args_iter.next()?;
    if args_iter.next().is_some() {
        return None;
    }
    // Restrict to integer-literal arg — anything dynamic
    // (variable, method call) would silently produce a wrong
    // test by losing the mocked-out coercion intent.
    arg.as_integer_node()?;
    let loc = node.location();
    Some(Substitution {
        start: loc.start_offset(),
        end: loc.end_offset(),
        replacement: slice(source, &arg),
    })
}

/// Alloc-free name comparison: each recogniser compares the
/// call's `name()` against fixed byte literals (b"==",
/// b"should", etc), avoiding the per-`CallNode` String alloc
/// the previous `cid_to_string`-based approach made.
///
/// The recognised matcher names (`should`, `should_not`,
/// `raise`, `==`, plus any `?`-suffixed predicate) are all
/// pure ASCII; the comparison just checks the call's name
/// bytes against an ASCII byte literal. Ruby itself allows
/// non-ASCII identifiers in user code, but no upstream
/// matcher we care about uses one — non-matching identifiers
/// simply fall through to passthrough, which is the right
/// behaviour.
fn name_is(id: ruby_prism::ConstantId<'_>, expected: &[u8]) -> bool {
    id.as_slice() == expected
}

fn slice(source: &str, node: &Node<'_>) -> String {
    let loc = node.location();
    source[loc.start_offset()..loc.end_offset()].to_string()
}

/// Apply substitutions in reverse byte order so earlier offsets
/// stay valid as later edits rewrite the tail of the string.
///
/// Tiebreaker on equal `start`: apply longer ranges before
/// zero-length insertions at the same point. v0.3 introduced
/// zero-length inserts (lifter prepending body to `it` blocks)
/// that can land at the same offset as a recogniser substitution
/// (which has start < end). Without the secondary `Reverse(end)`
/// key, sort would be unstable on ties and the order could go
/// either way — applying the insert first means the recogniser
/// range then lands BEFORE the inserted prefix, doubling /
/// corrupting bytes.
fn apply_substitutions(source: &str, mut subs: Vec<Substitution>) -> String {
    subs.sort_by_key(|s| (std::cmp::Reverse(s.start), std::cmp::Reverse(s.end)));
    let mut out = source.to_string();
    for sub in subs {
        out.replace_range(sub.start..sub.end, &sub.replacement);
    }
    out
}

// === v0.3 — `before :each` body lift ===================================

/// Walks the AST looking for `describe ... do ... end` blocks.
/// For each describe that contains a `before :each do BODY end`
/// followed by one or more `it "..." do IT_BODY end` siblings,
/// emits substitutions to:
///   - delete the `before` call itself, and
///   - prepend `BODY` to each sibling `it`'s body.
///
/// Scoping rule: a `before :each` is paired only with the `it`
/// blocks that are DIRECT children of its own `describe`'s
/// block body. Nested `describe`s get their OWN
/// process_describe call (the visitor walks every CallNode);
/// each handles its own direct children independently and the
/// scoping doesn't compose. `context` blocks, `before :all`,
/// and `after :*` are passthrough — their presence shows up in
/// the skip log instead.
struct BeforeEachLifter<'a> {
    source: &'a str,
    substitutions: Vec<Substitution>,
    /// Byte ranges of `before :each` calls the lifter consumed.
    /// `collect_unhandled` filters these out when scanning so the
    /// skip log doesn't double-flag what's already handled.
    consumed_before_ranges: Vec<(usize, usize)>,
}

impl<'pr> Visit<'pr> for BeforeEachLifter<'_> {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if name_is(node.name(), b"describe") {
            self.process_describe(node);
        }
        ruby_prism::visit_call_node(self, node);
    }
}

impl BeforeEachLifter<'_> {
    fn process_describe(&mut self, node: &ruby_prism::CallNode<'_>) -> Option<()> {
        let block_node = node.block()?;
        let block = block_node.as_block_node()?;
        let body = block.body()?;
        let stmts = body.as_statements_node()?;

        // First pass over direct children: capture `before :each`
        // body text and the call's byte range.
        let mut lifted_body_text: Option<String> = None;
        let mut before_call_range: Option<(usize, usize)> = None;

        for stmt in stmts.body().iter() {
            let Some(call) = stmt.as_call_node() else { continue };
            if !name_is(call.name(), b"before") { continue }
            let Some(args) = call.arguments() else { continue };
            let mut a = args.arguments().iter();
            let Some(first) = a.next() else { continue };
            // First arg must be exactly `:each`. Slice the source
            // verbatim — saves us figuring out prism's symbol-node
            // unescaping; the literal `:each` is unambiguous.
            if slice(self.source, &first) != ":each" { continue }
            // No additional args — `before :each, :foo do ... end`
            // (any extension form mspec or a custom DSL adds)
            // wouldn't have the meaning v0.3 lifts, so we bail.
            if a.next().is_some() { continue }
            let Some(b_block_node) = call.block() else { continue };
            let Some(b_block) = b_block_node.as_block_node() else { continue };
            let Some(b_body) = b_block.body() else { continue };
            // Run the recognisers on the body slice itself so any
            // `should ==` / predicate / lambda-raise patterns
            // inside `@hash = …` lines get rewritten BEFORE we
            // copy the body into each it block. Without this the
            // lifted copies inherit un-rewritten upstream calls
            // and the micro-runner trips on them at runtime.
            // (Plain `before :each do @x = expr.should == val end`
            // isn't idiomatic mspec, but defensive — bodies do
            // sometimes call helper methods that contain matchers.)
            let body_text_raw = slice(self.source, &b_body);
            lifted_body_text = Some(rewrite_recognisers(&body_text_raw));
            // Expand the delete range to include leading
            // whitespace on the `before`'s line and one trailing
            // newline. The expansion removes the WHOLE line
            // (indent + call + trailing `\n`) so the output
            // doesn't carry a whitespace-only blank line as an
            // editing artefact. A blank visual gap can still
            // appear if there was a blank line BEFORE the
            // `before` call — we don't try to swallow that, on
            // the theory that intentional spacing should
            // survive.
            let raw_start = call.location().start_offset();
            let raw_end = call.location().end_offset();
            let bytes = self.source.as_bytes();
            let mut s = raw_start;
            while s > 0 {
                let b = bytes[s - 1];
                if b == b' ' || b == b'\t' { s -= 1; } else { break; }
            }
            let mut e = raw_end;
            if e < bytes.len() && bytes[e] == b'\n' { e += 1; }
            before_call_range = Some((s, e));
            break;
        }

        let lifted = lifted_body_text?;
        let (start, end) = before_call_range?;

        // Second pass: gather all `it` body insertion points.
        // If ANY sibling `it` has an empty body (`do end` /
        // `{ }`), bail the entire lift rather than emit a
        // partial one — without the bail, we'd delete the
        // `before` and lift into only the non-empty siblings,
        // leaving the empty-body `it` running without its setup.
        // The asymmetry would be surprising; passing through
        // lets the human see the `before :each` via the skip
        // log and inline manually if they want.
        let mut insertion_points: Vec<usize> = Vec::new();
        let mut any_it_seen = false;
        for stmt in stmts.body().iter() {
            let Some(call) = stmt.as_call_node() else { continue };
            if !name_is(call.name(), b"it") { continue }
            any_it_seen = true;
            let it_block_node = call.block()?;
            let it_block = it_block_node.as_block_node()?;
            // Empty-body `it` (`do end` / `{ }`) — `?` returns
            // None from process_describe, aborting the whole
            // lift. Comment above explains why we bail rather
            // than emit a partial substitution.
            let it_body = it_block.body()?;
            insertion_points.push(it_body.location().start_offset());
        }
        if !any_it_seen {
            // No `it`s at all in this describe → no point lifting;
            // leave the before call alone (lands in skip log).
            return None;
        }

        // Emit substitutions.
        // 1. Delete the before call. The `(start, end)` range
        //    was expanded above to swallow the call's leading
        //    indent and trailing newline, so this removes the
        //    WHOLE line — the output doesn't carry a
        //    whitespace-only blank line as an editing artefact.
        self.substitutions.push(Substitution {
            start,
            end,
            replacement: String::new(),
        });
        // 2. Insert the lifted body at the start of each `it` body.
        //    The lifted text already carries its source-position
        //    indentation; we append a newline + 4-space indent so
        //    the original `it` body's first statement keeps its
        //    column. Indent mismatches a couple of columns under
        //    pathological nesting but the output still parses.
        for insert_at in insertion_points {
            self.substitutions.push(Substitution {
                start: insert_at,
                end: insert_at,
                replacement: format!("{lifted}\n    "),
            });
        }
        self.consumed_before_ranges.push((start, end));
        Some(())
    }
}

// === v0.3 — skip-log header ============================================

/// One pattern the extractor saw but didn't rewrite. Surfaced as a
/// bullet in the output's header so a human reviewer doesn't have
/// to grep for what's left.
struct UnhandledPattern {
    line: usize,
    name: String,
    detail: &'static str,
}

fn collect_unhandled<'pr>(
    source: &str,
    root: &Node<'pr>,
    consumed: &[(usize, usize)],
) -> Vec<UnhandledPattern> {
    // Precompute line-start byte offsets once. Each visit_call_node
    // hit then resolves its line number via binary search,
    // O(log N) vs the previous O(N) scan-from-start per pattern.
    // Real upstream files have hundreds of CallNodes — the per-
    // pattern hit count is small but the file size means the
    // linear scan was O(file_size * patterns).
    let mut line_starts: Vec<usize> = Vec::with_capacity(source.len() / 32);
    line_starts.push(0);
    for (i, b) in source.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }

    let mut visitor = UnhandledCollector {
        patterns: Vec::new(),
        consumed,
        line_starts: &line_starts,
    };
    visitor.visit(root);
    visitor.patterns
}

struct UnhandledCollector<'a> {
    patterns: Vec<UnhandledPattern>,
    consumed: &'a [(usize, usize)],
    /// Sorted byte offsets where each line starts. `line_starts[0]
    /// == 0`; `line_starts[i]` is the byte index of the first
    /// character on line `i+1`. Used by `line_at` for O(log N)
    /// offset-to-line lookups.
    line_starts: &'a [usize],
}

impl<'pr> Visit<'pr> for UnhandledCollector<'_> {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        let loc = node.location();
        let start = loc.start_offset();
        // Suppress ONLY when this is the `before` call the lifter
        // handled — its raw start sits inside the expanded delete
        // range. For any OTHER call whose start is inside that
        // range (a nested `it_behaves_like` / `mock` etc inside
        // the before body), don't suppress: those patterns end
        // up in the lifted copies that go into each `it`, so the
        // human still needs them flagged in the skip log.
        let name_bytes_for_consumed = node.name().as_slice();
        let was_consumed = name_bytes_for_consumed == b"before"
            && self.consumed.iter().any(|(s, e)| start >= *s && start < *e);
        if !was_consumed {
            let name_bytes = node.name().as_slice();
            // For mock_int, mirror try_mock_int's gate so the skip
            // log and the rewrite stay consistent. Three checks
            // in lock-step with try_mock_int:
            //   1. name == "mock_int"
            //   2. no receiver (top-level helper only)
            //   3. exactly one Integer-literal arg
            // If any check fails we don't claim the call was
            // substituted — it lands in the skip log so the human
            // sees it.
            let mock_int_substitutable = name_bytes == b"mock_int"
                && node.receiver().is_none()
                && {
                    let args = node.arguments();
                    if let Some(args) = args {
                        let mut iter = args.arguments().iter();
                        let first = iter.next();
                        let second = iter.next();
                        match (first, second) {
                            (Some(a), None) => a.as_integer_node().is_some(),
                            _ => false,
                        }
                    } else {
                        false
                    }
                };
            let detail: Option<&'static str> = match name_bytes {
                b"before" => Some("only `before :each` is lifted in v0.3"),
                b"after" => Some("not lifted; inline cleanup or skip the block"),
                b"context" => Some("micro-runner treats as describe; if you use `before :all` here it won't lift"),
                b"it_behaves_like" => Some("shared-example inlining is v0.4"),
                b"mock" => Some("no mock library in the micro-runner; hand-translate"),
                b"mock_int" if !mock_int_substitutable => Some("only `mock_int(literal_int)` with no receiver is substituted; other forms (explicit receiver, multi-arg, non-int-literal) pass through"),
                b"should_receive" => Some("mock expectations; hand-translate"),
                _ => None,
            };
            if let Some(detail) = detail {
                let line = line_at(self.line_starts, start);
                self.patterns.push(UnhandledPattern {
                    line,
                    name: String::from_utf8_lossy(name_bytes).into_owned(),
                    detail,
                });
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
}

/// 1-based line number for a byte offset, using a precomputed
/// list of line-start offsets. `line_starts` is sorted ascending
/// (constructed by walking the source once); `partition_point`
/// gives us O(log N) lookup vs `line_number`'s O(N)-per-call
/// scan that the v0.3 round-1 implementation used.
fn line_at(line_starts: &[usize], byte_offset: usize) -> usize {
    line_starts.partition_point(|&s| s <= byte_offset)
}

/// Render the bullet list as a Ruby block comment to prepend to
/// the extractor output.
fn render_skip_header(patterns: &[UnhandledPattern]) -> String {
    let mut out = String::new();
    out.push_str("# rubyrs-spec-extract v0.3: ");
    out.push_str(&format!("{} pattern(s) left for hand polish.\n", patterns.len()));
    out.push_str("# Each entry names the upstream line + reason. Address each\n");
    out.push_str("# (comment out, inline, or wait for a later extractor version)\n");
    out.push_str("# before the file is consumable by the micro-runner.\n");
    out.push_str("#\n");
    for p in patterns {
        out.push_str(&format!("#   - L{}: `{}` — {}\n", p.line, p.name, p.detail));
    }
    out.push('\n');
    out
}
