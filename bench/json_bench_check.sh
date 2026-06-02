#!/usr/bin/env bash
# bench/json_bench_check.sh — JSON-bench per-iteration regression gate.
#
# Builds `rubyrs` with the `_json_native` accelerator on, runs
# `bench/json_bench.rb`, parses each metric's `per_iter` µs reading,
# compares against the row's budget in `bench/json_bench_baselines.tsv`,
# fails non-zero if any metric exceeds.
#
# This is the JSON-shape sibling of `perf/check.sh`: same exit-code
# contract (0 = ok / 1 = budget exceeded / 2 = setup error), same
# absolute-baselines-no-master-comparison policy. See
# `bench/json_bench_results.md` for the perf-milestone history the
# baselines were calibrated against.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

RUBYRS_BIN="${RUBYRS_BIN:-target/release/rubyrs}"
BASELINES="${BASELINES:-bench/json_bench_baselines.tsv}"
BENCH_SCRIPT="${BENCH_SCRIPT:-bench/json_bench.rb}"
# Lower ITERS than the README default (5000) keeps the gate's wall
# under ~3 s even when round_trip is 150 µs. Min-of-RUNS still
# absorbs warm-up + GC. Override via env if a noisier CI host
# needs longer.
JSON_BENCH_ITERS="${JSON_BENCH_ITERS:-2000}"
JSON_BENCH_RUNS="${JSON_BENCH_RUNS:-3}"

if [[ ! -x "$RUBYRS_BIN" ]]; then
  echo "json_bench_check: rubyrs binary not found at $RUBYRS_BIN" >&2
  echo "json_bench_check: build with \`cargo build --release -p rubyrs --features default,stdlib,_http_server,_fiber,_json_native\` first" >&2
  exit 2
fi
if [[ ! -r "$BASELINES" ]]; then
  echo "json_bench_check: baselines not readable at $BASELINES" >&2
  exit 2
fi
if [[ ! -r "$BENCH_SCRIPT" ]]; then
  echo "json_bench_check: bench script not readable at $BENCH_SCRIPT" >&2
  exit 2
fi

# Run the bench once, capture stdout. `ITERS` + `RUNS` are read by
# bench/json_bench.rb from the env. Don't tee — the bench prints
# enough chrome (runtime label, iters, payload size) that we want
# all of it in the CI log for diagnostics on failure.
echo "json_bench_check: running ITERS=$JSON_BENCH_ITERS RUNS=$JSON_BENCH_RUNS"
bench_out=$(
  ITERS="$JSON_BENCH_ITERS" RUNS="$JSON_BENCH_RUNS" \
    "$RUBYRS_BIN" "$BENCH_SCRIPT"
)
echo "$bench_out"
echo ""

# Parse per_iter µs by metric name. The bench prints lines shaped:
#   `parse                   best_total=    87.51 ms   per_iter=   17.502 us`
# `awk` scans for the leading metric name + the value after `per_iter=`.
# Skip the Oj rows (`oj_parse` / `oj_generate` / `oj_round_trip`) —
# those are reference, not gated.
extract_per_iter() {
  local metric="$1"
  # `bench/json_bench.rb` emits each metric row as
  #   `<metric>  ...  per_iter=  XX.XXX us`
  # via sprintf("%-22s ... per_iter=%9.3f us", ...). The `%9.3f`
  # right-aligns the number in a 9-char field, so under awk's
  # whitespace field-split `per_iter=` and the number land on
  # SEPARATE fields (the equals sign isn't followed by a digit;
  # it's followed by the format-width padding). Match `per_iter=`
  # exactly and print the next field.
  awk -v m="$metric" '
    $1 == m {
      for (i = 1; i <= NF; i++) {
        if ($i == "per_iter=") {
          print $(i + 1);
          exit;
        }
      }
    }
  ' <<<"$bench_out"
}

budget_fail=0
setup_fail=0
total=0
printf "%-18s %-12s %-12s %s\n" "METRIC" "ACTUAL_US" "BUDGET_US" "STATUS"
printf "%-18s %-12s %-12s %s\n" "------" "---------" "---------" "------"

while IFS= read -r line; do
  # Skip comments / blank lines.
  case "$line" in ''|\#*) continue ;; esac
  # Split on whitespace (any width). The TSV header documents
  # tab-separated but in practice editors / `Write` tools often
  # round-trip to spaces — splitting on any whitespace is
  # tolerant of either. Columns: metric, budget_us, free-form note.
  metric=$(awk '{print $1}' <<<"$line")
  budget=$(awk '{print $2}' <<<"$line")
  if [[ ! "$budget" =~ ^[0-9]+(\.[0-9]+)?$ ]]; then
    echo "json_bench_check: row '$metric' has invalid budget '$budget' (expected number)" >&2
    printf "%-18s %-12s %-12s %s\n" "$metric" "ERR" "$budget" "SETUP"
    setup_fail=1
    continue
  fi
  total=$((total + 1))
  per_iter=$(extract_per_iter "$metric")
  if [[ -z "$per_iter" ]]; then
    echo "json_bench_check: could not parse per_iter for metric '$metric' from bench output" >&2
    printf "%-18s %-12s %-12s %s\n" "$metric" "ERR" "$budget" "SETUP"
    setup_fail=1
    continue
  fi
  # Float comparison via awk (bash arithmetic is integer-only).
  over=$(awk -v a="$per_iter" -v b="$budget" 'BEGIN { print (a + 0 > b + 0) ? "1" : "0" }')
  status="ok"
  if [[ "$over" == "1" ]]; then
    status="OVER"
    budget_fail=1
  fi
  printf "%-18s %-12s %-12s %s\n" "$metric" "$per_iter" "$budget" "$status"
done < "$BASELINES"

if (( setup_fail != 0 )); then
  echo ""
  echo "json_bench_check: one or more baseline rows are invalid or unparseable (see errors above)." >&2
  echo "json_bench_check: this is a setup/config error, not a perf regression." >&2
  exit 2
fi
if (( budget_fail != 0 )); then
  echo ""
  echo "json_bench_check: at least one metric exceeded its per-iter budget." >&2
  echo "json_bench_check: bump bench/json_bench_baselines.tsv ONLY if the growth" >&2
  echo "json_bench_check: is intentional. See the file's header comment for" >&2
  echo "json_bench_check: bumping etiquette (root-cause first, ratchet later)." >&2
  exit 1
fi

echo ""
echo "json_bench_check: all $total metric(s) within budget."
