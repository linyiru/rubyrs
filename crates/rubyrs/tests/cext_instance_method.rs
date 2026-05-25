//! Spike L3-C acceptance: `rb_define_method` instance dispatch.
//!
//! The L3-C mini-json wedge exercises rb_define_method's
//! registration path indirectly (the cext crate compiles +
//! exports the symbol; the vm side wires the dispatch table),
//! but mini-json's actual Ruby-side API is all singleton
//! methods on the MiniJson Module. Nothing in CI was actually
//! invoking the Value::Object dispatch arm.
//!
//! Reviewer flagged this gap (PR #27 review #5): the new
//! dispatch path can regress without notice. This test closes
//! the coverage hole by driving the existing counter-cext
//! Counter class through its new c.bump / c.peek instance
//! methods — same TypedData backing as the L3-B acceptance, new
//! dispatch path on top.
//!
//! Asserts:
//!   1. Instance method round-trip: c.bump three times → c.peek == 3.
//!   2. Pre-existing singleton dispatch still works: Counter.inc(c)
//!      after the instance bumps continues from where bump left off
//!      (proves both dispatch paths share the same backing C struct).
//!   3. STRESS_GC variant: every alloc triggers a sweep, so any
//!      GC root hole in the instance-dispatch path (review #4
//!      from the same PR) would surface as a use-after-free here.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_counter_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let example_dir = crate_dir.join("examples/counter-cext");
            let build_sh = example_dir.join("build.sh");
            assert!(build_sh.exists(), "missing build.sh at {}", build_sh.display());
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
            let bundle = example_dir.join(format!("counter_ext.{}", common::RUBY_DLEXT));
            assert!(bundle.exists(), "build.sh did not produce {}", bundle.display());
            bundle
        })
        .clone()
}

fn run_driver(stress_gc: bool) -> (String, String, bool) {
    let bundle = ensure_counter_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let suffix = if stress_gc { "_stress" } else { "" };
    let driver = PathBuf::from(driver_dir)
        .join(format!("cext_instance_method_driver{}.rb", suffix));
    fs::write(
        &driver,
        format!(
            r#"require "{}"

c = Counter.create

# 1. Instance method round-trip — these go through the new
#    rb_define_method dispatch (vm/dispatch.rs's
#    cext_instance_methods arm, not cext_class_methods).
c.bump
c.bump
c.bump
puts c.peek

# 2. Singleton + instance methods share the same TypedData
#    backing. Counter.inc(c) bumps the C-side `count` field once
#    more; c.peek (instance) sees it.
Counter.inc(c)
puts c.peek
puts Counter.value(c)

# 3. Multiple instances stay isolated through both paths.
d = Counter.create
d.bump
puts c.peek    # 4 (unchanged by d's bump)
puts d.peek    # 1
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let mut cmd = Command::new(rubyrs_bin);
    cmd.arg(&driver);
    if stress_gc {
        cmd.env("STRESS_GC", "1");
    }
    let run = cmd.output().expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    (stdout, stderr, run.status.success())
}

#[test]
fn cext_instance_method_round_trip() {
    let (stdout, stderr, ok) = run_driver(/*stress_gc=*/ false);
    assert!(
        ok,
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
    let expected = "\
3
4
4
4
1
";
    assert_eq!(
        stdout, expected,
        "instance-method dispatch mismatch.\nexpected:\n{}\ngot:\n{}\nstderr:\n{}",
        expected, stdout, stderr,
    );
}

/// Same script, STRESS_GC=1 — every alloc triggers a sweep. If
/// the cext_instance_methods dispatch path were missing the
/// PinGuard around `recv` + `args` (the bug fixed by review #4),
/// this would crash with ICE use-after-free.
#[test]
fn cext_instance_method_round_trip_under_stress_gc() {
    let (stdout, stderr, ok) = run_driver(/*stress_gc=*/ true);
    assert!(
        ok,
        "rubyrs exited non-zero under STRESS_GC.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
    let expected = "\
3
4
4
4
1
";
    assert_eq!(
        stdout, expected,
        "instance-method dispatch mismatch under STRESS_GC.\nexpected:\n{}\ngot:\n{}\nstderr:\n{}",
        expected, stdout, stderr,
    );
}

/// Regression from PR #27 code-review finding #1:
/// `Counter.new` (the generic `.new` path) allocates a plain
/// `HeapObj::Instance`, NOT a TypedData. Before the fix,
/// `Counter.new.bump` panicked the entire VM with "ICE: heap
/// slot is not a TypedData" from `rb_check_typeddata`. The fix
/// routes the type-check failure through `rb_raise(rb_eTypeError,
/// ...)` so script-level `rescue TypeError => e` catches it.
///
/// This test asserts the new behaviour: a clean Ruby-side
/// TypeError, no process abort, error message matches CRuby's
/// "wrong argument type" wording.
#[test]
fn cext_typeddata_mismatch_raises_typeerror() {
    let bundle = ensure_counter_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = std::path::PathBuf::from(driver_dir)
        .join("cext_typeddata_mismatch_driver.rb");
    std::fs::write(
        &driver,
        format!(
            r#"require "{}"
begin
  Counter.new.bump
  puts "fail: no raise"
rescue TypeError => e
  puts "rescued: #{{e.message}}"
end
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");
    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = std::process::Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "Counter.new.bump should rescue cleanly, not abort.\n\
         stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
    assert!(
        stdout.starts_with("rescued: wrong argument type"),
        "expected TypeError rescue with 'wrong argument type' message.\n\
         stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
}
