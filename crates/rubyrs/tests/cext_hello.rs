//! Acceptance test for the Level 0 C-ext compat spike.
//!
//! What we're verifying — the spike's load-bearing hypothesis:
//!   A hand-written CRuby-shape "hello world" C extension, compiled
//!   from unmodified source, can be `require`-d from rubyrs and its
//!   registered function dispatched, all the way to a `Value::Str`
//!   coming back into the interpreter.
//!
//! If this test passes the architecture (Option A handle-based VALUE,
//! single shared `STATE` via host-exported symbols, host-side
//! libloading-driven `Init_<name>` dispatch) is sound enough to start
//! the Level 1 spike against a real gem (bcrypt).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn hello_cext_round_trip() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let example_dir = crate_dir.join("examples/hello-cext");
    let build_sh = example_dir.join("build.sh");
    assert!(build_sh.exists(), "missing build.sh at {}", build_sh.display());

    // 1. Build the bundle (also builds rubyrs-cext if needed).
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

    // 2. Locate the produced artefact, host-aware.
    let ext = if cfg!(target_os = "macos") {
        "bundle"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    };
    let bundle = example_dir.join(format!("hello.{}", ext));
    assert!(
        bundle.exists(),
        "build.sh did not produce {}",
        bundle.display()
    );

    // 3. Write a tiny driver Ruby script — the rubyrs binary currently
    //    only accepts a file path (no `-e`), so we materialise one.
    //    The bundle path is passed *without extension* to exercise the
    //    auto-extension lookup in `Vm::cext_require`.
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_hello_driver.rb");
    fs::write(
        &driver,
        format!(r#"require "{}"
puts hello
"#, bundle_no_ext.display()),
    )
    .expect("failed to write driver.rb");

    // 4. Run the rubyrs binary against the driver.
    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        stdout,
        stderr,
    );
    assert_eq!(
        stdout.trim_end(),
        "hello from C",
        "unexpected stdout.\nfull stdout: {:?}\nstderr: {}",
        stdout,
        stderr
    );
}
