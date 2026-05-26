#!/bin/bash
# perf/time_microbench.sh — Empirical Path-A-vs-Path-B comparison
# for the `Time` design discussion. Runs each scenario from
# `crates/rubyrs/benches/time_path_microbench.rb` under
# `/usr/bin/time` N times, reports min-of-N wall, and prints the
# per-scenario A/B ratios.
#
# Purpose: answer "is the user-method-dispatch overhead in Path A
# (pure-Ruby `Time` vendor) acceptable, or do we need Path B (a
# Rust-side `Value::Time` primitive)?" — without doing the Path B
# implementation work just to find out.
#
# Reads:  `target/release/rubyrs` (build first), the .rb script.
# Writes: stdout report.
#
# Usage:
#   ./perf/time_microbench.sh                # default N=1_000_000, RUNS=5
#   BENCH_N=5_000_000 RUNS=3 ./perf/time_microbench.sh
#
# Same `LC_ALL=C /usr/bin/time` shape `perf/wasm_check.sh` uses;
# Darwin and Linux time outputs parse the same.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

BIN="${BIN:-target/release/rubyrs}"
SCRIPT="${SCRIPT:-crates/rubyrs/benches/time_path_microbench.rb}"
RUNS="${RUNS:-5}"
N="${BENCH_N:-1_000_000}"

if [[ ! -x "$BIN" ]]; then
  echo "missing $BIN — build with: cargo build --release -p rubyrs" >&2
  exit 2
fi
if [[ ! -f "$SCRIPT" ]]; then
  echo "missing $SCRIPT" >&2
  exit 2
fi

PLATFORM="$(uname -s)"

# Parse wall time (seconds) from `/usr/bin/time`'s stderr output.
# Darwin format: `        0.46 real         0.45 user         0.00 sys`
# Linux GNU format: `0.46user 0.00system 0:00.46elapsed ...`
# Both report `\d+\.\d+ real` (Darwin) or `0:00.46elapsed` (Linux);
# the awk picks whichever pattern hits first.
measure_ms() {
  local scenario="$1"
  local out
  out=$(BENCH_N="$N" BENCH_SCENARIO="$scenario" \
        LC_ALL=C /usr/bin/time "$BIN" "$SCRIPT" 2>&1 >/dev/null) || {
    echo "ERR" ; return
  }
  if [[ "$PLATFORM" == "Darwin" ]]; then
    # Darwin format — leading whitespace + `0.NN real`.
    local secs
    secs=$(awk '/real/ {print $1; exit}' <<<"$out")
    if [[ -z "$secs" ]]; then
      echo "ERR"; return
    fi
    awk -v s="$secs" 'BEGIN { printf "%d", s * 1000 + 0.5 }'
  else
    # Linux GNU /usr/bin/time format — `0:00.46elapsed`.
    local elapsed
    elapsed=$(awk -F'[ :elapsed]+' '/elapsed/ {
      # Walk fields, find the one ending in "elapsed".
      for (i = 1; i <= NF; i++) {
        if ($i ~ /elapsed$/) {
          gsub(/elapsed$/, "", $i)
          print $i
          exit
        }
      }
    }' <<<"$out")
    if [[ -z "$elapsed" ]]; then
      echo "ERR"; return
    fi
    # `0:00.46` → seconds = 0.46. Strip the m:ss form if present.
    awk -v e="$elapsed" 'BEGIN {
      if (e ~ /:/) {
        split(e, parts, ":")
        secs = parts[1] * 60 + parts[2]
      } else {
        secs = e
      }
      printf "%d", secs * 1000 + 0.5
    }'
  fi
}

min_of() {
  local scenario="$1"
  local min_ms="" ms
  for ((i = 1; i <= RUNS; i++)); do
    ms="$(measure_ms "$scenario")"
    if [[ "$ms" == "ERR" ]]; then
      echo "ERR"; return
    fi
    if [[ -z "$min_ms" || "$ms" -lt "$min_ms" ]]; then
      min_ms="$ms"
    fi
  done
  echo "$min_ms"
}

ratio() {
  local a_ms="$1" b_ms="$2"
  if [[ "$a_ms" == "ERR" || "$b_ms" == "ERR" || "$b_ms" -eq 0 ]]; then
    echo "n/a"
    return
  fi
  awk -v a="$a_ms" -v b="$b_ms" 'BEGIN { printf "%.1fx", a / b }'
}

echo "Time Path A vs B microbench"
echo "  binary  : $BIN"
echo "  script  : $SCRIPT"
echo "  N       : $N iterations per scenario"
echo "  runs    : MIN of $RUNS"
echo ""

printf "%-12s  %8s  %8s  %s\n" "scenario" "A ms" "B ms" "A/B"
printf "%-12s  %8s  %8s  %s\n" "--------" "----" "----" "---"
for pair in to_i plus cmp construct; do
  a_ms="$(min_of "a_$pair")"
  b_ms="$(min_of "b_$pair")"
  printf "%-12s  %8s  %8s  %s\n" "$pair" "$a_ms" "$b_ms" "$(ratio "$a_ms" "$b_ms")"
done

echo ""
echo "A = user-class dispatch (Pure-Ruby Time vendor shape)"
echo "B = primitive_call dispatch (Rust Value::Time shape, surrogate)"
echo "Ratios < ~10× → Path A's overhead is invisible at niche scales"
echo "(Brewfile / Sinatra-shape DSLs make O(10-100) Time calls per script)."
