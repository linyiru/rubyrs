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
# Cross-process safety against parallel cargo test binaries.
# `cargo test` runs integration-test binaries in parallel
# processes; each calls into this script. An in-process OnceLock
# in Rust dedupes within ONE binary but not across them, so two
# `cc ... -o $OUT` invocations would race on the same output
# and a parallel `dlopen` could see a half-written Mach-O
# ("file too short" / "invalid mach-o").
#
# Two layers of defense:
#   1. Optional flock-based dedup: if `flock(1)` is installed
#      (default on Linux util-linux, absent on macOS unless
#      brewed), serialize concurrent runs so only one cc fires.
#   2. Always: link to `$TMP` then `mv -f` into `$OUT`. POSIX
#      rename(2) is atomic on the same filesystem, so a reader
#      either dlopens the OLD bundle or the COMPLETE new one,
#      never a partial. Without flock the cc work is duplicated
#      but the output stays correct — only CPU is wasted.
if command -v flock >/dev/null 2>&1; then
    exec 9>"$OUT.lock"
    flock 9
fi
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

TMP="$OUT.tmp.$$"
trap 'rm -f "$TMP"' EXIT
cc "${LDFLAGS[@]}" \
   -fPIC -fno-strict-aliasing \
   -DHAVE_STDBOOL_H -DHAVE_STRNLEN \
   -DHAVE_RB_HASH_NEW_CAPA -DHAVE_RB_ENC_INTERNED_STR \
   -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include" \
   -I "$SCRIPT_DIR/vendor/msgpack" \
   "${SRCS[@]}" \
   -o "$TMP"
mv -f "$TMP" "$OUT"

echo "built $OUT"
