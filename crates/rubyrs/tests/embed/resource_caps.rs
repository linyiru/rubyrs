//! Resource-cap enforcement tests — fuel, heap, frames,
//! symbols, value_bytes, and the wall-clock deadline. The
//! caps that make rubyrs safe to run untrusted scripts (ADR
//! 0008 untrusted-input model), surfaced via `Config::fuel`
//! / `max_heap_objects` / `max_frames` / `max_symbols` /
//! `max_value_bytes` / `deadline`.
//!
//! Two cross-cutting cap tests live here too:
//!   - `interpolated_regex_respects_max_symbols_cap` — the
//!     regex compile path runs through the same interner as
//!     plain symbol literals, so the symbols cap applies.
//!   - `object_send_string_arg_respects_max_symbols_cap` —
//!     `Object#send("name")` interns the string arg, which
//!     also goes through the symbols cap.
//!
//! BigInt's `bigint_to_s_respects_max_value_bytes_cap` and the
//! various `sprintf_*_traps_via_pre_alloc_cap` tests stay in
//! the (future) `embed/bignum.rs` sub-mod alongside the rest
//! of the bignum surface — they exercise the cap on the way
//! to validating bignum arithmetic semantics, not the cap
//! itself.

use rubyrs::{Config, RubyError, Runtime};

#[test]
fn fuel_traps_infinite_loop() {
    let mut rt = Runtime::with_config(Config { fuel: Some(10_000), ..Default::default() });
    let err = rt.eval(r#"i = 0; while true; i = i + 1; end"#, "spin.rb").unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err
    );
}

#[test]
fn fuel_is_not_bypassed_by_block_iteration() {
    // Without per-op fuel inside dispatch_until, an each-block could spin forever.
    let mut rt = Runtime::with_config(Config { fuel: Some(50_000), ..Default::default() });
    let err = rt.eval(
        r#"
        nums = []
        i = 0
        while i < 100
          nums << i
          i = i + 1
        end
        nums.each { |x| j = 0; while true; j = j + 1; end }
        "#,
        "spin_in_block.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn unlimited_fuel_runs_normally() {
    let mut rt = Runtime::new();
    rt.eval(r#"i = 0; while i < 100; i = i + 1; end"#, "ok.rb").unwrap();
}

#[test]
fn heap_cap_traps_retained_allocations() {
    let mut rt = Runtime::with_config(Config { max_heap_objects: Some(50), ..Default::default() });
    // Each inner Array is retained via `all`, so live_count grows linearly.
    let err = rt.eval(
        r#"
        all = []
        i = 0
        while i < 200
          all << [i, i + 1]
          i = i + 1
        end
        "#,
        "alloc_storm.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn wall_clock_deadline_traps_long_running_eval() {
    // P2-14a: with `Config::deadline: Some(50ms)` a script that
    // would otherwise spin for seconds gets stopped within roughly
    // a millisecond of the budget (we only check every 1024 ops,
    // so there's a small tail).
    let mut rt = Runtime::with_config(Config {
        deadline: Some(std::time::Duration::from_millis(50)),
        ..Default::default()
    });
    let start = std::time::Instant::now();
    let err = rt.eval(
        r#"
        i = 0
        while true
          i = i + 1
        end
        "#,
        "spin.rb",
    ).unwrap_err();
    let elapsed = start.elapsed();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { ref msg } if msg.contains("deadline")),
        "expected ResourceExhausted/deadline, got {:?}",
        err.err,
    );
    // Generous upper bound — CI runners are noisy. The point is
    // we stopped before the test harness's own per-test timeout.
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "deadline did not fire in time; elapsed {:?}",
        elapsed,
    );
}

#[test]
fn deadline_resets_between_eval_calls() {
    // The deadline is per-eval, not lifetime-cumulative. After the
    // first script trips the budget, a second eval on the same
    // Runtime gets a fresh 50ms allotment and a fast script
    // succeeds.
    let mut rt = Runtime::with_config(Config {
        deadline: Some(std::time::Duration::from_millis(50)),
        ..Default::default()
    });
    let _ = rt.eval("while true; end", "spin.rb").unwrap_err();
    // The previous eval consumed the budget; a new eval re-anchors.
    rt.eval("puts 1 + 2", "fast.rb").unwrap();
}

