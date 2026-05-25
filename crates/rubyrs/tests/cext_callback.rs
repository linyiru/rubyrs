//! Acceptance test for Level 2's load-bearing hypothesis: a C
//! extension can call back into Ruby via `rb_funcall*`, get the
//! result, and return it.
//!
//! Builds `examples/callback-cext/callback_ext.c` (which uses
//! `rb_intern` at `Init_` time to cache method IDs and
//! `rb_funcallv` per-call to dispatch them) and exercises four
//! callbacks that each return a different value type:
//!
//!     apply_upcase("hello")    → "HELLO"   (String → String)
//!     string_length("rubyrs")  → 6         (String → Integer)
//!     nil_check(nil)           → true      (Nil    → Bool)
//!     nil_check("not nil")     → false     (String → Bool)
//!
//! Every line stresses three wires together:
//!   1. `rb_intern`'s process-wide intern table (cached IDs survive
//!      past `Init_` into the per-call dispatch).
//!   2. `rb_funcallv`'s reentrance into the host Vm (the topmost
//!      installed funcall callback bridges to `Vm::cext_invoke_method`).
//!   3. Handle ↔ Value translation for each return type (Str / Int
//!      / Bool covers the three non-trivial CValue arms).
//!
//! A regression in any of those wires fails one of the asserts with
//! a diff that names the failing branch.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Build the callback-cext bundle exactly once per test process.
///
/// `cargo test` runs integration tests in parallel by default. The
/// two tests in this file both invoke `examples/callback-cext/build.sh`,
/// which writes the same `callback_ext.{bundle,so,dll}` artifact.
/// Without serialisation the two `cc -o` invocations can race on
/// the output file → flaky CI.
///
/// `OnceLock::get_or_init` guarantees the closure runs at most once
/// across all threads in the process; concurrent callers block until
/// it returns. Each test calls `ensure_callback_bundle_built()` and
/// gets a no-op after the first build completes.
fn ensure_callback_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let example_dir = crate_dir.join("examples/callback-cext");
            let build_sh = example_dir.join("build.sh");
            assert!(
                build_sh.exists(),
                "missing build.sh at {}",
                build_sh.display()
            );
            let build = Command::new("bash")
                .arg(&build_sh)
                .output()
                .expect("failed to spawn build.sh");
            assert!(
                build.status.success(),
                "build.sh failed.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&build.stdout),
                String::from_utf8_lossy(&build.stderr),
            );
            let ext = if cfg!(target_os = "macos") {
                "bundle"
            } else if cfg!(windows) {
                "dll"
            } else {
                "so"
            };
            let bundle = example_dir.join(format!("callback_ext.{}", ext));
            assert!(
                bundle.exists(),
                "build.sh did not produce {}",
                bundle.display()
            );
            bundle
        })
        .clone()
}

#[test]
fn cext_rb_funcall_round_trip() {
    let bundle = ensure_callback_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_callback_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"
puts apply_upcase("hello")
puts string_length("rubyrs")
puts nil_check(nil)
puts nil_check("not nil")
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        stdout,
        stderr,
    );

    let expected = "\
HELLO
6
true
false
";

    assert_eq!(
        stdout, expected,
        "rb_funcall round trip mismatch.\n\
         expected:\n{}\n\
         got:\n{}\n\
         stderr:\n{}",
        expected, stdout, stderr,
    );
}

/// L2-3 acceptance: C-side Array + Hash builders round-trip
/// through the recursive translator into Ruby Value::Array /
/// Value::Hash on the heap.
///
/// Three exercises, increasing nesting:
///
///   1. `build_list` — flat Array of Int handles → Ruby Array of
///      Integers. Verifies `rb_ary_new` + `rb_ary_push` + Int
///      translation.
///   2. `build_pair("rubyrs")` — Hash with mixed Str + Int values,
///      where the Int comes from a nested `rb_funcall("rubyrs",
///      "length")`. Verifies rb_funcall callback nested inside a
///      Hash builder still finds the correct CExtState (the L2-2
///      nesting fix).
///   3. `build_records` — Array of Hashes (JSON-shape document).
///      Verifies the recursive translator handles Array-of-Hashes
///      with PinGuard correctness.
#[test]
fn cext_array_hash_round_trip() {
    let bundle = ensure_callback_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_collections_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"

# 1. Flat Array of Int.
a = build_list
puts a.length
puts a[0]
puts a[4]

# 2. Hash with String + Int values (Int via rb_funcall nested
#    inside a builder — exercises CExtState nesting).
h = build_pair("rubyrs")
puts h["name"]
puts h["len"]

# 3. Array of Hashes (JSON-shape document).
records = build_records
puts records.length
puts records[0]["lang"]
puts records[1]["lang"]
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        stdout,
        stderr,
    );

    let expected = "\
5
1
5
rubyrs
6
2
ruby
rust
";

    assert_eq!(
        stdout, expected,
        "Array/Hash round trip mismatch.\n\
         expected:\n{}\n\
         got:\n{}\n\
         stderr:\n{}",
        expected, stdout, stderr,
    );
}

/// L2.5 acceptance: a cyclic C-built Array surfaces as an
/// ArgumentError exception in Ruby.
///
/// Before L2.5 the recursive translator hit its
/// `CEXT_TRANSLATE_MAX_DEPTH` guard and silently substituted
/// `Value::Nil` (+ a stderr warning). After L2.5 the guard
/// returns `Err(Trap::ArgumentError { ... })`, which cascades up
/// through `cext_dispatch` and lands as a Ruby-catchable
/// `ArgumentError` exception.
///
/// This test is the *end-to-end* proof that the Result-threaded
/// translator actually surfaces the error to Ruby — not just
/// "the Rust types now line up." The C ext writes
/// `a = rb_ary_new(); rb_ary_push(a, a);` (a self-referential
/// Array), and the Ruby driver wraps the call in
/// `begin/rescue ArgumentError` + asserts the message.
#[test]
fn cext_cycle_surfaces_argument_error() {
    let bundle = ensure_callback_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_cycle_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"
begin
  a = build_cycle
  puts "fail: got #{{a.inspect}}"
rescue ArgumentError => e
  puts "rescued: #{{e.message}}"
end
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        stdout,
        stderr,
    );
    assert!(
        stdout.starts_with("rescued: "),
        "expected ArgumentError to be rescued.\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr,
    );
    assert!(
        stdout.contains("max translation depth"),
        "expected depth-limit message in rescued ArgumentError.\nstdout:\n{}",
        stdout,
    );
}
