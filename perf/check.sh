#!/usr/bin/env bash
# perf/check.sh — Per-workload peak-RSS + wall-time regression check.
#
# For each row in perf/baselines.tsv: run the workload through the
# release rubyrs binary three times under `/usr/bin/time`, take
# the min wall-time and min peak-RSS *independently* across those
# runs, and fail if either exceeds the row's `max_rss_kb` /
# `max_wall_ms`. (Independently: the reported pair may not come
# from the same run — e.g. run 1 had the lowest wall, run 2 had
# the lowest RSS. That's intentional, we want each metric's floor
# as the most stable estimate of its underlying cost.) A
# `max_wall_ms` of `0` disables the wall check for that workload
# (sub-100ms scripts where measurement noise dominates the
# signal).
#
# Both budgets are absolute baselines committed to source — same
# ratchet pattern as the panic budget. We deliberately don't
# compare against master at runtime, because relative gates
# chain-degrade across PRs. See perf/README.md for the policy.
#
# Exit codes:
#   0  — all workloads within their RSS and wall budgets
#   1  — at least one workload over a budget
#   2  — setup error (binary not built, time missing, row malformed)

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
if [[ ! -r "$BASELINES" ]]; then
  echo "perf/check: baselines file not readable at $BASELINES" >&2
  echo "perf/check: set BASELINES=<path> or restore perf/baselines.tsv" >&2
  exit 2
fi
# `RUNS` must be a positive integer. Empty / non-numeric / 0 would
# either skip the measure loop silently or fall through to a bash
# arithmetic error mid-run, neither of which surface as a clear
# setup-error code (the 0/1/2 contract).
if [[ ! "$RUNS" =~ ^[1-9][0-9]*$ ]]; then
  echo "perf/check: RUNS must be a positive integer, got '$RUNS'" >&2
  exit 2
fi
# `/usr/bin/time` is what we use to measure peak RSS — its absence
# is a setup error, not a perf regression. Pre-flight check avoids
# `set -e` killing the script mid-loop with a confusing exit code
# (often 127 from command-not-found).
if [[ ! -x /usr/bin/time ]]; then
  echo "perf/check: /usr/bin/time not executable at /usr/bin/time" >&2
  echo "perf/check: on Linux install \`time\` (GNU); on macOS this is preinstalled" >&2
  exit 2
fi

# `/usr/bin/time` flags differ: macOS BSD-time uses `-l`, GNU coreutils
# uses `-v`. We branch and parse accordingly. Both paths produce the
# same `wall_ms rss_kb` pair on stdout for the caller to consume.
PLATFORM="$(uname -s)"
measure_once() {
  local script="$1"
  local out rc=0
  # `set -e` would normally let any failure inside `/usr/bin/time`
  # (workload exiting non-zero, parse mismatch downstream) kill the
  # whole script with that command's exit code — violating the
  # 0/1/2 contract. Catch the failure here, return an empty
  # "$ms $kb" line, and let the caller treat that as a measurement
  # error worth surfacing.
  if [[ "$PLATFORM" == "Darwin" ]]; then
    # Force `C` locale so the label strings + decimal separator
    # stay English/`.`-formatted; otherwise a non-English LC_ALL
    # would translate "maximum resident set size" / "real" and
    # also produce `0,46` instead of `0.46`, breaking the awk
    # patterns and the `s*1000` arithmetic below.
    out=$(LC_ALL=C /usr/bin/time -l "$RUBYRS_BIN" "$script" 2>&1 >/dev/null) || rc=$?
    if (( rc != 0 )); then
      echo "perf/check: workload \`$script\` exited with status $rc" >&2
      [[ -n "$out" ]] && echo "$out" | sed 's/^/  | /' >&2
      echo "ERR ERR"
      return
    fi
    # `real` line: `        0.46 real         0.45 user         0.00 sys`
    # `maximum resident set size` in BYTES on macOS.
    local secs rss_b
    secs=$(awk '/real/ {print $1; exit}' <<<"$out")
    rss_b=$(awk '/maximum resident set size/ {print $1; exit}' <<<"$out")
    if [[ -z "$secs" || -z "$rss_b" ]]; then
      echo "perf/check: could not parse \`/usr/bin/time -l\` output for $script" >&2
      echo "$out" | sed 's/^/  | /' >&2
      echo "ERR ERR"
      return
    fi
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
    # Force `C` locale — see the macOS arm for rationale. GNU
    # `time -v` translates its labels (e.g. `Temps écoulé` on a
    # `fr_FR` system) and the wall-time string uses `,` as the
    # decimal separator, both of which would defeat the awk
    # patterns below.
    out=$(LC_ALL=C /usr/bin/time -v "$RUBYRS_BIN" "$script" 2>&1 >/dev/null) || rc=$?
    if (( rc != 0 )); then
      echo "perf/check: workload \`$script\` exited with status $rc" >&2
      [[ -n "$out" ]] && echo "$out" | sed 's/^/  | /' >&2
      echo "ERR ERR"
      return
    fi
    # `Elapsed (wall clock) time (h:mm:ss or m:ss): 0:00.46`
    # `Maximum resident set size (kbytes): 12345`
    local wall rss_kb
    wall=$(awk -F': ' '/Elapsed \(wall clock\)/ {print $2; exit}' <<<"$out")
    rss_kb=$(awk -F': ' '/Maximum resident set size/ {print $2; exit}' <<<"$out")
    if [[ -z "$wall" || -z "$rss_kb" ]]; then
      echo "perf/check: could not parse \`/usr/bin/time -v\` output for $script" >&2
      echo "$out" | sed 's/^/  | /' >&2
      echo "ERR ERR"
      return
    fi
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
    # Sentinel from measure_once when /usr/bin/time failed or
    # output didn't parse. Caller treats that as setup_fail.
    if [[ "$ms" == "ERR" || "$kb" == "ERR" ]]; then
      echo "ERR ERR"
      return
    fi
    (( ms < best_ms )) && best_ms=$ms
    (( kb < best_kb )) && best_kb=$kb
  done
  echo "$best_ms $best_kb"
}

