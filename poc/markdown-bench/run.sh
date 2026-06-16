#!/usr/bin/env bash
# Cross-language markdown render benchmark driver.
# Each CLI self-times a warmup + timed loop over the same corpus and
# prints "<engine>\t<ns_per_op>\t<mb_per_s>\t<out_bytes>". We tag each
# row with its language/runtime and print a table sorted by throughput.
set -euo pipefail
cd "$(dirname "$0")"

CORPUS="${CORPUS:-corpus/bench.md}"
N="${N:-400}"          # iters for compiled engines
NR="${NR:-120}"        # iters for Ruby (slower)
SIZE=$(wc -c < "$CORPUS" | tr -d ' ')

RAW=$(mktemp)
run() { # <lang> <cmd...>
  local lang="$1"; shift
  local line; line=$("$@")
  printf '%s\t%s\n' "$lang" "$line" >> "$RAW"
}

echo "corpus: $CORPUS ($SIZE bytes)   iters: compiled=$N ruby=$NR"
echo "building…"
( cd rust && cargo build --release -q )
( cd go && go build -o md-bench-go . )

RBIN=rust/target/release/md-bench-rust
GBIN=go/md-bench-go

# --- no-highlight raw engine throughput (apples-to-apples) ---
run "Rust"    "$RBIN" pulldown   "$CORPUS" "$N"
run "Rust"    "$RBIN" comrak     "$CORPUS" "$N"
run "Rust"    "$RBIN" rostdown   "$CORPUS" "$N"
run "Go"      "$GBIN" goldmark   "$CORPUS" "$N"
run "Go"      "$GBIN" blackfriday "$CORPUS" "$N"
run "JS/V8"   node js/bench.mjs  marked      "$CORPUS" "$N"
run "JS/V8"   node js/bench.mjs  markdown-it "$CORPUS" "$N"
run "Ruby"    ruby ruby/bench_ruby.rb kramdown "$CORPUS" "$NR"
if ruby -e 'require "commonmarker"' >/dev/null 2>&1; then
  run "Ruby→Rust" ruby ruby/bench_ruby.rb commonmarker "$CORPUS" "$N"
fi

echo
printf '%-14s %-14s %12s %10s %10s\n' "engine" "lang/runtime" "ns/op" "MB/s" "out_B"
printf '%-14s %-14s %12s %10s %10s\n' "------" "------------" "-----" "----" "-----"
sort -t$'\t' -k4 -nr "$RAW" | while IFS=$'\t' read -r lang engine ns mbs out; do
  printf '%-14s %-14s %12s %10s %10s\n' "$engine" "$lang" "$ns" "$mbs" "$out"
done
rm -f "$RAW"

echo
echo "Note: no syntax highlighting (all engines emit plain <pre><code>)."
echo "rostdown additionally does smart typography + heading auto-ids."
echo "Ruby gem end-to-end (kramdown vs kramdown-rostdown) — see bin/bench.rb."
