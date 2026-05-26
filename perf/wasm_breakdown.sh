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
for tool in wasmtime cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "wasm_breakdown: required tool not found: $tool" >&2
    exit 2
  fi
done

# We need ns-precision wall timing — macOS `/usr/bin/time -p` rounds
# to 10 ms, useless at sub-10 ms scale. Use the in-tree
# `rubyrs-wasm-timer` binary: ~50-200 us own overhead vs python's
# 1-2 ms interpreter init, so the wasmtime+wasi measurement is
# closer to a "naked" host-spawn reading.
TIMER_BIN="target/release/rubyrs-wasm-timer"
if [[ ! -x "$TIMER_BIN" ]]; then
  echo "[build] cargo build --release -p rubyrs-wasm-timer" >&2
  if ! cargo build --release -p rubyrs-wasm-timer >&2; then
    echo "wasm_breakdown: failed to build rubyrs-wasm-timer" >&2
    exit 2
  fi
fi

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
WIZER="$PERF_TMPDIR/rubyrs.wizer.wasm"
WIZER_OPT="$PERF_TMPDIR/rubyrs.wizer.opt.wasm"
CWASM="$PERF_TMPDIR/rubyrs.cwasm"

# Reorder vs the earlier `opt → wizer → opt` shape: do wizer
# FIRST, then a single wasm-opt -Oz post-wizer. On Linux's
# apt-installed binaryen (v116) the pre-wizer -Oz strips the
# `wizer.initialize` export despite it being an export root,
# breaking wizer with "the Wasm module does not have a
# wizer-initialize export". Running wizer first sidesteps the
# bug — by the time wasm-opt sees the .wasm, wizer.initialize
# is genuinely dead (the snapshot is taken) and DCE is correct.
# macOS binaryen (v123+) doesn't trigger this; reorder makes
# the pipeline portable.
if command -v wizer >/dev/null 2>&1; then
  if ! wizer --allow-wasi -o "$WIZER" "$RAW_WASM" 2>/dev/null; then
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
  # No wizer → fall back to the raw .wasm (optionally opt'd) for
  # the wasmtime compile step. The breakdown still runs, just
  # without the wizer snapshot benefit.
  echo "[skip] wizer not on PATH; cwasm will represent the no-wizer shape" >&2
  if command -v wasm-opt >/dev/null 2>&1; then
    if ! wasm-opt -Oz "$RAW_WASM" -o "$WIZER_OPT" >/dev/null; then
      echo "wasm_breakdown: wasm-opt -Oz failed on raw .wasm" >&2
      exit 2
    fi
  else
    WIZER_OPT="$RAW_WASM"
  fi
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
  # rubyrs-wasm-timer captures Instant::now() right before its
  # `Command::status` call and prints a sentinel line on stderr
  # after the child exits — appended after the rubyrs trace
  # lines in the same file.
  "$TIMER_BIN" wasmtime run --allow-precompiled \
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
  # rubyrs-wasm-timer appends one line per run:
  #   `wasm-timer\twall_us\t<microseconds>`
  # Distinct sentinel from the trace-startup lines so we can grep
  # unambiguously even if wasmtime adds verbose stderr.
  awk -F'\t' '$1=="wasm-timer" && $2=="wall_us" { print $3 }' \
    "$PERF_TMPDIR"/run.*.txt | sort -n | head -1
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

# 8. Layered baselines so the reader can decompose the runtime-shape
#    ceiling further. Without these, "wasmtime+wasi+load: 7800 us"
#    is an opaque blob; with them, it's:
#       /usr/bin/true:    ~1000 us (macOS fork+exec+dyld floor)
#       wasmtime --version: ~5500 us (above + wasmtime init)
#       full cwasm run:     ~8000 us (above + cwasm mmap + wasi init
#                                     + rubyrs main)
#    which makes "what can I actually attack?" obvious.
baseline_min() {
  # $1 = label, rest = command + args
  local label="$1"; shift
  local best=""
  for _ in $(seq 1 "$RUNS"); do
    local us
    us=$("$TIMER_BIN" "$@" 2>&1 1>/dev/null | awk -F'\t' '$1=="wasm-timer" && $2=="wall_us" { print $3 }')
    if [[ -z "$best" ]] || [[ "$us" -lt "$best" ]]; then best=$us; fi
  done
  printf "  %-22s %10s us\n" "$label" "$best"
}

echo ""
echo "=== process-spawn baselines (MIN of $RUNS runs, same timer) ==="
echo ""
baseline_min "/usr/bin/true:" /usr/bin/true
baseline_min "wasmtime --version:" wasmtime --version
echo ""
echo "  Subtract '/usr/bin/true' from any row to get the host-runtime-"
echo "  specific overhead above the macOS fork+exec floor."

# 9. Cross-runtime comparison. Same wizer'd .wasm, different host
#    runtimes — answers "would another wasm runtime cold-start
#    faster than wasmtime?" Each row needs its own input shape
#    (cwasm format is wasmtime-specific; wasmer has its own AOT
#    via `wasmer compile`; wasm3 takes raw .wasm and interprets).
#
#    Rows are skipped silently if the runtime / artifact isn't
#    present. Build deps:
#      rubyrs-wasm-embed   `cargo build --release -p rubyrs-wasm-embed`
#      wasmer (AOT .wasmu) `wasmer compile -o $TMP/rubyrs.wasmu $WIZER_OPT_WASM`
#      wasmer (JIT .wasm)  brew install wasmer
#      wasm3 (interp)      brew install wasm3

