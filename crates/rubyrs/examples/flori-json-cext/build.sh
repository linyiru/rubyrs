#!/usr/bin/env bash
# Build the flori-json-cext example (Spike L3-D).
#
# Mirrors bcrypt-cext's layout: build rubyrs (host exports rb_*),
# then compile the vendored flori/json sources into a single bundle
# with -undefined dynamic_lookup (macOS) so unresolved rb_* bind
# against the host process at dlopen time.
#
# Vendor sources are deliberately UNTOUCHED — any adaptation lives
# in rubyrs-cext/include/rubyrs.h and ruby/encoding.h shims.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PROFILE="${PROFILE:-debug}"
CARGO_PROFILE_FLAG=""
if [ "$PROFILE" = "release" ]; then
    CARGO_PROFILE_FLAG="--release"
fi

# 1. Build the rubyrs binary (provides rb_* symbols).
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

# Build parser.bundle and generator.bundle separately — flori/json
# ships them as two cexts that share fbuffer.h. The Ruby-side json
# gem loads json/ext/parser and json/ext/generator separately.
PARSER_OUT="$SCRIPT_DIR/parser.$EXT"
GENERATOR_OUT="$SCRIPT_DIR/generator.$EXT"

# Common compile flags. -DJSON_GENERATOR gates the right forward-
# decl block in fbuffer.h for generator only; parser is built
# without it so it gets the non-generator path.
COMMON_CFLAGS=(
    -fPIC
    -fno-strict-aliasing
    -DHAVE_STDBOOL_H
    -DHAVE_STRNLEN
    -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include"
    -I "$SCRIPT_DIR/vendor/fbuffer"
)

cc "${LDFLAGS[@]}" "${COMMON_CFLAGS[@]}" \
   "$SCRIPT_DIR/vendor/parser/parser.c" \
   -o "$PARSER_OUT"

cc "${LDFLAGS[@]}" "${COMMON_CFLAGS[@]}" \
   -DJSON_GENERATOR \
   "$SCRIPT_DIR/vendor/generator/generator.c" \
   -o "$GENERATOR_OUT"

echo "built $PARSER_OUT"
echo "built $GENERATOR_OUT"
