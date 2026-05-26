#!/usr/bin/env bash
# perf/build_embedder.sh — produce the single-binary `rubyrs-wasm-embed`
# shipping artifact.
#
# Pipeline (wizer BEFORE wasm-opt — see the "Pipeline reorder"
# rationale block below for the Linux binaryen DCE-strips-export
# bug that motivated this ordering):
#   1. cargo build --release --target wasm32-wasip1 --no-default-features
#   2. wizer (snapshot preamble + classes)    ←  ~0.5 ms cold-start cut
#   3. wasm-opt -Oz                           ←  ~21% size cut + compacts
#                                                 post-wizer memory layout
#   4. cargo build --profile release-min -p rubyrs-wasm-embed \
#         RUBYRS_WIZER_WASM=<step-3-output>   ←  build.rs precompiles to cwasm
#                                                 and bakes via include_bytes!
#
# Output: target/release-min/rubyrs-wasm-embed (~7 MB single binary
# on macOS arm64; varies by platform). After this, the embedder
# runs WITHOUT any external cwasm file:
#   ./target/release-min/rubyrs-wasm-embed your_script.rb
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
echo "[1/4] cargo build --release --target wasm32-wasip1 --no-default-features -p rubyrs"
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
WIZ="$PIPELINE_DIR/rubyrs.wizer.wasm"
WIZ_OPT="$PIPELINE_DIR/rubyrs.wizer.opt.wasm"

# Pipeline reorder vs perf/wasm_check.sh: do `wizer` BEFORE
# `wasm-opt -Oz` rather than after. The pre-wizer wasm-opt was
# pure size reduction — wizer reads the .wasm regardless of
# whether it's been opt'd — and on Linux's apt-installed
# binaryen (v116) the `-Oz` DCE pass strips the
# `wizer.initialize` export despite it being a roots-list entry,
# breaking the next step with `the Wasm module does not have a
# wizer-initialize export`. Doing wizer first avoids the issue
# entirely: by the time wasm-opt sees the .wasm, the
# wizer.initialize export is genuinely dead (wizer has consumed
# it) and the DCE is correct. macOS brew binaryen (v123+)
# doesn't have this bug; this reorder makes the pipeline
# portable across both.
echo "[2/4] wizer $RAW_WASM -> $WIZ"
# `--init-func wizer.initialize` is load-bearing for wizer v11+:
# v11 changed the default expected export name from
# `wizer.initialize` (dot) to `wizer-initialize` (hyphen) — see
# https://github.com/bytecodealliance/wizer/releases for v11.0.x
# notes. Our lib.rs still uses `#[unsafe(export_name =
# "wizer.initialize")]` (Rust idiom; both forms are valid wasm
# export names), so we have to TELL wizer which form to look for.
# v10 also accepts this flag, so the override is portable across
# wizer versions.
wizer --allow-wasi --init-func wizer.initialize -o "$WIZ" "$RAW_WASM"

echo "[3/4] wasm-opt -Oz $WIZ -> $WIZ_OPT"
wasm-opt -Oz "$WIZ" -o "$WIZ_OPT"

WIZ_OPT_ABS="$(cd "$(dirname "$WIZ_OPT")" && pwd)/$(basename "$WIZ_OPT")"
echo "[4/4] cargo build --profile release-min -p rubyrs-wasm-embed  (RUBYRS_WIZER_WASM=$WIZ_OPT_ABS)"
# `release-min` is the workspace profile defined in the root
# Cargo.toml — inherits from `release` (thin LTO, opt-level=3)
# and overrides only the size knobs that DON'T cost runtime perf:
# panic=abort, strip=symbols, debug=false. An earlier iteration
# also set opt-level=z + fat LTO + codegen-units=1, but that
# made cold start 3-19% SLOWER despite halving the binary; see
# the rationale in `[profile.release-min]` in Cargo.toml.
RUBYRS_WIZER_WASM="$WIZ_OPT_ABS" cargo build --profile release-min -p rubyrs-wasm-embed >&2

EMBED_BIN="target/release-min/rubyrs-wasm-embed"
if [[ ! -x "$EMBED_BIN" ]]; then
  echo "build_embedder: $EMBED_BIN not produced — see cargo output above" >&2
  exit 2
fi

# `stat -f %z` is BSD/macOS format, `stat -c %s` is GNU/Linux.
# Initial form (`stat -f ... || stat -c ...`) had it backwards:
# GNU `stat -f` is "filesystem mode" (block size etc.) and
# silently succeeds with garbage on Linux. Use `wc -c < file`
# which is POSIX and identical on both platforms.
WIZ_OPT_SIZE="$(wc -c < "$WIZ_OPT" | tr -d ' ')"
EMBED_SIZE="$(wc -c < "$EMBED_BIN" | tr -d ' ')"

echo ""
echo "=== built ==="
printf "  wizer'd .wasm input:   %10d bytes  %s\n" "$WIZ_OPT_SIZE" "$WIZ_OPT"
printf "  single-binary embedder: %10d bytes  %s\n" "$EMBED_SIZE" "$EMBED_BIN"
echo ""
echo "  Run:   ./$EMBED_BIN your_script.rb"
echo "  Override at runtime:  RUBYRS_CWASM=other.cwasm ./$EMBED_BIN your_script.rb"
