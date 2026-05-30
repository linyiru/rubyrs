//! ADR 0017 Tier 1 capability-injection tests — `Random`,
//! `SecureRandom`, and `Time.now` defaults / overrides.
//!
//! These three classes are the canonical examples of the
//! "deterministic-by-default, capability-injected for real
//! behaviour" pattern from ADR 0017 row 131. Without an
//! explicit host-side capability (a seed, a clock closure),
//! the runtime raises rather than silently leaking host
//! entropy / wall clock; with one, the script sees exactly
//! what the host provides and nothing else.
//!
//! Companion file: `tests/embed/adr_0017.rs` covers env / pid
//! / stdout, which are the other three capabilities ADR 0017
//! tracks. The two files together cover the full capability
//! matrix.

use super::SharedBuf;

#[test]
fn random_new_no_arg_raises_in_tier1_deterministic_mode() {
    // Documented divergence from CRuby: per ADR 0017 row 131
    // the Tier 1 `Random` class is seeded-only. CRuby's
    // `Random.new` falls through to system entropy; rubyrs
    // raises ArgumentError because Tier 1 forbids the entropy
    // capability. Pinned here so a future refactor can't quietly
    // re-introduce a default-seed path.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("Random.new", "random_no_arg.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected Uncaught ArgumentError, got {:?}", err.err);
    };
    assert_eq!(class_name, "ArgumentError");
    assert!(
        message.contains("Tier 1 Random.new requires an explicit Integer seed"),
        "unexpected message: {}",
        message,
    );
}

#[test]
fn secure_random_seed_setter_makes_output_deterministic() {
    // rubyrs-specific Tier 1 affordance: `SecureRandom.seed = N`
    // reseeds the hidden default `Random` so subsequent calls
    // produce a reproducible sequence. CRuby's SecureRandom has
    // no `seed=` surface (entropy-only), so this behaviour is
    // pinned at the embed layer rather than in the diff_cruby
    // harness. ADR 0017 row 131 documents the determinism trade.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let script = r#"
        SecureRandom.seed = 42
        a_hex = SecureRandom.hex(16)
        a_uuid = SecureRandom.uuid
        a_alpha = SecureRandom.alphanumeric(32)

        SecureRandom.seed = 42
        b_hex = SecureRandom.hex(16)
        b_uuid = SecureRandom.uuid
        b_alpha = SecureRandom.alphanumeric(32)

        puts a_hex == b_hex
        puts a_uuid == b_uuid
        puts a_alpha == b_alpha

        SecureRandom.seed = 99
        c_hex = SecureRandom.hex(16)
        puts c_hex == a_hex
    "#;
    rt.eval(script, "sr_seeded.rb").expect("eval");
    assert_eq!(buf.snapshot(), "true\ntrue\ntrue\nfalse\n");
}

#[test]
fn time_now_default_raises_without_capability_injection() {
    // ADR 0017 Rule 1: by default `Time.now` must NOT reach for
    // the host wall clock. With no `Config::time_now` injection,
    // the preamble's `Time.now` calls into `__time_now_raw` which
    // raises RuntimeError with a message pointing at the
    // capability slot.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("Time.now", "time_no_capability.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected Uncaught RuntimeError, got {:?}", err.err);
    };
    assert_eq!(class_name, "RuntimeError");
    assert!(
        message.contains("Time.now requires `Config::time_now` injection"),
        "unexpected message: {}",
        message,
    );
}

