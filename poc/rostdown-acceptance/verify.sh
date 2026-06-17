#!/usr/bin/env bash
# Byte-identity gate: for every prepared file, does rostdown's accepted
# output match kramdown EXACTLY? This is the contract `decline_scan` does
# NOT check (it only tallies accept-vs-decline). "Accepted" here means
# rostdown rendered AND the bytes equal kramdown's — so WRONG must be 0;
# any nonzero count is an accept-but-wrong bug (the cardinal sin) to fix by
# declining or correcting.
#
# Needs Ruby with the `kramdown` + `kramdown-parser-gfm` gems (unlike
# run.sh, which is Rust-only). Run ./fetch.sh first. Comparison is done on
# temp files with `cmp` so trailing bytes count (NOT $(...) capture, which
# strips trailing newlines).
set -u
cd "$(dirname "$0")"
ROOT=$(cd ../.. && pwd)
PREP=prepared
ORACLE="$PWD/kramdown_oracle.rb"

[ -d "$PREP" ] && [ -n "$(ls -A "$PREP" 2>/dev/null)" ] || {
  echo "no prepared corpus — run ./fetch.sh first" >&2
  exit 1
}

( cd "$ROOT" && cargo build -q -p rostdown --example render )
BIN="$ROOT/target/debug/examples/render"

acc=0; dec=0; wrong=0
WF=$(mktemp); RD=$(mktemp); KM=$(mktemp)
trap 'rm -f "$WF" "$RD" "$KM"' EXIT

while IFS= read -r f; do
  if ! "$BIN" "$f" --gfm > "$RD" 2>/dev/null < /dev/null; then
    dec=$((dec + 1)); continue
  fi
  acc=$((acc + 1))
  ruby "$ORACLE" < "$f" > "$KM" 2>/dev/null
  if ! cmp -s "$RD" "$KM"; then
    wrong=$((wrong + 1))
    sig=$(diff "$RD" "$KM" | grep -E '^[<>]' | head -1 | cut -c1-100)
    printf '%s\n    %s\n' "$f" "$sig" >> "$WF"
  fi
done < <(find "$PWD/$PREP" -type f)

tot=$((acc + dec))
pct=$(awk -v a="$acc" -v t="$tot" 'BEGIN{ if (t>0) printf "%.1f%%", 100*a/t; else print "n/a" }')
echo "byte-identical acceptance: $acc/$tot ($pct accepted, byte-identical to kramdown)"
echo "declined (→ Ruby fallback): $dec"
echo "ACCEPT-BUT-WRONG (must be 0):  $wrong"
if [ "$wrong" -gt 0 ]; then
  echo "--- accept-but-wrong files (first differing line) ---"
  cat "$WF"
  exit 1
fi
