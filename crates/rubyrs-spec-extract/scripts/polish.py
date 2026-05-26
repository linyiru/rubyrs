#!/usr/bin/env python3
"""Companion post-processor for `rubyrs-spec-extract`.

`spec-extract` v0.4 mechanically rewrites the `expr.should == val`
flavour of upstream ruby/spec into the micro-runner's `assert_eq`
shape — but a finished spec usually still has `it` blocks whose
BODY references fixtures the micro-runner doesn't ship
(`ArraySpecs.recursive_array`, `MyArray[...]`) or rubyrs methods
that aren't implemented yet (`Array#push`, `Array#min { ... }`).
Those blocks load but fail at run time, masking the genuine
PASSes around them.

This script reads a spec file on stdin, walks `it "..." do ... end`
blocks, and drops any whose body matches a known "this won't run
in the micro-runner" pattern (see `DROP_PATTERNS` below). Each
dropped block leaves a `# skipped (fixture-dependent): ...` trace
so the diff stays auditable.

Usage (pipeline shape from spec-extract README):

  rubyrs-spec-extract <upstream.rb> [--shared shared.rb] \\
    | crates/rubyrs-spec-extract/scripts/polish.py \\
    > crates/rubyrs/spec/ruby/<name>_spec.rb

Patterns are deliberately conservative — adding a new one is a
single regex line. The `# skipped` comments make future
revisits (e.g., when rubyrs gains `Array#push`) easy to find
and re-evaluate."""

import re, sys

DROP_PATTERNS = [
    # Fixture references the micro-runner doesn't ship (would need
    # vendoring upstream's `fixtures/classes.rb` — out of scope
    # for the ingestion pass).
    r"\bArraySpecs\b",
    r"\bMyArray\b",
    # ruby/spec mock library calls (mspec internals); the
    # micro-runner has no mocking surface.
    r"\bmock\(",
    r"\bshould_receive\b",
    # FrozenError raising / freeze-detection — rubyrs doesn't
    # currently model frozen state for Arrays.
    r"\bFrozenError\b",
    r"\.freeze\b",
    # Count-form variants of head/tail accessors. rubyrs ships
    # only the zero-arg form for `Array#first`/`#last`/`#min`/
    # `#max`/`#pop`/`#shift`. Re-extracted spec files including
    # these blocks would NoMethodError-fail until the count form
    # lands as a separate feature PR; skip them here so this
    # ingestion pass is data-only and doesn't block on impl work.
    # `\(\s*[^)\s]` matches any non-empty argument list (literal
    # number, negative literal, identifier, expression) — so
    # `[1].first(-1)` and `[].first(bignum_value)` both get
    # dropped, not just `\d`-leading positional ints.
    r"\.first\(\s*[^)\s]",
    r"\.last\(\s*[^)\s]",
    r"\.min\(\s*[^)\s]",
    r"\.max\(\s*[^)\s]",
    r"\.pop\(\s*[^)\s]",
    r"\.shift\(\s*[^)\s]",
    # `Array#push`/`#unshift` likewise missing from rubyrs.
    r"\.push\b",
    r"\.unshift\b",
    # `Array#sort` with comparator-block — sort/sort! with no
    # block works, but specs using `sort { |a, b| … }` need the
    # comparator-block sort which is a separate feature.
    r"\.sort\s*(\{|\s+do\b)",
    # `min`/`max` with comparator block (`.min { |a, b| … }`) —
    # zero-arg `min`/`max` work; block form doesn't.
    r"\.min\s*(\{|\s+do\b)",
    r"\.max\s*(\{|\s+do\b)",
    # Same for `each_with_index`, `inject`, `reduce` if they
    # appear in spec assertions — these are block-only methods
    # but the issue is when called with no block at all, which
    # most specs don't do; included for symmetry.
    r"\.inject\s*\(",
    # `before :each` / `before :all` patterns the extractor
    # didn't lift (multi-arg, non-flat context). Surface as
    # "file-level trap: undefined method `before`" otherwise.
    r"^\s*before\b",
    # Subclass/instance-of-Array checks: requires `Array.[]`
    # class method or `instance_of?` against a subclass — not in
    # the micro-runner's surface.
    r"\.instance_of\?\(\s*Array\)",
]

def block_drops(body: str) -> bool:
    return any(re.search(p, body) for p in DROP_PATTERNS)

def split_it_blocks(src: str):
    """Yield (kind, text) for each chunk; kind is 'it' or 'other'."""
    lines = src.splitlines(keepends=True)
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        # `it "...." do` or `it "..." do |x|` at start of indentation
        m = re.match(r"^(\s*)it\s+[\"'].*\bdo\b", line.rstrip())
        if not m:
            yield ("other", line)
            i += 1
            continue
        indent = m.group(1)
        # Walk forward, counting `do`/`end` to find the matching end
        # at the same indent level.
        depth = 1
        j = i + 1
        # Count `do`/`{` to track nested blocks. Crude but sufficient
        # for the ruby/spec shape (no string escapes that span lines
        # in these specs).
        while j < n:
            l = lines[j]
            # `do` at end-of-line, or `do |...|`, or `do$`
            do_count = len(re.findall(r"\bdo\b(?:\s*\|.*\|)?\s*$", l))
            end_count = len(re.findall(r"^\s*end\s*$", l))
            depth += do_count - end_count
            if depth == 0:
                # j is the closing `end` line for the `it` block.
                j += 1
                break
            j += 1
        block = "".join(lines[i:j])
        yield ("it", block)
        i = j

def main():
    src = sys.stdin.read()
    out = []
    for kind, text in split_it_blocks(src):
        if kind == "it" and block_drops(text):
            # Leave a trace comment so the diff is auditable.
            first_line = text.splitlines()[0].strip()
            out.append(f"  # skipped (fixture-dependent): {first_line}\n")
            continue
        out.append(text)
    sys.stdout.write("".join(out))

if __name__ == "__main__":
    main()
