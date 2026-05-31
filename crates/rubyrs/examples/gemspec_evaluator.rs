//! End-to-end rubund-style gemspec evaluator host.
//!
//! Demonstrates the four embed-API hardening features composing
//! into a realistic Bundler-shape host:
//!
//! 1. `Config::allow_filesystem_io: true`   — the gemspec needs
//!    to `require "fakegem/version"`, so the capability is on.
//! 2. `Config::allowed_paths: Some([gem_root])` — but scoped to
//!    the gem root; attempting to read `/etc/passwd` (or anything
//!    outside) traps with `IOError`.
//! 3. `Config::load_paths: Some([gem_root.join("lib")])` —
//!    declarative `$LOAD_PATH` seed so `require "fakegem/version"`
//!    resolves `lib/fakegem/version.rb` (the Bundler convention).
//!    No synthetic `$LOAD_PATH.unshift` as the first eval.
//! 4. `Runtime::eval` panic→Trap boundary — defensive net for any
//!    Rust panic in a host-fn callback (registered via
//!    `register_fn`).
//!
//! What this example simulates: rubund's gemspec evaluator reading
//! a real gem's `.gemspec` file to extract metadata
//! (name / version / dependencies). The host CAN'T just regex the
//! gemspec because gemspecs are Ruby code — they can call methods,
//! interpolate from constants, do conditional version bumps, etc.
//! So the host evals it under a tight sandbox, captures the
//! resulting `Gem::Specification.new` data via host_fn callbacks,
//! and rejects any escape attempts.
//!
//! Run with: `cargo run --release --example gemspec_evaluator`
//!
//! No external setup needed — the example materializes a fake
//! gem under `std::env::temp_dir()` and tears it down on exit
//! via the `GemRoot` RAII guard. (Earlier drafts tried to read
//! `CARGO_TARGET_TMPDIR` via `option_env!` for a "cargo test"
//! path, but examples never run via `cargo test` and Cargo only
//! sets that env var at runtime for integration tests anyway —
//! `option_env!` is compile-time, so the branch never fired.)

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use rubyrs::{Config, RubyError, Runtime, Value};

// ---------- Tempdir RAII guard (test-style cleanup) ----------

struct GemRoot {
    path: PathBuf,
}

