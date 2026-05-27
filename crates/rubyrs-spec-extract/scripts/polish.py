#!/usr/bin/env python3
"""Companion post-processor for `rubyrs-spec-extract`.

`spec-extract` v0.4 mechanically rewrites the `expr.should == val`
flavour of upstream ruby/spec into the micro-runner's `assert_eq`
shape — but a finished spec usually still has `it` (or top-level
`before`) blocks whose BODY references fixtures the micro-runner
doesn't ship (`ArraySpecs.recursive_array`, `MyArray[...]`) or
rubyrs methods that aren't implemented in a specific form (`Array#min { ... }`
block-comparator, count-form `Array#first(n)`, multi-arg
`Array#push(a, b, c)`). Those blocks load but fail at run time,
masking the genuine PASSes around them.

This script reads a spec file on stdin, walks `it "..." do … end`
AND top-level `before :each do … end` / `after :each do … end`
blocks (ruby/spec uses symbol args on hooks, not strings — the
extractor preserves that shape), and drops any whose body matches
a known "this won't run in the micro-runner" pattern
(see `DROP_PATTERNS` below). Each dropped block leaves a
`# skipped (<category>): ...` trace at the dropped block's
original indentation so the diff stays auditable.

If the extractor wrote a "patterns left for hand polish" header at
the top of the file naming entries that polish actually addresses,
polish rewrites the header to reflect post-polish status — leaving
a stale header would mislead readers about whether the file is
ready for the micro-runner.

Usage (pipeline shape from spec-extract README):

  rubyrs-spec-extract <upstream.rb> [--shared shared.rb] \\
    | crates/rubyrs-spec-extract/scripts/polish.py \\
    > crates/rubyrs/spec/ruby/<name>_spec.rb

Patterns are deliberately conservative — adding a new one is a
single tuple in `DROP_PATTERNS`. The `# skipped` comments make
future revisits (e.g., when rubyrs gains `Array#push`) easy to
find with `git grep "# skipped (method-not-implemented)"`."""

import re
import sys

