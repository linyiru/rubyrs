//! Regression coverage for PR #2 review comment #1.
//!
//! Pre-fix behaviour: `cext_dispatch` always passed `Qnil` as the
//! implicit `self` to every C function pointer. Correct for
//! `rb_define_global_function`; incorrect for
//! `rb_define_singleton_method` where CRuby passes the class/module
//! object.
//!
//! This test compiles `examples/self-cext/self_test.c` and dispatches
//! three callbacks — two singletons and one global — that each return
//! a marker string indicating whether they observed `Qnil` or
//! something else. The expected pattern:
//!
//!     SelfCheck.from_module        → "ok: module singleton self is not Qnil"
//!     SelfCheck::Inner.from_class  → "ok: class singleton self is not Qnil"
//!     from_global                  → "ok: global function self is Qnil"
//!
//! If `cext_dispatch` regresses to passing Qnil everywhere, the
//! first two assertions fail. If it regresses to passing class
//! everywhere, the third fails. Either direction is named in the
//! diff.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

mod common;

#[test]
fn cext_self_is_class_for_singletons_qnil_for_globals() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let example_dir = crate_dir.join("examples/self-cext");
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

    let bundle = example_dir.join(format!("self_test.{}", common::RUBY_DLEXT));
    assert!(
        bundle.exists(),
        "build.sh did not produce {}",
        bundle.display()
    );

    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_self_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"
puts SelfCheck.from_module
puts SelfCheck::Inner.from_class
puts from_global
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
ok: module singleton self is not Qnil
ok: class singleton self is not Qnil
ok: global function self is Qnil
";

    assert_eq!(
        stdout, expected,
        "self-dispatch regression detected.\n\
         expected:\n{}\n\
         got:\n{}\n\
         stderr:\n{}",
        expected, stdout, stderr,
    );
}
