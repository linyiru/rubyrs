#!/usr/bin/env bash
# Build msgpack-ruby cext (Spike L3-E).
# Same shape as flori-json-cext/build.sh — vendored sources unchanged,
# rubyrs-cext shim header on -I, -undefined dynamic_lookup so missing
# rb_* defer to dlopen-time resolution against the host binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROFILE="${PROFILE:-debug}"
CARGO_FLAG=""
[ "$PROFILE" = "release" ] && CARGO_FLAG="--release"

( cd "$WORKSPACE_ROOT" && cargo build -p rubyrs $CARGO_FLAG )

case "$(uname -s)" in
    Darwin) EXT=bundle; LDFLAGS=(-shared -undefined dynamic_lookup);;
    Linux)  EXT=so;     LDFLAGS=(-shared -fPIC -Wl,--unresolved-symbols=ignore-all);;
    *) echo "build.sh: unsupported host $(uname -s)" >&2; exit 1;;
esac

OUT="$SCRIPT_DIR/msgpack.$EXT"
SRCS=(
    "$SCRIPT_DIR/vendor/msgpack/buffer.c"
    "$SCRIPT_DIR/vendor/msgpack/buffer_class.c"
    "$SCRIPT_DIR/vendor/msgpack/extension_value_class.c"
    "$SCRIPT_DIR/vendor/msgpack/factory_class.c"
    "$SCRIPT_DIR/vendor/msgpack/packer.c"
    "$SCRIPT_DIR/vendor/msgpack/packer_class.c"
    "$SCRIPT_DIR/vendor/msgpack/packer_ext_registry.c"
    "$SCRIPT_DIR/vendor/msgpack/rbinit.c"
    "$SCRIPT_DIR/vendor/msgpack/rmem.c"
    "$SCRIPT_DIR/vendor/msgpack/unpacker.c"
    "$SCRIPT_DIR/vendor/msgpack/unpacker_class.c"
    "$SCRIPT_DIR/vendor/msgpack/unpacker_ext_registry.c"
)

cc "${LDFLAGS[@]}" \
   -fPIC -fno-strict-aliasing \
   -DHAVE_STDBOOL_H -DHAVE_STRNLEN \
   -DHAVE_RB_HASH_NEW_CAPA -DHAVE_RB_ENC_INTERNED_STR \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   -I "$SCRIPT_DIR/vendor/msgpack" \
   "${SRCS[@]}" \
   -o "$OUT"

echo "built $OUT"