#[test]
fn interner_cap_traps_to_sym_in_loop() {
    // P2-14b: `String#to_sym` in a loop is the classic
    // interner-growth vector. With `Config::max_symbols` set, the
    // VM traps the moment the cap would be exceeded. Existing
    // symbols always re-resolve (no growth), so the script can
    // still `:foo.to_sym` many times — only fresh strings count.
    //
    // The cap is per-Runtime, not per-eval: the preamble pre-loads
    // a chunk of symbols (class names, method names) so we
    // measure relative to where the interner already sits.
    let mut rt0 = Runtime::new();
    rt0.eval("", "warmup.rb").unwrap();
    let baseline = rt0.symbol_count();
    let mut rt = Runtime::with_config(Config {
        max_symbols: Some(baseline + 20),
        ..Default::default()
    });
    let err = rt.eval(
        r#"
        i = 0
        while i < 1000
          ("k" + i.to_s).to_sym
          i = i + 1
        end
        "#,
        "intern_blowup.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { ref msg } if msg.contains("interner")),
        "expected ResourceExhausted/interner, got {:?}", err.err,
    );
}

#[test]
fn interner_cap_allows_reusing_existing_symbols() {
    // The cap should only fire when a *new* symbol would be
    // interned. Repeatedly calling `.to_sym` on the same string
    // re-resolves the existing slot and is free.
    let mut rt0 = Runtime::new();
    rt0.eval("", "warmup.rb").unwrap();
    let baseline = rt0.symbol_count();
    let mut rt = Runtime::with_config(Config {
        // 2 spare slots beyond baseline — enough for `"foo"` once.
        max_symbols: Some(baseline + 2),
        ..Default::default()
    });
    rt.eval(
        r#"
        i = 0
        while i < 500
          "foo".to_sym
          i = i + 1
        end
        "#,
        "intern_reuse.rb",
    ).unwrap();
}

#[test]
fn value_bytes_cap_traps_string_repeat_blowup() {
    // P2-14c: `"a" * N` is one heap object that quietly grabs N
    // bytes of RAM. Fuel doesn't catch it (it's a single op);
    // heap-object cap doesn't catch it (still one object).
    // max_value_bytes does.
    let mut rt = Runtime::with_config(Config {
        max_value_bytes: Some(1024),
        ..Default::default()
    });
    let err = rt.eval(r#"s = "a" * 10000"#, "blowup.rb").unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { ref msg } if msg.contains("bytes")),
        "expected ResourceExhausted/bytes, got {:?}", err.err,
    );
}

