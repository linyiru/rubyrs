#!/usr/bin/env bash
# perf/wasm_check.sh — wasm32-wasip1 wall-time ratchet.
#
# Sibling to `perf/check.sh` (which gates native rubyrs on wall +
# peak-RSS). Same absolute-baselines policy, same ratchet etiquette,
# scoped to the wasi shape because wasmtime startup + wasi syscall
# overhead make native budgets irrelevant here.
#
# For each row in `perf/wasm_baselines.tsv`: run the workload
# through `wasmtime` three times, take the MIN wall time across
# the runs, and fail if it exceeds the row's `max_wall_ms`.
#
# MIN-of-3 is the steady-state floor — drops both CI jitter
# spikes AND OS-level noise (page-cache warm-up of the .cwasm,
# wasmtime's own runtime cold start, process-spawn variance,
# `/usr/bin/time` 10ms-granularity rounding). Because the gate
# measures against the pre-compiled `.cwasm` produced by the
# build prelude below (wasm-opt → wasmtime compile) with
# `--allow-precompiled`, JIT cost has already been eliminated;
# there is no longer a per-run wasm-cache warm-up cycle for
# min-of-3 to filter. This means each of the 3 timed runs is
# essentially the same shape (load .cwasm + run script), and
# the MIN reducer is mostly there to absorb spawn/timer
# granularity jitter.
#
# Why no RSS gate: peak-RSS under wasmtime conflates the host VM's
# resident size with the guest's linear-memory working set, and
# there's no portable way to project just the guest's footprint
# back to the host. Native already has an RSS budget over in
# `baselines.tsv` — the wall metric here is the wasm-specific lever.
#
# Exit codes:
#   0       — every workload within its wall budget
#   1       — at least one workload over budget
#   2       — setup error (wasm not built, time/wasmtime missing,
#             row malformed)
#   128+sig — interrupted by a signal (Ctrl-C → 130 for SIGINT,
#             143 for SIGTERM). Cleanup still runs via the
#             EXIT/INT/TERM trap. Wrapping CI scripts that
#             classify exit codes should treat 128–255 as
#             "interrupted", separately from the setup/budget
#             buckets above.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

WASM="${WASM:-target/wasm32-wasip1/release/rubyrs.wasm}"
BASELINES="${BASELINES:-perf/wasm_baselines.tsv}"
RUNS="${RUNS:-3}"

if [[ ! -f "$WASM" ]]; then
  echo "wasm_check: rubyrs.wasm not found at $WASM" >&2
  echo "wasm_check: run \`cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features\` first" >&2
  exit 2
fi
if [[ ! -r "$BASELINES" ]]; then
  echo "wasm_check: baselines file not readable at $BASELINES" >&2
  exit 2
fi
if [[ ! "$RUNS" =~ ^[1-9][0-9]*$ ]]; then
  echo "wasm_check: RUNS must be a positive integer, got '$RUNS'" >&2
  exit 2
fi
if ! command -v wasmtime >/dev/null 2>&1; then
  echo "wasm_check: wasmtime not on PATH — see docs/DEVELOPMENT.md" >&2
  exit 2
fi
if [[ ! -x /usr/bin/time ]]; then
  echo "wasm_check: /usr/bin/time not executable" >&2
  exit 2
fi

