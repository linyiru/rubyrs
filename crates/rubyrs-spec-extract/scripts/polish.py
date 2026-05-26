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
    # only the zero-arg form for `Array#first`/`#last`/`#min`/
    # `#max`/`#pop`/`#shift`. `\(\s*[^)\s]` matches any non-empty
    # argument list (literal number, negative literal, identifier,
    # expression) — so `[1].first(-1)` and `[].first(bignum_value)`
    # both get dropped.
    (r"\.first\(\s*[^)\s]", "method-not-implemented"),
    (r"\.last\(\s*[^)\s]", "method-not-implemented"),
    (r"\.min\(\s*[^)\s]", "method-not-implemented"),
    (r"\.max\(\s*[^)\s]", "method-not-implemented"),
    (r"\.pop\(\s*[^)\s]", "method-not-implemented"),
    (r"\.shift\(\s*[^)\s]", "method-not-implemented"),
    # `Array#push` MULTI-ARG form is not yet in rubyrs — single-arg
    # `.push(x)` works, but `.push(a, b, c)` raises NoMethodError.
    # Match only the multi-arg shape: open paren + non-`)` content
    # + `,` (the comma is what makes it multi-arg). Single-arg
    # `[1].push(2)` does NOT match — `.push(\d+)` has no comma.
    (r"\.push\(\s*[^)]+,", "method-not-implemented"),
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
                do_count = len(re.findall(r"\bdo\b(?:\s*\|.*\|)?\s*$", l))
                stmt_open_count = len(
                    re.findall(
                        r"^\s*(?:if|unless|while|until)\b",
                        l,
                    )
                )
                kw_open_count = len(
                    re.findall(
                        r"^\s*(?:class|module|def|case|begin|for)\b",
                        l,
                    )
                )
                end_count = len(re.findall(r"\bend\b", l))
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
    # `before :all` and `after :each`/`:all` — distinct from the
    # top-level `before :each` blocks polish.py handles via
    # DROP_TOP_LEVEL_HEADS (those are caught and traced with
    # `# skipped (before-not-lifted)`). The :all variants need
    # different handling (run-once-per-describe instead of
    # per-`it`) and currently fall through unaddressed.
    r"^\s*before\s+:all\b",
    r"^\s*after\b",
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
    output (`it_behaves_like` without --shared, `context`,
    `before :all`, `after`), the header is still accurate and we
    leave it intact so readers know the file isn't yet
    micro-runner-ready.

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
            cat = categorize(text)
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