runtime_min() {
  # $1 = label, rest = command + args. Returns MIN of $RUNS runs
  # via the in-tree timer; prints nothing if the command is
  # missing or the probe-run failed (e.g. baked embedder with no
  # cwasm available, wasmer rejecting a wasm feature, etc.).
  local label="$1"; shift
  local first="$1"
  if ! command -v "$first" >/dev/null 2>&1 && [[ ! -x "$first" ]]; then
    return
  fi
  # Probe-run once. The timer ALWAYS prints its sentinel line —
  # the only reason to skip a row is if the child exited
  # non-zero. Without this guard, a fast-failing child (e.g.
  # embedder with empty baked cwasm) would record an artificially
  # low wall time and silently misrepresent the row.
  if ! "$@" >/dev/null 2>&1; then
    return
  fi
  local best=""
  for _ in $(seq 1 "$RUNS"); do
    local us
    us=$("$TIMER_BIN" "$@" 2>&1 1>/dev/null \
          | awk -F'\t' '$1=="wasm-timer" && $2=="wall_us" { print $3 }')
    if [[ -n "$us" ]] && { [[ -z "$best" ]] || [[ "$us" -lt "$best" ]]; }; then
      best=$us
    fi
  done
  if [[ -n "$best" ]]; then
    printf "  %-32s %10s us\n" "$label" "$best"
  fi
}

EMBED_BIN="target/release/rubyrs-wasm-embed"
WASMER_AOT="$PERF_TMPDIR/rubyrs.wasmu"
if command -v wasmer >/dev/null 2>&1; then
  # Pre-compile for wasmer once so the AOT row times the cold-load
  # path, matching what wasmtime cwasm does. Silent on failure —
  # if wasmer rejects the wasm (newer features etc.), just skip
  # the AOT row and let the JIT row carry that runtime's data.
  # Suppress wasmer's "Compiler: cranelift / Target: ..." chatter
  # by routing both streams to /dev/null.
  wasmer compile -o "$WASMER_AOT" "$WIZER_OPT" >/dev/null 2>&1 || true
fi

echo ""
echo "=== cross-runtime comparison (MIN of $RUNS runs, same wizer'd .wasm) ==="
echo ""
runtime_min "wasmtime CLI (AOT cwasm):"    wasmtime run --allow-precompiled --dir "$PERF_TMPDIR" "$CWASM" "$SCRIPT"
if [[ -x "$EMBED_BIN" ]]; then
  # The embedder accepts either a baked-in cwasm (no env var, built
  # via perf/build_embedder.sh) or an external one via
  # RUBYRS_CWASM. Surface both rows so the trade-off is visible:
  #   - Baked: single-binary ship shape, +~750 us cold start due to
  #     larger binary (macOS dyld page-load + code-signing cost).
  #   - External: two-file ship shape but faster cold start because
  #     the cwasm mmaps from a separate file.
  # The `runtime_min` helper passes env via the timer's inherited
  # environment, so prefixing RUBYRS_CWASM=… on the line below
  # only affects that one call.
  runtime_min "embedder (baked cwasm):"      "$EMBED_BIN" "$SCRIPT"
  RUBYRS_CWASM="$CWASM" runtime_min "embedder (RUBYRS_CWASM external):" "$EMBED_BIN" "$SCRIPT"
fi
if [[ -f "$WASMER_AOT" ]]; then
  runtime_min "wasmer (AOT .wasmu):"        wasmer run --volume "$PERF_TMPDIR:$PERF_TMPDIR" "$WASMER_AOT" -- "$SCRIPT"
fi
runtime_min "wasmer (JIT .wasm):"          wasmer run --volume "$PERF_TMPDIR:$PERF_TMPDIR" "$WIZER_OPT" -- "$SCRIPT"

# wasm3 has no preopen-dir flag and its wasi shim restricts the
# guest to the host process's cwd. The timer's `--cwd` switch
# lets us cd into the tempdir BEFORE the timer anchor, so the
# measurement excludes our chdir. wasm3 then sees the script via
# the basename and resolves it through its in-process wasi shim.
if command -v wasm3 >/dev/null 2>&1; then
  wasm3_best=""
  wasm3_script_base="$(basename "$SCRIPT")"
  wasm3_wasm_base="$(basename "$WIZER_OPT")"
  # $WIZER_OPT already lives in $PERF_TMPDIR (pipeline step above),
  # so the cwd-based wasm3 invocation can address it by basename.
  for _ in $(seq 1 "$RUNS"); do
    wasm3_us=$("$TIMER_BIN" --cwd "$PERF_TMPDIR" wasm3 "$wasm3_wasm_base" "$wasm3_script_base" 2>&1 1>/dev/null \
          | awk -F'\t' '$1=="wasm-timer" && $2=="wall_us" { print $3 }')
    if [[ -n "$wasm3_us" ]] && { [[ -z "$wasm3_best" ]] || [[ "$wasm3_us" -lt "$wasm3_best" ]]; }; then
      wasm3_best=$wasm3_us
    fi
  done
  if [[ -n "$wasm3_best" ]]; then
    printf "  %-32s %10s us\n" "wasm3 (interpreter):" "$wasm3_best"
  fi
fi

echo ""
echo "  Same wizer-pre-initialized .wasm in every row. The wasmtime"
echo "  cwasm and wasmer .wasmu are AOT-compiled artifacts of that"
echo "  .wasm; wasm3 and 'wasmer (JIT)' load the .wasm itself."
echo "  Embedder source: crates/rubyrs-wasm-embed/."
