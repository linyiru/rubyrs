//! Lock the Tilt shim that the lenient `require "tilt"` path
//! installs (kernel.rs → `always_on_stub_extras` → vendored
//! `tilt_shim.rb`). Sinatra 4 has `require 'tilt'` near the top
//! of `sinatra/base.rb`, but the only Tilt method it ever calls
//! during request handling is
//! `Tilt.default_mapping.extensions_for(engine)` from inside
//! `Sinatra::Base#find_template` (private, view-render-only).
//!
//! The shim therefore only needs to:
//!   (1) make `require "tilt"` succeed (Sinatra's load-time hop),
//!   (2) expose `Tilt.default_mapping.extensions_for(engine)`
//!       returning an iterable (we use a frozen empty Array), and
//!   (3) preserve the module/object identity across re-requires —
//!       same idempotency contract the URI parser shim established
//!       (PR #373) so anything that caches `Tilt.default_mapping`
//!       at module-load time doesn't see it swapped out later.
//!
//! Real template rendering (`Tilt[engine].new(...)`) remains
//! unimplemented — calls get NoMethodError, the right
//! "feature absent" signal per ADR 0017. A hello-world Sinatra
//! app that just answers `get '/' do "hi" end` doesn't need
//! any of that surface.

use std::path::PathBuf;
use std::process::Command;
use std::fs;

fn run(script: &str, name: &str) -> String {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = tmp.join(name);
    fs::write(&path, script).unwrap();
    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
    stdout
}

#[test]
fn require_tilt_defines_module_with_default_mapping() {
    // Pins (1) + (2): `require "tilt"` succeeds and exposes the
    // exact attribute Sinatra reaches for during view-render
    // (`Tilt.default_mapping.extensions_for(_)`). Without this
    // line resolving, `Sinatra::Base#find_template` would
    // NoMethodError as soon as a route asked Sinatra to render
    // anything — even a string response triggers Sinatra's
    // route-dispatch path that constructs `find_template`
    // closures, so the surface has to exist whether or not the
    // app actually uses templates.
    let out = run(
r#"
require "tilt"
puts "tilt-defined: #{defined?(Tilt)}"
puts "tilt-class: #{Tilt.class}"
puts "default-mapping-class: #{Tilt.default_mapping.class.name}"
puts "extensions-for-erb: #{Tilt.default_mapping.extensions_for(:erb).inspect}"
puts "extensions-for-unknown: #{Tilt.default_mapping.extensions_for(:nosuch).inspect}"
"#,
        "tilt_shim_basic_driver.rb",
    );
    assert_eq!(
        out,
        "tilt-defined: constant\n\
         tilt-class: Module\n\
         default-mapping-class: Tilt::EmptyMapping\n\
         extensions-for-erb: []\n\
         extensions-for-unknown: []\n",
        "Tilt shim load-time surface broken — Sinatra base.rb \
         require chain will not complete.\nstdout:\n{}",
        out,
    );
}

#[test]
fn tilt_default_mapping_preserves_identity_across_requires() {
    // Pins (3): a second `require "tilt"` must NOT replace
    // `Tilt.default_mapping` with a fresh `EmptyMapping`. Code
    // that caches the mapping at module-load (rare for tilt
    // specifically, but the shim's idempotency contract is the
    // same shape as the URI parser one — and a future Sinatra
    // version could grow to cache `Tilt.default_mapping` in a
    // class-level ivar at extend-time). The `unless
    // defined?(Tilt)` guard in tilt_shim.rb is what keeps the
    // instance stable.
    //
    // Also exercises the mapping's CLASS identity: a second
    // require shouldn't redefine the EmptyMapping class either —
    // otherwise `default_mapping.is_a?(...)` would split across
    // the two class objects. Use `.class` on the instance
    // instead of reaching into `Tilt::EmptyMapping` directly:
    // the shim declares EmptyMapping as `private_constant`, and
    // while rubyrs's current `private_constant` is a no-op stub,
    // a future implementation that honours it would make a
    // direct `Tilt::EmptyMapping` access raise NameError and
    // mask the contract this test is actually trying to lock.
    let out = run(
r#"
require "tilt"
first_mapping = Tilt.default_mapping
first_mapping_class = first_mapping.class

require "tilt"  # second require should be a no-op
puts "mapping-eq: #{first_mapping.equal?(Tilt.default_mapping)}"
puts "mapping-class-eq: #{first_mapping_class.equal?(Tilt.default_mapping.class)}"
"#,
        "tilt_shim_idempotency_driver.rb",
    );
    assert_eq!(
        out,
        "mapping-eq: true\nmapping-class-eq: true\n",
        "Tilt shim re-evaluated on second require — caches of \
         `Tilt.default_mapping` would silently diverge.\nstdout:\n{}",
        out,
    );
}

#[test]
fn tilt_engine_lookup_surface_returns_feature_absent_signal() {
    // Documents the deliberate ADR 0017 gap: real template
    // rendering surfaces (`Tilt[engine]` / `Tilt.register` /
    // `Tilt.new`) are NOT provided. A hello-world Sinatra app
    // doesn't need them. Pins the "feature absent" signal —
    // NoMethodError — so a future "add a half-implementation
    // that returns nil" change doesn't silently break the
    // diagnostic shape (which scripts may pattern-match on).
    let out = run(
r#"
require "tilt"
begin
  Tilt[:erb]
  puts "leaked: succeeded"
rescue NoMethodError => e
  puts "blocked: NoMethodError"
end
"#,
        "tilt_shim_engine_lookup_driver.rb",
    );
    assert_eq!(
        out, "blocked: NoMethodError\n",
        "Tilt engine-lookup surface diverged from \
         feature-absent signal.\nstdout:\n{}",
        out,
    );
}
