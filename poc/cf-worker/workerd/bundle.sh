#!/usr/bin/env bash
# Bundle src/worker.js + its @cloudflare/workers-wasi dependency
# into a single ES module that workerd's capnp module list can
# `embed` directly. Wrangler does this implicitly; workerd
# standalone needs every dep declared, and bundling sidesteps
# the need to enumerate workers-wasi's internal modules.
#
# `--external:*.wasm` keeps the `import wasmModule from
# "../wasm/rubyrs_worker.wasm"` line as a bare specifier so
# workerd's capnp `wasm =` entry resolves it instead of esbuild
# trying to inline a 1.4MB Uint8Array literal into the .mjs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$POC_DIR"

# workers-wasi pulls in its OWN memfs.wasm via a bare `import wasm
# from "./memfs.wasm"`. We mark wasm imports external so esbuild
# emits the same shape; the capnp config lists both .wasm modules.
echo "[bundle.sh] esbuild src/worker.js → workerd/dist/worker.mjs"
node_modules/.bin/esbuild src/worker.js \
    --bundle \
    --format=esm \
    --target=esnext \
    --external:*.wasm \
    --outfile=workerd/dist/worker.mjs \
    --log-level=warning

echo "[bundle.sh] $(wc -c < workerd/dist/worker.mjs) bytes"
