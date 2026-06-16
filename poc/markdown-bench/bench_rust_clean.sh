#!/usr/bin/env bash
# Clean rostdown-vs-Rust-peers measurement for the rostdown README.
# Default build: pulldown, comrak, rostdown (zero-dep/unsafe-free path).
# Turbo  build: rostdown (arena + NEON simd, ScopedAlloc global alloc).
# 7 interleaved runs per engine, N iters each; reports the MEDIAN.
set -euo pipefail
cd "$(dirname "$0")"
CORPUS="$(pwd)/corpus/bench.md"
N="${N:-1000}"
REPS="${REPS:-7}"
SIZE=$(wc -c < "$CORPUS" | tr -d ' ')
RAW=$(mktemp)

median() { sort -n | awk '{a[NR]=$0} END{print (NR%2)? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}'; }

echo "corpus: $SIZE bytes   N=$N   reps=$REPS   (Apple M2 Max, arm64)"
echo "building default (no arena/simd, System alloc)…"
( cd rust && cargo build --release -q )
BIN=rust/target/release/md-bench-rust

for r in $(seq 1 "$REPS"); do
  for e in pulldown comrak rostdown; do
    "$BIN" "$e" "$CORPUS" "$N" >> "$RAW"
  done
done

echo "building turbo (--features turbo)…"
( cd rust && cargo build --release -q --features turbo )
for r in $(seq 1 "$REPS"); do
  "$BIN" rostdown "$CORPUS" "$N" | sed 's/^rostdown/rostdown-turbo/' >> "$RAW"
done

echo
printf '%-18s %12s %10s %10s\n' "engine" "ns/op(med)" "MB/s(med)" "out_B"
printf '%-18s %12s %10s %10s\n' "------" "----------" "---------" "-----"
for e in rostdown-turbo rostdown pulldown comrak; do
  ns=$(awk -F'\t' -v e="$e" '$1==e{print $2}' "$RAW" | median)
  mb=$(awk -F'\t' -v e="$e" '$1==e{print $3}' "$RAW" | median)
  ob=$(awk -F'\t' -v e="$e" '$1==e{print $4; exit}' "$RAW")
  printf '%-18s %12.0f %10.1f %10s\n' "$e" "$ns" "$mb" "$ob"
done
rm -f "$RAW"
