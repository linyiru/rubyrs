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
OUT_DIR="$SCRIPT_DIR/wasm"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/rubyrs_worker.wasm"

if command -v wizer >/dev/null 2>&1; then
    # Wizer needs --allow-wasi --wasm-bulk-memory + the binary's
    # `wizer.initialize` export (lib.rs exports this). Skip if the
    # export is absent so we don't fail on bins without it.
    if wasm-objdump -x "$RAW" 2>/dev/null | grep -q "wizer.initialize"; then
        echo "[build.sh] wizer pre-init pass"
        wizer --allow-wasi --wasm-bulk-memory "$RAW" -o "$OUT"
    else
        echo "[build.sh] wizer skipped (no wizer.initialize export in this bin)"
        cp "$RAW" "$OUT"
    fi
else
    echo "[build.sh] wizer not on PATH — skipping pre-init pass (\`cargo install wizer-cli\`)"
    cp "$RAW" "$OUT"
fi

echo "[build.sh] $OUT ($(wc -c < "$OUT") bytes)"
