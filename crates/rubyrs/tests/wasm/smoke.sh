#!/usr/bin/env bash
# Wasm smoke runner — build rubyrs.wasm for wasm32-wasip1 with
# `--no-default-features` (the only meaningful wasi shape; see
# ADR 0015) and execute `tests/wasm/smoke.rb` under wasmtime,
# diffing stdout against `tests/wasm/smoke.expected`.
#
# Used by both the CI lane (`.github/workflows/ci.yml::wasm`)
# and local dev. Idempotent: re-running with no source changes
# is a fast no-op via cargo's incremental rebuild + the
# pre-existing wasm artifact.
#
# Requirements:
#   - rustup target wasm32-wasip1 installed
#   - WASI_SDK_PATH pointing at wasi-sdk 24 (or compatible)
#   - wasmtime on PATH
#
# Why a shell script (not a Rust integration test): a Rust test
# that cross-compiles to wasm + invokes wasmtime would tangle
# host-side `cargo test` with wasi tooling that not every dev
# environment has. Keeping it as a script makes the CI step a
# one-liner and lets local devs opt in without the wasi-sdk
# install becoming a `cargo test` prerequisite.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WORKSPACE_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

cd "$WORKSPACE_ROOT"

# Sanity-check toolchain presence so failures land on a useful
# message rather than a confusing rustc / cargo error mid-run.
# Each tool check has to come BEFORE the tool's invocation: with
# `set -e`, a bare `rustup target list ...` on a host without
# rustup installed would error out as "command not found" — not
# the actionable message we want.
for cmd in wasmtime rustup cargo; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "smoke.sh: $cmd not on PATH — see docs/DEVELOPMENT.md for setup" >&2
        exit 1
    fi
done
if ! rustup target list --installed | grep -qx wasm32-wasip1; then
    echo "smoke.sh: wasm32-wasip1 target not installed — \`rustup target add wasm32-wasip1\`" >&2
    exit 1
fi
if [ -z "${WASI_SDK_PATH:-}" ]; then
    echo "smoke.sh: WASI_SDK_PATH not set — see docs/DEVELOPMENT.md for wasi-sdk install" >&2
    exit 1
fi

WASM="target/wasm32-wasip1/release/rubyrs.wasm"
# wasmtime resolves paths the guest sees against its `--dir` mounts;
# passing an absolute host path makes wasi refuse the open. Keep
# both as workspace-relative so `--dir=.` is sufficient.
SMOKE_RB_REL="crates/rubyrs/tests/wasm/smoke.rb"
EXPECTED="$CRATE_DIR/tests/wasm/smoke.expected"

echo "[smoke.sh] cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features"
cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features

echo "[smoke.sh] $WASM built ($(wc -c < "$WASM") bytes)"

# Capture under the same working dir wasmtime needs --dir for.
# Using --dir=. is sufficient since the fixture is read by path
# relative to the workspace root. Write stdout to a tempfile +
# `cmp` so the comparison is byte-for-byte (command substitution
# would strip trailing newlines and could mask a real divergence).
ACTUAL_FILE="$(mktemp -t rubyrs-wasm-smoke.XXXXXX)"
trap 'rm -f "$ACTUAL_FILE"' EXIT
echo "[smoke.sh] wasmtime run $SMOKE_RB_REL"
wasmtime run --dir=. "$WASM" "$SMOKE_RB_REL" > "$ACTUAL_FILE"

if ! cmp -s "$ACTUAL_FILE" "$EXPECTED"; then
    echo "[smoke.sh] FAIL — stdout diverged from expected:" >&2
    diff -u "$EXPECTED" "$ACTUAL_FILE" >&2 || true
    exit 1
fi

echo "[smoke.sh] PASS"
