#!/usr/bin/env bash
# perf/build_embedder.sh — produce the single-binary `rubyrs-wasm-embed`
# shipping artifact.
#
# Pipeline:
#   1. cargo build --release --target wasm32-wasip1 --no-default-features
#   2. wasm-opt -Oz                           ←  ~21% size cut (1.5 MB → 1.2 MB)
#   3. wizer (snapshot preamble + classes)    ←  ~0.5 ms cold-start cut
#   4. wasm-opt -Oz again                     ←  compacts post-wizer memory
#   5. cargo build --release -p rubyrs-wasm-embed \
#         RUBYRS_WIZER_WASM=<step-4-output>   ←  build.rs precompiles to cwasm
#                                                 and bakes via include_bytes!
#
# Output: target/release/rubyrs-wasm-embed (~14 MB single binary).
# After this, the embedder runs WITHOUT any external cwasm file:
#   ./target/release/rubyrs-wasm-embed your_script.rb
#
# Exit codes mirror perf/wasm_check.sh: 0 = success, 2 = setup
# error, 128+sig = interrupted.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Setup checks. wasm-opt and wizer are REQUIRED here (unlike
# perf/wasm_check.sh where they're optional) because the whole
# point of this script is the full pipeline that produces a
# wizer'd, opt'd, AOT'd, baked single binary. A wasm-opt skip
# would defeat the size-reduction motivation; a wizer skip would
# defeat the cold-start motivation.
for tool in cargo wasm-opt wizer; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "build_embedder: required tool not found: $tool" >&2
    case "$tool" in
      wasm-opt) echo "  install: brew install binaryen  (macOS) / apt install binaryen  (Linux)" >&2 ;;
      wizer)    echo "  install: see https://github.com/bytecodealliance/wizer/releases" >&2 ;;
    esac
    exit 2
  fi
done

# `--no-default-features` because the `cext` default depends on
# libloading + dlopen — wasi has neither. Matches
# perf/wasm_check.sh's invocation so the wasm shape gated by CI
# is the same shape baked into the embedder.
echo "[1/5] cargo build --release --target wasm32-wasip1 --no-default-features -p rubyrs"
cargo build --release --target wasm32-wasip1 --no-default-features -p rubyrs >&2

RAW_WASM="target/wasm32-wasip1/release/rubyrs.wasm"
if [[ ! -f "$RAW_WASM" ]]; then
  echo "build_embedder: build artifact missing: $RAW_WASM" >&2
  exit 2
fi

# Use a stable output path under target/ so cargo's `rerun-if-
# changed` mechanism in build.rs picks up changes correctly. A
# fresh per-run mktemp would force a rebuild every time.
PIPELINE_DIR="target/wasm-pipeline"
mkdir -p "$PIPELINE_DIR"
OPT="$PIPELINE_DIR/rubyrs.opt.wasm"
WIZ="$PIPELINE_DIR/rubyrs.wizer.wasm"
WIZ_OPT="$PIPELINE_DIR/rubyrs.wizer.opt.wasm"

echo "[2/5] wasm-opt -Oz $RAW_WASM -> $OPT"
wasm-opt -Oz "$RAW_WASM" -o "$OPT"

echo "[3/5] wizer --allow-wasi $OPT -> $WIZ"
# Mirror `perf/wasm_check.sh`'s wizer invocation exactly —
# `--allow-wasi` is the only flag needed for the rubyrs shape.
# (Don't pass `--wasm-bulk-memory true`; wizer v11.0.3 parses
# that as a positional arg on some platforms, which then becomes
# a "module does not have wizer-initialize" error because the
# *wrong* file got loaded as the input.)
wizer --allow-wasi -o "$WIZ" "$OPT"

echo "[4/5] wasm-opt -Oz $WIZ -> $WIZ_OPT"
wasm-opt -Oz "$WIZ" -o "$WIZ_OPT"

WIZ_OPT_ABS="$(cd "$(dirname "$WIZ_OPT")" && pwd)/$(basename "$WIZ_OPT")"
echo "[5/5] cargo build --release -p rubyrs-wasm-embed  (RUBYRS_WIZER_WASM=$WIZ_OPT_ABS)"
RUBYRS_WIZER_WASM="$WIZ_OPT_ABS" cargo build --release -p rubyrs-wasm-embed >&2

EMBED_BIN="target/release/rubyrs-wasm-embed"
if [[ ! -x "$EMBED_BIN" ]]; then
  echo "build_embedder: $EMBED_BIN not produced — see cargo output above" >&2
  exit 2
fi

WIZ_OPT_SIZE="$(stat -f %z "$WIZ_OPT" 2>/dev/null || stat -c %s "$WIZ_OPT")"
EMBED_SIZE="$(stat -f %z "$EMBED_BIN" 2>/dev/null || stat -c %s "$EMBED_BIN")"

echo ""
echo "=== built ==="
printf "  wizer'd .wasm input:   %10d bytes  %s\n" "$WIZ_OPT_SIZE" "$WIZ_OPT"
printf "  single-binary embedder: %10d bytes  %s\n" "$EMBED_SIZE" "$EMBED_BIN"
echo ""
echo "  Run:   ./$EMBED_BIN your_script.rb"
echo "  Override at runtime:  RUBYRS_CWASM=other.cwasm ./$EMBED_BIN your_script.rb"
