#!/usr/bin/env bash
# Wasm diff matrix — third leg of the wasm correctness ladder.
#
#   1. `cargo build --target wasm32-wasip1` — the CI lane has
#      always verified the wasi-portable code compiles cleanly.
#   2. `smoke.sh` (PR #106) — added "actually runs end-to-end
#      under wasmtime" via one curated fixture.
#   3. THIS SCRIPT — runs a curated subset of `tests/diff/*.rb`
#      fixtures under BOTH `ruby` (CRuby oracle) and the built
#      `rubyrs.wasm` (under wasmtime), and asserts byte-identical
#      stdout. Catches behavioural divergence between native and
#      wasi-portable execution paths that compile-only checks
#      miss.
#
# The fixture list lives in `tests/wasm/diff_manifest.txt`. Why
# a separate manifest instead of running every `tests/diff/*.rb`
# under wasm: a few fixtures depend on wasi-hostile resources
# (ENV semantics, file I/O paths, subshells); the manifest
# curates a high-signal spread without those gotchas. Adding a
# new fixture takes one line in the manifest — see the file's
# header for selection rules.
#
# Requirements:
#   - rustup target wasm32-wasip1 installed
#   - WASI_SDK_PATH pointing at wasi-sdk 24 (or compatible)
#   - wasmtime on PATH
#   - ruby (CRuby) on PATH — same dependency the diff_cruby host
#     lane already has; wasm CI runs on ubuntu-latest which ships
#     with ruby preinstalled.
#
# Why CRuby-at-runtime instead of checked-in `.expected_wasm`
# files: the `tests/diff/` corpus is ~180 fixtures and the
# diff_cruby suite already runs CRuby as oracle in CI; pinning
# generated snapshots would just duplicate that work and create a
# coupling where every fixture edit triggers two file updates
# (the fixture AND its snapshot). Running CRuby live keeps the
# coupling at one file.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WORKSPACE_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

cd "$WORKSPACE_ROOT"

for cmd in wasmtime rustup cargo ruby; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "diff_matrix.sh: $cmd not on PATH — see docs/DEVELOPMENT.md for setup" >&2
        exit 1
    fi
done
if ! rustup target list --installed | grep -qx wasm32-wasip1; then
    echo "diff_matrix.sh: wasm32-wasip1 target not installed — \`rustup target add wasm32-wasip1\`" >&2
    exit 1
fi
if [ -z "${WASI_SDK_PATH:-}" ]; then
    echo "diff_matrix.sh: WASI_SDK_PATH not set — see docs/DEVELOPMENT.md for wasi-sdk install" >&2
    exit 1
fi

WASM="target/wasm32-wasip1/release/rubyrs.wasm"
MANIFEST="crates/rubyrs/tests/wasm/diff_manifest.txt"

if [ ! -f "$MANIFEST" ]; then
    echo "diff_matrix.sh: manifest not found at $MANIFEST" >&2
    exit 1
fi

# Build the wasm once up front. Cargo's incremental rebuild makes
# a no-op fast; building inline (rather than relying on the caller
# to have done it) keeps the script self-contained for local use.
echo "[diff_matrix.sh] cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features"
cargo build --release --target wasm32-wasip1 -p rubyrs --no-default-features

# Parse the manifest: skip blank lines and lines starting with `#`.
# Dedupe so accidental duplicates don't double-charge runtime.
FIXTURES=()
while IFS= read -r line; do
    line="${line%%#*}"            # strip trailing comments
    line="${line#"${line%%[![:space:]]*}"}"   # ltrim
    line="${line%"${line##*[![:space:]]}"}"   # rtrim
    [ -z "$line" ] && continue
    # Skip if already in the list
    skip=0
    for f in "${FIXTURES[@]:-}"; do
        if [ "$f" = "$line" ]; then skip=1; break; fi
    done
    [ "$skip" = "0" ] && FIXTURES+=("$line")
done < "$MANIFEST"

# Fail fast on an empty manifest — otherwise this script would
# exit 0 with "PASSED: 0 / 0", which CI would silently treat as
# a green wasm-correctness signal.
if [ "${#FIXTURES[@]}" -eq 0 ]; then
    echo "diff_matrix.sh: manifest has zero fixtures (after stripping comments / blanks) — refusing to proceed" >&2
    exit 1
fi

echo "[diff_matrix.sh] running ${#FIXTURES[@]} fixtures under wasmtime + CRuby"

PASSED=()
FAILED=()
ACTUAL_FILE="$(mktemp -t rubyrs-wasm-diff.XXXXXX)"
EXPECTED_FILE="$(mktemp -t rubyrs-wasm-diff-exp.XXXXXX)"
RUBY_STDERR="$(mktemp -t rubyrs-wasm-diff-ruby-err.XXXXXX)"
WASM_STDERR="$(mktemp -t rubyrs-wasm-diff-wasm-err.XXXXXX)"
trap 'rm -f "$ACTUAL_FILE" "$EXPECTED_FILE" "$RUBY_STDERR" "$WASM_STDERR"' EXIT

# Capture stderr to a tempfile per fixture and surface it on
# failure — CI logs need enough information to localise the
# regression without re-running locally.
for name in "${FIXTURES[@]}"; do
    rb="crates/rubyrs/tests/diff/${name}.rb"
    if [ ! -f "$rb" ]; then
        FAILED+=("$name (fixture not found at $rb)")
        continue
    fi
    # CRuby reference output (host oracle).
    if ! ruby --disable=gems "$rb" > "$EXPECTED_FILE" 2> "$RUBY_STDERR"; then
        FAILED+=("$name (CRuby itself failed — fixture bug?)")
        echo "    ruby stderr:" >&2
        sed 's/^/      /' "$RUBY_STDERR" >&2
        continue
    fi
    # wasm32-wasip1 output. Captured with `>` (not command-sub)
    # so trailing newlines are preserved byte-for-byte.
    if ! wasmtime run --dir=. "$WASM" "$rb" > "$ACTUAL_FILE" 2> "$WASM_STDERR"; then
        FAILED+=("$name (wasmtime exited non-zero)")
        echo "    wasmtime stderr:" >&2
        sed 's/^/      /' "$WASM_STDERR" >&2
        continue
    fi
    if cmp -s "$ACTUAL_FILE" "$EXPECTED_FILE"; then
        PASSED+=("$name")
    else
        FAILED+=("$name (stdout diverged)")
        # Show a unified diff so CI logs make the regression
        # immediately diagnosable. Limit to first 40 lines so a
        # large divergence doesn't spam the log.
        echo "    diff -u (ruby vs wasm) head -40:" >&2
        diff -u "$EXPECTED_FILE" "$ACTUAL_FILE" 2>/dev/null | head -40 | sed 's/^/      /' >&2 || true
    fi
done

echo
echo "[diff_matrix.sh] PASSED: ${#PASSED[@]} / ${#FIXTURES[@]}"
if [ "${#FAILED[@]}" -gt 0 ]; then
    echo "[diff_matrix.sh] FAILED: ${#FAILED[@]}"
    for f in "${FAILED[@]}"; do
        echo "  - $f"
    done
    exit 1
fi

echo "[diff_matrix.sh] all green"