# Each entry is (regex, category). The category labels the skip
# trace so readers can tell at a glance whether a dropped block
# was about a missing fixture, an unimplemented method, the mspec
# mock library, etc. — and so future revisits can grep by category
# (`git grep "# skipped (method-not-implemented)"` to find all
# blocks unlocked by a single feature PR).
DROP_PATTERNS = [
    # Fixture references the micro-runner doesn't ship (would need
    # vendoring upstream's `fixtures/classes.rb` — out of scope
    # for the ingestion pass).
    (r"\bArraySpecs\b", "fixture"),
    (r"\bMyArray\b", "fixture"),
    # ruby/spec mock library calls (mspec internals); the
    # micro-runner has no mocking surface.
    (r"\bmock\(", "mock"),
    (r"\bshould_receive\b", "mock"),
    # Note: a v0.1 entry here dropped any block mentioning
    # `FrozenError` or `.freeze`. Removed after PR #133 review
    # confirmed rubyrs DOES implement frozen semantics for
    # `String` — `"foo".freeze; "foo".frozen?` returns true and
    # mutation raises a real FrozenError. Only `Array`/`Hash`
    # freeze are currently no-ops, and Array specs that test
    # frozen behavior always go through `ArraySpecs.frozen_array`
    # (caught by the `\bArraySpecs\b` fixture pattern above).
    # Keeping a blanket FrozenError rule here would silently drop
    # runnable String specs in future batches — exactly the
    # over-fit hazard reviewer feedback flagged.
    # Count-form variants of head/tail accessors. rubyrs ships
    # only the zero-arg form for `Array#min`/`#max`/`#pop`/`#shift`.
    # `\(\s*[^)\s]` matches any non-empty argument list (literal
    # number, negative literal, identifier, expression) — so
    # `[1].min(-1)` and `[].pop(bignum_value)` both get dropped.
    #
    # `Array#first(n)` / `#last(n)` were also in this list pre-#140;
    # removed once PR #140 shipped (cap-to-length, ArgumentError on
    # negative, block-ignored). The two upstream incompatibilities
    # — bignum_value (NoMethodError vs CRuby's RangeError; no
    # BigInt arm) and `.replace`-based independence check (no
    # Array#replace) — are skipped per-file rather than via a
    # blanket polish rule.
    #
    # CAVEAT: these regexes match the LEFT-HAND TEXT of `.first(`
    # / `.last(`, not the receiver type — `(1..5).first(2)` and
    # `[1,2].first(2)` are indistinguishable to polish. Ingesting
    # `core/range/first_spec.rb` or `core/enumerable/first_spec.rb`
    # in the future means `Range#first(n)` / `Enumerable#first(n)`
    # blocks would slip through; verify those count forms are
    # implemented before adding the corresponding ingestion PR,
    # or re-add a narrower rule scoped via assert_eq's LHS.
    (r"\.min\(\s*[^)\s]", "method-not-implemented"),
    (r"\.max\(\s*[^)\s]", "method-not-implemented"),
    (r"\.pop\(\s*[^)\s]", "method-not-implemented"),
    (r"\.shift\(\s*[^)\s]", "method-not-implemented"),
    # `Array#push` MULTI-ARG form is not yet in rubyrs — single-arg
    # `.push(x)` works, but `.push(a, b, c)` raises NoMethodError.
    # Match only the multi-arg shape at the TOP level of the arg
    # list: `\.push\(` + zero-or-more chars that aren't a nesting
    # delimiter or comma + `,`. Excluding `()`, `{}`, `[]`, and
    # `,` from the repeated class means the regex stops at any
    # nested delimiter, so single-arg calls with nested-call,
    # hash-literal, or array-literal args are correctly LEFT
    # alone — the v1 regex `\.push\(\s*[^)]+,` over-dropped all
    # three shapes (caught by /code-review).
    (r"\.push\(\s*[^(){}\[\],]*,", "method-not-implemented"),
    # Note (was wrong): `\.unshift\b` and `\.inject\s*\(` rules were
    # removed after PR #133 review confirmed both methods are
    # implemented (unshift: single AND multi-arg; inject: block AND
    # symbol forms). The previous rules dropped runnable specs.
    # `Array#sort` with comparator block — zero-arg sort works,
    # block form doesn't.
    (r"\.sort\s*(\{|\s+do\b)", "method-not-implemented"),
    # `min`/`max` with comparator block (`.min { |a, b| … }`) —
    # zero-arg `min`/`max` work; block form doesn't.
    (r"\.min\s*(\{|\s+do\b)", "method-not-implemented"),
    (r"\.max\s*(\{|\s+do\b)", "method-not-implemented"),
    # NOTE: a v0.1 entry here dropped any block containing
    # `.instance_of?(Array)`, on the theory that subclass-vs-Array
    # checks weren't in the micro-runner surface. rubyrs DOES
    # implement `Object#instance_of?`, and the actual unsupported
    # case (calling it on a fixture-built subclass like
    # `ArraySpecs::MyArray`) is already caught by the `MyArray` /
    # `ArraySpecs` fixture patterns above. Removed the rule —
    # plain `Array.instance_of?(Array)` checks are runnable and
    # valuable, so dropping them by accident is exactly the
    # over-fitting reviewer feedback PR #133 caught.
]

# Standalone (top-level, not inside an `it`) blocks that the
# v0.3-era `before :each` lifter didn't pick up — they'd
# otherwise file-level-trap with `undefined method `before` for
# NilClass`. Categorized separately because the trigger is the
# block name, not its body.
DROP_TOP_LEVEL_HEADS = [
    (r"^\s*before\b", "before-not-lifted"),
    (r"^\s*after\b", "after-not-supported"),
]


