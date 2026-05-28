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

// ---------- allowlist scope (allow_filesystem_io: true + allowed_paths: Some) ----------

/// Build a tempdir + write a probe file under it, returning the
/// canonicalized tempdir path and the probe path. The tempdir is
/// canonicalized because `apply_config` canonicalizes
/// `allowed_paths` entries — using a non-canonical prefix would
/// silently slip past `starts_with` on macOS where `/tmp` is a
/// symlink to `/private/tmp`.
fn alloc_tempdir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let raw = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rubyrs-allowlist-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&raw).expect("mkdir tempdir");
    let dir = std::fs::canonicalize(&raw).expect("canonicalize tempdir");
    let probe = dir.join("probe.txt");
    std::fs::write(&probe, "probe contents").expect("write probe");
    (dir, probe)
}

#[test]
fn allowlist_permits_file_read_inside_prefix() {
    // `allow_filesystem_io: true` + `allowed_paths: Some([gem_root])`
    // is the rubund use case: open FS, but constrained to one
    // directory tree. A `File.read` inside the prefix succeeds.
    let (dir, probe) = alloc_tempdir("read-inside");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let script = format!(r#"File.read({:?})"#, probe.to_string_lossy());
    let v = rt.eval(&script, "test.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"probe contents"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowlist_blocks_file_read_outside_prefix() {
    // Same Runtime config, but the script tries to read a path
    // OUTSIDE the allowed prefix. Must trap with IOError — the
    // host's gem-root sandbox can't be escaped by passing a
    // different absolute path.
    let (dir, _probe) = alloc_tempdir("read-outside");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let err = rt
        .eval(r#"File.read("/etc/passwd")"#, "test.rb")
        .unwrap_err();
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, message }
            if class_name == "IOError" && message.contains("outside Config::allowed_paths")),
        "expected Uncaught/IOError outside-allowlist, got {:?}",
        err.err,
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowlist_blocks_traversal_out_of_prefix() {
    // Defense against the obvious bypass: `File.read("/allowed/../etc/passwd")`.
    // The path is lexically resolved BEFORE the `starts_with`
    // check, so `..` is collapsed and the resolved path
    // `/etc/passwd` doesn't start with the prefix.
    let (dir, _probe) = alloc_tempdir("read-traversal");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let traversal = format!("{}/../../../etc/passwd", dir.to_string_lossy());
    let script = format!(r#"File.read({:?})"#, traversal);
    let err = rt.eval(&script, "test.rb").unwrap_err();
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "IOError"),
        "expected IOError on traversal, got {:?}",
        err.err,
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowlist_permits_file_metadata_probe_inside_prefix() {
    // `File.exist?` / `.size` are gated too — verify allowlist
    // mode lets them through when the path is inside the prefix.
    let (dir, probe) = alloc_tempdir("metadata-inside");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let script = format!(r#"File.exist?({:?})"#, probe.to_string_lossy());
    let v = rt.eval(&script, "test.rb").unwrap();
    assert!(matches!(v, rubyrs::Value::Bool(true)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowlist_blocks_file_metadata_probe_outside_prefix() {
    // The metadata-probe path also leaks FS structure if
    // unguarded. Verify the allowlist scope applies.
    let (dir, _probe) = alloc_tempdir("metadata-outside");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let err = rt
        .eval(r#"File.exist?("/etc/passwd")"#, "test.rb")
        .unwrap_err();
    assert!(matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "IOError"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allow_filesystem_io_false_overrides_allowlist() {
    // Layered model: bool is the coarse gate. If `allow_filesystem_io:
    // false`, `allowed_paths` is ignored — sandbox is completely
    // shut, even paths "inside" a configured allowlist trap. Locks
    // in that hosts can't use `allowed_paths` to ACCIDENTALLY
    // re-open a sandbox they meant to keep closed.
    let (dir, probe) = alloc_tempdir("bool-wins");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: false,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let script = format!(r#"File.read({:?})"#, probe.to_string_lossy());
    let err = rt.eval(&script, "test.rb").unwrap_err();
    // Trap message says "filesystem I/O disabled" (the bool
    // gate's wording), NOT "outside Config::allowed_paths" (the
    // scope gate's wording) — proves bool fired first.
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, message }
            if class_name == "IOError"
            && message.contains("filesystem I/O disabled")
            && !message.contains("outside")),
        "expected bool-gate trap, got {:?}",
        err.err,
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowlist_none_is_full_open() {
    // Regression: with `allow_filesystem_io: true, allowed_paths:
    // None`, behaviour should match the bool-only mode that
    // shipped in PR #257 — no narrowing. The CLI binary uses
    // exactly this config.
    let (dir, probe) = alloc_tempdir("none-open");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: None,
        ..Default::default()
    });
    // Reading inside any path works (the existing opt-in test
    // covers a tempdir read; here we use the probe just to keep
    // the test self-contained).
    let script = format!(r#"File.read({:?})"#, probe.to_string_lossy());
    let v = rt.eval(&script, "test.rb").unwrap();
    assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"probe contents"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowlist_with_multiple_prefixes() {
    // `allowed_paths` is a Vec — passes when ANY prefix matches.
    // Use case: a host that wants to allow access to two
    // unrelated trees (e.g., gem root + vendor cache).
    let (dir_a, probe_a) = alloc_tempdir("multi-a");
    let (dir_b, probe_b) = alloc_tempdir("multi-b");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir_a.clone(), dir_b.clone()]),
        ..Default::default()
    });
    // Both probes readable.
    for probe in [&probe_a, &probe_b] {
        let script = format!(r#"File.read({:?})"#, probe.to_string_lossy());
        let v = rt.eval(&script, "test.rb").unwrap();
        assert!(matches!(&v, rubyrs::Value::Str(s) if &*s.borrow() == b"probe contents"));
    }
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn allowlist_permits_require_inside_prefix() {
    // `require <absolute-path>` resolves through ruby_source_candidates →
    // canonicalize → my new `check_load_allowed("require", Some(canon))`.
    // Inside the allowlist prefix → load proceeds.
    let (dir, _probe) = alloc_tempdir("require-inside");
    let lib_path = dir.join("hello_lib.rb");
    std::fs::write(&lib_path, "HELLO_LIB_LOADED = true").expect("write lib");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let script = format!(
        r#"require {:?}; HELLO_LIB_LOADED"#,
        lib_path.with_extension("").to_string_lossy()
    );
    let v = rt.eval(&script, "test.rb").unwrap();
    assert!(matches!(v, rubyrs::Value::Bool(true)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn allowlist_blocks_require_outside_prefix() {
    // Plant a real .rb file outside the allowlist prefix. Sandbox
    // is open (bool=true), but `allowed_paths` restricts to
    // `prefix_dir`. `require` must trap LoadError because canon
    // resolves outside that prefix.
    let (prefix_dir, _) = alloc_tempdir("require-outside-allowed");
    let (sibling_dir, _) = alloc_tempdir("require-outside-target");
    let outside_lib = sibling_dir.join("evil_lib.rb");
    std::fs::write(&outside_lib, "EVIL_LOADED = true").expect("write evil");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![prefix_dir.clone()]),
        ..Default::default()
    });
    let script = format!(
        r#"require {:?}"#,
        outside_lib.with_extension("").to_string_lossy()
    );
    let err = rt.eval(&script, "test.rb").unwrap_err();
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, message }
            if class_name == "LoadError"
            && message.contains("outside Config::allowed_paths")),
        "expected LoadError outside-allowlist, got {:?}",
        err.err,
    );
    let _ = std::fs::remove_dir_all(&prefix_dir);
    let _ = std::fs::remove_dir_all(&sibling_dir);
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn allowlist_permits_require_relative_inside_prefix() {
    // `require_relative` anchors to the eval'd source file's
    // directory. By passing the source filename as a path INSIDE
    // the allowlist tempdir, require_relative resolves siblings
    // there. Verify the canonicalize-then-scope path succeeds.
    let (dir, _) = alloc_tempdir("require-relative-inside");
    let lib_path = dir.join("rel_target.rb");
    std::fs::write(&lib_path, "REL_TARGET_LOADED = true").expect("write target");
    // The script is "called from" caller.rb inside dir, so
    // require_relative "rel_target" → dir/rel_target.rb.
    let caller_path = dir.join("caller.rb");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let v = rt
        .eval(
            r#"require_relative "rel_target"; REL_TARGET_LOADED"#,
            caller_path.to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(v, rubyrs::Value::Bool(true)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn allowlist_blocks_require_relative_traversal() {
    // require_relative with `..` traversal that escapes the
    // allowlist prefix. Canonicalize resolves the `..` to an
    // absolute path outside the prefix, so the scope gate must
    // trap LoadError. The target file has to exist for canon to
    // succeed (otherwise we'd hit the "cannot find" trap first
    // and never reach the scope gate — the gate only triggers on
    // a real escape attempt to an existing file).
    let (dir, _) = alloc_tempdir("require-relative-traversal-allowed");
    let (sibling_dir, _) = alloc_tempdir("require-relative-traversal-target");
    let outside = sibling_dir.join("escape.rb");
    std::fs::write(&outside, "ESCAPED = true").expect("write escape");
    let caller_path = dir.join("caller.rb");
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    // From dir/caller.rb, `../<sibling>/escape` walks out of dir.
    let sibling_name = sibling_dir.file_name().unwrap().to_string_lossy();
    let script = format!(
        r#"require_relative "../{}/escape""#,
        sibling_name
    );
    let err = rt.eval(&script, caller_path.to_str().unwrap()).unwrap_err();
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, message }
            if class_name == "LoadError"
            && message.contains("outside Config::allowed_paths")),
        "expected LoadError on traversal, got {:?}",
        err.err,
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&sibling_dir);
}

#[test]
#[cfg(all(not(target_os = "wasi"), unix))]
fn allowlist_blocks_cext_via_symlink_target() {
    // Defends the symlink-tight contract on the load family. Place
    // a (placeholder) .dylib at /allowed/inner.{so,dylib} as a
    // SYMLINK pointing to /sibling/real.{so,dylib} which is
    // OUTSIDE the allowlist prefix. cext_require's canonicalize
    // resolves the symlink to /sibling/real.* — outside scope —
    // and the post-canonicalize check_load_allowed traps LoadError
    // BEFORE dlopen runs. Pre-fix, the canonicalize-success path
    // already caught this, but the falsely-falling-back path
    // (`unwrap_or_else(|_| so_path.clone())`) would have let the
    // gate accept the in-scope symlink path and `Library::new`
    // would have followed it. The new hard-trap-on-canonicalize-
    // fail closes that gap.
    use std::os::unix::fs::symlink;
    let (allowed, _) = alloc_tempdir("cext-symlink-allowed");
    let (sibling, _) = alloc_tempdir("cext-symlink-target");
    // The file extension cext_require auto-probes. We pick `.so`
    // because it's the lookup target on Linux and a benign fallback
    // on macOS (macOS tries .dylib/.bundle first; the test still
    // exercises the scope gate either way because the require uses
    // an explicit extension).
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let target = sibling.join(format!("real.{ext}"));
    // Not a valid C ext — just bytes. dlopen would fail, but the
    // scope gate must fire first.
    std::fs::write(&target, b"placeholder").expect("write placeholder");
    let link = allowed.join(format!("inner.{ext}"));
    symlink(&target, &link).expect("symlink");

    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![allowed.clone()]),
        ..Default::default()
    });
    let script = format!(r#"require {:?}"#, link.with_extension("").to_string_lossy());
    let err = rt.eval(&script, "test.rb").unwrap_err();
    // Either:
    //   - cext feature on  → scope gate traps "outside Config::allowed_paths"
    //   - cext feature off → "built without cext feature" / "no .rb at"
    // Both classes are LoadError; we assert the gate triggered or the
    // require flow stopped before any dlopen. The critical contract
    // is that we never reach dlopen on the un-resolved symlink path.
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "LoadError"),
        "expected LoadError, got {:?}",
        err.err,
    );
    // When the cext feature is on the gate-specific message must
    // appear — proves canonicalize-then-scope ran (not the old
    // silent fallback).
    if cfg!(feature = "cext") {
        let RubyError::Uncaught { message, .. } = &err.err else { unreachable!() };
        assert!(
            message.contains("outside Config::allowed_paths"),
            "expected scope-gate message, got {message:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&allowed);
    let _ = std::fs::remove_dir_all(&sibling);
}

#[test]
#[should_panic(expected = "cannot be canonicalized")]
fn allowlist_panics_on_relative_prefix() {
    // A relative prefix is unusable as a sandbox boundary: per-op
    // resolution joins inputs with cwd to absolute form, so
    // `starts_with("gemroot")` against an absolute resolved input
    // is always false. Pre-fix this silently produced a dead
    // sandbox where every legitimate op trapped. Post-fix:
    // apply_config panics with a clear diagnostic so the host
    // sees the misconfig immediately.
    let _rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![std::path::PathBuf::from("gemroot")]),
        ..Default::default()
    });
}

#[test]
#[should_panic(expected = "cannot be canonicalized")]
fn allowlist_panics_on_nonexistent_traversal_prefix() {
    // A nonexistent prefix with `..` segments is the silent-widen
    // case: pre-fix, `lexically_resolve_path` collapsed `..` and
    // stored a BROADER path than the host typed (e.g.
    // `/nonexistent/x/../../etc` became `/etc`, granting access
    // to the host's /etc tree). Post-fix: apply_config refuses
    // any prefix that can't be canonicalized.
    let _rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        allowed_paths: Some(vec![std::path::PathBuf::from(
            "/nonexistent-rubyrs-test-prefix/x/../../etc",
        )]),
        ..Default::default()
    });
}
