#!/usr/bin/env bash
# Measure rostdown's real-world ACCEPT-vs-DECLINE rate over the prepared
# corpus: what fraction of real pages rostdown renders rather than declines
# (→ Ruby-kramdown fallback). Run ./fetch.sh first. Uses the rostdown
# `decline_scan` example (Rust-only, fast).
#
# NOTE: this does NOT prove the accepted output is byte-identical to
# kramdown — only that rostdown didn't decline. For the correctness gate
# (accepted ⇒ bytes equal kramdown, WRONG must be 0) run ./verify.sh.
set -euo pipefail
cd "$(dirname "$0")"
PREP=prepared
ROOT=$(cd ../.. && pwd)

[ -d "$PREP" ] && [ -n "$(ls -A "$PREP" 2>/dev/null)" ] || {
  echo "no prepared corpus — run ./fetch.sh first" >&2
  exit 1
}

# Build the example once so per-source runs don't interleave cargo output.
( cd "$ROOT" && cargo build -q -p rostdown --example decline_scan )

scan() { # <files…> → emits the decline_scan report
  (cd "$ROOT" && xargs cargo run -q -p rostdown --example decline_scan -- 2>/dev/null)
}

echo "rostdown real-content acceptance (front matter stripped; Liquid NOT"
echo "expanded — see README caveats). Higher = more pages accelerated;"
echo "the rest fall back to Ruby kramdown (still correct)."
echo
printf '%-14s %s\n' "source" "acceptance"
printf '%-14s %s\n' "------" "----------"
for d in "$PREP"/*/; do
  name=$(basename "$d")
  files=$(find "$PWD/$d" -type f)
  [ -n "$files" ] || continue
  acc=$(printf '%s\n' "$files" | scan | sed -n '2p' | sed 's/accepted: *//')
  printf '%-14s %s\n' "$name" "$acc"
done

echo
echo "=== combined acceptance + top decline reasons ==="
find "$PWD/$PREP" -type f | scan | sed -n '1,16p'
