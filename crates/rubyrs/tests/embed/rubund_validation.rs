//! CI-gated mirror of `examples/gemspec_evaluator.rs`.
//!
//! The example file is the human-readable narrative of the
//! rubund-style host shape (println!-driven walkthrough).
//! `cargo test` DOES compile example targets to verify they
//! build, but it does NOT run them — so a runtime contract
//! regression in how the four embed-API hardening features
//! compose would ship silently even if `cargo test` is green.
//! This file is the test counterpart: three #[test] functions
//! covering the same three phases, asserting via standard test
//! plumbing so failures land red in CI on every run.
//!
//! Features under composition:
//!   1. `Config::allow_filesystem_io: true`   — capability gate
//!   2. `Config::allowed_paths: Some([root])` — sandbox scope
//!   3. `Config::load_paths: Some([lib])`     — `$LOAD_PATH` seed
//!   4. `Runtime::eval` panic→Trap boundary
//!
//! Each test isolates its tempdir via the RAII pattern used in
//! `filesystem_sandbox.rs` and `gemspec_evaluator.rs` — cleanup
//! runs on every exit path including panic-unwind.

use std::path::{Path, PathBuf};

use rubyrs::{Config, RubyError, Runtime, Value};

// ---------- Per-test tempdir guard ----------

struct GemRoot {
    path: PathBuf,
}

impl Drop for GemRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Allocate an empty gem root at
/// `<CARGO_TARGET_TMPDIR>/rubund-validation-<tag>-<pid>`. Tag
/// distinguishes per-test directories so parallel test runs
/// don't collide. Uses the early-commit-then-update-path pattern
/// (PR #283 review) so any panic during init still triggers
/// cleanup via Drop.
///
/// Callers conventionally bind the first tuple element as
/// `_root_keep_alive` (the intent-bearing `_` prefix suppresses
/// the unused-variable lint while the suffix discourages a
/// 'simplification' refactor to bare `_`, which would discard
/// the `GemRoot` immediately and drop the tempdir BEFORE the
/// test runs).
///
/// Phase 1 layers the gemspec fixture on top via
/// `write_fixture(&root)`; Phase 2 only needs the root path
/// itself (for the allowlist config) so it calls this helper
/// directly; Phase 3 doesn't need a gem root at all (host-fn
/// panic-catch is config-independent) so it doesn't call here.
fn alloc_gem_root(tag: &str) -> (GemRoot, PathBuf) {
    let raw = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rubund-validation-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&raw);
    std::fs::create_dir_all(&raw).expect("mkdir gem root");
    // Commit the guard BEFORE canonicalize so a panic in
    // canonicalize / subsequent writes still cleans up.
    let mut guard = GemRoot { path: raw.clone() };
    let root = std::fs::canonicalize(&raw).expect("canonicalize gem root");
    guard.path = root.clone();
    (guard, root)
}

/// Write the Bundler-shape fixture (`lib/fakegem/version.rb` +
/// `fakegem.gemspec`) into an already-allocated gem root. Only
/// Phase 1 needs this — Phase 2's allowlist test doesn't read
/// any in-root file; Phase 3 doesn't open the root at all.
/// Splitting `alloc` from `write` makes the per-phase setup
/// honest about what each test actually depends on, so a future
/// fixture edit can't silently change a phase's effective scope.
fn write_fixture(root: &Path) {
    // Bundler layout: `lib/<gemname>/version.rb`. The Phase-1
    // gemspec requires "fakegem/version", which load_paths
    // resolves to lib/fakegem/version.rb.
    std::fs::create_dir_all(root.join("lib/fakegem")).expect("mkdir lib/fakegem");
    std::fs::write(
        root.join("lib/fakegem/version.rb"),
        r#"
module FakeGem
  VERSION = "1.2.3"
end
"#,
    )
    .expect("write version.rb");
    std::fs::write(
        root.join("fakegem.gemspec"),
        // NO inline `$LOAD_PATH.unshift` — the require MUST
        // resolve via Config::load_paths only. Mirrors the
        // fixture in examples/gemspec_evaluator.rs.
        r#"
require "fakegem/version"

class Spec
  def initialize
    @name = nil
    @version = nil
    @deps = []
  end
  def name=(n); @name = n; host_register_name(n); end
  def version=(v); @version = v; host_register_version(v); end
  def add_dependency(name, version)
    @deps << [name, version]
    host_register_dependency(name, version)
  end
end

s = Spec.new
s.name = "fakegem"
s.version = FakeGem::VERSION
s.add_dependency "rack", ">= 3.0"
s.add_dependency "puma", "~> 6.0"
"#,
    )
    .expect("write gemspec");
}

/// Shared Config shape across phases — single source of truth,
/// no drift between tests.
fn make_rt(gem_root: &Path) -> Runtime {
    Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![gem_root.to_path_buf()]),
        load_paths: Some(vec![gem_root.join("lib")]),
        ..Default::default()
    })
}

