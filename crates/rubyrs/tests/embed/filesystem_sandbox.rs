//! `Config::allow_filesystem_io` sandbox tests. The secure-by-
//! default capability gate: when `false`, every script-callable
//! path that touches the host filesystem traps with `IOError` /
//! `LoadError` instead of executing the syscall. When `true`,
//! the embed API behaves like the CLI binary (FS-open).
//!
//! Closes FUZZING.md #1 — the last cargo-fuzz-tier future-work
//! item. With this cap, the fuzz harness's cwd-tempdir trick is
//! defense-in-depth rather than the sole sandbox layer.

use rubyrs::{Config, RubyError, Runtime};

/// Shared assertion shape: evaluate `code` on a default Runtime
/// (sandbox-on via `Config::default`), expect it to trap, and
/// verify the trap is `Uncaught { class_name == class, message
/// contains msg_substr }`.
///
/// The default-deny tests share this scaffolding ~14 times.
/// Hoisting it gives each test a one-line body and produces
/// better failure messages than a raw `{:?}` dump.
fn assert_blocked(code: &str, class: &str, msg_substr: &str) {
    let mut rt = Runtime::new();
    let trap = match rt.eval(code, "test.rb") {
        Ok(v) => panic!("expected trap for {code:?}, got Ok({v:?})"),
        Err(t) => t,
    };
    match &trap.err {
        RubyError::Uncaught { class_name, message }
            if class_name == class && message.contains(msg_substr) => {}
        other => panic!(
            "expected Uncaught {{ class_name == {class:?}, message contains {msg_substr:?} }} \
             for {code:?}, got {other:?}",
        ),
    }
}

// ---------- Default-deny (allow_filesystem_io: false) ----------

