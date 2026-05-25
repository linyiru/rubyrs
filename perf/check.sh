#!/usr/bin/env bash
# perf/check.sh — Per-workload peak-RSS regression check.
#
# For each row in perf/baselines.tsv: run the workload through the
# release rubyrs binary three times under `/usr/bin/time`, take the
# MIN peak-RSS, and fail if it exceeds the row's `max_rss_kb`.
#
# Wall time is collected and printed for visibility but not enforced
# — CI-runner variance makes wall-time gating noisy at this scale.
# Once enough runs accumulate to know the noise floor, a follow-up
# can add a wall-time column to baselines.tsv. See perf/README.md
# for the policy and ratchet etiquette.
#
# Exit codes:
#   0  — all workloads under their RSS budgets
#   1  — at least one workload over budget
#   2  — setup error (binary not built, etc.)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

RUBYRS_BIN="${RUBYRS_BIN:-target/release/rubyrs}"
BASELINES="${BASELINES:-perf/baselines.tsv}"
RUNS="${RUNS:-3}"

if [[ ! -x "$RUBYRS_BIN" ]]; then
  echo "perf/check: rubyrs binary not found at $RUBYRS_BIN" >&2
  echo "perf/check: run \`cargo build --release -p rubyrs\` first" >&2
  exit 2
fi

# `/usr/bin/time` flags differ: macOS BSD-time uses `-l`, GNU coreutils
# uses `-v`. We branch and parse accordingly. Both paths produce the
# same `wall_ms rss_kb` pair on stdout for the caller to consume.
PLATFORM="$(uname -s)"
measure_once() {
  local script="$1"
  local out
  if [[ "$PLATFORM" == "Darwin" ]]; then
    # `time -l` prints to stderr; capture all of it.
    out=$(/usr/bin/time -l "$RUBYRS_BIN" "$script" 2>&1 >/dev/null)
    # `real` line: `        0.46 real         0.45 user         0.00 sys`
    # `maximum resident set size` in BYTES on macOS.
    local secs rss_b
    secs=$(awk '/real/ {print $1; exit}' <<<"$out")
    rss_b=$(awk '/maximum resident set size/ {print $1; exit}' <<<"$out")
    # Round UP on the bytes-to-KB conversion so the budget check is
    # conservative — a script using 4 MB + 1 byte should report as
    # 4097 KB rather than be rounded down to 4096 and slip past a
    # tight budget. Ceiling is the right semantic for a regression
    # gate; truncation could let sub-KB overruns past.
    awk -v s="$secs" -v b="$rss_b" 'BEGIN {
      kb = int(b / 1024);
      if (b % 1024 != 0) kb = kb + 1;
      printf "%d %d\n", s*1000, kb;
    }'
  else
    # GNU /usr/bin/time -v on Linux. RSS is already in KB.
    out=$(/usr/bin/time -v "$RUBYRS_BIN" "$script" 2>&1 >/dev/null)
    # `Elapsed (wall clock) time (h:mm:ss or m:ss): 0:00.46`
    # `Maximum resident set size (kbytes): 12345`
    local wall rss_kb
    wall=$(awk -F': ' '/Elapsed \(wall clock\)/ {print $2; exit}' <<<"$out")
    rss_kb=$(awk -F': ' '/Maximum resident set size/ {print $2; exit}' <<<"$out")
    # Parse `m:ss.ss` (or `h:mm:ss.ss`) into ms.
    local ms
    ms=$(awk -v w="$wall" 'BEGIN {
      n = split(w, parts, ":");
      total = 0;
      for (i = 1; i <= n; i++) total = total * 60 + parts[i];
      printf "%d", total * 1000;
    }')
    echo "$ms $rss_kb"
  fi
}

# Take min across $RUNS runs. Min (not mean) is more stable: on a
# noisy runner, outliers are typically *slower*, so the min is the
# best estimate of the underlying cost.
measure_min() {
  local script="$1"
  local best_ms=999999999 best_kb=999999999
  for ((i = 0; i < RUNS; i++)); do
    read -r ms kb <<<"$(measure_once "$script")"
    (( ms < best_ms )) && best_ms=$ms
    (( kb < best_kb )) && best_kb=$kb
  done
  echo "$best_ms $best_kb"
}

fail=0
total=0
printf "%-58s %-11s %-12s %-10s\n" "WORKLOAD" "WALL_MS_MIN" "RSS_KB_MIN" "BUDGET_KB"
printf "%-58s %-11s %-12s %-10s\n" "--------" "-----------" "----------" "---------"

while IFS=$'\t' read -r workload budget _note; do
  # Skip comments + blank lines.
  case "$workload" in ''|\#*) continue ;; esac
  if [[ ! -f "$workload" ]]; then
    echo "perf/check: workload not found: $workload" >&2
    fail=1
    continue
  fi
  total=$((total + 1))
  read -r ms kb <<<"$(measure_min "$workload")"
  status="ok"
  if (( kb > budget )); then
    status="OVER"
    fail=1
  fi
  printf "%-58s %-11s %-12s %-10s %s\n" "$workload" "$ms" "$kb" "$budget" "$status"
done < "$BASELINES"

if (( fail != 0 )); then
  echo ""
  echo "perf/check: at least one workload exceeded its RSS budget." >&2
  echo "perf/check: bump perf/baselines.tsv if the growth is intentional," >&2
  echo "perf/check: with a comment explaining what allocation grew and why." >&2
  exit 1
fi

echo ""
echo "perf/check: all $total workload(s) within budget."
