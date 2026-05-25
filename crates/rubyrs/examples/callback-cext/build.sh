#!/usr/bin/env bash
# Build the callback-cext example bundle. Same shape as hello-cext.
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

OUT="$SCRIPT_DIR/callback_ext.$EXT"
cc "${LDFLAGS[@]}" \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   "$SCRIPT_DIR/callback_ext.c" \
   -o "$OUT"

echo "built $OUT"
