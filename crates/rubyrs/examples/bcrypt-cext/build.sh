#!/usr/bin/env bash
# Build the bcrypt-cext example (Level 1 spike).
#
# Layout mirrors hello-cext: build rubyrs (which is what exports the
# rb_* symbols this bundle resolves at dlopen time), then compile
# bcrypt_ext.c with `-undefined dynamic_lookup` (macOS) or
# `--unresolved-symbols=ignore-all` (Linux) so those symbols bind
# against the host process rather than a separate cext .dylib.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROFILE="${PROFILE:-debug}"
CARGO_PROFILE_FLAG=""
if [ "$PROFILE" = "release" ]; then
    CARGO_PROFILE_FLAG="--release"
fi

# 1. Ensure the rubyrs binary is built — that's where the rb_*
#    symbols live (see crates/rubyrs/src/lib.rs: _CEXT_FORCE_EXPORT).
( cd "$WORKSPACE_ROOT" && cargo build -p rubyrs $CARGO_PROFILE_FLAG )

case "$(uname -s)" in
    Darwin)
        EXT=bundle
        LDFLAGS=(
            -shared
            -undefined dynamic_lookup
        )
        ;;
    Linux)
        EXT=so
        LDFLAGS=(
            -shared
            -fPIC
            -Wl,--unresolved-symbols=ignore-all
        )
        ;;
    *)
        echo "build.sh: unsupported host $(uname -s)" >&2
        exit 1
        ;;
esac

OUT="$SCRIPT_DIR/bcrypt_ext.$EXT"
cc "${LDFLAGS[@]}" \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   "$SCRIPT_DIR/bcrypt_ext.c" \
   -o "$OUT"

echo "built $OUT"