# Build pipeline for the artifact the gate actually times:
#
#   raw .wasm  --[wasm-opt -Oz]-->  .opt.wasm  --[wizer]-->  .wizer.wasm
#                  (optional)                  (optional)
#                                                  |
#                              [wasm-opt -Oz again]
#                                                  |
#                                             .wizer.opt.wasm
#                                                  |
#                                       [wasmtime compile]
#                                                  |
#                                              .cwasm  ← gate measures this
#
# Layered for three reasons:
#   1. wasm-opt -Oz shrinks the deliverable binary ~18% (3.64 MB →
#      3.00 MB locally, 2026-07-06) — what a downstream embedder would ship
#      if distributing the .wasm. Running it here keeps the AOT
#      input matching what a consumer would actually deploy.
#   2. wizer pre-initializes the Runtime (class registration +
#      preamble bytecode load) by calling the `wizer.initialize`
#      export and snapshotting linear memory. The post-wizer
#      binary skips that work at every invocation. Local measure
#      2026-07-06: ~8.4 ms cold-start saving (17.4 → 9.0 ms on
#      `puts 1+2` — the preamble grew Jekyll-era, so pre-init now
#      pays for roughly half the AOT wall). Wizer is OPTIONAL; the
#      script falls back to the no-wizer path if it's missing.
#      A second wasm-opt -Oz pass after wizer compacts the
#      snapshotted memory layout for further size reduction.
#   3. `wasmtime compile` AOT-compiles to a `.cwasm` that wasmtime
#      can `run --allow-precompiled` against — bypasses JIT for
#      every measured invocation, so the gate fences the "cold
#      start with pre-compiled module" path (the headline cold-
#      start story) rather than the every-invocation JIT cost.
#      Note that the .cwasm itself is NOT a shipping artifact —
#      it's wasmtime-version + host-arch specific machine code
#      and must be regenerated per consumer environment.
#
# Local numbers, `puts 1+2` cold start (Apple M2 Max, wasi-sdk 24,
# wasmtime 45, hyperfine --warmup 3, ≥15 runs, re-measured
# 2026-07-06 — matches the Cold-start table in docs/BENCHMARKS.md;
# the ~20/7.6/7.2 ms PoC-era numbers predate the Jekyll-era binary
# growth):
#   - raw .wasm + JIT-each-run:        ~44 ms
#   - opt + AOT cwasm:                 ~17.4 ms
#   - wizer + opt + AOT cwasm:          ~9.0 ms (this gate)
# See `perf/wasm_baselines.tsv` for the budget rationale.
#
# Derived build artifacts live in a per-invocation tempdir
# (cleaned via `trap` on EXIT), not next to the input `$WASM`.
# Reasons:
#   1. `$WASM` may point at a read-only / shared path under some
#      embedding setups; writing next to it would fail outright.
#   2. Leaving `.opt.wasm` / `.cwasm` siblings next to the source
#      surprises local runs and bloats incremental dev workflows.
#   3. wasmtime compile is fast enough (~0.5s) that caching the
#      output across runs isn't worth the surprise.
# A dedicated subdir under `$TMPDIR` is plenty for the gate's
# lifetime. Use the positional template form (`mktemp -d <prefix>XXXXXX`)
# rather than `-t` — BSD mktemp (macOS) and GNU mktemp (Linux)
# disagree on what `-t` means: BSD takes it as a literal prefix and
# adds its own randomization, embedding "XXXXXX" in the dir name on
# macOS. Positional template is portable.
#
# Wrap mktemp itself in an exit-2 path: a read-only $TMPDIR / full
# disk / restrictive sandbox makes it fail, and `set -e` would
# otherwise abort with an opaque non-zero code (no "setup error"
# diagnostic), violating the 0/1/2 contract the header documents.
PERF_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/rubyrs-wasm-perf.XXXXXX")" || {
  echo "wasm_check: mktemp -d failed (TMPDIR=${TMPDIR:-/tmp}); cannot stage AOT artifacts" >&2
  exit 2
}
# Cover SIGINT/SIGTERM too — `trap '...' EXIT` alone doesn't fire
# on signals (bash takes the default signal action and exits
# without running EXIT). Without the extra signal handlers a
# Ctrl-C during wasm-opt (2-10s) or wasmtime compile leaves the
# tempdir orphaned. The trap re-raises after cleanup so the exit
# code stays honest.
trap 'rm -rf "$PERF_TMPDIR"' EXIT
trap 'rm -rf "$PERF_TMPDIR"; trap - INT TERM; kill -INT $$' INT TERM

# wasm-opt is OPTIONAL — if `wasm-opt` isn't on PATH the script
# proceeds with the raw .wasm. Skipping it costs ~10% of the
# binary-size win but doesn't break the gate.
if ! command -v wasm-opt >/dev/null 2>&1; then
  echo "wasm_check: wasm-opt not on PATH — skipping the -Oz size pass (install \`binaryen\` to enable)"
  OPT_WASM="$WASM"
else
  OPT_WASM="$PERF_TMPDIR/rubyrs.opt.wasm"
  echo "[wasm_check] wasm-opt -Oz $WASM -> $OPT_WASM"
  # wasm-opt failure is a SETUP error (broken build tool, bad
  # input wasm, etc.), not a perf regression. Catch and exit 2
  # per the documented 0/1/2 contract instead of letting
  # `set -e` propagate wasm-opt's exit code (typically 1, which
  # would be misclassified as "budget exceeded").
  if ! wasm-opt -Oz "$WASM" -o "$OPT_WASM" >/dev/null; then
    echo "wasm_check: wasm-opt -Oz failed on $WASM" >&2
    exit 2
  fi
fi

