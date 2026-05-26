//! Regression test for the flori/json parser leg landed under
//! Spike L3 (rb_raise / TypedData / alloc func / variadic
//! arity -1) — see `docs/L2.5-FLORI-JSON-DISCOVERY.md` status
//! update for the architectural pieces this exercises.
//!
//! The flori-json-cext bundle has been built and used implicitly
//! by `cext_msgpack_cases.rs` (which loads it to parse the JSON
//! corpus reference), but no test pinned the parser path as a
//! first-class contract — so a regression in TypedData wrap,
//! cParser_initialize variadic dispatch, or the rb_raise →
//! rescue propagation would only surface as a knock-on failure
//! in the msgpack corpus diff, far from the actual cause. This
//! file makes those guarantees explicit.
//!
//! Two assertions, both end-to-end via the rubyrs binary loading
//! the unmodified flori/json 2.9.1 source compiled with our
//! cext header shims:
//!
//!   1. **Parse round-trip on a non-trivial JSON document.**
//!      Object with five primitive flavors (int, float, string,
//!      bool, null) plus a nested array of strings. Exercises:
//!      - TypedData_Make_Struct (parser state IS the C struct,
//!        allocated via the rb_define_alloc_func registration)
//!      - Variadic `cParser_initialize` (declared with arity -1
//!        for `def initialize(*args)`)
//!      - rb_funcallv-style hash/array building from inside C
//!      - Float CValue (the L3-I fix), bool/nil sentinels, the
//!        binary-safe rb_str_new path
//!
//!   2. **rb_raise from parser.c on malformed input propagates
//!      back to Ruby rescue.** This is the L3-A setjmp/longjmp
//!      shim's end-to-end contract: a C-side `rb_raise(...)`
//!      lands in a `rescue => e` clause without aborting the
//!      Vm. The exception class reported is RuntimeError because
//!      `rb_path2class("JSON::ParserError")` is currently a
//!      stub returning null (documented gap in
//!      `docs/L2.5-FLORI-JSON-DISCOVERY.md`); once that
//!      resolves to a real class, this assertion will need
//!      updating to expect `JSON::ParserError`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_parser_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| common::build_cext_bundle("flori-json-cext", "parser")).clone()
}

/// Drive a Ruby script via the rubyrs binary; return stdout.
fn run_rubyrs(script: &str, fixture_name: &str) -> String {
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join(format!("cext_flori_json_{}.rb", fixture_name));
    fs::write(&driver, script).expect("failed to write driver.rb");
    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}) for fixture {}.\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        fixture_name,
        stdout,
        stderr,
    );
    stdout
}

#[test]
fn cext_flori_json_parser_round_trip() {
    let bundle = ensure_parser_bundle_built();

    // Pull each value out of the parsed Hash individually so
    // the assertion is robust to Ruby Hash#inspect ordering
    // quirks (Ruby preserves insertion order, but better to
    // not depend on it cross-implementation). Each `puts`
    // line carries a name=value pair so the diagnostic on
    // failure points at the exact element that diverged.
    let script = format!(
        r##"require "{bundle}"

json = '{{"name":"rubyrs","ver":4.7,"tags":["ruby","rust"],"ok":true,"nil":null}}'
result = JSON::Ext::Parser.new(json).parse

puts "class=#{{result.class}}"
puts "name=#{{result["name"]}}"
puts "ver=#{{result["ver"]}}"
puts "tags=#{{result["tags"].inspect}}"
puts "ok=#{{result["ok"]}}"
puts "nil_is_nil=#{{result["nil"].nil?}}"
"##,
        bundle = bundle.display(),
    );

    let stdout = run_rubyrs(&script, "round_trip");
    let expected = "\
class=Hash
name=rubyrs
ver=4.7
tags=[\"ruby\", \"rust\"]
ok=true
nil_is_nil=true
";
    assert_eq!(
        stdout, expected,
        "flori/json parser round-trip mismatch.\nfull stdout:\n{}",
        stdout,
    );
}

#[test]
fn cext_flori_json_parser_rb_raise_propagates() {
    let bundle = ensure_parser_bundle_built();

    // Malformed JSON ('{' with no closing or value) makes
    // parser.c's parse_value branch fall to its
    // `rb_enc_raise(enc_utf8, rb_path2class("JSON::ParserError"),
    // "unexpected token at '%s'", ...)` line. The setjmp shim
    // (crates/rubyrs-cext/c/setjmp_shim.c) intercepts the
    // longjmp from rb_raise and surfaces the exception to the
    // Vm, where Ruby's `rescue` catches it.
    let script = format!(
        r##"require "{bundle}"

begin
  JSON::Ext::Parser.new('{{bad').parse
  puts "outcome=did-not-raise"
rescue => e
  puts "outcome=rescued"
  puts "class=#{{e.class}}"
end
"##,
        bundle = bundle.display(),
    );

    let stdout = run_rubyrs(&script, "rb_raise");
    // The exception class comes through as `RuntimeError`
    // because `rb_path2class("JSON::ParserError")` is
    // currently a stub returning null (see
    // crates/rubyrs-cext/src/lib.rs:~1740 and the
    // L2.5-FLORI-JSON-DISCOVERY status update). When
    // rb_path2class learns to resolve real class objects,
    // this assertion should be updated to expect
    // `JSON::ParserError`. The `outcome=rescued` line is
    // the load-bearing assertion regardless of class name
    // — it pins that rb_raise from inside C propagates back
    // to a Ruby rescue clause at all (the L3-A shim's
    // end-to-end contract).
    let expected = "\
outcome=rescued
class=RuntimeError
";
    assert_eq!(
        stdout, expected,
        "rb_raise propagation mismatch.\nfull stdout:\n{}",
        stdout,
    );
}
