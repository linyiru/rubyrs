#!/usr/bin/env bash
# Run the workloads in `perf/ic_stats_workloads/` through the
# `ic-stats`-instrumented rubyrs binary and emit a TSV summary on
# stdout.
#
# Usage:
#   cargo build -p rubyrs --features ic-stats --release
#   perf/ic_stats.sh
#
# Each row: workload | hits | misses | toplevel_hits | toplevel_misses | hit_rate
#
# Today the corpus is 01_monomorphic .. 05_gen_bump_churn covering
# the IC's design points (mono / 4-shape poly / 6-shape megamorphic /
# hot toplevel def / gen-bump churn). Any new `*.rb` dropped into
# the workloads dir picks up automatically — drop one in if you
# want to characterise a new dispatch shape, then update
# `perf/IC_STATS_BASELINE.md` so the doc and TSV stay in lockstep.
#
# Exit codes:
#   0 — every workload produced a parseable ic-stats line
#   1 — at least one workload failed at runtime (rubyrs crashed)
#   2 — setup error (missing binary, missing workloads, mktemp
#       failed) — matches the convention in perf/check.sh and
#       perf/time_microbench.sh

set -euo pipefail
# Empty workload glob would otherwise yield the literal pattern
# and confuse the loop; `nullglob` makes it expand to zero items
# so the explicit count-check below fires with a clear message.
shopt -s nullglob

# Resolve paths relative to the script's location, not the
# caller's CWD — matches the convention used by perf/check.sh,
# perf/wasm_check.sh, etc. Then `cd` to repo root so a
# relative `BIN=target/release/rubyrs` override (or other
# repo-relative path) resolves consistently regardless of the
# caller's CWD.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"
BIN="${BIN:-target/release/rubyrs}"
DIR="$SCRIPT_DIR/ic_stats_workloads"

if [[ ! -x "$BIN" ]]; then
    echo "build the ic-stats binary first:" >&2
    echo "    cargo build -p rubyrs --features ic-stats --release" >&2
    echo "(expected at: $BIN)" >&2
    exit 2
fi

workloads=("$DIR"/*.rb)
if [[ ${#workloads[@]} -eq 0 ]]; then
    echo "no workloads found in $DIR" >&2
    exit 2
fi

# Capture stderr to a per-iteration tmpfile so we can
# disentangle three distinct cases the parsing logic needs to
# handle: a successful workload run, a crashed workload, and a
# binary built without `--features ic-stats` (no `ic-stats` line
# in stderr at all). The tmpfile is cleared in-loop and removed
# on exit. Explicit `${TMPDIR:-/tmp}/<prefix>.XXXXXX` template
# matches the sibling-script convention (perf/wasm_check.sh:137)
# so BSD/GNU mktemp parity is held and a read-only TMPDIR
# surfaces a clear diagnostic rather than a silent failure.
stderr_file="$(mktemp "${TMPDIR:-/tmp}/rubyrs-ic-stats.XXXXXX")" || {
    echo "ic_stats: mktemp failed (TMPDIR=${TMPDIR:-/tmp}); cannot capture per-workload stderr" >&2
    exit 2
}
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
    # into an array so an unexpected number of fields fails fast
    # rather than silently folding extras into the last variable.
    # Without `-a`, a future 7th counter (e.g. `evictions=N`) on
    # the ic-stats line would land inside `rate_kv` and put a
    # literal tab inside the TSV row, breaking downstream
    # consumers with no diagnostic.
    IFS=$'\t' read -r -a fields <<<"$line"
    expected_fields=6  # `ic-stats` tag + 5 KV pairs
    if [[ ${#fields[@]} -ne $expected_fields ]]; then
        echo "ic_stats: parsed ${#fields[@]} fields from \`$name\` ic-stats line, expected $expected_fields — has the binary added a new IcStats counter? Full line: $line" >&2
        exit 1
    fi
    hits=${fields[1]#hits=}
    misses=${fields[2]#misses=}
    th=${fields[3]#toplevel_hits=}
    tm=${fields[4]#toplevel_misses=}
    rate=${fields[5]#hit_rate=}
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$hits" "$misses" "$th" "$tm" "$rate"
done