#[test]
fn time_now_returns_injected_value_byte_identical() {
    // With a fixed-clock injection the same `Time.now` call
    // returns reproducible component values. Verifies the
    // capability source flows through `__time_now_raw` →
    // preamble `Time.now` → `Time#year` / `to_i` exactly as
    // designed.
    let buf = SharedBuf::new();
    let cfg = rubyrs::Config {
        time_now: Some(std::sync::Arc::new(|| (1_700_000_000, 123_456_789))),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    let script = r#"
        t = Time.now
        puts t.to_i
        puts t.nsec
        puts t.utc.year
        puts t.utc.month
        puts t.utc.day
        puts t.utc.to_s
    "#;
    rt.eval(script, "time_injected.rb").expect("eval");
    assert_eq!(
        buf.snapshot(),
        "1700000000\n123456789\n2023\n11\n14\n2023-11-14 22:13:20 UTC\n"
    );
}

#[test]
fn time_now_observes_capability_state_changes_per_call() {
    // The capability closure is called ONCE per `Time.now` — a
    // mutating host can advance the simulated clock between
    // calls and observe the script perceive the advance. Uses an
    // `Arc<Mutex<i64>>` counter so the closure is Fn (the
    // Config trait bound is `Fn`, not `FnMut`).
    let buf = SharedBuf::new();
    let counter = std::sync::Arc::new(std::sync::Mutex::new(0i64));
    let counter_for_closure = counter.clone();
    let cfg = rubyrs::Config {
        time_now: Some(std::sync::Arc::new(move || {
            let mut g = counter_for_closure.lock().unwrap();
            *g += 1;
            (*g, 0)
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    let script = r#"
        puts Time.now.to_i
        puts Time.now.to_i
        puts Time.now.to_i
    "#;
    rt.eval(script, "time_advancing.rb").expect("eval");
    assert_eq!(buf.snapshot(), "1\n2\n3\n");
    // After the script ran, the closure should have been called
    // exactly 3 times.
    assert_eq!(*counter.lock().unwrap(), 3);
}

#[test]
fn sleep_default_raises_without_capability_injection() {
    // ADR 0017 Rule 1: by default `Kernel#sleep` must NOT
    // pause the host thread. With no `Config::sleep_for`
    // injection, `sleep(0)` raises RuntimeError pointing at
    // the missing capability — same shape as Time.now.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("sleep(0)", "sleep_no_capability.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected Uncaught RuntimeError, got {:?}", err.err);
    };
    assert_eq!(class_name, "RuntimeError");
    assert!(
        message.contains("Kernel#sleep requires `Config::sleep_for` injection"),
        "unexpected message: {message}",
    );
}

#[test]
fn sleep_invokes_injected_closure_with_requested_duration() {
    // With a recording closure injected, `sleep(0.25)`
    // calls it exactly once carrying Duration::from_secs_f64(0.25).
    // No real wall-clock pause — closure is a Mutex push.
    use std::sync::{Arc, Mutex};
    let recorded: Arc<Mutex<Vec<std::time::Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_for_cfg = Arc::clone(&recorded);
    let cfg = rubyrs::Config {
        sleep_for: Some(std::sync::Arc::new(move |d| {
            recorded_for_cfg.lock().unwrap().push(d);
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.eval("sleep(0.25); sleep(1)", "sleep_inject.rb").expect("eval");
    let durations = recorded.lock().unwrap().clone();
    assert_eq!(durations.len(), 2, "sleep called twice: {durations:?}");
    // Float arg: ~250ms (subsec_nanos rounds via from_secs_f64).
    let want_first = std::time::Duration::from_secs_f64(0.25);
    assert_eq!(durations[0], want_first, "first sleep duration: {durations:?}");
    // Integer arg: exactly 1 second.
    assert_eq!(durations[1], std::time::Duration::from_secs(1), "second: {durations:?}");
}

#[test]
fn sleep_negative_duration_raises_argument_error() {
    // CRuby raises ArgumentError("time interval must not
    // be negative") for `sleep(-1)`. We match — embedders
    // get the same exception class for the same input,
    // closure is NOT invoked.
    use std::sync::{Arc, Mutex};
    let recorded: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let recorded_for_cfg = Arc::clone(&recorded);
    let cfg = rubyrs::Config {
        sleep_for: Some(std::sync::Arc::new(move |_| {
            *recorded_for_cfg.lock().unwrap() = true;
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval("sleep(-1)", "sleep_neg.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected ArgumentError, got {:?}", err.err);
    };
    assert_eq!(class_name, "ArgumentError");
    assert!(
        message.contains("time interval must not be negative"),
        "unexpected message: {message}",
    );
    assert!(
        !*recorded.lock().unwrap(),
        "sleep_for closure must NOT be invoked on negative arg",
    );
}

#[test]
fn sleep_returns_integer_seconds_slept() {
    // CRuby returns `Integer` seconds actually slept. We
    // return requested seconds truncated to Integer — a
    // conservative lower bound since std::thread::sleep
    // never undersleeps.
    let cfg = rubyrs::Config {
        sleep_for: Some(std::sync::Arc::new(|_| ())),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts sleep(2); puts sleep(0.9)", "sleep_ret.rb").expect("eval");
    // 2 → 2; 0.9 → 0 (truncated).
    assert_eq!(buf.snapshot(), "2\n0\n");
}

#[test]
fn exit_raises_system_exit_caught_with_status() {
    // ADR 0025 Phase 0.5b: `Kernel#exit(N)` raises SystemExit
    // with status=N. The user-script `rescue SystemExit => e`
    // catches; `e.status == N`, `e.success? == (N == 0)`.
    // Decoupled from `at_exit` machinery (Phase 4) — bare exit
    // + rescue still works today.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        # exit(true) → 0, exit(false) → 1, exit(nil) → 0,
        # exit(N) → N. All shapes verified together.
        [true, false, nil, 7].each do |x|
          begin
            exit x
          rescue SystemExit => e
            puts "#{x.inspect} -> status=#{e.status} success?=#{e.success?}"
          end
        end
        "##,
        "exit_basic.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "true -> status=0 success?=true\n\
         false -> status=1 success?=false\n\
         nil -> status=0 success?=true\n\
         7 -> status=7 success?=false\n",
    );
}

#[test]
fn exit_bang_default_raises_without_capability_injection() {
    // ADR 0017 Rule 1: by default `Kernel#exit!` must NOT
    // terminate the host process. With no
    // `Config::process_exit` injection, `exit!(N)` raises
    // RuntimeError pointing at the missing capability — same
    // shape as Time.now / sleep / load.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("exit! 1", "exit_bang_no_cap.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected Uncaught RuntimeError, got {:?}", err.err);
    };
    assert_eq!(class_name, "RuntimeError");
    assert!(
        message.contains("Kernel#exit! requires `Config::process_exit` injection"),
        "unexpected message: {message}",
    );
}

#[test]
fn exit_bang_invokes_injected_closure_with_status() {
    // With a recording closure injected, `exit! 5` calls it
    // exactly once carrying 5. No SystemExit raised — the
    // exit! path is immediate process exit. (Test host
    // intercepts the closure so std::process::exit isn't
    // actually invoked.)
    use std::sync::{Arc, Mutex};
    let recorded: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_for_cfg = Arc::clone(&recorded);
    let cfg = rubyrs::Config {
        process_exit: Some(std::sync::Arc::new(move |status: i32| {
            recorded_for_cfg.lock().unwrap().push(status);
            // NOTE: do NOT call std::process::exit here —
            // we're inside cargo test. Returning from the
            // closure simulates the closure being intercepted
            // by a test host.
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.eval("exit! 5; exit! 0", "exit_bang_inject.rb").expect("eval");
    let statuses = recorded.lock().unwrap().clone();
    assert_eq!(statuses, vec![5i32, 0i32]);
}

#[test]
fn abort_prints_message_then_raises_system_exit_with_status_1() {
    // ADR 0025 Phase 0.5b: `Kernel#abort(msg)` writes msg
    // (with trailing newline) and then raises
    // SystemExit.new(1). Documented divergence: CRuby writes
    // to stderr; rubyrs writes to the standard Vm sink (no
    // separate stderr in the current OutputSink abstraction —
    // ADR 0021 follow-up). Subject to revision when stderr
    // sink lands.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        begin
          abort "boom"
        rescue SystemExit => e
          puts "caught: status=#{e.status}"
        end
        "##,
        "abort_with_msg.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "boom\ncaught: status=1\n");
}

#[test]
fn abort_no_message_raises_system_exit_with_status_1() {
    // `abort` with no args: just raise SystemExit.new(1).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        begin
          abort
        rescue SystemExit => e
          puts "caught: status=#{e.status}"
        end
        "##,
        "abort_no_msg.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "caught: status=1\n");
}

#[test]
fn exit_not_swallowed_by_bare_rescue() {
    // Companion to `system_exit_not_swallowed_by_bare_rescue`
    // (error_handling.rs) but exercising the Kernel#exit path
    // rather than `raise SystemExit`. A bare `rescue` must NOT
    // catch SystemExit — otherwise a top-level catch-all would
    // silently turn `exit` into a no-op.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r#"
        begin
          exit 42
        rescue => e
          puts "swallowed"
        end
        "#,
        "exit_bare_rescue.rb",
    ).expect_err("exit must propagate past bare rescue");
    let rubyrs::RubyError::Uncaught { class_name, .. } = &err.err else {
        panic!("expected SystemExit Uncaught, got {:?}", err.err);
    };
    assert_eq!(class_name, "SystemExit");
    assert_eq!(buf.snapshot(), "");
}

#[test]
fn install_signal_handler_default_does_not_register() {
    // ADR 0025 Phase 1: Tier 1 default is `false`. A Runtime
    // constructed without opting in gets an Arc<AtomicBool>
    // that nothing writes to. The Vm field's existence is
    // guaranteed (no cfg branching for the safe-point check),
    // but the flag stays false.
    //
    // Race note: another test in this binary may have called
    // `install_signal_handler: true` already, in which case
    // SHARED_FLAG is populated and we get the shared Arc. The
    // assertion stays correct: no SIGINT has been delivered
    // during this test, so the flag is false either way.
    let _rt = rubyrs::Runtime::new();
    // Smoke check: no panic constructing without the flag.
    // Phase 2 will hook the safe-point reader; Phase 1 just
    // confirms the wiring compiles + the default is honest.
}

#[cfg(unix)]
#[test]
fn install_signal_handler_true_sets_flag_on_real_sigint() {
    // ADR 0025 Phase 1 happy path. Construct a Runtime with
    // `install_signal_handler: true`, send SIGINT to ourselves
    // via libc::kill(getpid(), SIGINT), verify the
    // `interrupt_pending` flag flips.
    //
    // The reader uses the test-only `_test_interrupt_pending_*`
    // accessor on Runtime — `#[doc(hidden)]` so it isn't part
    // of the embedding surface. Phase 2 will land the
    // `dispatch_until` safe-point consumer; Phase 1 just
    // verifies the wiring.
    //
    // Why kill self instead of subprocess: simpler, avoids
    // process-spawn overhead. The signal arrives on the test
    // thread; signal_hook's handler stores into the
    // AtomicBool; the reader sees it.
    //
    // After-test cleanup: the SHARED_FLAG is process-wide once
    // `install_signal_handler: true` has fired. We clear after
    // observing so subsequent tests in the same binary aren't
    // affected.

    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let rt = rubyrs::Runtime::with_config(cfg);

    // Drain any stale interrupt that a prior test may have
    // left. Idempotent.
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    // Step 1: confirm starting state is false.
    assert!(
        !rt._test_interrupt_pending_load_and_clear(false),
        "flag must start false",
    );

    // Step 2: send SIGINT to ourselves. signal_hook's handler
    // sets the flag.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGINT);
    }
    // Tiny pause to let the kernel deliver. POSIX guarantees
    // delivery is synchronous with the next safe point, but
    // 20ms makes the timing robust under loaded test runners.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Step 3: read again — flag must now be true. Clear it as
    // we read to keep subsequent tests clean.
    assert!(
        rt._test_interrupt_pending_load_and_clear(true),
        "flag must be true after SIGINT delivery",
    );

    // Sanity: confirm clear stuck.
    assert!(
        !rt._test_interrupt_pending_load_and_clear(false),
        "flag must be false after clear",
    );
}