budget_fail=0   # at least one workload exceeded its budget → exit 1
setup_fail=0    # at least one row in baselines.tsv is malformed → exit 2
total=0
printf "%-58s %-11s %-11s %-12s %-12s %s\n" "WORKLOAD" "RSS_MIN_KB" "RSS_MAX_KB" "WALL_MIN_MS" "WALL_MAX_MS" "STATUS"
printf "%-58s %-11s %-11s %-12s %-12s %s\n" "--------" "----------" "----------" "-----------" "-----------" "------"

while IFS=$'\t' read -r workload rss_max wall_max _note; do
  # Skip comments + blank lines.
  case "$workload" in ''|\#*) continue ;; esac
  # Validate both numeric columns. A malformed value would blow up
  # the `(( … > … ))` tests as a bash arithmetic error — route to
  # setup_fail to keep the 0/1/2 exit-code contract honest.
  if [[ ! "$rss_max" =~ ^[0-9]+$ ]]; then
    echo "perf/check: row '$workload' has invalid max_rss_kb '$rss_max' (expected non-negative integer)" >&2
    setup_fail=1
    continue
  fi
  # `0` is a valid wall_max — it's the "disable wall check"
  # sentinel for tiny workloads (sub-100ms noise floor).
  if [[ ! "$wall_max" =~ ^[0-9]+$ ]]; then
    echo "perf/check: row '$workload' has invalid max_wall_ms '$wall_max' (expected non-negative integer; 0 disables)" >&2
    setup_fail=1
    continue
  fi
  # Missing workload paths are a setup/config error, not a budget
  # regression. Categorise them separately so the header-comment
  # exit-code contract (0/1/2) actually matches what we emit.
  if [[ ! -f "$workload" ]]; then
    echo "perf/check: workload not found: $workload" >&2
    setup_fail=1
    continue
  fi
  total=$((total + 1))
  # Display "-" instead of "0" when the wall check is disabled —
  # easier to read in the result table than a literal 0 budget.
  # Compute once, use in both the OK row and the SETUP row so the
  # "disabled" rendering is consistent regardless of measurement
  # outcome.
  local_wall_display="$wall_max"
  [[ "$wall_max" == "0" ]] && local_wall_display="-"
  read -r ms kb <<<"$(measure_min "$workload")"
  # measure_min emits "ERR ERR" when /usr/bin/time itself failed
  # or the output didn't parse. Already logged a workload-specific
  # message inside measure_once; route to setup_fail and skip the
  # arithmetic comparison (which would die on non-numeric input).
  if [[ "$ms" == "ERR" || "$kb" == "ERR" ]]; then
    printf "%-58s %-11s %-11s %-12s %-12s %s\n" "$workload" "ERR" "$rss_max" "ERR" "$local_wall_display" "SETUP"
    setup_fail=1
    continue
  fi
  status="ok"
  if (( kb > rss_max )); then
    status="RSS-OVER"
    budget_fail=1
  fi
  # Skip wall check when wall_max is 0 (sub-100ms workload).
  if (( wall_max > 0 && ms > wall_max )); then
    status=$([[ "$status" == "ok" ]] && echo "WALL-OVER" || echo "$status+WALL")
    budget_fail=1
  fi
  printf "%-58s %-11s %-11s %-12s %-12s %s\n" "$workload" "$kb" "$rss_max" "$ms" "$local_wall_display" "$status"
done < "$BASELINES"

if (( setup_fail != 0 )); then
  echo ""
  echo "perf/check: one or more baseline rows are invalid (see errors above)." >&2
  echo "perf/check: this is a setup/config error, not a perf regression." >&2
  echo "perf/check: possible causes: missing workload path, non-integer budget," >&2
  echo "perf/check: workload exited non-zero, /usr/bin/time output unparseable." >&2
  exit 2
fi
if (( budget_fail != 0 )); then
  echo ""
  echo "perf/check: at least one workload exceeded an RSS or wall-time budget." >&2
  echo "perf/check: STATUS column shows RSS-OVER / WALL-OVER / RSS-OVER+WALL." >&2
  echo "perf/check: bump perf/baselines.tsv if the growth is intentional," >&2
  echo "perf/check: with a comment explaining what grew and why." >&2
  exit 1
fi

echo ""
echo "perf/check: all $total workload(s) within budget."
