#!/usr/bin/env bash
# perf/wasm_breakdown.sh — cold-start phase breakdown for wasm32-wasip1.
#
# Companion to `perf/wasm_check.sh` (the budget gate). Where that
# script asks "are we under the wall-time budget", this one asks
# "WHERE is the wall-time being spent, phase by phase". Used during
# optimization spikes to know which knob actually moves the needle.
#
# Builds rubyrs with the `trace-startup` cargo feature on, which
# instruments `main.rs` with `Instant::now()` checkpoints at:
#   - entry          : first line of `main()`
#   - args           : after `env::args().collect()`
#   - env_collected  : after wasi env (or std env on host) is gathered
#   - runtime_ready  : after `take_wizer_runtime` or `with_config`
#                      finishes — i.e. preamble + classes done
#   - eval_done      : after `eval_file` returns
#
# Each checkpoint emits a `trace-startup\t<label>\t<micros>us` line on
# stderr. This script runs the cwasm N times under `/usr/bin/time`,
# takes the MIN per checkpoint (drops cache-warmup jitter), and
# prints a table with deltas. The "wasmtime+wasi+load" row is
# computed as `wall_total - last_checkpoint` — the time spent before
# rubyrs's first instruction runs (wasmtime startup, wasi init, cwasm
# mmap, _start dispatch).
#
# Exit codes: 0 if the breakdown ran end-to-end; 2 on any setup
# error (missing tools, build failure, no trace lines captured).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

RUNS="${RUNS:-5}"
SCRIPT_INLINE="${SCRIPT_INLINE:-puts 1 + 2}"

# Setup checks — every missing tool is a setup error (exit 2), same
# convention as `perf/wasm_check.sh`.
for tool in wasmtime cargo python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "wasm_breakdown: required tool not found: $tool" >&2
    exit 2
  fi
done
# We need ns-precision wall timing (the whole point of this script
# is to characterize sub-10ms phases). macOS `/usr/bin/time -p`
# rounds to 10 ms, so we wrap wasmtime in a python3 timer instead —
# `time.time_ns()` is available everywhere python3 is.
TIMER='import subprocess,sys,time; t=time.time_ns(); rc=subprocess.run(sys.argv[1:]).returncode; print((time.time_ns()-t)//1000, file=sys.stderr); sys.exit(rc)'

PERF_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/rubyrs-wasm-breakdown.XXXXXX")" || {
  echo "wasm_breakdown: mktemp -d failed" >&2
  exit 2
}
trap 'rm -rf "$PERF_TMPDIR"' EXIT
trap 'rm -rf "$PERF_TMPDIR"; trap - INT TERM; kill -INT $$' INT TERM

# 1. Build rubyrs with trace-startup for wasm32-wasip1. We require
#    the `cext` and `bignum`/`regex` features to stay OFF: cext has
#    no dynamic loader on wasi (build.rs panics) and the breakdown
#    is comparable to the perf gate, which also runs --no-default-features.
echo "[build] cargo build --release --target wasm32-wasip1 --no-default-features --features trace-startup -p rubyrs" >&2
if ! cargo build --release --target wasm32-wasip1 --no-default-features --features trace-startup -p rubyrs >&2; then
  echo "wasm_breakdown: cargo build failed" >&2
  exit 2
fi

RAW_WASM="target/wasm32-wasip1/release/rubyrs.wasm"
if [[ ! -f "$RAW_WASM" ]]; then
  echo "wasm_breakdown: build artifact missing: $RAW_WASM" >&2
  exit 2
fi

# 2. Pipeline matches `perf/wasm_check.sh`:
#    raw → wasm-opt -Oz → wizer → wasm-opt -Oz → wasmtime compile
#    wasm-opt and wizer are OPTIONAL (graceful skip with a note);
#    `wasmtime compile` is required because the headline AOT shape
#    is what the breakdown is here to characterize.
OPT="$PERF_TMPDIR/rubyrs.opt.wasm"
WIZER="$PERF_TMPDIR/rubyrs.wizer.wasm"
WIZER_OPT="$PERF_TMPDIR/rubyrs.wizer.opt.wasm"
CWASM="$PERF_TMPDIR/rubyrs.cwasm"

if command -v wasm-opt >/dev/null 2>&1; then
  if ! wasm-opt -Oz "$RAW_WASM" -o "$OPT" >/dev/null; then
    echo "wasm_breakdown: wasm-opt -Oz failed" >&2
    exit 2
  fi
else
  echo "[skip] wasm-opt not on PATH; using raw .wasm" >&2
  OPT="$RAW_WASM"
fi

if command -v wizer >/dev/null 2>&1; then
  if ! wizer --allow-wasi --wasm-bulk-memory true -o "$WIZER" "$OPT" 2>/dev/null; then
    echo "wasm_breakdown: wizer pre-init failed" >&2
    exit 2
  fi
  if command -v wasm-opt >/dev/null 2>&1; then
    if ! wasm-opt -Oz "$WIZER" -o "$WIZER_OPT" >/dev/null; then
      echo "wasm_breakdown: post-wizer wasm-opt failed" >&2
      exit 2
    fi
  else
    WIZER_OPT="$WIZER"
  fi