#[test]
fn default_runtime_blocks_file_read() {
    // `Runtime::new()` goes through `Config::default()`, which
    // sets `allow_filesystem_io: false`. `File.read(path)` of
    // any path must trap before the syscall. The trap that
    // escapes the unrescued eval is wrapped into
    // `Uncaught { class_name: "IOError", .. }` at the dispatch
    // boundary — see step.rs's trap-to-Uncaught conversion.
    assert_blocked(r#"File.read("/etc/passwd")"#, "IOError", "File.read blocked");
}

#[test]
fn sandbox_gate_runs_before_arg_type_check() {
    // Wrong-type arg under sandbox traps with IOError, not
    // TypeError — sandbox cap is the first gate, matching
    // require/require_relative/cext_require ordering. A script
    // probing whether a method is gated by passing wrong-typed
    // args (a small information disclosure) gets IOError too.
    assert_blocked(r#"File.read(123)"#, "IOError", "File.read blocked");
    assert_blocked(r#"File.exist?(:sym)"#, "IOError", "File.exist? blocked");
}

#[test]
fn default_runtime_blocks_file_write() {
    assert_blocked(
        r#"File.write("/tmp/sandbox-leak.txt", "x")"#,
        "IOError",
        "File.write blocked",
    );
}

#[test]
fn default_runtime_blocks_file_exist_probe() {
    // Even READ-ONLY metadata probes leak FS structure — a
    // sandbox-bypass attacker walking `File.exist?("/etc/passwd")`,
    // `File.exist?("/var/log/wtmp")`, ... can map host layout
    // without ever reading content. The three names (exist?,
    // exists? deprecated alias, file?) go through the same
    // dispatch arm and must all trap.
    assert_blocked(r#"File.exist?("/etc/passwd")"#, "IOError", "File.exist? blocked");
    assert_blocked(r#"File.exists?("/etc/passwd")"#, "IOError", "File.exists? blocked");
    assert_blocked(r#"File.file?("/etc/passwd")"#, "IOError", "File.file? blocked");
}

#[test]
fn default_runtime_blocks_file_directory_and_size() {
    assert_blocked(r#"File.directory?("/etc")"#, "IOError", "File.directory? blocked");
    assert_blocked(r#"File.size("/etc/passwd")"#, "IOError", "File.size blocked");
}

#[test]
fn default_runtime_blocks_require() {
    // `require` traps with `LoadError` (matches CRuby's
    // require-failure exception class) so scripts using
    // `rescue LoadError` catch the sandbox trap. The name
    // "some-gem" is intentionally NOT in `is_stdlib_stub_name`
    // — its absence forces the dispatch to fall through to the
    // cext_require fallback where `check_load_allowed` fires.
    assert_blocked(r#"require "some-gem""#, "LoadError", "require blocked");
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn stdlib_stub_require_works_under_sandbox() {
    // ADR 0017's Tier 1 lenient stub: `require 'uri'` materialises
    // a constant shell (so `defined?(URI)` reports "constant") but
    // doesn't touch the FS. The sandbox should let this through —
    // the gate is fine-grained, only blocking branches that
    // actually reach disk. Pre-fix, `check_load_allowed` ran at the
    // dispatch entry, blocking this in-process path collaterally.
    let mut rt = Runtime::new();
    let v = rt.eval(r#"require "uri""#, "test.rb").unwrap();
    // First load returns true; subsequent loads return false
    // (CRuby's loaded-features dedup semantics).
    assert!(matches!(v, rubyrs::Value::Bool(true)));
    let v = rt.eval(r#"require "uri""#, "test.rb").unwrap();
    assert!(matches!(v, rubyrs::Value::Bool(false)));
    // Constant shell materialised.
    let v = rt.eval(r#"defined?(URI)"#, "test.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"constant"));
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn default_runtime_blocks_require_relative() {
    assert_blocked(
        r#"require_relative "lib/foo""#,
        "LoadError",
        "require_relative blocked",
    );
}

// ---------- Lexical-only paths (NOT gated) ----------

#[test]
fn pure_lexical_path_methods_work_under_sandbox() {
    // `File.basename` / `File.dirname` / `File.extname` are
    // pure string manipulation. They take a path-shaped string
    // and return a path-shaped string with NO syscall. A
    // sandboxed script doing `File.basename("/etc/passwd")` is
    // querying the filename suffix, not reading the file —
    // gating these adds no security and breaks legitimate
    // string-manipulation use.
    let mut rt = Runtime::new();
    let v = rt.eval(r#"File.basename("/etc/passwd")"#, "t.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"passwd"));

    let v = rt.eval(r#"File.dirname("/etc/passwd")"#, "t.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"/etc"));

    let v = rt.eval(r#"File.extname("foo.rb")"#, "t.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b".rb"));
}

#[test]
fn file_expand_path_returns_lexical_form_under_sandbox() {
    // `File.expand_path` under the sandbox: skips both the
    // `canonicalize` syscall and the cwd-leak. With explicit
    // base, returns the lexically resolved path.
    let mut rt = Runtime::new();
    let v = rt
        .eval(r#"File.expand_path("foo.rb", "/tmp/proj")"#, "t.rb")
        .unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"/tmp/proj/foo.rb"));

    // Without a base arg, sandboxed expand_path uses `/` as the
    // safe sentinel (no cwd leak). The lexical resolver joins
    // it with the relative path, yielding an absolute string —
    // matches CRuby's "expand_path always returns absolute"
    // contract that gem code relies on (`$LOAD_PATH.unshift
    // File.expand_path('lib', __dir__)`, etc.).
    let v = rt.eval(r#"File.expand_path("foo.rb")"#, "t.rb").unwrap();
    assert!(
        matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"/foo.rb"),
        "expected '/foo.rb' (absolute via `/` sentinel base), got {:?}",
        v,
    );
}

#[test]
fn dunder_dir_returns_lexical_parent_under_sandbox() {
    // `__dir__` skips canonicalize when sandboxed; returns
    // `Path::parent` of the source filename lexically. The
    // existing canonicalize-fail fallback already had this
    // behaviour for non-existent files, so script code that
    // does `$LOAD_PATH.unshift __dir__` keeps working.
    let mut rt = Runtime::new();
    let v = rt.eval(r#"__dir__"#, "/abs/proj/script.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"/abs/proj"));
}

#[test]
fn dunder_dir_returns_dot_for_relative_source_under_sandbox() {
    // Embed test setups often pass relative filenames to
    // `rt.eval(source, "test.rb")`. `Path::new("test.rb").parent()`
    // returns `Some("")` (empty PathBuf, NOT None), so a bare
    // unwrap_or wouldn't fall back to ".". Guarded with a
    // `.filter(|s| !s.is_empty())` so the empty-parent case
    // collapses to "." — matches what a script doing
    // `$LOAD_PATH.unshift __dir__` would expect.
    let mut rt = Runtime::new();
    let v = rt.eval(r#"__dir__"#, "test.rb").unwrap();
    assert!(
        matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"."),
        "expected '.' for empty-parent case, got {:?}",
        v,
    );
}

// ---------- Opt-in (allow_filesystem_io: true) ----------

#[test]
fn opt_in_runtime_permits_file_read_and_write() {
    // The CLI binary path: explicit `allow_filesystem_io: true`
    // makes File.* class methods reach the filesystem.
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        ..Default::default()
    });
    // Use the env-provided temp dir to avoid /tmp racing with
    // other tests.
    let tmp = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fs-sandbox-opt-in-{}.txt", std::process::id()));
    let tmp_str = tmp.to_string_lossy().into_owned();
    // write
    rt.eval(
        &format!(r#"File.write({tmp_str:?}, "hello sandbox")"#),
        "write.rb",
    )
    .unwrap();
    // read it back
    let v = rt
        .eval(&format!(r#"File.read({tmp_str:?})"#), "read.rb")
        .unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"hello sandbox"));
    // cleanup
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn apply_config_mid_life_can_lock_down_filesystem() {
    // Embedder pattern: open Runtime for trusted setup, then
    // tighten before processing untrusted input. `apply_config`
    // flipping `allow_filesystem_io: true → false` takes effect
    // on the next eval.
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        ..Default::default()
    });
    // Setup eval that needs FS — works under the loose config.
    let tmp = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fs-sandbox-tighten-{}.txt", std::process::id()));
    let tmp_str = tmp.to_string_lossy().into_owned();
    rt.eval(
        &format!(r#"File.write({tmp_str:?}, "setup")"#),
        "setup.rb",
    )
    .unwrap();
    // Tighten — apply_config fully overwrites, so explicit
    // false is needed (Default's false would also work).
    rt.apply_config(Config {
        allow_filesystem_io: false,
        ..Default::default()
    });
    // Same workload now traps.
    let err = rt
        .eval(&format!(r#"File.read({tmp_str:?})"#), "read.rb")
        .unwrap_err();
    assert!(matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "IOError"));
    // cleanup
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn ioerror_is_rescuable_in_script() {
    // The trap uses `class_name: "IOError"`, which means
    // script-level `rescue IOError => e` catches it like any
    // other Ruby exception. Locks the contract that sandbox
    // traps behave like real Ruby errors at the script layer,
    // not opaque embed-level failures.
    let mut rt = Runtime::new();
    let v = rt
        .eval(
            r#"
            result = begin
              File.read("/etc/passwd")
              "unexpectedly read it"
            rescue IOError => e
              "caught"
            end
            result
            "#,
            "rescue.rb",
        )
        .unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"caught"));
}
