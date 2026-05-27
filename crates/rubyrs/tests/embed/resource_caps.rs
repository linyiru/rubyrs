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
fn interner_cap_traps_symbol_succ_in_loop() {
    // `Symbol#succ` re-interns the successor name and was
    // previously bypassing the cap that `String#to_sym` honours.
    // With the cap in place, an unbounded `sym = sym.succ` loop
    // traps the same way `("k" + i.to_s).to_sym` does.
    let mut rt0 = Runtime::new();
    rt0.eval("", "warmup.rb").unwrap();
    let baseline = rt0.symbol_count();
    let mut rt = Runtime::with_config(Config {
        max_symbols: Some(baseline + 20),
        ..Default::default()
    });
    let err = rt.eval(
        r#"
        sym = :a
        i = 0
        while i < 1000
          sym = sym.succ
          i = i + 1
        end
        "#,
        "succ_blowup.rb",
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
fn config_default_does_not_read_stress_gc_env() {
    // Inverted regression guard (was
    // `config_default_picks_up_stress_gc_env` before the
    // pre-Phase-1 cleanup in commit `a12126ec`).
    //
    // The previous behaviour — `Config::default()` reading
    // `STRESS_GC` from the host process env — leaked host
    // state into a public library-API field, violating the
    // spirit of ADR 0017 Rule 1 ("deterministic from script
    // inputs"). Library embedders constructing
    // `Config::default()` expect their bool fields to be
    // exactly the documented defaults, not silently
    // overridden by inherited env.
    //
    // The CLI binary `rubyrs` (`main.rs::env_lookup`) reads
    // `STRESS_GC` explicitly and sets the field — that's the
    // supported "STRESS_GC=1 cargo test" path going forward.
    // Subprocess-based tests (diff_cruby, cext_*) still pick
    // it up via the CLI; in-process `Runtime::new()` callers
    // do not.
    //
    // SAFETY: `std::env::set_var` / `remove_var` are unsafe
    // in 2024 edition because they aren't thread-safe. No
    // other test in the suite reads `STRESS_GC` at runtime
    // (the `*_survives_stress_gc` tests hard-code the flag
    // in Config), so the race window is empty.
    let prev = std::env::var("STRESS_GC").ok();
    unsafe { std::env::remove_var("STRESS_GC") };
    assert!(
        !rubyrs::Config::default().stress_gc,
        "Config::default().stress_gc must be false when STRESS_GC unset (baseline)",
    );

    unsafe { std::env::set_var("STRESS_GC", "1") };
    assert!(
        !rubyrs::Config::default().stress_gc,
        "Config::default().stress_gc must STILL be false even when STRESS_GC=1 is set — the library API does not read host env (post pre-Phase-1 cleanup; see commit a12126ec)",
    );

    match prev {
        Some(v) => unsafe { std::env::set_var("STRESS_GC", v) },
        None => unsafe { std::env::remove_var("STRESS_GC") },
    }
}

#[test]
fn runtime_functional_after_construction_under_realistic_caps() {
    // Originally added as `preamble_fits_under_tight_resource_caps`
    // (PR #116 cycle 7) when the preamble DID run under user-
    // supplied caps — the test pinned "preamble fits under {fuel:
    // 10_000, max_heap_objects: 50, ...}" so a future preamble-
    // growth PR would break this canary loudly instead of
    // surfacing as an ICE inside an unrelated cap-trap test.
    //
    // PR #204 changed the contract: the preamble is internal
    // infrastructure and runs UNBOUNDED regardless of user caps
    // (see `with_config_succeeds_under_*` tests below for the
    // per-cap contract). The preamble-growth canary the original
    // test was guarding against is moot now.
    //
    // Repurpose as a Runtime-functionality smoke test under a
    // realistic embedder Config: caps tight enough to be
    // production-shaped but loose enough that a trivial user eval
    // still succeeds. If a future refactor of `with_config`'s
    // cap-lift sequencing breaks the post-preamble cap restoration
    // (e.g. caps stay zeroed because of a bug in the restore
    // path), `1 + 1` will still succeed — but a regression that
    // leaks preamble state into user-visible Vm counters would
    // fail this trivial eval.
    let cfg = rubyrs::Config {
        max_heap_objects: Some(50),
        max_symbols: Some(64),
        fuel: Some(10_000),
        max_frames: Some(64),
        max_value_bytes: Some(1024),
        deadline: Some(std::time::Duration::from_millis(50)),
        // Pin off so STRESS_GC=1 in the runner env can't make
        // this test interact differently with GC-frequency
        // assumptions in the cap-restore path.
        stress_gc: false,
        ..Default::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    // Sanity: realistic-budget construction still allows a
    // trivial eval.
    rt.eval("1 + 1", "post_preamble_smoke.rb")
        .expect("trivial eval must succeed after construction under realistic caps");
    // Real assertion: the fuel cap WAS restored after preamble.
    // Without this second eval, a regression that left caps
    // zeroed by skipping the restore would pass silently (every
    // budget-respecting eval succeeds when budget is `None`).
    // Run a tight loop that needs more than the configured 10k
    // fuel and assert ResourceExhausted — proves the restore
    // path is live.
    let err = rt
        .eval(
            "i = 0; while i < 100_000; i = i + 1; end",
            "post_preamble_fuel_check.rb",
        )
        .expect_err("restored fuel cap must trap the loop");
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted from restored fuel cap, got {:?}",
        err.err,
    );
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

// --- Preamble bypasses user-supplied resource caps -------------------
//
// Before this lane existed, `Runtime::with_config` applied user
// caps before running the exception-class preamble, so a tight
// budget (any of: `fuel < ~9k`, `max_frames < ~30`,
// `max_heap_objects < ~50`, sub-millisecond `deadline`) panicked
// during construction with `ICE: failed to load exception
// preamble`. Surfaced by the cargo-fuzz harness in PR #180 and
// fixed by lifting resource caps for the duration of preamble
// load, then restoring them. These tests pin both halves of the
// contract: construction succeeds under tight caps, and the
// caps still apply to every subsequent `eval()`.

#[test]
fn with_config_succeeds_under_zero_fuel() {
    // `fuel: Some(0)` is the most extreme case — every op
    // dispatched should trap. Pre-fix, this panicked on the
    // first preamble op.
    let mut rt = Runtime::with_config(Config {
        fuel: Some(0),
        stress_gc: false, // see runtime_functional_after_construction_under_realistic_caps
        ..Default::default()
    });
    let err = rt.eval("1 + 1", "tight.rb").unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected user eval to still hit fuel cap, got {:?}", err.err,
    );
}

#[test]
fn with_config_succeeds_under_tight_frames_cap() {
    // The preamble defines several classes whose bodies push
    // frames — `max_frames: Some(1)` would have trapped on the
    // first `class StandardError; ... end` body pre-fix.
    let mut rt = Runtime::with_config(Config {
        max_frames: Some(1),
        stress_gc: false,
        ..Default::default()
    });
    let err = rt.eval(
        "def deep(n); deep(n + 1); end; deep(0)",
        "frames.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected user eval to still hit frames cap, got {:?}", err.err,
    );
}

#[test]
fn with_config_succeeds_under_tight_heap_cap() {
    // The preamble allocates several `HeapObj::Class` slots
    // (one per Exception subclass + Object + Comparable + ...).
    // `max_heap_objects: Some(1)` would have trapped on the
    // second class allocation pre-fix.
    let mut rt = Runtime::with_config(Config {
        max_heap_objects: Some(1),
        stress_gc: false,
        ..Default::default()
    });
    let err = rt.eval("Array.new(100) { |i| i }", "heap.rb").unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected user eval to still hit heap cap, got {:?}", err.err,
    );
}

#[test]
fn with_config_succeeds_under_sub_ms_deadline() {
    // A nanosecond-grade deadline trips on the first
    // `Op::CheckDeadline` (fires every 1024 ops). Any
    // sub-millisecond deadline used to panic during
    // construction; now construction succeeds and the
    // deadline only applies to user `eval()` calls.
    let cfg = Config {
        deadline: Some(std::time::Duration::from_nanos(1)),
        stress_gc: false,
        ..Default::default()
    };
    let mut rt = Runtime::with_config(cfg);
    // Run a script long enough to cross at least one 1024-op
    // deadline checkpoint. An empty eval (or `1 + 1`) finishes
    // in well under 1024 ops and may complete before the
    // deadline check fires — pinning the eval result would be
    // flaky. A `while true` spin guarantees the checkpoint
    // hits, so we can assert ResourceExhausted definitively.
    let err = rt
        .eval("while true; end", "deadline.rb")
        .expect_err("1ns deadline must trap user eval on the first checkpoint");
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected user eval to hit the restored deadline cap, got {:?}",
        err.err,
    );
}

#[test]
fn with_config_succeeds_under_combined_tight_caps() {
    // All caps tightened at once — the original failure mode
    // for fuzz harnesses (`fuel: Some(50_000)` was the magic
    // workaround number in PR #180 before this fix landed).
    // Verify a host can now ask for the kind of sandbox a
    // fuzzer or untrusted-script evaluator actually wants.
    let cfg = Config {
        fuel: Some(10),
        max_frames: Some(8),
        max_heap_objects: Some(16),
        max_symbols: Some(64),
        max_value_bytes: Some(1024),
        deadline: Some(std::time::Duration::from_millis(50)),
        stress_gc: false,
        ..Default::default()
    };
    // Just constructing it would have panicked before. The
    // sandbox is functional — user eval traps cleanly on the
    // first cap that bites (fuel here, since 10 ops is well
    // below what a 5-element iteration with `* 2` consumes).
    let mut rt = Runtime::with_config(cfg);
    let err = rt.eval("[1,2,3,4,5].map { |x| x * 2 }", "sandbox.rb").unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted from one of the tight caps, got {:?}", err.err,
    );
}
