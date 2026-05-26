#!/usr/bin/env bash
# Build rubyrs.wasm for the CF Workers PoC.
#
# Pipeline:
#   1. cargo build wasm_worker bin for wasm32-wasip1, --no-default-features
#      (cext requires dlopen which wasi has no equivalent for).
#   2. (Optional) wizer pre-init pass: snapshots classes + preamble
#      bytecode into the wasm so cold-start on Workers doesn't burn
#      the 1s top-level CPU budget re-doing that work. Skipped when
#      `wizer` is not on PATH so first-time PoC contributors don't
#      need to install it before seeing the round-trip work.
#   3. Copy the artifact to poc/cf-worker/wasm/ so wrangler picks
#      it up via the [[rules]] CompiledWasm glob.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$WORKSPACE_ROOT"

if [ -z "${WASI_SDK_PATH:-}" ]; then
    echo "build.sh: WASI_SDK_PATH not set (needed for wasi_stub.c compile in build.rs)." >&2
    echo "  Install wasi-sdk from https://github.com/WebAssembly/wasi-sdk/releases" >&2
    echo "  and export WASI_SDK_PATH=/path/to/wasi-sdk-XX.0" >&2
    exit 1
fi

# Prefer rustup's shim over any Homebrew (or other) rustc that may
# shadow PATH — those distributions usually lack the wasm32-wasip1
# rust-std component, and cargo errors with a misleading "target
# may not be installed" even when `rustup target add wasm32-wasip1`
# succeeded for the rustup toolchain.
if [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! rustup target list --installed | grep -qx wasm32-wasip1; then
    echo "build.sh: wasm32-wasip1 target missing — \`rustup target add wasm32-wasip1\`" >&2
    exit 1
fi

echo "[build.sh] cargo build --release --target wasm32-wasip1 --bin wasm_worker --no-default-features"
cargo build --release --target wasm32-wasip1 \
    --bin wasm_worker -p rubyrs --no-default-features

RAW="$WORKSPACE_ROOT/target/wasm32-wasip1/release/wasm_worker.wasm"
# Final artifact lands NEXT TO src/worker.js so both wrangler and
# workerd can resolve `import "./rubyrs_worker.wasm"`. A historical
# poc/cf-worker/wasm/ location worked for wrangler (default
# CompiledWasm glob walks the project) but not for workerd, which
# rejects `..`-containing module specifiers. Co-locating is the
# minimum-friction shape that satisfies both runtimes.
OUT_DIR="$SCRIPT_DIR/src"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/rubyrs_worker.wasm"

# Optional wasm-opt pass. Pick the level via `WASM_OPT_LEVEL`:
#   skip     → no wasm-opt (default — see note below)
#   -O2      → balanced speed/size
#   -O3      → aggressive speed
#   -Oz      → aggressive size
#
# Why default = skip: rubyrs PoC measurement found that `-Oz` on
# the wasm32-wasip1 binary improves *workerd local* cold-start
# (57→27 ms with wizer) but REGRESSES V8 execution perf on
# Cloudflare Workers' edge (heavy loop 173 ms → 416 ms,
# `puts 1+1` 8 ms → 60 ms). Working hypothesis: `-Oz`'s
# aggressive size shrinks (function-deduplication, inlining
# inhibition, instruction substitution) break V8's wasm
# tier-up heuristics. Until that's debugged the conservative
# default is no opt; the env var lets benchmarks opt in.
#
# Order: wasm-opt FIRST, then wizer. wasm-opt restructures code
# (function indices, instruction layout); wizer snapshots linear
# memory at init time AFTER seeing the final code shape, so
# running it the other way around would have wasm-opt
# invalidate the snapshot's function-index references.
WIZER_IN="$RAW"
WASM_OPT_LEVEL="${WASM_OPT_LEVEL:-skip}"
if [ "$WASM_OPT_LEVEL" = "skip" ]; then
    echo "[build.sh] wasm-opt skipped (WASM_OPT_LEVEL=skip)"
elif command -v wasm-opt >/dev/null 2>&1; then
    OPT="$WORKSPACE_ROOT/target/wasm32-wasip1/release/wasm_worker.opt.wasm"
    echo "[build.sh] wasm-opt $WASM_OPT_LEVEL"
    wasm-opt "$WASM_OPT_LEVEL" --enable-bulk-memory "$RAW" -o "$OPT"
    WIZER_IN="$OPT"
    echo "[build.sh]   $(wc -c < "$RAW") → $(wc -c < "$OPT") bytes"
else
    echo "[build.sh] wasm-opt not on PATH — skipping size pass (\`brew install binaryen\`)"
fi

if command -v wizer >/dev/null 2>&1; then
    # Wizer needs --allow-wasi --wasm-bulk-memory + the binary's
    # `wizer.initialize` export (lib.rs exports this). Skip if the
    # export is absent so we don't fail on bins without it.
    #
    # Stage the objdump output to a tempfile rather than piping
    # straight into `grep -q`. `grep -q` closes its stdin after
    # the first match, which sends SIGPIPE upstream — under
    # `set -o pipefail` (which we want everywhere else in this
    # script) that turns the successful detection into a
    # failure-coded pipe, and we'd silently fall through to the
    # "wizer skipped" branch even when the export is present.
    DUMP="$(mktemp -t rubyrs-wasm-dump.XXXXXX)"
    trap 'rm -f "$DUMP"' EXIT
    wasm-objdump -x "$WIZER_IN" > "$DUMP" 2>/dev/null || true
    if grep -q "wizer.initialize" "$DUMP"; then
        echo "[build.sh] wizer pre-init pass"
        wizer --allow-wasi --wasm-bulk-memory true "$WIZER_IN" -o "$OUT"
    else
        echo "[build.sh] wizer skipped (no wizer.initialize export in this bin)"
        cp "$WIZER_IN" "$OUT"
    fi
else
    echo "[build.sh] wizer not on PATH — skipping pre-init pass (\`cargo install wizer-cli\`)"
    cp "$WIZER_IN" "$OUT"
fi

echo "[build.sh] $OUT ($(wc -c < "$OUT") bytes)"
