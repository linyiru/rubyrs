#!/usr/bin/env bash
# Run the zeitwerk self-test suite under rubyrs and tally
# runs/failures/errors. The measuring stick for the zeitwerk spike.
#
# Requirements:
#   - rubyrs built WITH the stdlib feature (zeitwerk needs real Set,
#     ERB, etc.):  cargo build --release -p rubyrs --bin rubyrs --features stdlib
#   - the zeitwerk gem installed (its lib/ is auto-located via `gem which`)
#   - minitest 5.25.4 installed (rubyrs runs it zero-shim; newer
#     minitest hasn't been validated)
#   - the zeitwerk SOURCE repo (the gem ships no tests). Cloned to
#     $ZW_SRC on first run if absent.
#
# Env overrides: RUBYRS, ZWLIB, MT, ZW_SRC.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

RUBYRS="${RUBYRS:-$HERE/../../target/release/rubyrs}"
ZWLIB="${ZWLIB:-$(dirname "$(gem which zeitwerk 2>/dev/null)")}"
MT="${MT:-$(ls -d "$HOME"/.rbenv/versions/*/lib/ruby/gems/*/gems/minitest-5.25.4/lib 2>/dev/null | head -1)}"
ZW_SRC="${ZW_SRC:-/tmp/zeitwerk-src}"

[ -x "$RUBYRS" ] || { echo "rubyrs binary not found at $RUBYRS (build with --features stdlib)"; exit 1; }
[ -d "$ZWLIB/zeitwerk" ] || { echo "zeitwerk gem lib not found ($ZWLIB)"; exit 1; }
[ -d "$MT" ] || { echo "minitest 5.25.4 lib not found ($MT)"; exit 1; }
if [ ! -d "$ZW_SRC/test" ]; then
  echo "cloning zeitwerk source (for its tests) into $ZW_SRC ..."
  git clone --depth 1 https://github.com/fxn/zeitwerk "$ZW_SRC" || exit 1
fi
TESTDIR="$ZW_SRC/test"

export ZW_LOADPATH="$MT:$HERE/harness:$ZWLIB:$TESTDIR:$TESTDIR/lib:$TESTDIR/lib/zeitwerk"

# macOS lacks `timeout`; use a perl alarm wrapper.
run_to() { perl -e 'alarm shift; exec @ARGV' "$@"; }

TOTAL_R=0; TOTAL_A=0; TOTAL_F=0; TOTAL_E=0; FILES=0; RAN=0; CLEAN=0
for f in $(find "$TESTDIR/lib" -name "test_*.rb" | sort); do
  FILES=$((FILES+1))
  out=$(run_to 30 "$RUBYRS" "$HERE/run.rb" "$f" 2>&1)
  line=$(echo "$out" | grep -oE "[0-9]+ runs, [0-9]+ assertions, [0-9]+ failures, [0-9]+ errors, [0-9]+ skips" | tail -1)
  if [ -z "$line" ]; then
    echo "LOAD-FAIL  $(basename "$f")"
    continue
  fi
  RAN=$((RAN+1))
  r=$(echo "$line"  | grep -oE '^[0-9]+')
  a=$(echo "$line"  | sed -E 's/.* ([0-9]+) assertions.*/\1/')
  fa=$(echo "$line" | sed -E 's/.* ([0-9]+) failures.*/\1/')
  e=$(echo "$line"  | sed -E 's/.* ([0-9]+) errors.*/\1/')
  [ "$fa" = "0" ] && [ "$e" = "0" ] && CLEAN=$((CLEAN+1)) && echo "GREEN      $(basename "$f")  ($line)"
  TOTAL_R=$((TOTAL_R+r)); TOTAL_A=$((TOTAL_A+a)); TOTAL_F=$((TOTAL_F+fa)); TOTAL_E=$((TOTAL_E+e))
done

echo "==================================================="
echo "zeitwerk self-test on rubyrs (--features stdlib)"
echo "  test files:   $FILES  (ran $RAN, fully-green $CLEAN)"
echo "  aggregate:    $TOTAL_R runs, $TOTAL_A assertions, $TOTAL_F failures, $TOTAL_E errors"
echo "  passing:      $((TOTAL_R - TOTAL_F - TOTAL_E)) / $TOTAL_R"
echo "==================================================="