def categorize(body: str):
    """Return the category of the first DROP_PATTERN that matches
    the body, or None if no pattern matches."""
    for pat, cat in DROP_PATTERNS:
        if re.search(pat, body):
            return cat
    return None


def categorize_head(line: str):
    """Return the category of the first DROP_TOP_LEVEL_HEADS that
    matches `line`, or None."""
    for pat, cat in DROP_TOP_LEVEL_HEADS:
        if re.match(pat, line):
            return cat
    return None


# Patterns for SINGLE-LINE string/comment stripping. Used by
# `split_blocks`'s depth counter so `arr.push("end")` doesn't
# trip the `\bend\b` end-counter and prematurely close blocks.
# These are deliberately simple: no escape handling, no
# interpolation parsing — just the most common shapes in
# ruby/spec input. Heredocs span multiple lines and are NOT
# stripped here (see KNOWN_LIMITATIONS in this module's
# docstring).
_LINE_COMMENT = re.compile(r"(?:^|(?<=[^\w#]))#.*$")
_DOUBLE_STR = re.compile(r'"[^"\\]*(?:\\.[^"\\]*)*"')
_SINGLE_STR = re.compile(r"'[^'\\]*(?:\\.[^'\\]*)*'")
_SYMBOL_LITERAL = re.compile(r":[a-zA-Z_]\w*[!?=]?")


def _strip_strings_and_comments(line: str) -> str:
    """Return `line` with string literals, line comments, and
    symbol literals collapsed to empty placeholders. Used by
    `split_blocks`'s depth counter so keyword-shaped tokens
    inside strings/comments don't false-positive the `do`/`end`
    accounting.

    Order matters: strings BEFORE comments, so `# inside a
    "string"` isn't truncated at the `#`. Symbols last, so
    `"foo:bar"` doesn't get its `:bar` portion stripped.
    Single-line forms only — multi-line heredocs / regex
    literals pass through unchanged (a documented limit)."""
    # Drop quoted-string contents first. The non-greedy `[^"\\]*`
    # with `\\.` for escapes handles `"a \" b"` correctly.
    s = _DOUBLE_STR.sub('""', line)
    s = _SINGLE_STR.sub("''", s)
    # Then line comments. Don't strip `#{...}` interpolations
    # (those start with `#{`, which the look-behind in
    # _LINE_COMMENT excludes). Pure `#` at line start or after
    # whitespace becomes a comment.
    s = _LINE_COMMENT.sub("", s)
    # Finally symbols (`:end`, `:do`, etc.) — collapse to empty
    # so the keyword inside the symbol can't match.
    s = _SYMBOL_LITERAL.sub("", s)
    return s


# Match the opening line of an `it "..." do` or `before :sym do`
# block. Indent captured so the skip trace lands at the same
# column as the original block opener — without this the trace
# floats to column 0 inside nested `describe` blocks.
IT_OPEN = re.compile(r"^(\s*)it\s+[\"'].*\bdo\b")
BEFORE_OPEN = re.compile(r"^(\s*)(?:before|after)\s.*\bdo\b")


