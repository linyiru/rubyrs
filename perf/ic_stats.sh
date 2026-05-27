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

BIN="${BIN:-./target/release/rubyrs}"
DIR="$(cd "$(dirname "$0")" && pwd)/ic_stats_workloads"

if [[ ! -x "$BIN" ]]; then
    echo "build the ic-stats binary first: cargo build -p rubyrs --features ic-stats --release" >&2
    exit 1
fi

printf 'workload\thits\tmisses\ttoplevel_hits\ttoplevel_misses\thit_rate\n'
for f in "$DIR"/*.rb; do
    name="$(basename "$f" .rb)"
    # The script's `puts` goes to stdout; ic-stats line is the
    # last stderr line. Discard stdout to keep the table clean.
    line="$(RUBYRS_IC_STATS=1 "$BIN" "$f" 2>&1 1>/dev/null | tail -1)"
    # Parse `ic-stats\thits=N\tmisses=N\ttoplevel_hits=N\ttoplevel_misses=N\thit_rate=R`
    hits=$(echo "$line" | awk -F'\t' '{print $2}' | cut -d= -f2)
    misses=$(echo "$line" | awk -F'\t' '{print $3}' | cut -d= -f2)
    th=$(echo "$line" | awk -F'\t' '{print $4}' | cut -d= -f2)
    tm=$(echo "$line" | awk -F'\t' '{print $5}' | cut -d= -f2)
    rate=$(echo "$line" | awk -F'\t' '{print $6}' | cut -d= -f2)
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$hits" "$misses" "$th" "$tm" "$rate"
done
