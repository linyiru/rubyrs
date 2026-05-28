//! `RubyError` / `Trap` shape + `format_trap` + `rescue` +
//! SyntaxError contract tests. Covers four overlapping
//! concerns that share the same exception surface:
//!
//!   1. **Direct vs Uncaught variant shapes** — host-fn-raised
//!      Traps surface as `RubyError::Foo { .. }` directly;
//!      script-raised exceptions surface as
//!      `RubyError::Uncaught { class_name, message }`.
//!      `RubyError::is` / `is_a` normalise across both shapes.
//!   2. **`format_trap` output** — CRuby-style "filename:line:in
//!      `Klass': message" formatting for embedding hosts that
//!      log Traps.
//!   3. **`rescue` semantics** — bare `rescue`, `rescue X` /
//!      hierarchy-walking subclass match, unresolved-class
//!      behaviour, and the ADR 0008 ResourceExhausted-uncatchable
//!      guarantee.
//!   4. **SyntaxError shape** — that prism / AST translation
//!      failures surface as Trap (not panic), that the message
//!      is human-readable, and that the `require_relative` path
//!      preserves file context.

use rubyrs::{Config, RubyError, Runtime};

use super::SharedBuf;

#[test]
fn ruby_error_is_normalises_direct_and_uncaught_shapes() {
    // The `is(&str)` helper matches the bare Ruby class name
    // regardless of whether the variant is a direct host-side
    // `RubyError::Foo` or the script-raised wrapped form
    // `Uncaught { class_name: "Foo" }`. Locks the API
    // contract in so embed tests can write
    // `assert!(err.err.is("X"))` without re-doing the case split.

    // Direct variant via a host-fn-raised trap.
    let mut rt = Runtime::new();
    rt.register_fn("boom", |_| Err(rubyrs::Trap {
        err: RubyError::ArgumentError { msg: "no good".into() },
        backtrace: vec![],
    }));
    let direct = rt.eval(r#"boom"#, "t.rb").unwrap_err();
    assert!(direct.err.is("ArgumentError"));
    assert!(!direct.err.is("NoMethodError"));

    // Uncaught wrapped form via a script-raised exception.
    let wrapped = rt.eval(r#"nil.no_such_method"#, "t.rb").unwrap_err();
    assert!(wrapped.err.is("NoMethodError"));
    assert!(!wrapped.err.is("ArgumentError"));
    // Bare name match — no hierarchy walk. RuntimeError is a
    // StandardError in CRuby, but `is("StandardError")` returns
    // false here by design. For hierarchy-aware matching use
    // `is_a` (see `ruby_error_is_a_walks_builtin_hierarchy`).
    let runtime = rt.eval(r#"raise "boom""#, "t.rb").unwrap_err();
    assert!(runtime.err.is("RuntimeError"));
    assert!(!runtime.err.is("StandardError"));
}

#[test]
fn ruby_error_is_a_walks_builtin_hierarchy() {
    // Hierarchy-aware variant of `is`. The static parent table
    // in `error.rs::BUILTIN_EXCEPTION_PARENT` mirrors
    // `preamble/exceptions.rb`; these assertions lock the walk
    // shape for every chain that file documents. If
    // `preamble/exceptions.rb` adds or reshapes a class, this
    // test (along with the table) must be updated in step.

    let mut rt = Runtime::new();

    // Direct variants — start from a host-fn-raised Trap so the
    // RubyError comes through unwrapped (not via Uncaught).
    // Each fn name maps to one specific RubyError variant; using
    // a closure-per-variant matches the `register_fn` signature
    // (`Fn(&[Value]) -> Result<Value, Trap>`) without an extra
    // factory helper.

    // RuntimeError chain: RuntimeError < StandardError < Exception.
    rt.register_fn("raise_runtime", |_| Err(rubyrs::Trap {
        err: RubyError::RuntimeError { msg: "".into() },
        backtrace: vec![],
    }));
    let e = rt.eval("raise_runtime", "t.rb").unwrap_err().err;
    assert!(e.is_a("RuntimeError"), "self");
    assert!(e.is_a("StandardError"), "direct parent");
    assert!(e.is_a("Exception"), "grandparent root");
    assert!(!e.is_a("ScriptError"), "different branch");
    assert!(!e.is_a("KeyError"), "sibling/unrelated");

    // KeyError chain: KeyError < IndexError < StandardError < Exception.
    // Locks the *intermediate* hop — without the table this would skip.
    rt.register_fn("raise_key", |_| Err(rubyrs::Trap {
        err: RubyError::KeyError { msg: "".into() },
        backtrace: vec![],
    }));
    let e = rt.eval("raise_key", "t.rb").unwrap_err().err;
    assert!(e.is_a("KeyError"));
    assert!(e.is_a("IndexError"), "intermediate parent");
    assert!(e.is_a("StandardError"));
    assert!(e.is_a("Exception"));

    // FrozenError < RuntimeError < StandardError < Exception.
    // The cross-branch hop (FrozenError under RuntimeError, not
    // directly under StandardError) is the failure mode the
    // static table closes.
    rt.register_fn("raise_frozen", |_| Err(rubyrs::Trap {
        err: RubyError::FrozenError { msg: "".into() },
        backtrace: vec![],
    }));
    let e = rt.eval("raise_frozen", "t.rb").unwrap_err().err;
    assert!(e.is_a("FrozenError"));
    assert!(e.is_a("RuntimeError"), "parent");
    assert!(e.is_a("StandardError"), "grandparent");
    assert!(e.is_a("Exception"));

    // ResourceExhausted is deliberately `< Exception` directly,
    // NOT under StandardError (ADR 0008 — bare `rescue` must not
    // swallow resource traps). Load-bearing for the security
    // posture, so the assertion stays explicit.
    rt.register_fn("raise_resource", |_| Err(rubyrs::Trap {
        err: RubyError::ResourceExhausted { msg: "".into() },
        backtrace: vec![],
    }));
    let e = rt.eval("raise_resource", "t.rb").unwrap_err().err;
    assert!(e.is_a("ResourceExhausted"));
    assert!(e.is_a("Exception"));
    assert!(!e.is_a("StandardError"), "ADR 0008: must NOT be under StandardError");
    assert!(!e.is_a("RuntimeError"));

    // Uncaught path — the chain starts from class_name and walks
    // the same table, so a script-raised RuntimeError matches
    // StandardError just like a host-raised one would.
    let wrapped = rt.eval(r#"raise "boom""#, "t.rb").unwrap_err().err;
    assert!(wrapped.is_a("RuntimeError"));
    assert!(wrapped.is_a("StandardError"), "Uncaught hierarchy walk");
    assert!(wrapped.is_a("Exception"));

    // User-defined subclass — class_name isn't in the static
    // table, so the walk terminates at exact match only. This
    // is the documented conservative behaviour; embedding hosts
    // that need hierarchy walk on script-defined classes must
    // consult the live `Runtime` class table directly.
    let user = rt.eval(
        r#"
        class MyErr < StandardError; end
        raise MyErr, "user"
        "#,
        "t.rb",
    ).unwrap_err().err;
    assert!(user.is_a("MyErr"), "exact match still works");
    assert!(!user.is_a("StandardError"), "no walk for user-defined subclass");
}

#[test]
fn format_trap_emits_cruby_style_line() {
    let mut rt = Runtime::new();
    let trap = rt.eval(r#"nil.foo"#, "snippet.rb").unwrap_err();
    let formatted = rt.format_trap(&trap);
    assert!(formatted.contains("snippet.rb:1"));
    assert!(formatted.contains("undefined method"));
    assert!(formatted.contains("NoMethodError"));
}

#[test]
fn syntax_error_does_not_panic() {
    let mut rt = Runtime::new();
    let res = rt.eval(r#"def foo("#, "broken.rb");
    assert!(res.is_err(), "syntax errors should bubble up as Trap");
}

#[test]
fn resource_exhausted_cannot_be_swallowed_by_bare_rescue() {
    // P0-1: `ResourceExhausted < Exception` (not StandardError), so bare
    // `rescue => e` — which CRuby-style filters on StandardError — must
    // not catch it. Otherwise a hostile script can spin in a rescue loop
    // and burn fuel forever, defeating the kill switch entirely.
    //
    // We give the script a generous outer fuel budget. The inner
    // `while true` will trip the fuel trap; if the bare `rescue`
    // swallowed it, the script would either run to completion (printing
    // "caught" once per outer iteration) or loop forever. Instead we
    // expect `eval` itself to surface the ResourceExhausted trap to
    // the host because no in-script handler matched.
    let buf = SharedBuf::new();
    let mut rt = Runtime::with_config(Config { fuel: Some(50_000), ..Default::default() });
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r#"
        begin
          i = 0
          while true
            i = i + 1
          end
        rescue => e
          puts "caught"
        end
        puts "after"
        "#,
        "uncatchable.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted to propagate past `rescue => e`, got {:?}",
        err.err,
    );
    let out = buf.snapshot();
    assert!(
        !out.contains("caught") && !out.contains("after"),
        "bare rescue should not have run; stdout was:\n{out}",
    );
}

#[test]
fn rescue_still_catches_standard_error_descendants() {
    // Locking in the partner invariant: bare `rescue` is now class-
    // filtered, but it must still catch StandardError + descendants the
    // way Ruby programs expect — every existing fixture relies on this.
    // `raise "boom"` normalises to RuntimeError, which is rooted under
    // StandardError, so the rescue clause runs.
    let buf = SharedBuf::new();
    let mut rt = Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        begin
          raise "boom"
        rescue => e
          puts "got: #{e.message}"
        end
        "#,
        "rescue_runtime.rb",
    ).unwrap();
    assert_eq!(buf.snapshot(), "got: boom\n");
}

#[test]
fn resource_exhausted_is_uncatchable_even_with_rescue_exception() {
    // P0-1 / P1-10 contract clarification: ResourceExhausted is
    // a HOST-level Trap, not a Ruby-level `raise`. It bypasses
    // `unwind_with_exception` entirely — the trap propagates up
    // via `?` from `Vm::run` straight to `Runtime::eval`. That
    // means even a script that explicitly writes
    // `rescue Exception => e` cannot intercept it. The trap is
    // not a Ruby exception at all; it's the embedding API's
    // way of saying "the script has used its budget, stop".
    let buf = SharedBuf::new();
    let mut rt = Runtime::with_config(Config { fuel: Some(50_000), ..Default::default() });
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r#"
        begin
          while true
          end
        rescue Exception => e
          puts "should not run"
        end
        "#,
        "explicit_catch.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
    assert!(!buf.snapshot().contains("should not run"));
}

#[test]
fn rescue_class_filter_catches_matching_subclass() {
    // Bread-and-butter P1-10 case: a user class hierarchy under
    // StandardError, and `rescue ParentClass` catches a child.
    let buf = SharedBuf::new();
    let mut rt = Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        class AppError < StandardError; end
        class NotFound < AppError; end
        begin
          raise NotFound, "missing"
        rescue AppError => e
          puts "got: #{e.message}"
        end
        "#,
        "subclass_catch.rb",
    ).unwrap();
    // Our `Object#class` returns the class; to_display formats it
    // as the class name.
    assert!(buf.snapshot().contains("missing"), "stdout: {}", buf.snapshot());
}

#[test]
fn rescue_with_unresolved_class_does_not_catch() {
    // Documented divergence from CRuby. CRuby raises NameError
    // eagerly when the rescue clause would fire. rubyrs silently
    // skips the clause: the class lookup at PushRescue time
    // misses, and the unwinder treats a non-ensure handler with
    // an unresolved filter as "matches nothing". The outer
    // rescue then catches the original exception.
    let buf = SharedBuf::new();
    let mut rt = Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        class Real < StandardError
        end
        begin
          begin
            raise Real, "boom"
          rescue NeverDefined => e
            puts "inner should not match"
          end
        rescue Real => e
          puts "outer: #{e.message}"
        end
        "#,
        "unresolved_rescue.rb",
    ).unwrap();
    assert_eq!(buf.snapshot(), "outer: boom\n");
}

#[test]
fn syntax_error_message_is_human_readable() {
    // Regression: `Runtime::eval` and the require_relative path
    // used to stringify `ruby_prism::Diagnostic` via its derived
    // Debug impl, which prints the internal `NonNull<...>` pointer
    // fields and a `PhantomData<...>` marker. The user-facing
    // SyntaxError leaked raw pointers like
    //   "Diagnostic { diag: 0x153370, parser: 0x1358e0, marker: PhantomData<...> }"
    // — useless to embedders and unstable across runs. The fix
    // formats `.message()` + `line:col` from `.location()`. Lock
    // both halves: the human-readable text shows up AND the
    // pointer-shaped fields don't.
    let mut rt = Runtime::new();
    let err = rt.eval("def x(", "syntax_err.rb").unwrap_err();
    let RubyError::SyntaxError { msg } = &err.err else {
        panic!("expected SyntaxError, got {:?}", err.err);
    };
    assert!(!msg.contains("Diagnostic {"), "Debug-format leaked: {msg}");
    assert!(!msg.contains("PhantomData"), "Debug-format leaked: {msg}");
    assert!(!msg.contains("0x"), "raw pointer leaked: {msg}");
    // Prism emits "expected a `)` to close the parameters" for
    // this input; assert the actionable substring is present.
    assert!(msg.contains("expected"), "missing diagnostic text: {msg}");
    // Line/column prefix from format_prism_errors.
    assert!(msg.starts_with("L1:"), "missing L<line>:<col> prefix: {msg}");
}

#[test]
fn syntax_error_via_require_relative_is_human_readable() {
    // Companion to `syntax_error_message_is_human_readable`. The
    // Debug-leak fix touched two call sites — `Runtime::eval` and
    // the `require_relative` load path in vm/kernel.rs's
    // `compile_and_run_source`. The eval site is covered above;
    // this case exercises the require_relative path so a future
    // edit that re-introduces the leak in `compile_and_run_source`
    // (e.g. copy-paste of `format!("{:?}", e)` during a refactor)
    // also fails CI.
    //
    // Drops a malformed `.rb` into the OS temp dir, then evals a
    // Ruby snippet that `require_relative`s its absolute path —
    // rubyrs's path-resolution joins through Rust's `Path::join`,
    // which treats an absolute argument as the full path, so this
    // works regardless of the caller's anchor file.
    use std::io::Write;
    let mut tmp = std::env::temp_dir();
    // Per-test-process file name to avoid races between parallel
    // cargo-test invocations sharing the same temp dir.
    tmp.push(format!("rubyrs_bad_syntax_{}.rb", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp).expect("write temp .rb");
        // Same shape as the eval-path test; multiple cascading
        // Prism diagnostics, all of which would have been
        // pointer-formatted under the bug.
        write!(f, "def x(").unwrap();
    }
    // Strip the `.rb` so the require_relative path matches
    // require's convention of "name without extension".
    let path_no_ext = tmp.with_extension("");
    let snippet = format!(
        "require_relative {:?}",
        path_no_ext.to_string_lossy(),
    );
    // `Runtime::new()` defaults to `allow_filesystem_io: false`
    // since the secure-by-default sandbox landed; this test needs
    // require_relative to actually reach the temp file, so opt in.
    let mut rt = Runtime::with_config(Config {
        allow_filesystem_io: true,
        ..Default::default()
    });
    let err = rt.eval(&snippet, "(syntax_err_via_require)").unwrap_err();
    // Cleanup before assertions so a failing assertion still
    // leaves /tmp tidy.
    let _ = std::fs::remove_file(&tmp);
    let RubyError::SyntaxError { msg } = &err.err else {
        panic!("expected SyntaxError, got {:?}", err.err);
    };
    assert!(!msg.contains("Diagnostic {"), "Debug-format leaked: {msg}");
    assert!(!msg.contains("PhantomData"), "Debug-format leaked: {msg}");
    assert!(!msg.contains("0x"), "raw pointer leaked: {msg}");
    assert!(msg.contains("expected"), "missing diagnostic text: {msg}");
    assert!(msg.starts_with("L1:"), "missing L<line>:<col> prefix: {msg}");
}

#[test]
fn unsupported_ast_node_returns_syntax_error_trap_not_panic() {
    // P0-4: prior to this change, any Prism node the AST translator
    // didn't handle (case/when, regex literal, lambda, etc.) hit
    // `panic!("unsupported node: ...")` and tore down the host
    // process. With rubund evaluating gemspecs from rubygems.org —
    // arbitrary third-party Ruby — that's a denial-of-service waiting
    // to happen.
    //
    // `for x in [...]` (ForNode) — old-style imperative
    // for-loop, vanishingly rare in real-world Ruby (Rubocop
    // even flags it). Currently outside the supported subset
    // and reaches the unsupported-node fallback. We expect a
    // SyntaxError Trap back, not a SIGABRT.
    // (Previous canaries: `case`, `Foo::Bar = 1`, `@@count` —
    // all landed as supported. Pick something genuinely
    // unimplemented each time the gap closes.)
    let mut rt = Runtime::new();
    let err = rt.eval(
        r#"
        for x in [1, 2, 3]
          puts x
        end
        "#,
        "for_loop.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, RubyError::SyntaxError { .. }),
        "expected SyntaxError, got {:?}",
        err.err,
    );
}

#[test]
fn uncaught_exception_returns_trap_not_process_exit() {
    // Before this fix the VM called `std::process::exit(1)` from
    // `unwind_with_exception` when no rescue clause matched — fine
    // for the CLI, fatal for any embedded host that has work to do
    // after the script returns. Now an uncaught exception surfaces
    // as `RubyError::Uncaught { class_name, message }`. The host
    // can pattern-match, log, retry, or carry on.
    let mut rt = Runtime::new();
    let err = rt.eval(
        r#"
        class MyError < StandardError; end
        raise MyError, "boom"
        "#,
        "uncaught.rb",
    ).unwrap_err();
    match err.err {
        RubyError::Uncaught { class_name, message } => {
            assert_eq!(class_name, "MyError");
            assert_eq!(message, "boom");
        }
        other => panic!("expected Uncaught, got {:?}", other),
    }
}

#[test]
fn host_can_continue_after_uncaught_exception() {
    // Companion to the test above — the *whole point* of the
    // change. After an uncaught exception, the same Runtime can
    // still evaluate fresh scripts. eval-after-Trap state reset
    // (P2-14a side-fix) keeps frames/stack/pinned clean.
    let mut rt = Runtime::new();
    let _ = rt.eval(r#"raise "first""#, "first.rb").unwrap_err();
    rt.eval(r#"puts 1 + 2"#, "second.rb").unwrap();
}

#[test]
fn uncaught_exception_format_trap_uses_script_class_name() {
    // `format_trap` should print the Ruby exception class
    // (`MyError`), not the host-side `Uncaught` tag.
    let mut rt = Runtime::new();
    let err = rt.eval(
        r#"
        class MyError < StandardError; end
        raise MyError, "boom"
        "#,
        "fmt.rb",
    ).unwrap_err();
    let formatted = rt.format_trap(&err);
    assert!(formatted.contains("(MyError)"), "got: {formatted}");
    assert!(formatted.contains("boom"), "got: {formatted}");
    assert!(!formatted.contains("Uncaught"), "should not leak host tag: {formatted}");
}


// ---------- Host-side panic → Trap conversion (C4) ----------

#[test]
fn host_fn_panic_becomes_runtime_error_trap() {
    // A `register_fn` callback that Rust-panics must NOT unwind
    // through `Runtime::eval` — embedders calling eval from
    // `extern "C"` would hit UB on the cross-boundary unwind.
    // The eval wrapper catches and converts to a RuntimeError
    // Trap whose message preserves the panic payload string.
    let mut rt = Runtime::new();
    rt.register_fn("explode", |_| {
        panic!("host fn boom");
    });
    let err = rt.eval(r#"explode"#, "test.rb").unwrap_err();
    match &err.err {
        rubyrs::RubyError::RuntimeError { msg } => {
            assert!(
                msg.contains("host-side panic during eval"),
                "expected panic-trap prefix in message, got {msg:?}",
            );
            assert!(
                msg.contains("host fn boom"),
                "expected original panic payload preserved, got {msg:?}",
            );
        }
        other => panic!("expected RuntimeError, got {other:?}"),
    }
}

#[test]
fn host_fn_panic_with_static_str_payload_is_preserved() {
    // `panic!("static literal")` boxes the str as `&'static str`,
    // NOT String. The payload-extractor must handle both shapes
    // — otherwise static-str panics surface as
    // `<non-string panic payload>`, which would lose actionable
    // diagnostic info in the most common shape.
    let mut rt = Runtime::new();
    rt.register_fn("explode", |_| {
        panic!("static literal payload");
    });
    let err = rt.eval(r#"explode"#, "test.rb").unwrap_err();
    let rubyrs::RubyError::RuntimeError { msg } = &err.err else {
        panic!("expected RuntimeError, got {:?}", err.err);
    };
    assert!(
        msg.contains("static literal payload"),
        "static-str payload should be preserved, got {msg:?}",
    );
}

#[test]
fn host_fn_panic_with_panic_any_payload_falls_back_gracefully() {
    // `panic_any(custom_struct)` boxes the struct opaquely;
    // downcast::<String> and downcast::<&str> both fail. The
    // helper should fall back to a type-id-bearing diagnostic
    // rather than silently dropping the panic context.
    let mut rt = Runtime::new();
    rt.register_fn("explode", |_| {
        std::panic::panic_any(42_u64);
    });
    let err = rt.eval(r#"explode"#, "test.rb").unwrap_err();
    let rubyrs::RubyError::RuntimeError { msg } = &err.err else {
        panic!("expected RuntimeError, got {:?}", err.err);
    };
    assert!(
        msg.contains("non-string panic payload"),
        "expected fallback diagnostic, got {msg:?}",
    );
}

#[test]
fn host_can_continue_after_host_fn_panic() {
    // After a caught host-fn panic, the same Runtime must still
    // be usable for fresh evals — the eval entry's
    // frames/stack/pinned clear is the safety net. This is the
    // shape rubund's batch evaluator and `_http_server`'s
    // request loop care about: one bad gemspec or one bad
    // request handler shouldn't kill the process.
    let mut rt = Runtime::new();
    rt.register_fn("explode", |_| panic!("boom"));
    let _ = rt.eval(r#"explode"#, "first.rb").unwrap_err();
    let v = rt.eval(r#"1 + 2"#, "second.rb").unwrap();
    assert!(matches!(v, rubyrs::Value::Int(3)));
}

#[test]
fn host_fn_returning_trap_is_not_rewrapped_by_panic_catcher() {
    // Sanity contract: the panic→Trap path mustn't accidentally
    // hijack callbacks that legitimately return `Err(Trap)`.
    // Those propagate through script-level dispatch and surface
    // as `Uncaught { class_name: "KeyError", ... }` (the
    // existing "Uncaught" variant shape — see
    // `ruby_error_is_normalises_direct_and_uncaught_shapes`).
    // What MUST NOT happen is re-wrapping into a generic
    // RuntimeError with "host-side panic during eval" — that
    // would lose the original class.
    let mut rt = Runtime::new();
    rt.register_fn("raise_key", |_| Err(rubyrs::Trap {
        err: rubyrs::RubyError::KeyError { msg: "expected".into() },
        backtrace: vec![],
    }));
    let err = rt.eval(r#"raise_key"#, "test.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected Uncaught variant, got {:?}", err.err);
    };
    assert_eq!(class_name, "KeyError", "original class must survive");
    assert_eq!(message, "expected", "original message must survive");
    assert!(
        !message.contains("host-side panic"),
        "Trap path must not be re-wrapped by panic catcher; got {message:?}",
    );
}