def split_blocks(src: str):
    """Yield (kind, text, indent) for each chunk.

    `kind` is one of `"it"` (an `it "..." do … end` block),
    `"head"` (a top-level `before`/`after :each do … end`
    block — outside any `it`), or `"other"` (everything else).
    `indent` is the leading whitespace string of the opening
    line, captured so the caller can reproduce indentation when
    emitting skip-trace comments."""
    lines = src.splitlines(keepends=True)
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        for pat, kind in ((IT_OPEN, "it"), (BEFORE_OPEN, "head")):
            m = pat.match(line.rstrip())
            if not m:
                continue
            indent = m.group(1)
            # Walk forward, counting `do`/`end` to find the matching
            # `end` at the same level.
            depth = 1
            j = i + 1
            while j < n:
                l = lines[j]
                # Count every `end`-introducing keyword, not just
                # `do`. Ruby has multiple block-ending forms (any
                # `end`-terminated construct closes a depth):
                #
                #   * `do |args|` at end-of-line — explicit blocks
                #   * `class Foo` / `module Foo` / `def foo`
                #   * `if cond` / `unless cond` / `case expr`
                #   * `begin` / `while cond` / `until cond` / `for x in y`
                #
                # The MODIFIER form of `if`/`unless`/`while`/`until`
                # (`do_stuff if cond`) doesn't take an `end` and
                # MUST NOT increment depth. Restrict those to
                # statement position only: leading `\s*` then the
                # keyword, followed by a word boundary. The other
                # openers (`class`/`def`/`module`/`case`/`begin`/
                # `for`) have no modifier form, so a word-boundary
                # match anywhere on the line is safe — but in
                # practice they too only appear at statement
                # position in ruby/spec input.
                #
                # Reviewer feedback PR #133: the prior `\bdo\b`-only
                # counter would mis-count `end` for an inner
                # `class`/`def`/`if` body as the closing `end` of
                # the outer `it` block, corrupting the output.
                #
                # `end_count` matches `\bend\b` ANYWHERE on the
                # line (not just `^\s*end\b`) so a single-line
                # `def foo; ... end` self-cancels: the `def`
                # opener and inline `end` closer both count once
                # and net to zero depth change. This matters in
                # ruby/spec because `def receiver.method(...); ...
                # end` shows up inside `it` blocks that synthesize
                # methods on test fixtures (e.g.
                # array_include_spec.rb's mock-based equality
                # test).
                #
                # `kw_open_count` is ANCHORED to statement position
                # (`^\s*`) rather than `\b...\b` anywhere — the
                # bare-word match would treat method-call accessors
                # like `some_var.class` / `obj.method.case` /
                # `arr.begin` as block openers, incrementing depth
                # without a matching `end` and ultimately
                # swallowing the rest of the file into a single
                # block. Reviewer feedback PR #133: this was a
                # real bug — `some_var.class` is the most common
                # offender in ruby/spec input. Single-line
                # `def receiver.method(); ... end` is unaffected
                # because the leading `def` IS at statement
                # position.
                # Detect statement openers before counting `do` so
                # we can suppress the trailing-`do` count on
                # constructs that USE the optional-`do` syntax
                # (`while cond do`, `until cond do`, `for x in y do`).
                # Without this, `while cond do … end` would
                # increment depth twice (once for `while`, once for
                # the trailing `do`) and only decrement once on
                # `end`, leaking depth and ultimately swallowing
                # the rest of the file. Reviewer feedback PR #133.
                # Strip strings + comments BEFORE counting keywords
                # so a string literal like `"end"` or a comment
                # `# end of section` doesn't decrement depth and
                # corrupt block boundaries. Reviewer feedback
                # PR #133 caught this: `arr.push("end")` would
                # otherwise trip `\bend\b` and prematurely close
                # the `it` block. Single-line string/comment
                # stripping only — heredocs span multiple lines
                # and need a richer lexer; documented as a known
                # limitation (see `KNOWN_LIMITATIONS` section
                # below).
                scan = _strip_strings_and_comments(l)
                stmt_open_count = len(
                    re.findall(
                        r"^\s*(?:if|unless|while|until)\b",
                        scan,
                    )
                )
                kw_open_count = len(
                    re.findall(
                        r"^\s*(?:class|module|def|case|begin|for)\b",
                        scan,
                    )
                )
                # `if cond` / `unless cond` don't take an optional
                # `do` (they use `then` or newline), so only
                # while/until/for can produce the double-count.
                # Match those keywords at line start AND a
                # trailing `do` on the same line.
                trailing_do_after_stmt = bool(
                    re.search(
                        r"^\s*(?:while|until|for)\b.*\bdo\b(?:\s*\|.*\|)?\s*$",
                        scan,
                    )
                )
                do_count = len(re.findall(r"\bdo\b(?:\s*\|.*\|)?\s*$", scan))
                if trailing_do_after_stmt:
                    do_count = 0  # already counted via stmt_open
                end_count = len(re.findall(r"\bend\b", scan))
                depth += do_count + stmt_open_count + kw_open_count - end_count
                if depth == 0:
                    j += 1
                    break
                j += 1
            yield (kind, "".join(lines[i:j]), indent)
            i = j
            break
        else:
            yield ("other", line, "")
            i += 1


