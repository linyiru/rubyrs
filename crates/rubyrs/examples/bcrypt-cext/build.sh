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

# Vendored crypt_blowfish: Solar Designer's bcrypt, public domain.
# Compile all .c files together — bcrypt-ruby's extconf does the same
# (it generates a Makefile that includes them all). We deliberately
# do NOT define HAVE_RUBY_THREAD_H or HAVE_RB_EXT_RACTOR_SAFE so
# bcrypt_ext.c's #ifdef-gated paths take the simple synchronous
# branches that don't need APIs we haven't implemented.
VENDOR_SRC=(
    "$SCRIPT_DIR/vendor/crypt_blowfish.c"
    "$SCRIPT_DIR/vendor/crypt_gensalt.c"
    "$SCRIPT_DIR/vendor/wrapper.c"
)
# crypt.c is the system-crypt() compatibility entry; not needed
# when we expose crypt_ra/crypt_gensalt_ra directly.

cc "${LDFLAGS[@]}" \
   -D__SKIP_GNU \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   -I "$SCRIPT_DIR/vendor" \
   "$SCRIPT_DIR/bcrypt_ext.c" \
   "${VENDOR_SRC[@]}" \
   -o "$OUT"

echo "built $OUT"
