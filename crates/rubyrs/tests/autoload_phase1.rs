//! Phase 1 of issue #224 — toplevel autoload trigger.
//!
//! Spec coverage (`spec/ruby/autoload_phase1_spec.rb`) handles the
//! FS-free bits (registration round-trip + arity/type guards).
//! This file covers the end-to-end trigger: register an autoload
//! pointing at a real `.rb` file, reference the constant, observe
//! the file get loaded and the constant resolve.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

#[test]
fn autoload_fires_require_on_first_reference() {
    let dir = tmp();
    let lib = dir.join("autoload_p1_target.rb");
    let driver = dir.join("autoload_p1_driver.rb");
    fs::write(&lib,
        "class AutoloadP1Target; def self.greet; 'hello from autoload'; end; end\n",
    ).unwrap();
    let lib_stem = lib.with_extension("");
    fs::write(&driver, format!(
        "autoload(:AutoloadP1Target, {:?})\n\
         puts AutoloadP1Target.greet\n",
        lib_stem.display(),
    )).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs).arg(&driver).output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );
    assert_eq!(stdout.trim(), "hello from autoload");
}

#[test]
fn autoload_entry_cleared_after_fire() {
    // After the trigger fires, `autoload?(:Foo)` returns nil
    // because the entry is popped BEFORE require (CRuby
    // semantics — prevents re-entry into the same autoload
    // while the require is mid-flight).
    let dir = tmp();
    let lib = dir.join("autoload_p1_cleared_target.rb");
    let driver = dir.join("autoload_p1_cleared_driver.rb");
    fs::write(&lib,
        "class AutoloadP1Cleared; end\n",
    ).unwrap();
    let lib_stem = lib.with_extension("");
    fs::write(&driver, format!(
        "autoload(:AutoloadP1Cleared, {:?})\n\
         puts \"before: #{{autoload?(:AutoloadP1Cleared).inspect}}\"\n\
         AutoloadP1Cleared\n\
         puts \"after:  #{{autoload?(:AutoloadP1Cleared).inspect}}\"\n",
        lib_stem.display(),
    )).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs).arg(&driver).output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );
    let lines: Vec<&str> = stdout.lines().collect();
    // The exact path string echoed depends on host path
    // formatting; just verify "before" was non-nil (starts with
    // `"`, i.e. a String) and "after" is nil.
    assert!(lines.first().map(|l| l.starts_with("before: \"")).unwrap_or(false),
        "expected before line to be non-nil String, got: {:?}", lines.first());
    assert_eq!(lines.get(1).copied(), Some("after:  nil"));
}

#[test]
fn autoload_subsequent_reference_does_not_re_require() {
    // The second reference must NOT call require again (the
    // entry has been removed and the constant is now resolved).
    // We verify by having the loaded file print a marker —
    // appears exactly once even with multiple references.
    let dir = tmp();
    let lib = dir.join("autoload_p1_once_target.rb");
    let driver = dir.join("autoload_p1_once_driver.rb");
    fs::write(&lib,
        "puts \"LOADED\"\nclass AutoloadP1Once; def self.tag; :tag; end; end\n",
    ).unwrap();
    let lib_stem = lib.with_extension("");
    fs::write(&driver, format!(
        "autoload(:AutoloadP1Once, {:?})\n\
         AutoloadP1Once.tag\n\
         AutoloadP1Once.tag\n\
         AutoloadP1Once.tag\n\
         puts \"DONE\"\n",
        lib_stem.display(),
    )).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs).arg(&driver).output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );
    let loaded_count = stdout.lines().filter(|l| l.trim() == "LOADED").count();
    assert_eq!(loaded_count, 1,
        "LOADED appeared {} times, expected 1.\nstdout:\n{}",
        loaded_count, stdout);
    assert!(stdout.contains("DONE"));
}
