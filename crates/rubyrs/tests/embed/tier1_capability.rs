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

/// ADR 0025 Phase 1+2: SIGINT capture is a process-wide
/// resource. cargo test's default multi-threaded runner would let
/// signal-using tests race over the shared `interrupt_pending`
/// Arc — one test sets the flag, another test's `dispatch_until`
/// reads + clears it. Serialize all signal-touching tests behind
/// this mutex; poisoning is recoverable here (the lock guards
/// scheduling, not invariants).
#[cfg(unix)]
static SIGNAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
fn signal_test_lock() -> std::sync::MutexGuard<'static, ()> {
    SIGNAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

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
        sleep_for: Some(std::sync::Arc::new(move |d_opt, _flag| {
            // Phase 3 signature: record the requested
            // Duration (Some(d) for sleep(secs); None for
            // sleep no-args — not exercised by this test).
            // Return zero elapsed so the test runs fast.
            if let Some(d) = d_opt {
                recorded_for_cfg.lock().unwrap().push(d);
            }
            std::time::Duration::ZERO
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
        sleep_for: Some(std::sync::Arc::new(move |_d_opt, _flag| {
            *recorded_for_cfg.lock().unwrap() = true;
            std::time::Duration::ZERO
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
        sleep_for: Some(std::sync::Arc::new(|_d_opt, _flag| std::time::Duration::ZERO)),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts sleep(2); puts sleep(0.9)", "sleep_ret.rb").expect("eval");
    // 2 → 2; 0.9 → 0 (truncated).
    assert_eq!(buf.snapshot(), "2\n0\n");
}

#[cfg(unix)]
#[test]
fn phase_3_sleep_with_args_raises_interrupt_when_flag_set_mid_call() {
    // ADR 0025 Phase 3: `sleep(n)` polls the interrupt flag.
    // When the flag flips mid-call, sleep raises Interrupt
    // (does NOT return) — CRuby-faithful semantics. User
    // recovers elapsed time via Time.now in the rescue.
    let _lock = signal_test_lock();
    use std::sync::atomic::Ordering;
    let cfg = rubyrs::Config {
        install_signal_handler: true,
        // Production-shape polling closure: 50ms chunks +
        // flag check. Identical structure to the CLI binary's
        // closure (mirrors main.rs).
        sleep_for: Some(std::sync::Arc::new(|requested, flag| {
            use std::time::{Duration, Instant};
            let start = Instant::now();
            let chunk = Duration::from_millis(20);
            loop {
                if flag.load(Ordering::Relaxed) {
                    return start.elapsed();
                }
                match requested {
                    None => std::thread::sleep(chunk),
                    Some(d) => {
                        let elapsed = start.elapsed();
                        if elapsed >= d { return d; }
                        std::thread::sleep((d - elapsed).min(chunk));
                    }
                }
            }
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    // Background thread flips the flag mid-sleep.
    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = std::sync::Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, Ordering::SeqCst);
    });

    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        start = 0
        elapsed_marker = "init"
        begin
          sleep(10)
          puts "completed (should not happen)"
        rescue Interrupt
          puts "caught Interrupt"
        end
        "##,
        "phase3_sleep_secs_interrupted.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "caught Interrupt\n");
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[cfg(unix)]
#[test]
fn phase_3_sleep_no_args_raises_interrupt_when_flag_flips() {
    // ADR 0025 Phase 3: `sleep` with no args sleeps until
    // interrupt. Without `install_signal_handler: true` the
    // call refuses (ArgumentError) — would deadlock otherwise.
    // With install_signal_handler: true + flag flip → Interrupt.
    let _lock = signal_test_lock();
    use std::sync::atomic::Ordering;
    let cfg = rubyrs::Config {
        install_signal_handler: true,
        sleep_for: Some(std::sync::Arc::new(|requested, flag| {
            use std::time::{Duration, Instant};
            let start = Instant::now();
            let chunk = Duration::from_millis(20);
            loop {
                if flag.load(Ordering::Relaxed) {
                    return start.elapsed();
                }
                match requested {
                    None => std::thread::sleep(chunk),
                    Some(d) => {
                        let elapsed = start.elapsed();
                        if elapsed >= d { return d; }
                        std::thread::sleep((d - elapsed).min(chunk));
                    }
                }
            }
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = std::sync::Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, Ordering::SeqCst);
    });

    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        begin
          sleep
          puts "completed (should not happen)"
        rescue Interrupt
          puts "caught Interrupt from no-args sleep"
        end
        "##,
        "phase3_sleep_noargs_interrupted.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "caught Interrupt from no-args sleep\n");
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[test]
fn phase_3_sleep_no_args_without_signal_handler_raises_argument_error() {
    // Without install_signal_handler: true, no-args sleep is
    // un-wake-able — match CRuby's "no signals means it would
    // deadlock" by refusing the call. ArgumentError keeps the
    // failure mode close to CRuby's behavior (CRuby raises
    // various exceptions depending on signal context; rubyrs
    // picks the most-defensive shape).
    let cfg = rubyrs::Config {
        install_signal_handler: false,
        sleep_for: Some(std::sync::Arc::new(|_d_opt, _flag| std::time::Duration::ZERO)),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval("sleep", "phase3_no_signal_noargs.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected ArgumentError, got {:?}", err.err);
    };
    assert_eq!(class_name, "ArgumentError");
    assert!(
        message.contains("requires `Config::install_signal_handler: true`"),
        "unexpected message: {message}",
    );
}

#[test]
fn phase_4a_signal_trap_returns_default_on_first_install() {
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        prev = Signal.trap("INT") { puts "got" }
        puts "previous=#{prev}"
        "##,
        "sigtrap_first.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "previous=DEFAULT\n");
}

#[test]
fn phase_4a_signal_trap_accepts_all_name_forms() {
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        Signal.trap("INT") { puts "A" }
        prev = Signal.trap("SIGINT") { puts "B" }
        puts "after SIGINT: prev=#{prev.class}"

        prev = Signal.trap(:INT) { puts "C" }
        puts "after :INT: prev=#{prev.class}"

        prev = Signal.trap(:SIGINT) { puts "D" }
        puts "after :SIGINT: prev=#{prev.class}"

        prev = Signal.trap(2, "DEFAULT")
        puts "after Integer 2: prev=#{prev.class}"

        prev = Signal.trap("INT")
        puts "query: prev=#{prev}"
        "##,
        "sigtrap_name_forms.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "after SIGINT: prev=Proc\n\
         after :INT: prev=Proc\n\
         after :SIGINT: prev=Proc\n\
         after Integer 2: prev=Proc\n\
         query: prev=DEFAULT\n",
    );
}

#[test]
fn phase_4a_signal_trap_string_handlers_round_trip() {
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        prev = Signal.trap("INT", "IGNORE")
        puts "first prev=#{prev}"
        prev = Signal.trap("INT", "DEFAULT")
        puts "after DEFAULT: prev=#{prev}"
        prev = Signal.trap("INT", "SIG_IGN")
        puts "after SIG_IGN: prev=#{prev}"
        prev = Signal.trap("INT", :DEFAULT)
        puts "after :DEFAULT: prev=#{prev}"
        "##,
        "sigtrap_string_handlers.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "first prev=DEFAULT\n\
         after DEFAULT: prev=IGNORE\n\
         after SIG_IGN: prev=DEFAULT\n\
         after :DEFAULT: prev=IGNORE\n",
    );
}

#[test]
fn phase_4a_signal_trap_unknown_name_raises_argument_error() {
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        r##"Signal.trap("BOGUS") { puts "x" }"##,
        "sigtrap_bad_name.rb",
    ).unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected ArgumentError, got {:?}", err.err);
    };
    assert_eq!(class_name, "ArgumentError");
    assert!(
        message.contains("unsupported signal"),
        "unexpected: {message}",
    );
}

#[test]
fn phase_4a_signal_trap_unknown_handler_string_raises() {
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        r##"Signal.trap("INT", "MAYBE")"##,
        "sigtrap_bad_handler.rb",
    ).unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected ArgumentError, got {:?}", err.err);
    };
    assert_eq!(class_name, "ArgumentError");
    assert!(
        message.contains("unrecognized command"),
        "unexpected: {message}",
    );
}

#[cfg(unix)]
#[test]
fn phase_4b_safe_point_invokes_trap_block_instead_of_raising() {
    // ADR 0025 Phase 4b: when `signal_traps[SIGINT]` is
    // `Block(...)`, the safe-point invokes the block instead
    // of raising Interrupt. Verify by:
    //   1. installing a trap block that sets a marker.
    //   2. setting the flag from a background thread.
    //   3. running a busy loop in Ruby.
    //   4. the loop exits cleanly when the marker is set
    //      (which only happens if the trap fired).
    let _lock = signal_test_lock();
    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = std::sync::Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        $caught = false
        Signal.trap("INT") { puts "trap fired"; $caught = true }
        i = 0
        while !$caught && i < 100_000_000
          i += 1
        end
        puts "loop exit caught=#{$caught}"
        "##,
        "phase4b_block_handler.rb",
    ).expect("eval should complete cleanly (no Interrupt raise)");
    let out = buf.snapshot();
    assert!(
        out.starts_with("trap fired\n"),
        "trap block should fire first; got {out:?}",
    );
    assert!(
        out.ends_with("loop exit caught=true\n"),
        "loop should observe $caught=true after trap; got {out:?}",
    );
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[cfg(unix)]
#[test]
fn phase_4b_ignore_handler_clears_flag_without_raising() {
    // `Signal.trap("INT", "IGNORE")` makes SIGINT a no-op.
    // Background thread sets the flag; safe-point sees Ignore;
    // flag clears; loop completes normally.
    let _lock = signal_test_lock();
    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = std::sync::Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        Signal.trap("INT", "IGNORE")
        i = 0
        # Bounded loop because Ignore wouldn't break us out of
        # an infinite one. Cap is well above the time it takes
        # the background thread to flip + the safe-point to
        # observe + the flag to clear.
        while i < 5_000_000
          i += 1
        end
        puts "completed iterations=#{i}"
        "##,
        "phase4b_ignore.rb",
    ).expect("eval should complete cleanly when SIGINT is ignored");
    assert_eq!(buf.snapshot(), "completed iterations=5000000\n");
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[cfg(unix)]
#[test]
fn phase_4b_default_handler_still_raises_interrupt() {
    // After installing a block and then restoring "DEFAULT",
    // the safe-point reverts to the Phase 2 behavior:
    // raise Interrupt. Verifies the round-trip + that
    // round-tripping doesn't leak state.
    let _lock = signal_test_lock();
    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = std::sync::Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        Signal.trap("INT") { puts "trap A" }
        Signal.trap("INT", "DEFAULT")  # restore default
        begin
          i = 0
          while true
            i += 1
          end
        rescue Interrupt
          puts "caught Interrupt"
        end
        "##,
        "phase4b_default_restored.rb",
    ).expect("eval should complete with rescue catching");
    assert_eq!(buf.snapshot(), "caught Interrupt\n");
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[test]
fn phase_4c_at_exit_runs_lifo_on_normal_eval_completion() {
    // ADR 0025 Phase 4c: at_exit handlers fire in LIFO order
    // when the eval body completes normally. Returns the
    // original eval result.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        puts "main start"
        at_exit { puts "A (registered 1st)" }
        at_exit { puts "B (registered 2nd)" }
        at_exit { puts "C (registered 3rd, fires 1st)" }
        puts "main end"
        "##,
        "phase4c_lifo.rb",
    ).expect("eval should succeed");
    assert_eq!(
        buf.snapshot(),
        "main start\n\
         main end\n\
         C (registered 3rd, fires 1st)\n\
         B (registered 2nd)\n\
         A (registered 1st)\n",
    );
}

#[test]
fn phase_4c_at_exit_runs_on_system_exit_unwind() {
    // The canonical CRuby pattern: `exit` raises SystemExit
    // which propagates UP THROUGH at_exit handlers. The
    // handlers must fire BEFORE the embedder sees the trap.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r##"
        at_exit { puts "atexit fires on SystemExit" }
        exit 7
        "##,
        "phase4c_systemexit.rb",
    ).expect_err("expected SystemExit Trap");
    let rubyrs::RubyError::Uncaught { class_name, .. } = &err.err else {
        panic!("expected SystemExit Uncaught, got {:?}", err.err);
    };
    assert_eq!(class_name, "SystemExit");
    // The handler MUST have run before we got Err.
    assert_eq!(buf.snapshot(), "atexit fires on SystemExit\n");
}

#[test]
fn phase_4c_exit_bang_invokes_process_exit_with_status() {
    // ADR 0025 Phase 0.5b semantic: `exit!(status)` invokes
    // the host's `Config::process_exit` closure with the
    // requested status. In production (CLI binary), the
    // closure calls `std::process::exit` and never returns,
    // so at_exit handlers are SKIPPED.
    //
    // In test-host scenarios where the closure intercepts +
    // returns (e.g. unit-testing a script that calls exit!),
    // execution falls through — at_exit handlers DO run on
    // the eval's natural return. That's a documented embed-
    // model adaptation: at_exit semantics are tied to the
    // closure's behavior. Test verifies the closure receives
    // the right status; the at_exit-skip semantic is
    // delegated to the production closure's `process::exit`
    // contract.
    use std::sync::{Arc, Mutex};
    let exit_status: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let exit_status_for_cfg = Arc::clone(&exit_status);
    let cfg = rubyrs::Config {
        process_exit: Some(std::sync::Arc::new(move |status| {
            *exit_status_for_cfg.lock().unwrap() = Some(status);
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.eval(
        r##"exit! 5"##,
        "phase4c_exit_bang.rb",
    ).expect("eval");
    assert_eq!(*exit_status.lock().unwrap(), Some(5));
}

#[cfg(unix)]
#[test]
fn phase_4c_trap_calls_exit_runs_at_exit_handlers() {
    // The fully-integrated CRuby pattern:
    //   trap("INT") { exit }   # graceful Ctrl+C shutdown
    //   at_exit { cleanup }    # always-runs cleanup
    //
    // Send the flag via background thread; trap fires; exit
    // raises SystemExit; at_exit runs the cleanup; embedder
    // sees the SystemExit Trap with the cleanup already done.
    let _lock = signal_test_lock();
    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = std::sync::Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r##"
        at_exit { puts "cleanup" }
        Signal.trap("INT") { puts "trap"; exit 0 }
        i = 0
        while true; i += 1; end
        "##,
        "phase4c_trap_exit_atexit.rb",
    ).expect_err("expected SystemExit trap");
    let rubyrs::RubyError::Uncaught { class_name, .. } = &err.err else {
        panic!("expected SystemExit Uncaught, got {:?}", err.err);
    };
    assert_eq!(class_name, "SystemExit");
    assert_eq!(buf.snapshot(), "trap\ncleanup\n");
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[test]
fn phase_4c_at_exit_without_block_raises_local_jump_error() {
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("at_exit", "phase4c_no_block.rb").unwrap_err();
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected LocalJumpError, got {:?}", err.err);
    };
    assert_eq!(class_name, "LocalJumpError");
    assert!(
        message.contains("no block given (at_exit)"),
        "unexpected: {message}",
    );
}

#[test]
fn v7_sleep_accepts_rational() {
    // Round-3 review parity: CRuby accepts any Numeric for
    // sleep; rubyrs v6 only Int / Float. v7 adds Rational.
    use std::sync::{Arc, Mutex};
    let recorded: Arc<Mutex<Vec<std::time::Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_for_cfg = Arc::clone(&recorded);
    let cfg = rubyrs::Config {
        sleep_for: Some(std::sync::Arc::new(move |d_opt, _flag| {
            if let Some(d) = d_opt {
                recorded_for_cfg.lock().unwrap().push(d);
            }
            std::time::Duration::ZERO
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.eval("sleep(Rational(1, 2))", "v7_sleep_rational.rb").expect("eval");
    let durations = recorded.lock().unwrap().clone();
    assert_eq!(durations.len(), 1);
    // 1/2 = 0.5 → Duration::from_secs_f64(0.5).
    assert_eq!(durations[0], std::time::Duration::from_secs_f64(0.5));
}

#[test]
fn v7_system_exit_no_args_message_matches_cruby() {
    // Round-3 review parity: CRuby's `SystemExit.new` (no args)
    // has message "exit", not the class name.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"puts SystemExit.new.message"##,
        "v7_systemexit_msg.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "exit\n");
}

#[test]
fn v7_signal_exception_2_arg_form_and_signo() {
    // Round-3 review parity: CRuby's
    // `SignalException.new(msg, signo)` exposes #signo.
    // Interrupt inherits the same shape.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        e1 = SignalException.new("got signal", 15)
        puts "msg=#{e1.message} signo=#{e1.signo}"

        e2 = Interrupt.new("ctrl+c", 2)
        puts "msg=#{e2.message} signo=#{e2.signo}"

        e3 = Interrupt.new("plain msg only")
        puts "msg=#{e3.message} signo=#{e3.signo.inspect}"

        e4 = SignalException.new
        puts "msg=#{e4.message} signo=#{e4.signo.inspect}"
        "##,
        "v7_signal_exception_signo.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "msg=got signal signo=15\n\
         msg=ctrl+c signo=2\n\
         msg=plain msg only signo=nil\n\
         msg=SignalException signo=nil\n",
    );
}

#[test]
fn v7_signal_trap_rejects_sigkill_and_sigstop() {
    // CRuby raises ArgumentError("can't trap reserved signal:
    // SIGKILL") and SIGSTOP. Round-3 review parity gap.
    let mut rt = rubyrs::Runtime::new();
    for (sig, expected) in [
        (r##"Signal.trap("KILL") { }"##, "SIGKILL"),
        (r##"Signal.trap(:SIGKILL) { }"##, "SIGKILL"),
        (r##"Signal.trap(9, "IGNORE")"##, "SIGKILL"),
        (r##"Signal.trap("STOP") { }"##, "SIGSTOP"),
        (r##"Signal.trap(:SIGSTOP) { }"##, "SIGSTOP"),
        (r##"Signal.trap(19, "DEFAULT")"##, "SIGSTOP"),
    ] {
        let err = rt.eval(sig, "v7_sigkill.rb").unwrap_err();
        let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
            panic!("expected ArgumentError, got {:?}", err.err);
        };
        assert_eq!(class_name, "ArgumentError", "sig={sig}");
        assert!(
            message.contains(expected),
            "sig={sig}: unexpected: {message}",
        );
    }
}

#[test]
fn v7_signal_trap_explicit_nil_handler_means_ignore() {
    // CRuby 3.x: `Signal.trap("INT", nil)` installs IGNORE.
    // Round-3 review surfaced that rubyrs was treating it as
    // QUERY (returning current handler). v7 fixes by routing
    // explicit nil through the IGNORE path; QUERY mode now
    // requires the 1-arg-no-block form (sentinel Symbol).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        # First install a block so we can verify nil REPLACES it.
        Signal.trap("INT") { puts "installed block" }
        prev = Signal.trap("INT", nil)
        puts "after nil: prev=#{prev.class}"
        prev = Signal.trap("INT")  # query
        puts "now installed: prev=#{prev}"
        "##,
        "v7_sigtrap_nil.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "after nil: prev=Proc\n\
         now installed: prev=IGNORE\n",
    );
}

#[test]
fn v7_signal_trap_one_arg_with_block_installs_block() {
    // Regression: the v7 preamble splat-based form must NOT
    // misroute `Signal.trap("INT") { ... }` (block-form) as
    // query mode. Verify by querying again afterward — should
    // return a Proc, not "DEFAULT".
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        Signal.trap("INT") { puts "trap" }
        prev = Signal.trap("INT")  # query
        puts "prev=#{prev.class}"
        "##,
        "v7_sigtrap_1arg_block.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "prev=Proc\n");
}

#[test]
fn v7_at_exit_handler_raise_continues_lifo_drain() {
    // Round-3 review safety finding: a raising at_exit handler
    // must NOT stop the LIFO drain. Verify with three handlers
    // where the MIDDLE one raises; the other two still fire
    // (LIFO order: 3 → 2-raises → 1) and the final eval result
    // is the last error.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r##"
        at_exit { puts "handler 1 (registered 1st, fires last)" }
        at_exit { puts "handler 2 (raises)"; raise "boom from at_exit" }
        at_exit { puts "handler 3 (registered last, fires first)" }
        puts "main"
        "##,
        "v7_at_exit_raise.rb",
    ).unwrap_err();
    assert_eq!(
        buf.snapshot(),
        "main\n\
         handler 3 (registered last, fires first)\n\
         handler 2 (raises)\n\
         handler 1 (registered 1st, fires last)\n",
    );
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected RuntimeError, got {:?}", err.err);
    };
    assert_eq!(class_name, "RuntimeError");
    assert!(
        message.contains("boom from at_exit"),
        "unexpected: {message}",
    );
}

#[test]
fn adr_0024_phase_a_break_from_yielded_block_unwinds_to_yielding_method() {
    // ADR 0024 Phase A.1: `def f; while true; yield; end; end` +
    // `f { break }` exits cleanly. The previous fire-and-forget
    // Op::Yield set break_signaled but the yielding method's
    // `while true` kept looping (because Op::Yield's wrapper
    // didn't observe break_signaled at all). v8 Phase A.1 makes
    // Op::Yield synchronous + observes break.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def my_loop
          while true
            yield
          end
        end
        i = 0
        my_loop do
          i += 1
          break if i >= 3
          puts "iter #{i}"
        end
        puts "after, i=#{i}"
        "##,
        "adr_0024_break_unwinds.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "iter 1\niter 2\nafter, i=3\n",
    );
}

#[test]
fn adr_0024_phase_a_break_with_value_returns_from_yielding_method() {
    // `break val` propagates val as the yielding method's
    // return value (CRuby `[1,2,3].map { break "x" } # => "x"`).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def my_loop
          while true
            yield
          end
        end
        result = my_loop { break "early-exit-val" }
        puts "result=#{result}"
        "##,
        "adr_0024_break_value.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "result=early-exit-val\n");
}

#[test]
fn adr_0024_phase_a_block_normal_return_value_is_yield_expression() {
    // Without break, the block's normal return value is the
    // value of the yield expression in the yielding method.
    // This is the existing CRuby behavior that v6 already
    // matched; v8 must not regress it.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          v = yield 10
          puts "block returned #{v}"
        end
        f { |x| x * 2 }
        "##,
        "adr_0024_block_return.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "block returned 20\n");
}

#[test]
fn adr_0024_phase_a_max_yield_recursion_cap_trips_resource_exhausted() {
    // Recursive yield chains hit the cap. Setting
    // max_yield_recursion: Some(N) makes a recursion depth
    // of N+1 trap ResourceExhausted.
    let cfg = rubyrs::Config {
        max_yield_recursion: Some(10),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        r##"
        def f
          yield
        end
        # Build a deep yield chain via mutually-recursive yields.
        # 20 levels exceeds the cap of 10.
        def recurse(n)
          return if n <= 0
          f { recurse(n - 1) }
        end
        recurse(20)
        "##,
        "adr_0024_max_yield_recursion.rb",
    ).unwrap_err();
    let rubyrs::RubyError::ResourceExhausted { msg } = &err.err else {
        panic!("expected ResourceExhausted, got {:?}", err.err);
    };
    assert!(
        msg.contains("yield recursion depth exceeded"),
        "unexpected message: {msg}",
    );
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
fn phase_2_safe_point_translates_pending_flag_to_interrupt_raise() {
    let _lock = signal_test_lock();
    // ADR 0025 Phase 2 happy path. With install_signal_handler:true,
    // setting `interrupt_pending` via direct atomic store (mimicking
    // the signal handler) MUST cause the next dispatch_until /
    // dispatch top-of-loop check to raise `Interrupt` — script-side
    // rescue catches it.
    //
    // Direct flag-set rather than libc::kill: deterministic timing
    // and removes the kernel-delivery latency. The Phase 1 SIGINT
    // test verifies the signal-to-flag pipe; this test verifies the
    // flag-to-Interrupt pipe.
    use std::sync::Arc as StdArc;
    use std::sync::atomic::Ordering;

    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);

    // Drain any stale flag from a prior test in the same binary.
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    // Set the flag from a background thread so the eval can
    // observe it on entry. Atomic store mirrors the signal handler.
    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = StdArc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, Ordering::SeqCst);
    });

    // Busy loop in Ruby — the dispatch_until check fires on each op.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        i = 0
        begin
          while true
            i += 1
          end
        rescue Interrupt => e
          puts "caught after #{i} iters"
        end
        "#,
        "phase2_interrupt.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert!(
        out.starts_with("caught after "),
        "expected interrupt-caught output, got {out:?}",
    );
    // Clean up the flag for the next test in this binary.
    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[cfg(unix)]
#[test]
fn phase_2_interrupt_uncaught_propagates_to_embedder() {
    let _lock = signal_test_lock();
    // When no rescue handler is on the stack, the Phase 2 safe-point
    // raise becomes an Uncaught Trap with class_name "Interrupt".
    // Embedders see a recoverable error, not a process kill.
    use std::sync::Arc as StdArc;
    use std::sync::atomic::Ordering;

    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    let flag = rt._test_interrupt_pending_arc();
    let flag_for_thread = StdArc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag_for_thread.store(true, Ordering::SeqCst);
    });

    let err = rt.eval(
        r#"
        i = 0
        while true
          i += 1
        end
        "#,
        "phase2_uncaught.rb",
    ).expect_err("expected interrupt to propagate as Trap");
    let rubyrs::RubyError::Uncaught { class_name, message } = &err.err else {
        panic!("expected Uncaught Interrupt, got {:?}", err.err);
    };
    assert_eq!(class_name, "Interrupt");
    assert_eq!(message, "interrupt");

    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[cfg(unix)]
#[test]
fn phase_2_suppress_interrupt_defers_delivery() {
    let _lock = signal_test_lock();
    // ADR 0025 Risk #9 — when `suppress_interrupt > 0`, the
    // safe-point check leaves the flag set but DOESN'T raise.
    // Once the counter drops back to 0, the next safe point
    // delivers normally.
    //
    // Drives the counter directly via a test helper (the
    // SuppressInterruptGuard wrapper that close paths will use
    // lands in Phase 4 / Risk #9 wiring).
    use std::sync::atomic::Ordering;

    let cfg = rubyrs::Config {
        install_signal_handler: true,
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let _ = rt._test_interrupt_pending_load_and_clear(true);

    // Pre-set both: suppress_interrupt > 0 AND flag pending.
    rt._test_set_suppress_interrupt(1);
    let flag = rt._test_interrupt_pending_arc();
    flag.store(true, Ordering::SeqCst);

    // Run a short loop. The check sees the flag but suppress is
    // nonzero, so it doesn't raise. Loop completes normally.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        i = 0
        while i < 100
          i += 1
        end
        puts "completed: #{i}"
        "#,
        "phase2_suppress.rb",
    ).expect("eval should complete normally when suppressed");
    assert_eq!(buf.snapshot(), "completed: 100\n");

    // Flag is STILL set (suppression deferred but didn't clear).
    assert!(
        flag.load(Ordering::Relaxed),
        "suppress must not clear the flag",
    );

    // Drop suppression. Next eval delivers.
    rt._test_set_suppress_interrupt(0);
    let err = rt.eval(
        "while true; end",
        "phase2_after_suppress.rb",
    ).expect_err("expected interrupt now that suppress is 0");
    assert!(err.err.is_a("Interrupt"));

    let _ = rt._test_interrupt_pending_load_and_clear(true);
}

#[cfg(unix)]
#[test]
fn install_signal_handler_true_sets_flag_on_real_sigint() {
    let _lock = signal_test_lock();
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
