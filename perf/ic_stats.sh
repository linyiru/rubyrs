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

# Capture stderr to a per-iteration tmpfile so we can disentangle
# the three things the previous pipeline conflated: a successful
# workload, a crashed workload, and a binary built without
# `--features ic-stats`. The tmpfile is cleared in-loop and
# removed on exit.
stderr_file="$(mktemp)"
trap 'rm -f "$stderr_file"' EXIT

printf 'workload\thits\tmisses\ttoplevel_hits\ttoplevel_misses\thit_rate\n'
for f in "${workloads[@]}"; do
    name="$(basename "$f" .rb)"
    : >"$stderr_file"
    # Run rubyrs with stdout discarded (workload's `puts` is
    # noise here) and stderr captured. Don't let `set -e` abort
    # on a non-zero rubyrs exit — handle that explicitly below
    # so a crashed workload surfaces a clear "workload failed"
    # message instead of getting buried under the
    # "no ic-stats line" diagnostic.
    rubyrs_exit=0
    RUBYRS_IC_STATS=1 "$BIN" "$f" >/dev/null 2>"$stderr_file" || rubyrs_exit=$?
    if [[ $rubyrs_exit -ne 0 ]]; then
        echo "workload $name failed (rubyrs exit $rubyrs_exit). stderr was:" >&2
        cat "$stderr_file" >&2
        exit 1
    fi
    # Filter for a line that starts with `ic-stats\t` rather
    # than blindly taking the last stderr line — that way an
    # unrelated stderr message (e.g. fuel/heap trap surfaced
    # before exit) doesn't get mis-parsed as the stats row.
    line="$(grep $'^ic-stats\t' "$stderr_file" | tail -1 || true)"
    if [[ -z "$line" ]]; then
        echo "no ic-stats line for $name — was the binary built with --features ic-stats?" >&2
        exit 1
    fi
    # Parse `ic-stats\thits=N\tmisses=N\ttoplevel_hits=N\ttoplevel_misses=N\thit_rate=R`
    # in one read+strip pass — five `awk | cut` invocations per
    # row would dominate the script's wall time on a battery
    # this small.
    IFS=$'\t' read -r _ hits_kv misses_kv th_kv tm_kv rate_kv <<<"$line"
    hits=${hits_kv#hits=}
    misses=${misses_kv#misses=}
    th=${th_kv#toplevel_hits=}
    tm=${tm_kv#toplevel_misses=}
    rate=${rate_kv#hit_rate=}
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$hits" "$misses" "$th" "$tm" "$rate"
done
