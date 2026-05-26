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
# spikes AND wasmtime's first-run wasm-cache cold population
# (subsequent runs hit `~/.cache/wasmtime`, so the warm-cache
# wall is what `min` picks). This measures rubyrs interpreter
# cost under wasi, not wasmtime's own cache-warmup time. If a
# future row needs literal cold-cache measurement, add it as
# its own row that clears the cache or passes `--disable-cache`
# between runs — see `perf/wasm_baselines.tsv` for the rationale.
#
# Why no RSS gate: peak-RSS under wasmtime conflates the host VM's
# resident size with the guest's linear-memory working set, and
# there's no portable way to project just the guest's footprint
# back to the host. Native already has an RSS budget over in
# `baselines.tsv` — the wall metric here is the wasm-specific lever.
#
# Exit codes:
#   0  — every workload within its wall budget
#   1  — at least one workload over budget
#   2  — setup error (wasm not built, time/wasmtime missing,
#        row malformed)

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
#   raw .wasm  --[wasm-opt -Oz]-->  .opt.wasm  --[wasmtime compile]-->  .cwasm
#                  (optional)                          (always)
#
# Layered for two reasons:
#   1. wasm-opt -Oz shrinks the deliverable binary ~21% (1.48 MB →
#      1.17 MB locally) and is what an embedder downstream would
#      want to ship; running it here keeps the cwasm we measure
#      against the realistic shipping shape.
#   2. `wasmtime compile` AOT-compiles to a `.cwasm` that wasmtime
#      can `run --allow-precompiled` against — bypasses JIT for
#      every measured invocation, so the gate fences the "cold
#      start with pre-compiled module" path (the headline cold-
#      start story) rather than the every-invocation JIT cost.
#
# Local PoC: this combo drops the wasmtime startup_floor from
# ~20 ms steady (raw .wasm) to ~10 ms (.cwasm) — and from a
# 200 ms first-run cold to ~10 ms (no more per-run JIT). See
# `perf/wasm_baselines.tsv` for the budget rationale.
#
# wasm-opt is OPTIONAL — if `wasm-opt` isn't on PATH the script
# proceeds with the raw .wasm. Skipping it costs ~10% of the
# binary-size win but doesn't break the gate.
if ! command -v wasm-opt >/dev/null 2>&1; then
  echo "wasm_check: wasm-opt not on PATH — skipping the -Oz size pass (install \`binaryen\` to enable)"
  OPT_WASM="$WASM"
else
  OPT_WASM="${WASM%.wasm}.opt.wasm"
  echo "[wasm_check] wasm-opt -Oz $WASM -> $OPT_WASM"
  wasm-opt -Oz "$WASM" -o "$OPT_WASM" >/dev/null
fi

CWASM="${OPT_WASM%.wasm}.cwasm"
echo "[wasm_check] wasmtime compile $OPT_WASM -> $CWASM"
wasmtime compile "$OPT_WASM" -o "$CWASM" >/dev/null

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
