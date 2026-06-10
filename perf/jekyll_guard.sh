#!/usr/bin/env bash
# Feature sanity guard for Jekyll/perf measurements.
#
# THE TRAP: any `cargo build` / `cargo test --release` WITHOUT the full
# feature set silently overwrites target/release/rubyrs with a
# default-feature binary. A Jekyll measurement against that binary
# produces garbage — typically `Set#include?` NoMethodError inside
# liquid's require chain, which aborts the build in ~0.06s and reads
# like a miraculous speedup. This bit three measurements on 2026-06-10
# alone. Run this guard (or source it) before timing ANYTHING.
#
# Usage:
#   perf/jekyll_guard.sh                  # checks target/release/rubyrs
#   RUBYRS_BIN=path perf/jekyll_guard.sh  # checks a specific binary
set -euo pipefail

BIN="${RUBYRS_BIN:-target/release/rubyrs}"
if [ ! -x "$BIN" ]; then
  echo "jekyll_guard: no binary at $BIN" >&2
  exit 1
fi

probe="$(mktemp /tmp/rubyrs-feature-probe.XXXXXX.rb)"
trap 'rm -f "$probe"' EXIT
cat > "$probe" <<'RUBY'
# Feature fingerprints: each accelerator registers host fns the
# default build lacks; stdlib's Set is the canonical clobber victim.
missing = []
begin
  require "set"
  missing << "stdlib(Set)" unless Set.new([1]).include?(1)
rescue Exception
  missing << "stdlib"
end
missing << "sass" unless defined?(RubyrsSass)
missing << "_rouge_native" unless defined?(__rubyrs_rouge_native_table)
missing << "_kramdown_native" unless defined?(__rubyrs_kd_scan)
missing << "_yaml_native" unless defined?(__rubyrs_yaml_parse)
missing << "_liquid_native" unless defined?(__rubyrs_liquid_compile)
if missing.empty?
  puts "FEATURES-OK"
else
  puts "MISSING: #{missing.join(",")}"
  exit 1
end
RUBY

if out="$("$BIN" "$probe" 2>&1)"; then
  # mimalloc has no Ruby-visible surface, so probe the binary's
  # symbol table instead (the cargo feature links mi_* statically).
  # Measurement builds carry it since 2026-06-10 (wall −8.5% on the
  # Jekyll benches, RSS +1.3% post-lazy-regex); a binary without it
  # is a clobber suspect just like a missing accelerator.
  # Pattern is the broad `_mi_` prefix: the specific __mi_arenas
  # symbols' external visibility turned out to vary across builds
  # (one rebuild dropped them from -gU while 80+ other mi_ symbols
  # stayed), which made the narrow probe a false alarm.
  if ! nm -gU "$BIN" 2>/dev/null | grep -q "_mi_"; then
    echo "jekyll_guard: FEATURE SANITY FAILED — MISSING: mimalloc (no mi_malloc symbol)" >&2
    echo "jekyll_guard: rebuild with:" >&2
    echo "  cargo build --release -p rubyrs --features stdlib,sass,_rouge_native,_kramdown_native,_yaml_native,_liquid_native,mimalloc" >&2
    exit 1
  fi
  echo "jekyll_guard: $out ($BIN)"
else
  echo "jekyll_guard: FEATURE SANITY FAILED — $out" >&2
  echo "jekyll_guard: the binary was probably clobbered by a default-feature build/test." >&2
  echo "jekyll_guard: rebuild with:" >&2
  echo "  cargo build --release -p rubyrs --features stdlib,sass,_rouge_native,_kramdown_native,_yaml_native,_liquid_native,mimalloc" >&2
  exit 1
fi
