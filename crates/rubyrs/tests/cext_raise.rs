//! Spike L3-A acceptance: rb_raise across the FFI boundary.
//!
//! Builds `examples/raise-cext/raise_ext.c` (which calls
//! `rb_raise(rb_eArgumentError, fmt, ...)` from inside its C
//! functions) and drives it from Ruby. Asserts:
//!
//!   1. The normal-return path still works (no raise) — proves we
//!      didn't break cext_dispatch's happy path while wedging in
//!      the longjmp catcher.
//!   2. `rb_raise(rb_eArgumentError, ...)` is caught by Ruby-side
//!      `rescue ArgumentError` with the formatted message.
//!   3. `rb_raise(rb_eRuntimeError, ...)` is caught by `rescue
//!      RuntimeError` with the formatted message.
//!   4. A conditional raise inside an otherwise-normal C function
//!      works both ways from the same dispatch entry point.
//!
//! Every raise message uses a vsnprintf format string so we also
//! exercise the variadic shim in rubyrs-cext/c/raise.c.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_raise_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let example_dir = crate_dir.join("examples/raise-cext");
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
            let bundle = example_dir.join(format!("raise_ext.{}", common::DYLIB_EXT));
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
fn cext_rb_raise_is_caught_by_ruby_rescue() {
    let bundle = ensure_raise_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_raise_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"

# 1. Normal path — no raise, returns the input integer.
puts raise_unless_positive(5)

# 2. rb_eArgumentError → rescue ArgumentError.
begin
  raise_argument_error(nil)
  puts "fail: no raise"
rescue ArgumentError => e
  puts "rescued ArgumentError: #{{e.message}}"
end

# 3. rb_eRuntimeError → rescue RuntimeError.
begin
  raise_runtime_error(nil)
  puts "fail: no raise"
rescue RuntimeError => e
  puts "rescued RuntimeError: #{{e.message}}"
end

# 4. Conditional raise — same entry point as the normal path
#    above, but with input that triggers the raise branch.
begin
  raise_unless_positive(-1)
  puts "fail: no raise"
rescue ArgumentError => e
  puts "rescued conditional ArgumentError: #{{e.message}}"
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

    let expected = "\
5
rescued ArgumentError: bogus arg: expected positive
rescued RuntimeError: runtime boom in raise_runtime_error
rescued conditional ArgumentError: expected positive, got -1
";

    assert_eq!(
        stdout, expected,
        "rb_raise round trip mismatch.\n\
         expected:\n{}\n\
         got:\n{}\n\
         stderr:\n{}",
        expected, stdout, stderr,
    );
}
