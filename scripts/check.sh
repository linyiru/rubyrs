#!/usr/bin/env bash
# scripts/check.sh — run what CI runs, locally, with quiet output.
#
#   bash scripts/check.sh [quick|pr|full] [--keep-going] [--dry-run] [--only=N,M]
#
#   quick  doctor, lints, lockfile, panic budget, workspace clippy, release
#          diff suite. The loop to run while iterating.
#   pr     everything the ubuntu Test job runs (3 clippy sets, build,
#          tests, STRESS_GC=1 tests, both JIT diff legs) plus the
#          lockfile, panic-budget and peak-RSS ratchets. Run before
#          pushing. This is the default.
#   full   pr, plus the cext example bundles, the JSON per-iter budget and
#          the coverage ratchet (needs cargo-llvm-cov).
#
# Not covered (CI-only): wasm32-wasip1, Windows, Miri, framework parity,
# macOS-vs-Linux differences.
#
# Steps are taken from .github/workflows/ci.yml through scripts/ci-step.rb,
# so they are CI's exact commands, not copies. Each step's full output goes
# to a log file; the terminal gets one PASS/FAIL line per step and, on a
# failure, the last lines of that step's log plus the command to re-run it.
#
# --keep-going  run every step and report all failures (default: stop at
#               the first one)
# --dry-run     list the numbered steps of the tier without running them
# --only=N,M   run only those steps (numbers from --dry-run); a failure
#               prints this form as its re-run command, so the step gets
#               the same environment (scripts/dev-env.sh) as the full run
#
# Exit codes: 0 all passed, 1 a step failed, 2 usage error.

set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$here/.." && pwd)"
cd "$REPO_ROOT" || exit 2

tier=pr keep_going=0 dry_run=0 only=""
for arg in "$@"; do
  case "$arg" in
    quick|pr|full) tier="$arg" ;;
    --keep-going) keep_going=1 ;;
    --dry-run) dry_run=1 ;;
    --only=*) only=",${arg#--only=}," ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "usage: scripts/check.sh [quick|pr|full] [--keep-going] [--dry-run] [--only=N,M]" >&2; exit 2 ;;
  esac
done

# shellcheck source=scripts/dev-env.sh
. "$here/dev-env.sh"
# perf/check.sh measures this binary; it must be a default-feature build,
# which is why the ratchet runs right after `Build` and before the
# jit-native steps rebuild it with other features.
RUBYRS_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/release/rubyrs"
export RUBYRS_BIN

log_dir="${TMPDIR:-/tmp}/rubyrs-check/$(basename "$REPO_ROOT")-$(date +%Y%m%d-%H%M%S)"
labels=() cmds=()
add() { labels+=("$1"); cmds+=("$2"); }
# Step names are single-quoted so the printed re-run command is readable
# and pasteable; no ci.yml step name contains a single quote.
ci() { add "$2" "ruby scripts/ci-step.rb $1 '$2'"; }

add "doctor" "bash scripts/doctor.sh"
ci test "GC rooting lint (issue #90)"
ci test "Rustdoc orientation lint"
ci lockfile "Assert Cargo.lock satisfies every manifest"
ci panic-budget "Count panics"
if [[ "$tier" == quick ]]; then
  ci test "Clippy (workspace, -D warnings)"
  add "diff suite (release)" "cargo test --release --locked -p rubyrs --test diff_cruby"
else
  ci test "Clippy (standard measurement set, -D warnings)"
  ci test "Clippy (everything combo, -D warnings)"
  ci test "Clippy (workspace, -D warnings)"
  ci test "Build"
  ci perf-budget "Check peak-RSS budgets"
  ci test "Run tests"
  ci test "Run tests (STRESS_GC=1)"
  ci test "Diff parity under native JIT (interpreter == JIT == CRuby)"
  ci test "Diff parity under tier-2 JIT (interpreter == tier-2 == CRuby)"
fi
if [[ "$tier" == full ]]; then
  ci test "Verify cext example bundles build"
  ci json-perf-budget "Build (release, _json_native enabled)"
  ci json-perf-budget "Check JSON bench per-iter budgets"
  ci coverage "Generate LCOV report"
  ci coverage "Ratchet per-file coverage"
fi

if (( dry_run )); then
  for i in "${!labels[@]}"; do printf '%2d  %-62s %s\n' "$((i + 1))" "${labels[$i]}" "${cmds[$i]}"; done
  # Resolving every CI step name is the point of a dry run: it fails when
  # ci.yml renamed or dropped a step this script relies on.
  status=0
  for i in "${!cmds[@]}"; do
    case "${cmds[$i]}" in
      "ruby scripts/ci-step.rb "*) eval "${cmds[$i]/ci-step.rb /ci-step.rb --print }" > /dev/null || status=1 ;;
    esac
  done
  exit "$status"
fi

mkdir -p "$log_dir"
echo "check: tier=$tier  logs=$log_dir"
failed=() ran=0
total_start=$SECONDS
for i in "${!labels[@]}"; do
  [[ -z "$only" || "$only" == *",$((i + 1)),"* ]] || continue
  label="${labels[$i]}" cmd="${cmds[$i]}"
  log="$log_dir/$(printf '%02d' "$((i + 1))")-$(printf '%s' "$label" | tr -c 'A-Za-z0-9' '-' | cut -c1-40).log"
  start=$SECONDS ran=$((ran + 1))
  if eval "$cmd" > "$log" 2>&1; then
    printf 'PASS  %-62s %4ds\n' "$label" "$((SECONDS - start))"
  else
    printf 'FAIL  %-62s %4ds\n' "$label" "$((SECONDS - start))"
    # Cargo's progress lines and the GitHub log-group markers are noise
    # in a failure excerpt; the error is near the end.
    grep -vE '^\s+(Compiling|Checking|Fresh|Running|Downloaded|Downloading) |^::' "$log" | tail -n 40 | sed 's/^/      | /'
    echo "      log:    $log"
    echo "      re-run: $cmd"
    failed+=("$label")
    (( keep_going )) || break
  fi
done

elapsed=$((SECONDS - total_start))
if (( ${#failed[@]} )); then
  echo "check: $tier FAILED in ${elapsed}s: ${failed[*]}"
  exit 1
fi
echo "check: $tier passed in ${elapsed}s ($ran steps)"
