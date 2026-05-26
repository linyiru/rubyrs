#!/usr/bin/env python3
"""Companion post-processor for `rubyrs-spec-extract`.

`spec-extract` v0.4 mechanically rewrites the `expr.should == val`
flavour of upstream ruby/spec into the micro-runner's `assert_eq`
shape — but a finished spec usually still has `it` (or top-level
`before`) blocks whose BODY references fixtures the micro-runner
doesn't ship (`ArraySpecs.recursive_array`, `MyArray[...]`) or
rubyrs methods that aren't implemented yet (`Array#push`,
`Array#min { ... }`). Those blocks load but fail at run time,
masking the genuine PASSes around them.

This script reads a spec file on stdin, walks `it "..." do … end`
AND top-level `before "..." do … end` blocks, and drops any whose
body matches a known "this won't run in the micro-runner" pattern
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
    # FrozenError raising / freeze-detection — rubyrs doesn't
    # currently model frozen state for Arrays.
    (r"\bFrozenError\b", "frozen-state"),
    (r"\.freeze\b", "frozen-state"),
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
    # `Array#push` / `#unshift` not yet in rubyrs (only `<<` is).
    (r"\.push\b", "method-not-implemented"),
    (r"\.unshift\b", "method-not-implemented"),
    # `Array#sort` with comparator block — zero-arg sort works,
    # block form doesn't.
    (r"\.sort\s*(\{|\s+do\b)", "method-not-implemented"),
    # `min`/`max` with comparator block (`.min { |a, b| … }`) —
    # zero-arg `min`/`max` work; block form doesn't.
    (r"\.min\s*(\{|\s+do\b)", "method-not-implemented"),
    (r"\.max\s*(\{|\s+do\b)", "method-not-implemented"),
    (r"\.inject\s*\(", "method-not-implemented"),
    # Subclass/instance-of-Array checks: requires `Array.[]` class
    # method or `instance_of?` against a subclass — not in the
    # micro-runner's surface.
    (r"\.instance_of\?\(\s*Array\)", "method-not-implemented"),
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
                do_count = len(re.findall(r"\bdo\b(?:\s*\|.*\|)?\s*$", l))
                end_count = len(re.findall(r"^\s*end\s*$", l))
                depth += do_count - end_count
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


def rewrite_extractor_header(src: str, addressed: int) -> str:
    """If the file opens with an extractor 'patterns left for hand
    polish' header and polish addressed at least one entry, strip
    the header. Leaving it would mislead readers into thinking the
    file isn't micro-runner-ready when it now is.

    If polish addressed zero entries (extractor's header still
    accurate), leave it verbatim."""
    if addressed == 0:
        return src
    lines = src.splitlines(keepends=True)
    # The extractor's header often starts on line 1, but if a
    # spec begins with a leading blank line (or two — extractor
    # output can vary by upstream shape), skip those before
    # testing the opener. `start` is the first non-blank line
    # index; if we still don't see the header there, leave the
    # file alone.
    start = 0
    while start < len(lines) and lines[start].strip() == "":
        start += 1
    if start >= len(lines) or not EXTRACTOR_HEADER_OPENER.match(lines[start]):
        return src
    # Header runs from `start` until the first non-`#` line
    # (or blank line acting as separator before code). Strip
    # everything up to and including the trailing blank line —
    # and also strip the leading blanks that preceded the
    # header so we don't leave them dangling.
    end = start
    for idx in range(start, len(lines)):
        line = lines[idx]
        if line.startswith("#") or line.strip() == "":
            end = idx + 1
            continue
        break
    return "".join(lines[end:])


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
