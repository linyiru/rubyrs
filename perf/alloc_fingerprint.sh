#!/bin/sh
# Allocator fingerprint for perf measurements.
#
# THE TRAP (same shape jekyll_guard.sh defends, fixed once before in
# 8dc84b18): a benchmark timed against a binary built WITHOUT
# `mimalloc` measures the system allocator, which no shipped CLI
# runs (`cli-defaults` has carried mimalloc since 2026-06-07, ADR
# 0019 v3) and which understates the CLI by 2-19% depending on
# allocation intensity (2026-07-04 re-measure, mimalloc v3.3.2 —
# see docs/BENCHMARKS.md "Standard measurement feature set").
# mimalloc has no Ruby-visible surface, so the only fingerprint is
# the binary itself. Run this before timing ANYTHING.
#
# Usage:
#   perf/alloc_fingerprint.sh [BIN]              # default target/release/rubyrs
#   RUBYRS_BIN=path perf/alloc_fingerprint.sh    # env form, like jekyll_guard.sh
#
# Exit codes: 0 = mimalloc present; 1 = mimalloc ABSENT (do not
# measure); 2 = setup error (no binary / no probe tool available).
#
# Probe strategy, in order:
#   1. `nm` for the broad `mi_` symbol prefix. Covers macOS nm
#      (`_mi_malloc`, underscore-mangled) and Linux/GNU nm
#      (`mi_malloc`) with one grep, and survives the
#      symbol-visibility drift that made a narrow single-symbol
#      probe a false alarm in jekyll_guard.sh's history. Retried
#      once: macOS nm fails transiently (observed 2026-06-10).
#   2. `strings` for the `mimalloc: ` message prefix baked into
#      libmimalloc's warning/error strings — works on stripped
#      binaries where nm sees no symbol table.
set -eu

BIN="${1:-${RUBYRS_BIN:-target/release/rubyrs}}"
if [ ! -x "$BIN" ]; then
  echo "alloc_fingerprint: no executable at $BIN" >&2
  exit 2
fi

# nm output is "addr type name"; match the name field on both
# manglings without -g/-U flags (GNU and BSD nm disagree on those).
nm_probe() { nm "$BIN" 2>/dev/null | grep -q '[ _]mi_malloc'; }
strings_probe() { strings "$BIN" 2>/dev/null | grep -q '^mimalloc: '; }

have_nm=0;      command -v nm >/dev/null 2>&1 && have_nm=1
have_strings=0; command -v strings >/dev/null 2>&1 && have_strings=1
if [ "$have_nm" = 0 ] && [ "$have_strings" = 0 ]; then
  echo "alloc_fingerprint: neither nm nor strings available — cannot verify $BIN" >&2
  exit 2
fi

verdict=absent
if [ "$have_nm" = 1 ] && { nm_probe || { sleep 1; nm_probe; }; }; then
  verdict="present (nm: mi_ symbols)"
elif [ "$have_strings" = 1 ] && strings_probe; then
  verdict="present (strings: mimalloc message prefix; symbols stripped or nm errored)"
fi

case "$verdict" in
  present*)
    echo "alloc_fingerprint: OK — mimalloc linked [$verdict] ($BIN)"
    ;;
  *)
    echo "alloc_fingerprint: FAILED — mimalloc NOT linked in $BIN" >&2
    echo "alloc_fingerprint: this binary runs the system allocator; perf numbers" >&2
    echo "alloc_fingerprint: from it understate the shipped CLI by 2-19%." >&2
    echo "alloc_fingerprint: rebuild with the standard measurement set:" >&2
    echo "  cargo build --release -p rubyrs --features stdlib,jit-native,_fiber,_json_native,mimalloc" >&2
    exit 1
    ;;
esac
