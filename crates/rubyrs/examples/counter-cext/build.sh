#!/usr/bin/env bash
# Build the counter-cext example bundle (L3-B TypedData acceptance).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROFILE="${PROFILE:-debug}"
CARGO_PROFILE_FLAG=""
if [ "$PROFILE" = "release" ]; then
    CARGO_PROFILE_FLAG="--release"
fi

( cd "$WORKSPACE_ROOT" && cargo build -p rubyrs $CARGO_PROFILE_FLAG )

case "$(uname -s)" in
    Darwin)
        EXT=bundle
        LDFLAGS=(-shared -undefined dynamic_lookup)
        ;;
    Linux)
        EXT=so
        LDFLAGS=(-shared -fPIC -Wl,--unresolved-symbols=ignore-all)
        ;;
    *)
        echo "build.sh: unsupported host $(uname -s)" >&2
        exit 1
        ;;
esac

OUT="$SCRIPT_DIR/counter_ext.$EXT"
# Cross-process safety against parallel cargo test binaries —
# `cext_typeddata.rs` and `cext_instance_method.rs` both call
# this script, each from its own test process. Same defense as
# msgpack-cext/build.sh: optional flock-based MUTUAL EXCLUSION
# (the cc invocations still run sequentially when both callers
# arrive — this is serialization, not deduplication; the work
# is duplicated but the output stays correct) + always atomic
# tmpfile + mv so a parallel dlopen sees the OLD bundle or the
# COMPLETE new one, never a partial. Reviewer Copilot caught
# the original race on PR #82.
if command -v flock >/dev/null 2>&1; then
    exec 9>"$OUT.lock"
    flock 9
fi

TMP="$OUT.tmp.$$"
trap 'rm -f "$TMP"' EXIT
cc "${LDFLAGS[@]}" \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   "$SCRIPT_DIR/counter_ext.c" \
   -o "$TMP"
mv -f "$TMP" "$OUT"

echo "built $OUT"