// ---------- Phase 1: scope + load_paths composition ----------

#[test]
#[cfg(not(target_os = "wasi"))]
fn phase1_gemspec_evaluates_under_scoped_sandbox() {
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default, Debug, Clone)]
    struct Captured {
        name: Option<String>,
        version: Option<String>,
        deps: Vec<(String, String)>,
    }

    let (_root_keep_alive, root) = alloc_gem_root("phase1");
    write_fixture(&root);
    let mut rt = make_rt(&root);
    let captured = Rc::new(RefCell::new(Captured::default()));

    let cap1 = captured.clone();
    rt.register_fn("host_register_name", move |args| {
        if let [Value::Str(name)] = args {
            cap1.borrow_mut().name = Some(name.to_string_lossy());
        }
        Ok(Value::Nil)
    });
    let cap2 = captured.clone();
    rt.register_fn("host_register_version", move |args| {
        if let [Value::Str(v)] = args {
            cap2.borrow_mut().version = Some(v.to_string_lossy());
        }
        Ok(Value::Nil)
    });
    let cap3 = captured.clone();
    rt.register_fn("host_register_dependency", move |args| {
        if let [Value::Str(name), Value::Str(ver)] = args {
            cap3.borrow_mut()
                .deps
                .push((name.to_string_lossy(), ver.to_string_lossy()));
        }
        Ok(Value::Nil)
    });

    let gemspec_path = root.join("fakegem.gemspec");
    let source = std::fs::read_to_string(&gemspec_path).expect("read gemspec");
    rt.eval(&source, gemspec_path.to_str().expect("utf-8 path"))
        .expect("scoped gemspec eval must succeed");

    let cap = captured.borrow().clone();
    assert_eq!(cap.name.as_deref(), Some("fakegem"));
    // load_paths-driven resolution: lib/fakegem/version.rb was
    // required successfully and the VERSION constant
    // interpolated into the gemspec's version assignment.
    assert_eq!(cap.version.as_deref(), Some("1.2.3"));
    // Exact-contents assertion locks both order and (name, ver)
    // pairs — a buggy callback swapping / duplicating would slip
    // past a bare `.len() == 2` check.
    assert_eq!(
        cap.deps,
        vec![
            ("rack".to_string(), ">= 3.0".to_string()),
            ("puma".to_string(), "~> 6.0".to_string()),
        ],
    );
}

// ---------- Phase 2: scope rejects out-of-scope reads ----------

#[test]
fn phase2_out_of_scope_read_traps_ioerror() {
    // Phase 2 only needs the gem root path for the allowlist
    // config — never reads any fixture file. Calling
    // `alloc_gem_root` (not `write_fixture`) keeps the test's
    // setup honest about its actual dependency.
    let (_root_keep_alive, root) = alloc_gem_root("phase2");
    let mut rt = make_rt(&root);
    let trap = rt
        .eval(r#"File.read("/etc/passwd")"#, "<phase2>")
        .expect_err("read of /etc/passwd MUST trap under scoped sandbox");
    let RubyError::Uncaught { class_name, message } = &trap.err else {
        panic!("expected Uncaught, got {:?}", trap.err);
    };
    assert_eq!(class_name, "IOError");
    assert!(
        message.contains("outside Config::allowed_paths"),
        "expected scope-gate message, got {message:?}",
    );
}

// ---------- Phase 3: panic→Trap + post-panic reusability ----------

#[test]
fn phase3_host_fn_panic_becomes_runtime_error_trap() {
    // Phase 3 uses the secure-by-default `Runtime::new()` — not
    // `make_rt`. The panic→Trap contract is a baseline Runtime
    // feature (PR #279), independent of any sandbox config; if
    // we used the rubund-shape config here, a future regression
    // that let a DIFFERENT panic site fire (e.g. during require-
    // walk if Phase 3 ever ate a require statement) would still
    // be caught and the assertion would still pass with the
    // payload string preserved through unwind — locking in the
    // weaker contract "any Runtime catches host_fn panics" by
    // accident. Minimal config makes the assertion mean what
    // its docstring says.
    let mut rt = Runtime::new();
    rt.register_fn("explode", |_| panic!("simulated host-fn bug"));
    let trap = rt
        .eval(r#"explode"#, "<phase3>")
        .expect_err("panicking host_fn MUST convert to Trap, not crash");
    let RubyError::RuntimeError { msg } = &trap.err else {
        panic!("expected RuntimeError, got {:?}", trap.err);
    };
    assert!(
        msg.contains("host-side panic during eval"),
        "expected panic-trap prefix, got {msg:?}",
    );
    assert!(
        msg.contains("simulated host-fn bug"),
        "expected original payload preserved, got {msg:?}",
    );
    // Runtime must remain usable after the catch — the per-eval
    // cleanup contract from PR #279.
    let v = rt
        .eval(r#"1 + 2"#, "<phase3-post>")
        .expect("post-panic eval MUST succeed");
    assert!(matches!(v, Value::Int(3)));
}
