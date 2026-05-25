#!/usr/bin/env bash
# Build the hello-cext example.
#
# Steps:
#   1. Build rubyrs-cext as a shared library so the C ext can link
#      against `-lrubyrs_cext`.
#   2. Compile hello.c into hello.{so,dylib,bundle}, linking against
#      that shared library and baking the workspace `target/` path
#      into its rpath so dlopen-at-runtime finds it without
#      LD_LIBRARY_PATH gymnastics.
#
# Output: crates/rubyrs/examples/hello-cext/hello.<ext>
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROFILE="${PROFILE:-debug}"
CARGO_PROFILE_FLAG=""
if [ "$PROFILE" = "release" ]; then
    CARGO_PROFILE_FLAG="--release"
fi
TARGET_DIR="$WORKSPACE_ROOT/target/$PROFILE"

# 1. Build rubyrs-cext (cdylib + rlib).
( cd "$WORKSPACE_ROOT" && cargo build -p rubyrs-cext $CARGO_PROFILE_FLAG )

# 2. Pick host-specific shared-library suffix and link flags.
case "$(uname -s)" in
    Darwin)
        EXT=bundle
        # `-undefined dynamic_lookup`: leave `rb_*` symbols unresolved
        # at link time; macOS's dynamic linker resolves them against
        # the host process (rubyrs binary) at dlopen time.
        LDFLAGS=(
            -shared
            -undefined dynamic_lookup
        )
        ;;
    Linux)
        EXT=so
        # `--unresolved-symbols=ignore-all`: same idea as macOS's
        # `dynamic_lookup` — defer `rb_*` to runtime resolution
        # against the host process's exported symbols. The host
        # binary is built with `--export-dynamic` (see
        # crates/rubyrs/build.rs) so dlsym can find them.
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

# 3. Compile hello.c. Note: no `-lrubyrs_cext`. We deliberately do not
# link the bundle against a separate cdylib — that would give us two
# physically distinct copies of the cext thread-local STATE. Instead
# the bundle's `rb_*` references are left unresolved and bind to the
# host process at dlopen time. See crates/rubyrs-cext/Cargo.toml.
OUT="$SCRIPT_DIR/hello.$EXT"
cc "${LDFLAGS[@]}" \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   "$SCRIPT_DIR/hello.c" \
   -o "$OUT"

echo "built $OUT"
