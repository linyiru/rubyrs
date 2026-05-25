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
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let example_dir = crate_dir.join("examples/counter-cext");
            let build_sh = example_dir.join("build.sh");
            // Assert build.sh exists so a missing script surfaces as
            // a targeted failure rather than a dlopen-time NotFound
            // (review #5; matches the callback-cext pattern).
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
            let bundle = example_dir.join(format!("counter_ext.{}", common::DYLIB_EXT));
            // Sanity-check the bundle actually got produced (review #5).
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
