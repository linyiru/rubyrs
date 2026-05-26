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
fn require_satisfied_by_pre_registered_module_no_ops() {
    // Embedder-flavour case: a host or earlier script defines
    // a top-level module/class whose name is the camelized form
    // of the require path. The require should treat that as
    // already-loaded — Bool(true) on first observation,
    // Bool(false) thereafter — and NOT fall through to
    // cext_require (which would error with "cannot find C ext").
    //
    // Exercises three angles in one driver:
    //   1. snake_to_camel match (`module Rack` satisfies
    //      `require "rack"`)
    //   2. subpath match (`require "rack/show_exceptions"` is
    //      also satisfied because the first segment maps to
    //      the same already-defined `Rack`)
    //   3. case-insensitive fallback for non-conventional
    //      capitalization (`class IPAddr` satisfies
    //      `require "ipaddr"` — `snake_to_camel_case("ipaddr")`
    //      returns `Ipaddr`, neither shape matches `IPAddr`
    //      directly, so the case-insensitive walk has to
    //      catch it)
    //   4. unknown paths still error
    //   5. dedup semantics — second require returns false
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver_path = tmp.join("require_rb_existing_const_driver.rb");
    fs::write(&driver_path,
        r#"
module Rack; end
class IPAddr; end

r1 = require "rack"
r2 = require "rack"
r3 = require "rack/show_exceptions"
r4 = require "ipaddr"

begin
  require "definitely_not_a_real_module_xyz_abc_999"
  reject = "loaded-unexpectedly"
rescue RuntimeError => e
  reject = "errored: #{e.class}"
end

puts "rack-first=#{r1}"
puts "rack-second=#{r2}"
puts "rack-subpath=#{r3}"
puts "ipaddr-canonical=#{r4}"
puts reject
"#
    ).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr
    );
    let expected = "\
rack-first=true
rack-second=false
rack-subpath=true
ipaddr-canonical=true
errored: RuntimeError
";
    assert_eq!(
        stdout, expected,
        "fallback behavior mismatch.\nfull stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
}

#[test]
fn require_satisfied_by_all_caps_constant() {
    // Pins the upper-of-input probe in
    // `require_satisfied_by_existing_constant`.
    //
    // The original `require_satisfied_by_pre_registered_module_no_ops`
    // test exercised only the camel path (`rack` → `Rack`) and
    // the case-insensitive walk (`ipaddr` → `IPAddr`); the
    // upper-of-input shape (`json` → `JSON`, `uri` → `URI`)
    // wasn't pinned. This test defines a constant under an
    // ALL-CAPS name and requires its lowercase form — with the
    // camelized form `Foo` deliberately ABSENT — so the require
    // can only succeed via the upper probe or the
    // case-insensitive walk.
    //
    // Note: the case-insensitive walk would also catch
    // `FOO`/`foo`, so this test pins the union behavior rather
    // than the upper-of-input branch in isolation. That's an
    // acceptable contract: what matters end-to-end is "this
    // requires returns true", and either probe being broken
    // alone would only be caught if the OTHER also fails to
    // cover the case. To isolate the upper-of-input branch
    // would require white-box testing the helper directly;
    // pinning the integration-level contract here matches the
    // rest of this file's style.
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver_path = tmp.join("require_rb_all_caps_driver.rb");
    fs::write(&driver_path,
        r#"
# Define ONLY the all-caps form — no camelized Foo.
module FOO; end

r1 = require "foo"
r2 = require "foo/subpath"

puts "foo-allcaps=#{r1}"
puts "foo-subpath=#{r2}"
"#
    ).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr
    );
    assert_eq!(
        stdout, "foo-allcaps=true\nfoo-subpath=true\n",
        "all-caps fallback mismatch.\nfull stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
}

#[test]
fn require_rejects_leading_underscore_path() {
    // Pins the leading-underscore guard added per Copilot
    // review on PR #135. `snake_to_camel_case("_rack")` would
    // otherwise return `"Rack"` (the empty segment before the
    // first `_` contributes nothing) and over-match — so
    // `require "_rack"` would silently succeed against a
    // host-registered `Rack`. The guard rejects any first-seg
    // char that isn't ASCII alphabetic so `_rack` falls through
    // to cext_require, matching the diagnostic shape any other
    // unrecoverable require produces.
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver_path = tmp.join("require_rb_underscore_driver.rb");
    fs::write(&driver_path,
        r#"
module Rack; end

begin
  require "_rack"
  puts "leaked-true"
rescue RuntimeError => e
  puts "rejected: #{e.class}"
end
"#
    ).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(), "rejected: RuntimeError",
        "leading-underscore guard mismatch.\nfull stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
}

#[test]
fn require_rejects_path_traversal_in_subsegments() {
    // Pins the per-segment validation added per Copilot review
    // on PR #135. Without it, `require "rack/../missing"` would
    // see `first_seg == "rack"` (which matches the pre-defined
    // `module Rack`) and silently no-op against the namespace
    // constant, bypassing the filesystem-shaped failure path.
    // The guard now rejects any path containing empty, `.`, or
    // `..` segments before consulting the constant table.
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver_path = tmp.join("require_rb_traversal_driver.rb");
    fs::write(&driver_path,
        r#"
module Rack; end

[
  "rack/../missing",   # parent-traversal mid-path
  "rack/./foo",        # current-dir mid-path
  "rack//empty",       # empty mid-segment
].each do |p|
  begin
    require p
    puts "leaked: #{p}"
  rescue RuntimeError => e
    puts "rejected: #{p}"
  end
end
"#
    ).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr
    );
    let expected = "\
rejected: rack/../missing
rejected: rack/./foo
rejected: rack//empty
";
    assert_eq!(
        stdout, expected,
        "path-traversal guard mismatch.\nfull stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
}

#[test]
fn require_does_not_match_core_preamble_classes() {
    // Pins the core-class blocklist added per Copilot review on
    // PR #135. `self.classes` always contains `String`, `Array`,
    // `Hash`, `Integer`, etc. from the preamble — without this
    // blocklist, `require "string"` / `require "array"` would
    // silently succeed via the case-insensitive walk (or via
    // the upper-of-input probe for `require "STRING"`), masking
    // a genuinely missing dependency. The blocklist ensures
    // these paths fall through to cext_require with the standard
    // "cannot find" diagnostic.
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver_path = tmp.join("require_rb_core_class_block_driver.rb");
    fs::write(&driver_path,
        r#"
# No user-defined modules here — the preamble's String / Array /
# Hash / Integer / Object / Exception are the ONLY classes by
# these names. require must NOT short-circuit on them.
[
  "string", "array", "hash", "integer", "object", "exception",
].each do |p|
  begin
    require p
    puts "leaked: #{p}"
  rescue RuntimeError => e
    puts "rejected: #{p}"
  end
end
"#
    ).unwrap();

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver_path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr
    );
    let expected = "\
rejected: string
rejected: array
rejected: hash
rejected: integer
rejected: object
rejected: exception
";
    assert_eq!(
        stdout, expected,
        "core-class blocklist mismatch.\nfull stdout:\n{}\nstderr:\n{}",
        stdout, stderr,
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
