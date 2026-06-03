#!/usr/bin/env bash
# lint-doc-orientation.sh — flag rustdoc-orientation regressions
# where a `///` doc block ends up attached to the wrong function.
#
# The hazard: when inserting a new helper between an existing
# doc comment and its function, the doc visually slides onto
# the new helper. Caught twice in review (PR #338 cycle 1,
# PR #349 code-review post-cycle):
#
#     /// X's doc — mentions `self.X(...)`
#     pub(crate) fn Y(...) { ... }   <- doc now attached to Y
#
#     pub(crate) fn X(...) { ... }   <- undocumented
#
# RULE: for each `pub fn` / `pub(crate) fn NAME` with a `///`
# doc block directly above, if that doc text contains
# `self.OTHER(` where `OTHER` is ANOTHER `pub` (or
# `pub(crate)`) fn defined in the same file (and OTHER !=
# NAME), the doc was probably meant for OTHER. Flag the
# doc-bearing (suspected mis-attached) site — the report names
# OTHER so the fix site is obvious from grep.
#
# False positives are possible (legitimate cross-references in
# docs). Suppress with `// allow: doc-orientation` in either:
#   1. As a trailing comment on the `pub fn` declaration line,
#      e.g. `pub(crate) fn X(...) { // allow: doc-orientation`.
#   2. On the line directly above the `///` doc block (so the
#      `///` stays adjacent to the fn — putting it between
#      breaks the doc attachment we are trying to verify).
#
# Run from repo root. Scans crates/rubyrs/src/.
#
# Exit codes:
#   0  — no suspects (or all annotated as allowed).
#   1  — at least one suspect; lines printed to stderr.
#   2  — ROOT directory does not exist (usage error: wrong cwd
#        or invalid `ROOT=` override).

set -euo pipefail

ROOT="${ROOT:-crates/rubyrs/src}"

if [[ ! -d "$ROOT" ]]; then
    echo "lint-doc-orientation: ROOT '$ROOT' does not exist" >&2
    exit 2
fi

python3 - "$ROOT" <<'PY'
import re, sys, glob

root = sys.argv[1]
doc_re = re.compile(r'^\s*///')
fn_re = re.compile(r'^\s*pub(?:\(crate\))?\s*(?:async\s+|const\s+|unsafe\s+|extern\s+(?:"[^"]*"\s+)?)*fn\s+(\w+)')
attr_re = re.compile(r'^\s*#\[')
allow_re = re.compile(r'allow:\s*doc-orientation')

suspects = []

for path in sorted(glob.glob(f'{root}/**/*.rs', recursive=True)):
    with open(path, encoding='utf-8') as f:
        lines = f.readlines()
    # First pass — collect every `pub fn NAME` declared here.
    same_file_fns = set()
    for line in lines:
        m = fn_re.match(line)
        if m:
            same_file_fns.add(m.group(1))
    # Pre-pass: classify each line as 'attr' / 'doc' / 'blank' /
    # 'other'. Multi-line attributes such as
    #     #[cfg(any(
    #         foo,
    #         bar
    #     ))]
    # are folded into a single 'attr' span by tracking square
    # bracket depth — without this, backward-scan would stop at
    # the inner lines and miss the doc block above the attr,
    # creating false negatives for exactly the kind of
    # mis-attached doc this lint exists to catch.
    klass = [None] * len(lines)
    in_attr = False
    attr_depth = 0
    for idx, line in enumerate(lines):
        s = line.strip()
        if in_attr:
            attr_depth += s.count('[') - s.count(']')
            klass[idx] = 'attr'
            if attr_depth <= 0:
                in_attr = False
        elif attr_re.match(line):
            attr_depth = s.count('[') - s.count(']')
            klass[idx] = 'attr'
            if attr_depth > 0:
                in_attr = True
        elif doc_re.match(line):
            klass[idx] = 'doc'
        elif s == '':
            klass[idx] = 'blank'
        else:
            klass[idx] = 'other'
    # Second pass — for each `pub fn NAME`, check doc text above.
    for i, line in enumerate(lines):
        m = fn_re.match(line)
        if not m:
            continue
        fn_name = m.group(1)
        # Walk backward, skipping blanks and attribute spans
        # (single-line or multi-line, handled by the pre-pass).
        j = i - 1
        while j >= 0 and klass[j] in ('blank', 'attr'):
            j -= 1
        # Collect contiguous `///` lines (the doc block).
        doc_lines = []
        while j >= 0 and doc_re.match(lines[j]):
            doc_lines.insert(0, lines[j])
            j -= 1
        if not doc_lines:
            continue
        doc_text = ''.join(doc_lines)
        # Allow-list opt-out — two suppression sites:
        #   (a) trailing `// allow: doc-orientation` on the
        #       `pub fn` declaration line itself; or
        #   (b) `// allow: doc-orientation` on the line directly
        #       above the `///` doc block.
        # Putting it BETWEEN the doc and the fn would break the
        # rustdoc attachment we are trying to verify, so we do
        # NOT accept that placement.
        if allow_re.search(lines[i]):
            continue
        if j >= 0 and allow_re.search(lines[j]):
            continue
        # Look for `self.OTHER(` references where OTHER is a fn
        # defined in this file and OTHER != fn_name.
        # `\bself\.` requires the literal `self.` token —
        # filters out unrelated `OTHER(` mentions in prose.
        callees = re.findall(r'\bself\.([a-z_][a-z_0-9]*)\s*\(', doc_text)
        for callee in callees:
            if callee != fn_name and callee in same_file_fns:
                suspects.append((path, i + 1, fn_name, callee))
                break  # one finding per fn is enough

if suspects:
    for path, line, name, callee in suspects:
        print(
            f"{path}:{line}: rustdoc-orientation: `fn {name}` has a `///` "
            f"doc block mentioning `self.{callee}(...)`, but `{callee}` is a "
            f"different fn defined in the same file. The doc may have been "
            f"orphaned by an inserted helper (PR #338 cycle 1 / PR #349 pattern). "
            f"Move the doc to immediately precede `fn {callee}`, or annotate "
            f"with `// allow: doc-orientation`.",
            file=sys.stderr,
        )
    sys.exit(1)
PY