# Match the extractor's "patterns left for hand polish" preamble
# at the top of the file. The header is the first contiguous run
# of `#`-comment lines starting with the version marker —
# `rubyrs-spec-extract v...: N pattern(s) left for hand polish.`
# — followed by indented `#   - L<n>: ...` entry lines and one
# blank-comment separator. Replace it with a polish-state note so
# readers know the file IS micro-runner-ready (or is genuinely
# not, if polish couldn't address everything — then we keep the
# header verbatim).
EXTRACTOR_HEADER_OPENER = re.compile(
    r"^#\s*rubyrs-spec-extract v[\d.]+: \d+ pattern\(s\) left for hand polish\."
)

# Patterns the EXTRACTOR currently passes through (lists in
# its skip-log header) but polish doesn't address. If any of
# these appear in the post-polish output, the header is still
# accurate — keep it. Reviewer feedback PR #133: stripping the
# header when only SOME entries got addressed hides the
# remaining work and makes the file look micro-runner-ready when
# it isn't. Pattern list mirrors the extractor's documented
# "passthrough" allow-list (see crates/rubyrs-spec-extract/README.md
# "What the extractor recognises" table).
EXTRACTOR_LEFTOVER_PATTERNS = [
    # `it_behaves_like` without a matching --shared inlined by
    # the extractor stays as a literal call in the output. Polish
    # has no shared-registry to resolve it from.
    r"\bit_behaves_like\b",
    r"\bit_should_behave_like\b",
    # `context "..." do ... end` — micro-runner doesn't define
    # `context`. Polish doesn't touch these because they CAN be
    # nested inside an `it`-bearing `describe`, dropping the
    # whole block would lose passing examples.
    r"^\s*context\s+[\"']",
    # Curly-brace hook forms (`before { ... }`, `before(:each) {
    # ... }`, `after { ... }`). DROP_TOP_LEVEL_HEADS only catches
    # the do-form via `BEFORE_OPEN`'s `\bdo\b` requirement; curly
    # hooks would otherwise slip through polish AND the extractor
    # header would get stripped — making the file look polished
    # while it still file-level-traps on the unknown `before`
    # method. /code-review caught this gap. (do-form hooks remain
    # the common case in ruby/spec; this list is defense for the
    # less-common shape.)
    r"^\s*(?:before|after)\b[^\n]*\{",
]


def has_unaddressed_passthroughs(src: str) -> bool:
    """Scan `src` for any leftover passthrough patterns polish
    doesn't handle. Used by `rewrite_extractor_header` to decide
    whether the extractor's header is still accurate."""
    for pat in EXTRACTOR_LEFTOVER_PATTERNS:
        # `re.MULTILINE` so `^` matches per-line, not just
        # at the start of the whole string.
        if re.search(pat, src, re.MULTILINE):
            return True
    return False


