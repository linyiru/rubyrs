#!/bin/bash
# perf/p2a_compare.sh — P2-A pivot decision-gate measurement.
#
# Runs the Brewfile-shape DSL workload
# (`crates/rubyrs/tests/wasm/brewfile_dsl.rb`) under three shapes:
#
#   1. raw rubyrs.wasm (no AOT)              — what we ship to embedders
#   2. AOT rubyrs.cwasm                      — what embedders run locally
#   3. raw ruby.wasm   (no AOT)              — MRI 3.4 wasi-minimal
#   4. AOT ruby.cwasm                        — same after `wasmtime compile`
#
# Reports MIN-of-5 wall ms per shape and the cross-runtime ratio.
# The numbers feed the P2-A decision-gate row in `docs/BENCHMARKS.md`.
#
# Prerequisites (the wider rubyrs project already needs these for the
# wasm perf gate; this script just consumes them):
#
#   - `target/wasm32-wasip1/release/rubyrs.wasm` (build via
#     `cargo build --release --target wasm32-wasip1 -p rubyrs
#     --no-default-features`)
#   - `wasm-opt`, `wasmtime`, `/usr/bin/time` on PATH
#   - ruby.wasm 3.4 wasi-minimal unpacked at the path in `RUBYWASM` below
#     (https://github.com/ruby/ruby.wasm/releases — pick the
#     `ruby-3.4-wasm32-unknown-wasip1-minimal.tar.gz` asset)

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

RUBYRS_WASM="${RUBYRS_WASM:-target/wasm32-wasip1/release/rubyrs.wasm}"
RUBYWASM="${RUBYWASM:-$HOME/ruby-wasm/ruby-3.4-wasm32-unknown-wasip1-minimal/usr/local/bin/ruby}"
SCRIPT="${SCRIPT:-crates/rubyrs/tests/wasm/brewfile_dsl.rb}"
RUNS="${RUNS:-5}"

if [[ ! -f "$RUBYRS_WASM" ]]; then
  echo "missing $RUBYRS_WASM — build with:" >&2
  echo "  cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features" >&2
  exit 2
fi
if [[ ! -f "$RUBYWASM" ]]; then
  echo "missing $RUBYWASM — download ruby.wasm 3.4 wasi-minimal from" >&2
  echo "  https://github.com/ruby/ruby.wasm/releases and extract to \$HOME/ruby-wasm/" >&2
  exit 2
fi
for tool in wasm-opt wasmtime; do
  command -v "$tool" >/dev/null || { echo "missing $tool on PATH" >&2; exit 2; }
done

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# AOT-compile both side's wasm for fair comparison
wasm-opt -Oz "$RUBYRS_WASM" -o "$TMP/rubyrs.opt.wasm"  >/dev/null
wasmtime compile -o "$TMP/rubyrs.cwasm" "$TMP/rubyrs.opt.wasm" >/dev/null
wasmtime compile -o "$TMP/ruby.cwasm" "$RUBYWASM" >/dev/null

run_min() {
  local label="$1" cmd_prefix="$2"
  local min_ms="" sec
  for ((i = 1; i <= RUNS; i++)); do
    sec=$(LC_ALL=C /usr/bin/time $cmd_prefix "$SCRIPT" 2>&1 >/dev/null \
          | awk '/real/ {print $1; exit}')
    local ms
    ms=$(awk -v s="$sec" 'BEGIN { printf "%d", s * 1000 + 0.5 }')
    if [[ -z "$min_ms" || "$ms" -lt "$min_ms" ]]; then
      min_ms="$ms"
    fi
  done
  printf "  %-32s min(%d) = %d ms\n" "$label" "$RUNS" "$min_ms"
}

size_mb() { ls -l "$1" | awk '{ printf "%.2f", $5 / 1024 / 1024 }'; }

echo "P2-A pivot — Brewfile DSL across runtimes (MIN of $RUNS runs)"
echo "workload : $SCRIPT"
echo
echo "Sizes:"
printf "  %-32s %s MB\n" "rubyrs.wasm (release)"           "$(size_mb "$RUBYRS_WASM")"
printf "  %-32s %s MB\n" "rubyrs.wasm (wasm-opt -Oz)"      "$(size_mb "$TMP/rubyrs.opt.wasm")"
printf "  %-32s %s MB\n" "rubyrs.cwasm (AOT)"              "$(size_mb "$TMP/rubyrs.cwasm")"
printf "  %-32s %s MB\n" "ruby.wasm 3.4 minimal"           "$(size_mb "$RUBYWASM")"
printf "  %-32s %s MB\n" "ruby.cwasm 3.4 minimal (AOT)"    "$(size_mb "$TMP/ruby.cwasm")"
echo
echo "Wall (end-to-end, includes wasmtime spawn + load + run):"
run_min "raw rubyrs.wasm"              "wasmtime run --dir=. $RUBYRS_WASM"
run_min "AOT rubyrs.cwasm"             "wasmtime run --allow-precompiled --dir=. $TMP/rubyrs.cwasm"
run_min "raw ruby.wasm"                "wasmtime run --dir=. $RUBYWASM"
run_min "AOT ruby.cwasm"               "wasmtime run --allow-precompiled --dir=. $TMP/ruby.cwasm"
