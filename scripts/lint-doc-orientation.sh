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
# RULE: for each `pub(crate) fn NAME` with a `///` doc block
# directly above, if that doc text contains `self.OTHER(`
# where `OTHER` is ANOTHER `pub(crate) fn` defined in the same
# file (and OTHER != NAME), the doc was probably meant for
# OTHER. Flag both sites for review.
#
# False positives are possible (legitimate cross-references in
# docs). Annotate with `// allow: doc-orientation` on the line
# above the `pub fn` declaration to suppress.
#
# Run from repo root. Scans crates/rubyrs/src/.
#
# Exit codes:
#   0  — no suspects (or all annotated as allowed).
#   1  — at least one suspect; lines printed to stderr.

set -euo pipefail

ROOT="${ROOT:-crates/rubyrs/src}"

if [[ ! -d "$ROOT" ]]; then
    echo "lint-doc-orientation: ROOT '$ROOT' does not exist" >&2
    exit 2
fi

python3 - "$ROOT" <<'PY'
import os, re, sys, glob

root = sys.argv[1]
doc_re = re.compile(r'^\s*///')
fn_re = re.compile(r'^\s*pub(?:\(crate\))?\s*fn\s+(\w+)')
attr_re = re.compile(r'^\s*#\[')
allow_re = re.compile(r'allow:\s*doc-orientation')

suspects = []

for path in sorted(glob.glob(f'{root}/**/*.rs', recursive=True)):
    with open(path) as f:
        lines = f.readlines()
    # First pass — collect every `pub fn NAME` declared here.
    same_file_fns = set()
    for line in lines:
        m = fn_re.match(line)
        if m:
            same_file_fns.add(m.group(1))
    # Second pass — for each `pub fn NAME`, check doc text above.
    for i, line in enumerate(lines):
        m = fn_re.match(line)
        if not m:
            continue
        fn_name = m.group(1)
        # Walk backward, skipping blanks and #[..] attributes.
        j = i - 1
        while j >= 0 and (lines[j].strip() == '' or attr_re.match(lines[j])):
            j -= 1
        # Collect contiguous `///` lines (the doc block).
        doc_lines = []
        while j >= 0 and doc_re.match(lines[j]):
            doc_lines.insert(0, lines[j])
            j -= 1
        if not doc_lines:
            continue
        doc_text = ''.join(doc_lines)
        # Allow-list opt-out: `// allow: doc-orientation` on the
        # line just above the fn (i.e. after the doc block).
        for ann_line in lines[i:i+1]:
            if allow_re.search(ann_line):
                doc_text = ''  # skip
                break
        # Also accept the annotation directly above the doc block.
        if j >= 0 and allow_re.search(lines[j]):
            doc_text = ''
        if not doc_text:
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
            f"{path}:{line}: rustdoc-orientation: `pub fn {name}` has a `///` "
            f"doc block mentioning `self.{callee}(...)`, but `{callee}` is a "
            f"different fn defined in the same file. The doc may have been "
            f"orphaned by an inserted helper (PR #338 cycle 1 / PR #349 pattern). "
            f"Move the doc to immediately precede `pub fn {callee}`, or annotate "
            f"with `// allow: doc-orientation`.",
            file=sys.stderr,
        )
    sys.exit(1)
PY