def rewrite_extractor_header(src: str, addressed: int) -> str:
    """If the file opens with an extractor 'patterns left for hand
    polish' header and polish addressed at least one entry, strip
    the header — but ONLY when every passthrough the extractor
    could have listed has been resolved by polish. If any
    leftover passthrough patterns remain in the post-polish
    output (`it_behaves_like` without --shared,
    `it_should_behave_like`, or a `context "..." do` block —
    `before`/`after` hooks of any shape are handled separately
    by `DROP_TOP_LEVEL_HEADS` and don't appear here), the
    header is still accurate and we leave it intact so readers
    know the file isn't yet micro-runner-ready.

    If polish addressed zero entries, leave the header verbatim
    (no work happened that could have invalidated it)."""
    if addressed == 0:
        return src
    if has_unaddressed_passthroughs(src):
        # Header still accurate — polish addressed some entries
        # but at least one passthrough pattern remains. Leaving
        # the header is the conservative choice. (Reviewer can
        # tell post-polish vs pre-polish state from the in-body
        # `# skipped (<category>)` traces.)
        return src
    lines = src.splitlines(keepends=True)
    # The extractor's header often starts on line 1, but
    # upstream spec files (and therefore extractor output) can
    # carry a preamble of shebang + magic comments BEFORE the
    # extractor header. Reviewer feedback PR #133 caught that
    # the v0.1 "skip leading blanks only" approach left these
    # files with the stale header intact. Skip past:
    #   - leading blank lines
    #   - shebang line  (`^#!...`)
    #   - magic comments (`# encoding: ...`, `# frozen_string_literal: ...`)
    # …then look for the extractor header opener within the
    # next contiguous `#` block. If still not found, leave the
    # file alone.
    PREAMBLE_PATTERNS = (
        re.compile(r"^#!"),
        re.compile(r"^#\s*encoding\s*:", re.IGNORECASE),
        re.compile(r"^#\s*frozen_string_literal\s*:", re.IGNORECASE),
    )
    start = 0
    while start < len(lines):
        line = lines[start]
        if line.strip() == "":
            start += 1
            continue
        if any(p.match(line) for p in PREAMBLE_PATTERNS):
            start += 1
            continue
        break
    if start >= len(lines) or not EXTRACTOR_HEADER_OPENER.match(lines[start]):
        return src
    # Header runs from `start` until the first non-`#` line
    # (or blank line acting as separator before code). The
    # range `start..end` covers the header block plus any
    # trailing blank line that separates it from the code.
    end = start
    for idx in range(start, len(lines)):
        line = lines[idx]
        if line.startswith("#") or line.strip() == "":
            end = idx + 1
            continue
        break
    # Preserve the preamble lines (shebang / magic comments)
    # that precede the header — `lines[:start]` covers them
    # plus any blanks between them and the header. Dropping
    # the preamble alongside the header (the v1 behavior) was a
    # real bug per PR #133 review: a `# encoding: utf-8` or
    # `# frozen_string_literal: true` line affects Ruby's
    # parser semantics, not just documentation.
    return "".join(lines[:start] + lines[end:])


def main():
    src = sys.stdin.read()
    out = []
    addressed = 0
    for kind, text, indent in split_blocks(src):
        if kind == "it":
            # Categorize against the body ONLY (skip the opener
            # line). Otherwise an `it "demonstrates push behavior"`
            # description containing words like "push" could match
            # the `.push\(\s*[^)]+,` body-pattern (no — that needs
            # a parenthesized literal arg-list), but description
            # phrases like "first(n)" or "min { ... }" used in
            # human prose CAN match by coincidence. Splitting off
            # `lines[1:]` excludes the `it "..." do` opener, so
            # only the actual code inside the block contributes
            # to categorization. Reviewer feedback PR #133.
            lines_after_opener = text.split("\n", 1)
            body = lines_after_opener[1] if len(lines_after_opener) > 1 else ""
            cat = categorize(body)
            if cat:
                first_line = text.splitlines()[0].strip()
                out.append(f"{indent}# skipped ({cat}): {first_line}\n")
                addressed += 1
                continue
        elif kind == "head":
            cat = categorize_head(text.splitlines()[0])
            if cat:
                first_line = text.splitlines()[0].strip()
                out.append(f"{indent}# skipped ({cat}): {first_line}\n")
                addressed += 1
                continue
        out.append(text)
    polished = rewrite_extractor_header("".join(out), addressed)
    sys.stdout.write(polished)


if __name__ == "__main__":
    main()
