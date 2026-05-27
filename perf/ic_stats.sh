#!/usr/bin/env bash
# Run a small fixed corpus of workloads through the `ic-stats`-
# instrumented rubyrs binary and emit a TSV summary on stdout.
#
# Usage:
#   cargo build -p rubyrs --features ic-stats --release
#   perf/ic_stats.sh
#
# Each row: workload | hits | misses | toplevel_hits | toplevel_misses | hit_rate
# The intent is to validate the IC's design points (mono / 4-way
# poly / 5-way megamorphic / hot toplevel def / gen-bump churn)
# and surface any below-threshold site that wants attention.

set -euo pipefail
# Empty workload glob would otherwise yield the literal pattern
# and confuse the loop; `nullglob` makes it expand to zero items
# so the explicit count-check below fires with a clear message.
shopt -s nullglob

# Resolve paths relative to the script's location, not the
# caller's CWD — matches the convention used by perf/check.sh,
# perf/wasm_check.sh, etc.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="${BIN:-$REPO_ROOT/target/release/rubyrs}"
DIR="$SCRIPT_DIR/ic_stats_workloads"

if [[ ! -x "$BIN" ]]; then
    echo "build the ic-stats binary first:" >&2
    echo "    cargo build -p rubyrs --features ic-stats --release" >&2
    echo "(expected at: $BIN)" >&2
    exit 1
fi

workloads=("$DIR"/*.rb)
if [[ ${#workloads[@]} -eq 0 ]]; then
    echo "no workloads found in $DIR" >&2
    exit 1
fi

printf 'workload\thits\tmisses\ttoplevel_hits\ttoplevel_misses\thit_rate\n'
for f in "${workloads[@]}"; do
    name="$(basename "$f" .rb)"
    # The script's `puts` goes to stdout; ic-stats writes a
    # tab-separated line to stderr. Discard stdout (we don't
    # care what the workload prints) and look for a line that
    # starts with `ic-stats\t` rather than blindly taking the
    # last stderr line — that way an unrelated stderr message
    # (e.g. fuel/heap trap surfaced before exit) doesn't get
    # mis-parsed as the stats row.
    line="$(RUBYRS_IC_STATS=1 "$BIN" "$f" 2>&1 1>/dev/null | grep $'^ic-stats\t' | tail -1 || true)"
    if [[ -z "$line" ]]; then
        echo "no ic-stats line for $name — was the binary built with --features ic-stats?" >&2
        exit 1
    fi
    # Parse `ic-stats\thits=N\tmisses=N\ttoplevel_hits=N\ttoplevel_misses=N\thit_rate=R`
    # in one pass to avoid the awk/cut process-spawn overhead
    # of the previous per-field extraction.
    IFS=$'\t' read -r _ hits_kv misses_kv th_kv tm_kv rate_kv <<<"$line"
    hits=${hits_kv#hits=}
    misses=${misses_kv#misses=}
    th=${th_kv#toplevel_hits=}
    tm=${tm_kv#toplevel_misses=}
    rate=${rate_kv#hit_rate=}
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$hits" "$misses" "$th" "$tm" "$rate"
done
