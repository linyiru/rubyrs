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
# Capture wall ms AND peak resident-set-size KB from one run.
# Returns "<ms>\t<rss_kb>" on success, "ERR\tERR" on failure.
measure_one() {
  local scenario="$1"
  local out
  # `/usr/bin/time -l` (Darwin) and `/usr/bin/time -v` (GNU) both
  # report maximum-RSS; the default no-flag format doesn't. Use
  # the verbose form so we get both numbers from a single run.
  local TIME_FLAG=""
  if [[ "$PLATFORM" == "Darwin" ]]; then
    TIME_FLAG="-l"
  else
    TIME_FLAG="-v"
  fi
  out=$(BENCH_N="$N" BENCH_SCENARIO="$scenario" \
        LC_ALL=C /usr/bin/time $TIME_FLAG "$BIN" "$SCRIPT" 2>&1 >/dev/null) || {
    echo -e "ERR\tERR" ; return
  }
  if [[ "$PLATFORM" == "Darwin" ]]; then
    # Darwin `-l` output adds a block of resource lines after the
    # `real / user / sys` line. Wall is the first `real`; peak
    # RSS is the `maximum resident set size` line in BYTES.
    local secs
    secs=$(awk '/real/ {print $1; exit}' <<<"$out")
    local rss_bytes
    rss_bytes=$(awk '/maximum resident set size/ {print $1; exit}' <<<"$out")
    if [[ -z "$secs" || -z "$rss_bytes" ]]; then
      echo -e "ERR\tERR"; return
    fi
    local ms rss_kb
    ms=$(awk -v s="$secs" 'BEGIN { printf "%d", s * 1000 + 0.5 }')
    rss_kb=$(awk -v b="$rss_bytes" 'BEGIN { printf "%d", b / 1024 + 0.5 }')
    echo -e "${ms}\t${rss_kb}"
  else
    # Linux GNU `/usr/bin/time -v` format — multi-line key:value
    # output. Walk for `Elapsed (wall clock) time` and
    # `Maximum resident set size`.
    local elapsed rss_kb
    elapsed=$(awk -F': ' '/Elapsed \(wall clock\)/ {print $2; exit}' <<<"$out")
    rss_kb=$(awk -F': ' '/Maximum resident set size/ {print $2; exit}' <<<"$out")
    if [[ -z "$elapsed" || -z "$rss_kb" ]]; then
      echo -e "ERR\tERR"; return
    fi
    local ms
    ms=$(awk -v e="$elapsed" 'BEGIN {
      if (e ~ /:/) {
        split(e, parts, ":")
        secs = parts[1] * 60 + parts[2]
      } else {
        secs = e
      }
      printf "%d", secs * 1000 + 0.5
    }')
    echo -e "${ms}\t${rss_kb}"
  fi
}

# MIN-of-RUNS over both ms and rss_kb, INDEPENDENTLY — same
# policy `perf/check.sh` uses for native rubyrs. Prints
# "<min_ms>\t<min_rss_kb>".
min_of() {
  local scenario="$1"
  local min_ms="" min_rss="" line ms rss
  for ((i = 1; i <= RUNS; i++)); do
    line="$(measure_one "$scenario")"
    ms="${line%%$'\t'*}"
    rss="${line##*$'\t'}"
    if [[ "$ms" == "ERR" || "$rss" == "ERR" ]]; then
      echo -e "ERR\tERR"; return
    fi
    if [[ -z "$min_ms" || "$ms" -lt "$min_ms" ]]; then
      min_ms="$ms"
    fi
    if [[ -z "$min_rss" || "$rss" -lt "$min_rss" ]]; then
      min_rss="$rss"
    fi
  done
  echo -e "${min_ms}\t${min_rss}"
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

# Run each scenario once, then split the "<ms>\t<rss>" pair.
run_and_print() {
  local label="$1" scenario="$2"
  local line; line="$(min_of "$scenario")"
  local ms rss
  ms="${line%%$'\t'*}"
  rss="${line##*$'\t'}"
  printf "%-22s  %7s ms  %8s KB\n" "$label" "$ms" "$rss"
  # Stash for later A/B math.
  eval "STASH_${scenario}_ms=\"\$ms\""
  eval "STASH_${scenario}_rss=\"\$rss\""
}

printf "%-22s  %10s  %11s\n" "scenario" "wall (min)" "peak RSS (min)"
printf "%-22s  %10s  %11s\n" "----------------------" "----------" "-----------"
echo ""
echo "[A] pure-Ruby Time class (user-method dispatch + Object alloc):"
for s in a_to_i a_plus a_cmp a_construct a_workload; do
  run_and_print "  $s" "$s"
done
echo ""
echo "[B floor] bare primitive (BinOpInt fast path where it applies):"
for s in b_to_i b_plus b_cmp b_construct; do
  run_and_print "  $s" "$s"
done
echo ""
echo "[B realistic] send-dispatched + Range-shaped (actual dispatch a Rust Time would pay):"
for s in b_to_i_send b_to_i_range b_plus_send b_cmp_send b_construct_range b_workload; do
  run_and_print "  $s" "$s"
done

echo ""
echo "A vs B ratios (A / B):"
printf "%-22s  %8s  %8s\n" "" "wall" "RSS"
printf "%-22s  %8s  %8s\n" "----------------------" "----" "----"
ab() {
  local label="$1" a_var="$2" b_var="$3"
  local a_ms b_ms a_rss b_rss
  a_ms="$(eval "echo \$STASH_${a_var}_ms")"
  b_ms="$(eval "echo \$STASH_${b_var}_ms")"
  a_rss="$(eval "echo \$STASH_${a_var}_rss")"
  b_rss="$(eval "echo \$STASH_${b_var}_rss")"
  printf "%-22s  %8s  %8s\n" "  $label" "$(ratio "$a_ms" "$b_ms")" "$(ratio "$a_rss" "$b_rss")"
}
ab "to_i (floor)"        "a_to_i"      "b_to_i"
ab "to_i (send)"         "a_to_i"      "b_to_i_send"
ab "to_i (range)"        "a_to_i"      "b_to_i_range"
ab "plus (floor)"        "a_plus"      "b_plus"
ab "plus (send)"         "a_plus"      "b_plus_send"
ab "cmp  (floor)"        "a_cmp"       "b_cmp"
ab "cmp  (send)"         "a_cmp"       "b_cmp_send"
ab "construct (floor)"   "a_construct" "b_construct"
ab "construct (range)"   "a_construct" "b_construct_range"
ab "workload (mix)"      "a_workload"  "b_workload"

echo ""
echo "Surrogate notes:"
echo "  B floor    — Integer with bare op; BinOpInt fast-path applies on '+'."
echo "               UNDER-estimates realistic Path B for arithmetic."
echo "  B realistic—  send(:op, x) forces primitive_call (no BinOpInt fuse)"
echo "               OR Range#begin / (..) construct for heap-backed shape."
echo "               REALISTIC Path B sits BETWEEN floor and Path A."
echo ""
echo "Decision rule: if A's workload-row absolute cost stays below"
echo "what the niche scripts spend on I/O (typically O(ms) total),"
echo "Path A's per-op overhead is invisible end-to-end."
