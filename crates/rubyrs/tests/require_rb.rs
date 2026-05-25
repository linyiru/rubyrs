//! A5 acceptance: `require "..."` accepts `.rb` files (Ruby
//! source), not just cext bundles.
//!
//! Previously `require` was an alias for `cext_require` — only
//! `.dylib` / `.bundle` / `.so` could be loaded. That blocked
//! every gem that ships a pure-Ruby `lib/` helper file (msgpack's
//! `register_type` wrapper, BigInt integration, Time hooks, etc.)
//! from working without the caller hand-rolling the wrapper.
//!
//! The fix factors a shared `load_ruby_source_from_canon` out of
//! `require_relative` and routes `require` to it when the
//! resolved path ends in `.rb`. The cext path stays as the
//! fallback for native extensions.
//!
//! Detection rule:
//!   1. If the input has a `.rb` extension and the file exists,
//!      use the Ruby loader.
//!   2. If the input has no extension and a `.rb` sibling
//!      exists, use the Ruby loader.
//!   3. Otherwise fall through to `cext_require` (native ext).
//!
//! This test covers cases 1 + 2 by creating temp .rb files and
//! requiring them. Case 3 is covered by the existing
//! `cext_msgpack` / `cext_mini_json` tests.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn require_loads_rb_file_with_explicit_extension() {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let lib_path = tmp.join("require_rb_explicit_lib.rb");
    let driver_path = tmp.join("require_rb_explicit_driver.rb");
    fs::write(&lib_path,
        "class Greeter; def self.hello; 'hello from required lib'; end; end\n"
    ).unwrap();
    fs::write(&driver_path, format!(
        "require \"{}\"\nputs Greeter.hello\n",
        lib_path.display()
    )).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );
    assert_eq!(stdout.trim(), "hello from required lib");
}

#[test]
fn require_loads_rb_file_with_auto_extension() {
    // Same as above but pass the path WITHOUT the `.rb`
    // extension — the require should auto-append it and find
    // the file.
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let lib_path = tmp.join("require_rb_auto_lib.rb");
    let driver_path = tmp.join("require_rb_auto_driver.rb");
    fs::write(&lib_path,
        "class AutoLib; def self.ping; 'pong'; end; end\n"
    ).unwrap();
    // Strip the .rb so require has to find it.
    let lib_stem = lib_path.with_extension("");
    fs::write(&driver_path, format!(
        "require \"{}\"\nputs AutoLib.ping\n",
        lib_stem.display()
    )).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );
    assert_eq!(stdout.trim(), "pong");
}

#[test]
fn require_dedup_loaded_features_across_calls() {
    // Loading the same file twice via `require` should return
    // true the first time, false the second time. Same dedup
    // contract as `require_relative` (the load helper is
    // shared).
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let lib_path = tmp.join("require_rb_dedup_lib.rb");
    let driver_path = tmp.join("require_rb_dedup_driver.rb");
    // rubyrs doesn't support $-globals or @@class-vars yet, so
    // the side-effect channel is an Array TRACK_LOG defined at
    // top level in the driver and appended to from the lib.
    // Each load pushes "loaded"; dedup-OK means the array has
    // length 1 after two require calls.
    fs::write(&lib_path, "TRACK_LOG.push(\"loaded\")\n").unwrap();
    fs::write(&driver_path, format!(
        r#"
TRACK_LOG = []
r1 = require "{lib}"
r2 = require "{lib}"
puts "first=" + r1.inspect
puts "second=" + r2.inspect
puts "counter=" + TRACK_LOG.length.to_s
"#,
        lib = lib_path.display(),
    )).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );
    assert_eq!(
        stdout, "first=true\nsecond=false\ncounter=1\n",
        "dedup mismatch.\nfull stdout:\n{}", stdout,
    );
}

#[test]
fn require_missing_rb_falls_back_to_cext_or_errors() {
    // Path with no .rb sibling and no cext sibling should
    // error out cleanly (RuntimeError-shape via the cext
    // path's "cannot find C ext" message).
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver_path = tmp.join("require_rb_missing_driver.rb");
    fs::write(&driver_path,
        r#"
begin
  require "/tmp/this_path_does_not_exist_98765"
  puts "unexpectedly loaded"
rescue RuntimeError => e
  puts "caught: #{e.class}"
end
"#
    ).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout:\n{}", stdout);
    assert_eq!(stdout.trim(), "caught: RuntimeError");
}