else
  echo "[skip] wizer not on PATH; cwasm will represent the no-wizer shape" >&2
  WIZER_OPT="$OPT"
fi

if ! wasmtime compile "$WIZER_OPT" -o "$CWASM" 2>/dev/null; then
  echo "wasm_breakdown: wasmtime compile failed" >&2
  exit 2
fi

# 3. Test script — kept trivial so post-wizer eval is the floor.
#    Whatever extra time `eval_done - runtime_ready` shows for this
#    script is the "minimum useful work" baseline.
SCRIPT="$PERF_TMPDIR/breakdown.rb"
printf '%s\n' "$SCRIPT_INLINE" > "$SCRIPT"

# 4. Run N times. Capture stderr (trace lines + /usr/bin/time -p
#    output) per run; drop stdout (script output, "3\n" here).
echo ""
echo "Running $RUNS iterations of cwasm + '$SCRIPT_INLINE'..." >&2
for i in $(seq 1 "$RUNS"); do
  # `--dir "$PERF_TMPDIR"` grants wasmtime read access to the
  # tempdir holding the script (wasi sandboxes the filesystem;
  # without an explicit grant, `eval_file` gets `No such file`).
  # The python3 wrapper prints `<wall_us>\n` to stderr after the
  # child exits — appended after the rubyrs trace lines in the
  # same file. The wall measurement INCLUDES python3's own
  # subprocess.run dispatch (~1-2 ms), which we accept: the
  # alternative (gdate + arithmetic) is non-portable. The python
  # overhead is roughly constant across runs, so MIN-of-N filters
  # it the same way it filters wasmtime jitter.
  python3 -c "$TIMER" wasmtime run --allow-precompiled \
    --dir "$PERF_TMPDIR" \
    "$CWASM" "$SCRIPT" \
    >/dev/null 2>"$PERF_TMPDIR/run.$i.txt" || {
      echo "wasm_breakdown: wasmtime run failed on iteration $i" >&2
      echo "--- last stderr ---" >&2
      cat "$PERF_TMPDIR/run.$i.txt" >&2
      exit 2
    }
done

# 5. Extract MIN per label, MIN of wall-clock total.
extract_min() {
  # $1 = label
  awk -F'\t' -v label="$1" '
    $1=="trace-startup" && $2==label { gsub(/us/, "", $3); print $3 }
  ' "$PERF_TMPDIR"/run.*.txt | sort -n | head -1
}

extract_wall_us_min() {
  # Python wrapper appends a bare integer (microseconds) on its
  # own line at the end of stderr — pick the last numeric-only
  # line per run (any trace line is `trace-startup\t...`).
  for f in "$PERF_TMPDIR"/run.*.txt; do
    awk '/^[0-9]+$/ { last=$1 } END { if (last != "") print last }' "$f"
  done | sort -n | head -1
}

LABELS=("entry" "args" "env_collected" "runtime_ready" "eval_done")

# 6. Verify we actually captured trace lines (the trace-startup
#    feature is the load-bearing assumption; if missing, the user
#    likely built without it).
if [[ -z "$(extract_min "${LABELS[0]}")" ]]; then
  echo "wasm_breakdown: no trace-startup lines captured" >&2
  echo "  (was the binary built with --features trace-startup?)" >&2
  exit 2
fi

WALL_US="$(extract_wall_us_min)"
if [[ -z "$WALL_US" ]]; then
  echo "wasm_breakdown: failed to parse /usr/bin/time output" >&2
  exit 2
fi

# 7. Print the breakdown.
echo ""
echo "=== cold-start breakdown (MIN of $RUNS runs, microseconds) ==="
echo ""
printf "  %-22s %10s %14s\n" "phase" "cumulative" "delta"
printf "  %-22s %10s %14s\n" "-----" "----------" "-----"

PREV=0
for label in "${LABELS[@]}"; do
  cur="$(extract_min "$label")"
  if [[ -z "$cur" ]]; then continue; fi
  delta=$((cur - PREV))
  if [[ "$label" == "entry" ]]; then
    printf "  %-22s %10d us %14s\n" "$label" "$cur" "—"
  else
    printf "  %-22s %10d us  %12d us\n" "$label" "$cur" "$delta"
  fi
  PREV=$cur
done
EVAL_DONE_US=$PREV

# Pre-`entry` is everything the host spent before our first
# `Instant::now()` ran: wasmtime CLI launch, runtime init, cwasm
# mmap + symbol resolution, wasi-libc startup (allocator, stdio,
# __environ population), C `_start` dispatch into Rust `main`.
PRE_ENTRY_US=$((WALL_US - EVAL_DONE_US))

echo ""
printf "  %-22s %10s\n" "wall total (MIN):" "$WALL_US us"
printf "  %-22s %10s\n" "  rubyrs main():" "$EVAL_DONE_US us  ← we own this"
printf "  %-22s %10s\n" "  wasmtime+wasi+load:" "$PRE_ENTRY_US us  ← runtime-shape ceiling"
echo ""
echo "  '${SCRIPT_INLINE}' end-to-end via wasmtime run --allow-precompiled."
echo "  Set RUNS=N or SCRIPT_INLINE='...' to vary."