#[test]
fn value_bytes_cap_traps_string_concat_blowup() {
    // `s = s + "a"` in a loop is the slow-growth flavour of the
    // same attack — each iteration allocates a fresh string one
    // byte longer than the last.
    let mut rt = Runtime::with_config(Config {
        max_value_bytes: Some(512),
        ..Default::default()
    });
    let err = rt.eval(
        r#"
        s = "a"
        i = 0
        while i < 1000
          s = s + "a"
          i = i + 1
        end
        "#,
        "concat.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn value_bytes_cap_traps_array_unbounded_push() {
    // `arr << x` in a hot loop grows the backing Vec linearly.
    // 100 elements × ~24 bytes/Value = ~2400 bytes; cap at 1000
    // and we should trap well before the loop finishes.
    let mut rt = Runtime::with_config(Config {
        max_value_bytes: Some(1000),
        ..Default::default()
    });
    let err = rt.eval(
        r#"
        arr = []
        i = 0
        while i < 1000
          arr << i
          i = i + 1
        end
        "#,
        "push.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn value_bytes_cap_allows_small_strings_and_arrays() {
    // Sanity check the no-trap path: with a generous cap, normal
    // ops should run untouched.
    let mut rt = Runtime::with_config(Config {
        max_value_bytes: Some(1_000_000),
        ..Default::default()
    });
    rt.eval(
        r#"
        s = "hello, " + "world"
        arr = [1, 2, 3]
        arr << 4
        h = { a: 1 }
        h[:b] = 2
        puts s
        puts arr.length
        puts h.size
        "#,
        "small.rb",
    ).unwrap();
}

#[test]
fn frame_cap_traps_deep_recursion() {
    let mut rt = Runtime::with_config(Config { max_frames: Some(20), ..Default::default() });
    let err = rt.eval(
        r#"
        def rec(n)
          rec(n + 1)
        end
        rec(0)
        "#,
        "deep.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn interpolated_regex_respects_max_symbols_cap() {
    // PR #99 review coverage: dynamic patterns intern into the
    // same interner used by `String#to_sym`, so the same
    // `Config::max_symbols` cap that bounds `to_sym` must also
    // bound interpolated regex pattern interning. Without the cap
    // check inside `Op::CompileRegex`, untrusted scripts could
    // build distinct patterns in a loop to grow the interner
    // (and the SymId-keyed `regex_cache`) without bound.
    let cfg = rubyrs::Config { max_symbols: Some(64), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        r#"
        i = 0
        while i < 10_000
          /#{i}/
          i += 1
        end
        "#,
        "regex_symbol_storm.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted trap from regex-pattern symbol storm, got {:?}",
        err.err,
    );
}

#[test]
fn object_send_string_arg_respects_max_symbols_cap() {
    // PR #98 review coverage: `obj.send("dyn_#{i}")` interns the
    // String arg as a method name. Without the same cap check
    // `String#to_sym` uses, untrusted scripts could grow the
    // interner unbounded by passing distinct dynamic strings to
    // `send` in a loop. The fresh name has to be a String literal
    // (Symbol args don't intern); we burn through the cap deliberately
    // and then expect ResourceExhausted on the very next fresh name.
    let cfg = rubyrs::Config { max_symbols: Some(64), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        r#"
        i = 0
        while i < 10_000
          begin
            "x".send("dyn_#{i}")
          rescue NoMethodError
            # most synthetic names don't resolve — that's fine,
            # we just want the interner to keep growing.
          end
          i += 1
        end
        "#,
        "send_symbol_storm.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted trap from send-name symbol storm, got {:?}",
        err.err,
    );
}

#[test]
fn config_default_picks_up_stress_gc_env() {
    // Regression guard for PR #116 review: removing the
    // `env::var("STRESS_GC")` read from `Vm::new` (to satisfy
    // wizer's no-imports rule on wasm32-wasip1) silently broke
    // ci.yml's "Run tests (STRESS_GC=1)" job — that step re-runs
    // `cargo test` with the env var set, expecting every
    // `Runtime::new()` in the suite to flip into stress mode for
    // broader GC-rooting coverage. The compensating read lives in
    // `Config::default()` instead; this test pins it so a future
    // cleanup of `Config::default` doesn't re-introduce the silent
    // CI coverage gap.
    //
    // SAFETY: `std::env::set_var` / `remove_var` are unsafe in 2024
    // edition because they aren't thread-safe. No other test in the
    // suite currently reads `STRESS_GC` at runtime (the
    // `*_survives_stress_gc` tests hard-code the flag in Config and
    // don't consult env), so the race window is empty today. If a
    // future test starts reading the env, gate this behind a
    // serial-test crate.
    let prev = std::env::var("STRESS_GC").ok();
    unsafe { std::env::remove_var("STRESS_GC") };
    assert!(
        !rubyrs::Config::default().stress_gc,
        "Config::default().stress_gc must be false when STRESS_GC unset",
    );

    unsafe { std::env::set_var("STRESS_GC", "1") };
    assert!(
        rubyrs::Config::default().stress_gc,
        "Config::default().stress_gc must be true when STRESS_GC=1 — the CI stress-mode gate relies on this",
    );

    match prev {
        Some(v) => unsafe { std::env::set_var("STRESS_GC", v) },
        None => unsafe { std::env::remove_var("STRESS_GC") },
    }
}

#[test]
fn preamble_fits_under_tight_resource_caps() {
    // Regression guard for PR #116 cycle 7 refactor: moving
    // `apply_config` BEFORE `load_preamble` in `Runtime::with_config`
    // means the built-in preamble (Exception hierarchy + ancillary
    // classes) now runs UNDER user-supplied caps. The 67 existing
    // embed tests currently pass with caps like `max_heap_objects:
    // Some(50)`, `max_symbols: Some(64)`, `fuel: Some(10_000)`,
    // `deadline: Some(50ms)` — but nothing asserts the preamble
    // actually fits. A future contributor adding even a handful of
    // built-in classes (e.g. Comparable, Numeric, Range methods)
    // could silently push the preamble past one of these budgets,
    // panicking deep inside `load_preamble().expect("ICE: …")`
    // during Runtime construction — manifesting as an inscrutable
    // ICE in tests that on the surface look like they're testing
    // user-script caps.
    //
    // Pin a budget tighter than ANY cap used elsewhere in this
    // file (sweep above shows `max_heap_objects: Some(50)` and
    // `max_symbols: Some(64)` as the floors). If preamble growth
    // breaks this canary first, the failure points at the right
    // root cause; without it, the failure surfaces as a panic in
    // an unrelated cap-trap test.
    let cfg = rubyrs::Config {
        max_heap_objects: Some(50),
        max_symbols: Some(64),
        fuel: Some(10_000),
        max_frames: Some(64),
        max_value_bytes: Some(1024),
        deadline: Some(std::time::Duration::from_millis(50)),
        ..Default::default()
    };
    // Construction itself is the assertion: if any cap trips during
    // preamble eval, `load_preamble().expect(...)` panics here.
    let mut rt = rubyrs::Runtime::with_config(cfg);
    // Sanity: the resulting Runtime should still be able to eval a
    // trivial expression — i.e. the preamble didn't quietly consume
    // every last unit of fuel/heap. The caps are intentionally
    // tight, so this only succeeds if there's headroom left.
    rt.eval("1 + 1", "preamble_canary.rb").expect("trivial eval must succeed after preamble under tight caps");
}


#[cfg(feature = "bignum")]
#[test]
fn integer_iter_loops_trap_under_fuel_cap() {
    // A.3 — DoS guard for the BigInt iter surface. Pre-this-test,
    // `(2**100).times { }` / `0.upto(10**18) { }` will run
    // essentially forever on a host that hasn't configured fuel
    // OR deadline (the runtime's "explicit opt-in" cap model —
    // no implicit DoS protection, same as the rest of the
    // resource caps). When EITHER cap IS set, the loop trips:
    // this test pins the fuel-trip path via the existing
    // `Config::fuel` mechanism (decremented per dispatched op),
    // which trips inside the block-invocation dispatch loop and
    // raises `ResourceExhausted: "out of fuel"`.
    //
    // This test pins that behaviour across all three iter
    // methods (times / upto / downto) for both Int recv (the
    // existing arms) and BigInt recv (the Phase B.6 arms). The
    // fuel ticks because every iteration calls invoke_block,
    // which dispatches at least one op (the block's return).
    // Without fuel, ops never decrement; with fuel set, the
    // loop trips after a bounded number of iterations regardless
    // of the receiver's magnitude.
    // Each script has a fail-fast break at iteration 1_000_000.
    // The break is well above the fuel-trip horizon (10_000 fuel
    // ÷ ~5 ops per iteration ≈ 2_000 iterations max), so it's
    // dead code when fuel works correctly. If the fuel guard
    // ever regresses, the test still terminates within a few
    // seconds instead of hanging the test suite indefinitely —
    // CI fails fast with an assertion miss rather than a
    // wall-clock timeout.
    for script in [
        // BigInt-recv iter arms (Phase B.6)
        "i = 0; (2 ** 100).times { i += 1; break if i > 1_000_000 }",
        "i = 0; (2 ** 80).upto(2 ** 100) { i += 1; break if i > 1_000_000 }",
        "i = 0; (2 ** 100).downto(0) { i += 1; break if i > 1_000_000 }",
        // Int-recv iter arms with very large bounds
        "i = 0; 0.upto(1_000_000_000_000) { i += 1; break if i > 1_000_000 }",
        "i = 0; 1_000_000_000_000.downto(0) { i += 1; break if i > 1_000_000 }",
        "i = 0; 10_000_000.times { i += 1; break if i > 1_000_000 }",
    ] {
        let mut rt = Runtime::with_config(Config {
            fuel: Some(10_000),
            ..Default::default()
        });
        let err = rt.eval(script, "iter_fuel.rb").unwrap_err();
        // Pin the exact "out of fuel" message so the test fails
        // closed if a future regression accidentally trips a
        // different ResourceExhausted source (e.g. max_value_bytes,
        // max_heap_objects, max_frames) — those caps aren't
        // configured here, so reaching them would indicate the
        // fuel guard slipped and a different cap caught it
        // incidentally. Match on `&err.err` so the failure
        // message can still include the actual error.
        assert!(
            matches!(
                &err.err,
                RubyError::ResourceExhausted { msg } if msg == "out of fuel"
            ),
            "expected ResourceExhausted(\"out of fuel\") for {:?}, got {:?}",
            script, err.err,
        );
    }
}