# wizer is OPTIONAL — when present, run the binary's
# `wizer.initialize` export to pre-build Runtime state (classes +
# preamble), then re-pass through wasm-opt -Oz to compact the
# snapshotted memory. Yields ~0.5 ms cold-start improvement.
# When absent, the script proceeds with the already-optimised
# .wasm (gate still PASSes; just loses the wizer win).
WIZER_WASM="$OPT_WASM"
if command -v wizer >/dev/null 2>&1; then
    WIZER_WASM_OUT="$PERF_TMPDIR/rubyrs.wizer.wasm"
    echo "[wasm_check] wizer $OPT_WASM -> $WIZER_WASM_OUT"
    # --allow-wasi lets the wizer pass over our .wasm even though
    # it imports wasi syscalls; our wizer.initialize itself does
    # NOT call any imports (per wizer's rule), but the import
    # table includes wasi for `_start` use later.
    #
    # `--init-func wizer.initialize` is load-bearing for wizer
    # v11+ (CI's pinned version): v11 changed the default
    # expected export name from `wizer.initialize` (dot) to
    # `wizer-initialize` (hyphen). Our `crates/rubyrs/src/lib.rs`
    # still uses the dot form via `#[unsafe(export_name =
    # "wizer.initialize")]`, so we have to tell wizer which name
    # to look up. v10 accepts the flag too — portable override.
    if ! wizer --allow-wasi --init-func wizer.initialize "$OPT_WASM" -o "$WIZER_WASM_OUT" >/dev/null 2>&1; then
        echo "wasm_check: wizer pre-init failed on $OPT_WASM" >&2
        exit 2
    fi
    # Second wasm-opt pass compacts the wizer-snapshotted data
    # section. Optional — skip on wasm-opt absence (already
    # warned above).
    if command -v wasm-opt >/dev/null 2>&1; then
        WIZER_OPT_WASM="$PERF_TMPDIR/rubyrs.wizer.opt.wasm"
        if ! wasm-opt -Oz "$WIZER_WASM_OUT" -o "$WIZER_OPT_WASM" >/dev/null; then
            echo "wasm_check: wasm-opt -Oz (post-wizer) failed" >&2
            exit 2
        fi
        WIZER_WASM="$WIZER_OPT_WASM"
    else
        WIZER_WASM="$WIZER_WASM_OUT"
    fi
else
    echo "wasm_check: wizer not on PATH — skipping pre-init pass (install \`wizer\` to enable; expect ~0.5 ms cold-start savings)"
fi

CWASM="$PERF_TMPDIR/rubyrs.cwasm"
echo "[wasm_check] wasmtime compile $WIZER_WASM -> $CWASM"
# wasmtime compile failure (incompatible subcommand, malformed
# wasm, etc.) is likewise a setup error — same 0/1/2 contract
# reasoning as the wasm-opt arm above.
if ! wasmtime compile "$WIZER_WASM" -o "$CWASM" >/dev/null; then
  echo "wasm_check: wasmtime compile failed on $WIZER_WASM" >&2
  exit 2
fi

# `/usr/bin/time` parsing differs by platform (same shape as the
# host check.sh). Only wall is consumed here; RSS lines are ignored.
# All wasmtime invocations below use `--allow-precompiled` so the
# AOT `.cwasm` path is exercised end-to-end.
PLATFORM="$(uname -s)"
measure_wall_ms() {
  local script="$1"
  local out rc=0
  if [[ "$PLATFORM" == "Darwin" ]]; then
    out=$(LC_ALL=C /usr/bin/time wasmtime run --allow-precompiled --dir=. "$CWASM" "$script" 2>&1 >/dev/null) || rc=$?
    if (( rc != 0 )); then
      echo "wasm_check: workload \`$script\` exited with status $rc under wasmtime" >&2
      [[ -n "$out" ]] && echo "$out" | sed 's/^/  | /' >&2
      echo "ERR"; return
    fi
    # macOS BSD-time format: `        0.46 real         ...`
    local secs
    secs=$(awk '/real/ {print $1; exit}' <<<"$out")
    if [[ -z "$secs" ]]; then
      echo "wasm_check: could not parse macOS time output for $script" >&2
      echo "$out" | sed 's/^/  | /' >&2
      echo "ERR"; return
    fi
    # Round up (conservative — see baselines.tsv rationale).
    awk -v s="$secs" 'BEGIN {
      ms_f = s * 1000;
      ms = int(ms_f);
      if (ms_f > ms) ms = ms + 1;
      printf "%d\n", ms;
    }'
  else
    out=$(LC_ALL=C /usr/bin/time -v wasmtime run --allow-precompiled --dir=. "$CWASM" "$script" 2>&1 >/dev/null) || rc=$?
    if (( rc != 0 )); then
      echo "wasm_check: workload \`$script\` exited with status $rc under wasmtime" >&2
      [[ -n "$out" ]] && echo "$out" | sed 's/^/  | /' >&2
      echo "ERR"; return
    fi
    # GNU time -v: `Elapsed (wall clock) time (h:mm:ss or m:ss): 0:00.46`
    local wall
    wall=$(awk -F': ' '/Elapsed \(wall clock\)/ {print $2; exit}' <<<"$out")
    if [[ -z "$wall" ]]; then
      echo "wasm_check: could not parse GNU time output for $script" >&2
      echo "$out" | sed 's/^/  | /' >&2
      echo "ERR"; return
    fi
    # `m:ss.ss` or `h:mm:ss.ss`
    # On an unexpected `wall` shape the awk branch must emit
    # the same `ERR` sentinel the upstream parse errors use —
    # an empty string would slip past the caller's
    # `"$ms" == "ERR"` check and fail later inside the integer
    # `if [[ ... -lt ... ]]` comparison with a confusing shell
    # error rather than the documented setup-failure exit code.
    local parsed
    parsed=$(awk -v t="$wall" 'BEGIN {
      n = split(t, parts, ":");
      if (n == 2) {
        ms_f = (parts[1] * 60 + parts[2]) * 1000;
      } else if (n == 3) {
        ms_f = (parts[1] * 3600 + parts[2] * 60 + parts[3]) * 1000;
      } else {
        print "ERR"; exit 0;
      }
      ms = int(ms_f);
      if (ms_f > ms) ms = ms + 1;
      printf "%d\n", ms;
    }')
    if [[ "$parsed" == "ERR" ]]; then
      echo "wasm_check: GNU time wall \`$wall\` had unexpected shape for $script" >&2
      echo "ERR"; return
    fi
    printf "%s\n" "$parsed"
  fi
}

