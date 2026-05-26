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

# Cross-process safety against parallel cargo test binaries.
# Mirrors msgpack-cext/build.sh's rationale: `cargo test` runs
# integration-test binaries in parallel processes; each calls
# this script. An in-process OnceLock in Rust dedupes within ONE
# binary but not across them, so two concurrent `cc ... -o $OUT`
# invocations would race on the same output file and a parallel
# `dlopen` could see a half-written Mach-O / ELF ("file too short"
# / invalid object) — flaky CI failure.
#
# Two layers with non-overlapping jobs:
#   1. Always-on: link each cc invocation to a `$TMP` then `mv -f`
#      atomically into `$OUT`. POSIX rename(2) is atomic on the
#      same filesystem, so a parallel `dlopen` sees either the
#      OLD bundle or the COMPLETE new one — never a truncated
#      file. This alone guarantees correctness regardless of
#      how many concurrent builders fire.
#   2. Optional flock-based MUTUAL EXCLUSION across BOTH
#      `cc` invocations: if `flock(1)` is installed (default
#      on Linux util-linux, absent on macOS unless brewed),
#      serialise concurrent runs of THIS script so only one
#      compile pair is in flight at a time. Both callers
#      still ultimately compile (serialisation, not
#      deduplication), but cumulative CPU stays linear
#      instead of N callers × full compile in parallel.
#      Pure performance — has no effect on output
#      correctness, which layer 1 already guarantees.
#
# Why one shared lock file vs. two: parser.c and generator.c
# don't share output paths, so a per-output lock would let two
# callers compile parser+generator concurrently with each other.
# That's fine for correctness but wastes CPU duplicating work.
# Holding one lock across both compiles keeps the serialisation
# tight.
if command -v flock >/dev/null 2>&1; then
    exec 9>"$SCRIPT_DIR/.build.lock"
    flock 9
fi

# Common compile flags. -DJSON_GENERATOR gates the right forward-
# decl block in fbuffer.h for generator only; parser is built
# without it so it gets the non-generator path.
COMMON_CFLAGS=(
    -fPIC
    -fno-strict-aliasing
    -DHAVE_STDBOOL_H
    -DHAVE_STRNLEN
    # Take the direct rb_enc_interned_str path for hash-key
    # construction instead of the fallback `rb_funcall(s, :uminus, 0)`
    # — rubyrs Strings don't yet implement `String#-@`, which would
    # collapse every parsed hash key to Qnil.
    -DHAVE_RB_ENC_INTERNED_STR
    -I "$WORKSPACE_ROOT/crates/rubyrs-cext/include"
    -I "$SCRIPT_DIR/vendor/fbuffer"
)

PARSER_TMP="$PARSER_OUT.tmp.$$"
GENERATOR_TMP="$GENERATOR_OUT.tmp.$$"
trap 'rm -f "$PARSER_TMP" "$GENERATOR_TMP"' EXIT

cc "${LDFLAGS[@]}" "${COMMON_CFLAGS[@]}" \
   "$SCRIPT_DIR/vendor/parser/parser.c" \
   -o "$PARSER_TMP"
mv -f "$PARSER_TMP" "$PARSER_OUT"

cc "${LDFLAGS[@]}" "${COMMON_CFLAGS[@]}" \
   -DJSON_GENERATOR \
   "$SCRIPT_DIR/vendor/generator/generator.c" \
   -o "$GENERATOR_TMP"
mv -f "$GENERATOR_TMP" "$GENERATOR_OUT"

echo "built $PARSER_OUT"
echo "built $GENERATOR_OUT"
