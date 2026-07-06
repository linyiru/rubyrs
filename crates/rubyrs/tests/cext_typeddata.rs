//! Spike L3-B acceptance: TypedData wrap + GC-managed dfree.
//!
//! Builds `examples/counter-cext/counter_ext.c` which:
//!   - Defines a `Counter` class via rb_define_class_under(rb_cObject).
//!   - On `Counter.create`, mallocs a `{ long count; }` C struct,
//!     wraps it via TypedData_Wrap_Struct(Counter, &counter_type, c),
//!     and returns the wrapped VALUE.
//!   - On `Counter.inc(c)` / `Counter.value(c)`, calls
//!     TypedData_Get_Struct(c, Counter, &counter_type, sval) and
//!     manipulates the C struct directly.
//!   - On `Counter.free_count`, returns a static long that
//!     `counter_free` increments — used by the test below to
//!     verify the dfree callback actually ran.
//!
//! The test asserts three properties end-to-end:
//!
//!   1. Create + use round-trips: a Counter survives method calls,
//!      its state is preserved across them.
//!   2. dfree fires on GC: after dropping the only Ruby reference
//!      AND triggering a GC sweep (forced by STRESS_GC + a
//!      throwaway allocation), the C-side static counter that
//!      counter_free increments goes from 0 to 1. This is the
//!      load-bearing claim of L3-B — that a C extension can rely
//!      on rubyrs's GC to release its native resources.
//!
//! The test runs under STRESS_GC=1 so a single post-drop
//! allocation deterministically triggers a sweep. Without that
//! the sweep would only fire at the next_gc threshold, making
//! the assertion timing-dependent.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_counter_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| common::build_cext_bundle("counter-cext", "counter_ext")).clone()
}

#[test]
fn cext_typeddata_create_and_dfree() {
    let bundle = ensure_counter_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_typeddata_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"

# 1. Round-trip: create, manipulate, read back.
c = Counter.create
Counter.inc(c)
Counter.inc(c)
Counter.inc(c)
puts Counter.value(c)
puts Counter.free_count

# 2. dfree fires on GC. Drop the Ruby reference, force a sweep
#    via STRESS_GC + a throwaway allocation. The C-side
#    counter_free callback runs on the swept TypedData slot,
#    bumping the static g_free_count from 0 to 1.
c = nil
[1].each {{ |x| x }}
puts Counter.free_count
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .env("STRESS_GC", "1")
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
3
0
1
";

    assert_eq!(
        stdout, expected,
        "TypedData round trip mismatch.\n\
         expected:\n{}\n\
         got:\n{}\n\
         stderr:\n{}",
        expected, stdout, stderr,
    );
}

/// M7: `Runtime::reset()` must run the dfree of every TypedData a
/// rolled-back eval allocated — the public-API twin of the in-crate
/// zombie-shape tests (src/lib.rs `reset_typeddata_dfree_tests`).
/// Uses the same counter cext, but IN-PROCESS through an embedded
/// Runtime (reset() is an embedding API; the CLI never calls it):
///
///   1. `require` the bundle, create Counters, keep them rooted in
///      globals so no GC sweeps them early.
///   2. `reset()` — drops the user heap slots; the fix under test
///      invokes `counter_free` on each wrapped struct.
///   3. Re-`require` (reset cleared `loaded_features`; the dylib
///      stays loaded via the bridge's `mem::forget`, so the C
///      static `g_free_count` PERSISTS) and read
///      `Counter.free_count` back.
#[test]
fn cext_typeddata_reset_runs_dfree() {
    let bundle = ensure_counter_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let mut rt = rubyrs::Runtime::with_config(rubyrs::Config {
        // `require` of the bundle path needs script-level fs IO.
        allow_filesystem_io: true,
        // Honour the repo's STRESS_GC=1 rerun convention (the
        // library default never reads env; this test opts in the
        // same way the reset soak does).
        stress_gc: std::env::var_os("STRESS_GC").is_some_and(|v| v == "1"),
        ..Default::default()
    });
    let req = format!(r#"require "{}""#, bundle_no_ext.display());
    rt.eval(&req, "req.rb").expect("first require");
    let v = rt
        .eval(
            r#"
            $a = Counter.create
            $b = Counter.create
            Counter.inc($a)
            [Counter.value($a), Counter.value($b), Counter.free_count].inspect
            "#,
            "make.rb",
        )
        .expect("create counters");
    assert!(
        matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"[1, 0, 0]"),
        "pre-reset state, got {v:?}",
    );
    rt.reset();
    // Post-reset: the Counter class (user-eval state) is gone; the
    // dylib and its g_free_count static are not. Re-require and
    // read the count: both wrapped structs must have been freed by
    // reset(), exactly once each.
    rt.eval(&req, "req2.rb").expect("re-require after reset");
    let v = rt.eval("Counter.free_count", "count.rb").expect("free_count");
    assert_eq!(
        format!("{v:?}"),
        "Int(2)",
        "reset() must have run counter_free for both rolled-back Counters",
    );
}