impl Drop for GemRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn build_fake_gem() -> GemRoot {
    let raw = std::env::temp_dir()
        .join(format!("rubyrs-gemspec-eval-{}", std::process::id()));
    let _ = fs::remove_dir_all(&raw); // clean slate
    fs::create_dir_all(&raw).expect("mkdir gem root");
    let root = fs::canonicalize(&raw).expect("canonicalize gem root");
    // Lay out a minimal but realistic gem structure — matches the
    // Bundler convention `lib/<gemname>/version.rb` which
    // `require "fakegem/version"` resolves to:
    //   <root>/
    //     fakegem.gemspec   (the file we evaluate)
    //     lib/
    //       fakegem/
    //         version.rb   (the require'd file)
    fs::create_dir_all(root.join("lib/fakegem")).expect("mkdir lib/fakegem");
    fs::write(
        root.join("lib/fakegem/version.rb"),
        // Realistic Bundler-shape version file. Defines a constant
        // the gemspec interpolates into the version string.
        r#"
module FakeGem
  VERSION = "1.2.3"
end
"#,
    )
    .expect("write version.rb");
    fs::write(
        root.join("fakegem.gemspec"),
        // Realistic gemspec shape, simplified. host_register_*
        // callbacks capture each spec field as the script runs.
        // No `$LOAD_PATH.unshift` in the fixture — the require
        // MUST resolve via the host-supplied `Config::load_paths`
        // seed, not a script-side mutation. That's the contract
        // Phase 1 is validating; an inline unshift would mask a
        // broken seed.
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
    // Phase 2's out-of-scope read uses `/etc/passwd` — universally
    // present on the platforms this example targets, no fixture
    // needed. (An earlier draft planted a per-process file outside
    // the gem root, but its cleanup wasn't covered by GemRoot's
    // RAII guard and the file was never actually read by the
    // sandbox check — flagged in PR #302 review.)
    GemRoot { path: root }
}

// ---------- Captured gemspec metadata ----------

#[derive(Default, Debug)]
struct CapturedSpec {
    name: Option<String>,
    version: Option<String>,
    deps: Vec<(String, String)>,
}

// ---------- The host ----------

fn main() {
    println!("================================================================");
    println!("  rubund-style gemspec evaluator — embed-API hardening field test");
    println!("================================================================\n");

    let gem_root = build_fake_gem();
    let root_str = gem_root.path.to_string_lossy().into_owned();
    println!("gem root: {root_str}");
    println!("  ├─ fakegem.gemspec");
    println!("  └─ lib/");
    println!("      └─ fakegem/");
    println!("          └─ version.rb\n");

    let captured = Rc::new(RefCell::new(CapturedSpec::default()));

    // ============================================================
    // Phase 1: Evaluate the gemspec under the full sandbox.
    // ============================================================
    println!("[Phase 1] Evaluate fakegem.gemspec under the scoped sandbox");
    println!("----------------------------------------------------------------");
    {
        let mut rt = Runtime::with_config(Config {
            // Capability gate ON — the gemspec uses `require`,
            // which is a load-class FS op.
            allow_filesystem_io: true,
            // Scope: only the gem root tree. Any read outside
            // (Phase 2 below tries `/etc/passwd`) traps with
            // IOError before the syscall.
            allowed_paths: Some(vec![gem_root.path.clone()]),
            // Seed $LOAD_PATH with the gem's lib/ so
            // `require "fakegem/version"` resolves the bundled
            // file declaratively, with no synthetic
            // `$LOAD_PATH.unshift` as the first eval.
            load_paths: Some(vec![gem_root.path.join("lib")]),
            ..Default::default()
        });

        // ----------- host_fn callbacks ----------
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

        // Read the gemspec source ourselves (the host owns
        // file I/O — rubyrs doesn't see the read), then hand the
        // contents to eval. This is the rubund-style pattern: the
        // sandbox protects the SCRIPT, not the host's own reads.
        let gemspec_path = gem_root.path.join("fakegem.gemspec");
        let source = fs::read_to_string(&gemspec_path).expect("read gemspec");

        match rt.eval(&source, gemspec_path.to_str().expect("utf-8 path")) {
            Ok(_) => {
                let cap = captured.borrow();
                println!("  ✅ gemspec evaluated cleanly");
                println!("     name    = {:?}", cap.name);
                println!("     version = {:?}", cap.version);
                println!("     deps    = {:?}", cap.deps);
                assert_eq!(cap.name.as_deref(), Some("fakegem"));
                // The interpolated VERSION constant came from
                // `lib/fakegem/version.rb` — load_paths-driven
                // resolution worked.
                assert_eq!(cap.version.as_deref(), Some("1.2.3"));
                // Exact-contents assertion — a buggy host callback
                // that swapped, duplicated, or corrupted entries
                // would slip past a bare `.len() == 2` check. The
                // demo is meant to validate the full captured
                // gemspec tuple, so lock the order and the
                // (name, version) pairs.
                assert_eq!(
                    cap.deps,
                    vec![
                        ("rack".to_string(), ">= 3.0".to_string()),
                        ("puma".to_string(), "~> 6.0".to_string()),
                    ],
                );
            }
            Err(t) => {
                // Panic (not std::process::exit) so the GemRoot
                // guard's Drop runs during unwind and the
                // tempdir is cleaned up. std::process::exit
                // bypasses Drop unconditionally.
                panic!("Phase 1 unexpected trap: {}", rt.format_trap(&t));
            }
        }
    }

    // ============================================================
    // Phase 2: Confirm out-of-scope reads trap with IOError.
    // ============================================================
    println!("\n[Phase 2] Attempt out-of-scope read — must trap IOError");
    println!("----------------------------------------------------------------");
    {
        let mut rt = Runtime::with_config(Config {
            allow_filesystem_io: true,
            allowed_paths: Some(vec![gem_root.path.clone()]),
            load_paths: Some(vec![gem_root.path.join("lib")]),
            ..Default::default()
        });
        // Try to read /etc/passwd — well outside the gem root.
        let trap = rt
            .eval(r#"File.read("/etc/passwd")"#, "<phase2>")
            .expect_err("read of /etc/passwd MUST trap under scoped sandbox");
        match &trap.err {
            RubyError::Uncaught { class_name, message }
                if class_name == "IOError" && message.contains("outside Config::allowed_paths") =>
            {
                println!("  ✅ IOError raised:");
                println!("     {message}");
            }
            other => {
                // Panic to let GemRoot::drop run; see Phase 1 above.
                panic!("Phase 2 wrong trap shape: {other:?}");
            }
        }
    }

    // ============================================================
    // Phase 3: Demonstrate the panic→Trap boundary.
    // ============================================================
    println!("\n[Phase 3] Host-fn panic → RuntimeError Trap");
    println!("----------------------------------------------------------------");
    // Note on stderr noise: Rust's default panic hook prints a
    // `thread 'main' panicked at ...` line to stderr BEFORE
    // `catch_unwind` catches the unwind. The catch still works —
    // eval returns Err(Trap) as expected — but the host sees the
    // panic-hook message above the trap text. Production hosts
    // that want clean output should install a no-op
    // `std::panic::set_hook(Box::new(|_| {}))` for the duration
    // of the eval call (and restore the previous hook after).
    // Out of scope for this demo; the contract is correct.
    {
        let mut rt = Runtime::with_config(Config {
            allow_filesystem_io: true,
            allowed_paths: Some(vec![gem_root.path.clone()]),
            load_paths: Some(vec![gem_root.path.join("lib")]),
            ..Default::default()
        });
        rt.register_fn("explode", |_| {
            panic!("simulated host-fn bug");
        });
        let trap = rt
            .eval(r#"explode"#, "<phase3>")
            .expect_err("panicking host_fn MUST convert to Trap, not crash");
        match &trap.err {
            RubyError::RuntimeError { msg }
                if msg.contains("host-side panic during eval")
                    && msg.contains("simulated host-fn bug") =>
            {
                println!("  ✅ Panic converted to RuntimeError Trap:");
                println!("     {msg}");
            }
            other => {
                // Panic to let GemRoot::drop run; see Phase 1 above.
                panic!("Phase 3 wrong trap shape: {other:?}");
            }
        }

        // Critical bit: the Runtime is still usable after the
        // caught panic — Vm state was scrubbed in the catch's Err
        // arm (the per-eval cleanup contract). A long-running
        // host loop (rubund batch evaluator, _http_server request
        // handler) survives one bad gemspec without crashing.
        let v = rt
            .eval(r#"1 + 2"#, "<phase3-post>")
            .expect("post-panic eval must succeed — Runtime should be reusable");
        assert!(matches!(v, Value::Int(3)));
        println!("  ✅ Runtime remained usable after caught panic");
    }

    println!("\n================================================================");
    println!("  All four embed-API hardening contracts validated end-to-end.");
    println!("================================================================");
    // GemRoot's Drop runs at scope exit (and on panic), cleaning
    // up the tempdir. No manual cleanup needed.
}
