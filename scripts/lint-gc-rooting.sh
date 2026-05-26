#!/usr/bin/env bash
# lint-gc-rooting.sh — flag the structural GC-rooting hole
# described in issue #90.
#
# The hazard: in VM code, a `Value` taken out of `self.stack`
# or `args` (via `pop` / `swap_remove` / `drain`) becomes a
# Rust local. It is NOT in any GC root set — not `self.stack`,
# not `self.pinned`, not any frame's `locals`, not globals or
# constants. If `self.maybe_gc()` runs while only that local
# holds a reference, STRESS_GC=1 reaps the slot, and the
# subsequent `self.heap.alloc(...)` allocates a HeapObj
# referencing a recycled slot — surfacing as
# `class_of called on non-Object slot` or stack overflow in
# `to_inspect`.
#
# The structural fix is to wrap the path in `PinGuard::new`
# and pin every heap-bearing Value across the `maybe_gc` +
# `alloc` window. That pattern shows up in the source as
# `g.vm.maybe_gc()` / `g.vm.heap.alloc(...)`, which this
# script considers safe.
#
# RULE: flag any `self.maybe_gc()` line where ALL of the
# following hold:
#   1. The next FWIN lines contain a `self.heap.alloc(`.
#   2. The preceding LOOKBACK lines contain a Value-bearing
#      drain — `self.stack.pop()`, `self.stack.drain`,
#      `self.stack.swap_remove`, `args.swap_remove`,
#      `args.drain`, `args.pop`, or a direct `args[N]`
#      reference.
#   3. There is no `PinGuard::new(self)` in the preceding
#      LOOKBACK lines (PinGuard scope makes the path safe;
#      such sites also typically use `g.vm.maybe_gc()` and
#      so won't match rule #1's anchor either).
#   4. The maybe_gc line does not carry an inline
#      `// allow: gc-rooting` justification.
#
# This is the exact shape described in issue #90. It is a
# heuristic, not a proof: a Value held inside a tuple,
# inside a struct field, or reached through a chain of
# borrows could escape the regex. Escalate to Option 2 of
# the issue if such a hole slips past.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FWIN=10        # lines forward from maybe_gc to find heap.alloc
LOOKBACK=15    # lines backward to find a Value drain (in the
               # same logical block — the backward walk also
               # stops at any line that is just `}` / `},` / `};`,
               # which terminates the enclosing scope and prevents
               # false-positives from drains in unrelated earlier
               # match arms or Op handlers)
# Portable mktemp invocation: GNU mktemp accepts no-template form,
# BSD/macOS mktemp requires either a template path or `-t prefix`
# with a 6+ X-block. `-t` is supported by both flavours, but BSD
# uses TMPDIR + prefix and GNU uses prefix.XXXXXX in TMPDIR — so
# we hand BSD's stricter form (a template suffix) which GNU also
# honours. Required for the macos-latest CI runner.
TMP="$(mktemp -t gc-rooting-lint.XXXXXX)"
trap 'rm -f "$TMP"' EXIT

# Walk every vm/*.rs file. awk is the natural fit: per-line
# state with sliding windows is awkward in pure bash, trivial
# in awk. Aggregate results into $TMP so the FOUND signal
# survives the subshell created by the pipe.
find "$ROOT/crates/rubyrs/src/vm" -name '*.rs' -print0 |
while IFS= read -r -d '' file; do
    rel="${file#$ROOT/}"
    awk -v FWIN="$FWIN" -v LB="$LOOKBACK" -v FILE="$rel" '
        { lines[NR] = $0 }
        END {
            n = NR
            for (i = 1; i <= n; i++) {
                line = lines[i]
                # The unprotected maybe_gc form. PinGuard sites
                # use `g.vm.maybe_gc()` and are intentionally
                # not matched.
                if (line !~ /self\.maybe_gc\(\)/) continue
                if (line ~ /allow: gc-rooting/) continue
                # Forward window: must find heap.alloc nearby.
                hit_alloc = 0
                for (j = i + 1; j <= n && j <= i + FWIN; j++) {
                    if (lines[j] ~ /self\.heap\.alloc\(/) { hit_alloc = j; break }
                }
                if (hit_alloc == 0) continue
                # Backward window: must find a Value drain AND
                # must NOT find a PinGuard::new(self) (which
                # would mean the path is already pinned).
                drain = 0
                pinned = 0
                low = (i - LB < 1) ? 1 : i - LB
                for (k = i - 1; k >= low; k--) {
                    p = lines[k]
                    if (p ~ /PinGuard::new\(self\)/) { pinned = 1; break }
                    # Scope-boundary heuristic: a bare `}` (or
                    # `},` / `};`) at any indent closes the block
                    # we are scanning. Drains beyond it live in a
                    # different match arm or Op handler.
                    if (p ~ /^[[:space:]]*\}[,;]?[[:space:]]*$/) break
                    if (p ~ /self\.stack\.pop\(\)/) drain = 1
                    else if (p ~ /self\.stack\.drain/) drain = 1
                    else if (p ~ /self\.stack\.swap_remove/) drain = 1
                    else if (p ~ /args\.swap_remove/) drain = 1
                    else if (p ~ /args\.drain/) drain = 1
                    else if (p ~ /args\.pop/) drain = 1
                    else if (p ~ /args\[[^]]+\]/) drain = 1
                }
                if (pinned) continue
                if (!drain) continue
                printf("%s:%d: self.maybe_gc() + self.heap.alloc() (line %d) follows a Value drain in the same scope without PinGuard or `// allow: gc-rooting`\n", FILE, i, hit_alloc)
            }
        }
    ' "$file"
done > "$TMP"

if [ -s "$TMP" ]; then
    cat "$TMP"
    cat <<'EOF' >&2

GC rooting lint failed. Each flagged site must either:

  1. Use a PinGuard around the alloc:
       let mut g = PinGuard::new(self);
       g.pin(value_held_across_alloc.clone());
       g.vm.maybe_gc();
       let id = g.vm.heap.alloc(...);
     The `g.vm.maybe_gc()` form bypasses this lint because
     the PinGuard keeps the value in `self.pinned`, part of
     the GC root set.

  2. Be justified inline:
       self.maybe_gc(); // allow: gc-rooting — <reason>
     e.g. "alloc'd HeapObj holds only Value::Int / Sym, no
     heap-bearing slot at risk".

See issue #90 for the full pattern, the seven prior incidents,
and the rationale behind each escape hatch.
EOF
    exit 1
fi

echo "GC rooting lint passed."