declare -i regression_seen=0
declare -i setup_failure_seen=0
declare -i seen=0

# Read TSV. Skip comments + blank lines. `IFS=$'\t'` keeps
# tab-separated fields intact even when a `note` contains spaces.
while IFS=$'\t' read -r script max_wall_ms note; do
  [[ -z "${script:-}" ]] && continue
  case "$script" in \#*) continue ;; esac

  if [[ -z "${max_wall_ms:-}" || ! "$max_wall_ms" =~ ^[0-9]+$ ]]; then
    echo "wasm_check: malformed row (max_wall_ms): \`$script\t$max_wall_ms\t${note:-}\`" >&2
    exit 2
  fi
  if [[ ! -f "$script" ]]; then
    echo "wasm_check: workload not found at $script (in $BASELINES)" >&2
    exit 2
  fi

  seen+=1
  echo "[wasm_check] $script (budget ${max_wall_ms} ms)"
  best_ms=""
  for ((i=1; i<=RUNS; i++)); do
    ms=$(measure_wall_ms "$script")
    if [[ "$ms" == "ERR" ]]; then
      # Measurement failure (workload crashed mid-run, time
      # output unparseable, etc.) is a SETUP issue — escalate
      # to exit 2 per the documented 0/1/2 contract, not the
      # exit-1-budget-exceeded path. Mixing these would surface
      # an interpreter ICE under wasi as a "perf regression",
      # which sends a future debugger to the wrong file.
      setup_failure_seen=1
      best_ms=""
      break
    fi
    echo "  run $i: ${ms} ms"
    if [[ -z "$best_ms" || "$ms" -lt "$best_ms" ]]; then best_ms="$ms"; fi
  done
  if [[ -z "$best_ms" ]]; then continue; fi
  if [[ "$max_wall_ms" -eq 0 ]]; then
    # `0` is the documented sentinel for "wall check disabled" —
    # see baselines.tsv. Print that explicitly so CI logs don't
    # imply a 0 ms budget was met (which would read as
    # surprisingly tight to anyone scanning the log).
    echo "  ok: best wall ${best_ms} ms (wall check disabled)"
  elif [[ "$best_ms" -gt "$max_wall_ms" ]]; then
    echo "  FAIL: best wall ${best_ms} ms > budget ${max_wall_ms} ms" >&2
    regression_seen=1
  else
    echo "  ok: best wall ${best_ms} ms (≤ ${max_wall_ms} ms)"
  fi
done < "$BASELINES"

if [[ "$seen" -eq 0 ]]; then
  echo "wasm_check: zero workloads parsed from $BASELINES" >&2
  exit 2
fi
# Exit-code priority: setup failure (2) outranks budget regression
# (1). A measurement that never completed can't be a clean perf
# signal anyway, so reporting it as the higher-severity setup
# code is honest about what failed.
if [[ "$setup_failure_seen" -ne 0 ]]; then exit 2; fi
if [[ "$regression_seen" -ne 0 ]]; then exit 1; fi
exit 0
