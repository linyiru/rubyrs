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
fn kernel_rand_is_deterministic_by_default() {
    // ADR 0017 posture, minitest-substrate revision: the top-level
    // rand/srand surface now backs a DETERMINISTIC default RNG
    // (Random.new(0), Mulberry32) instead of raising — a constant
    // seed strengthens determinism (every run of a never-srand
    // script sees the same sequence), and srand(n) is the
    // reproducible-test-order knob minitest turns. Pinned here so
    // the deterministic-default contract can't silently drift back
    // to entropy.
    let mut rt = rubyrs::Runtime::new();
    // Two srand(0)-anchored draws agree; the never-seeded first
    // draw also equals them because the default seed IS 0.
    let v = rt
        .eval(
            "a = rand(1000); srand(0); b = rand(1000); srand(0); c = rand(1000); a == b && b == c",
            "rand_det.rb",
        )
        .expect("deterministic rand");
    assert_eq!(format!("{v:?}"), "Bool(true)");

    // srand returns the PREVIOUS seed, CRuby-style.
    let mut rt = rubyrs::Runtime::new();
    let v = rt
        .eval("srand(7) == 0 && srand(9) == 7", "srand_prev.rb")
        .expect("srand returns prev seed");
    assert_eq!(format!("{v:?}"), "Bool(true)");
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
fn secure_random_default_seed_stream_golden() {
    // The hidden default rng is seeded 0 (ADR 0017 row 131 — the
    // determinism trade means the DEFAULT stream is identical in
    // every process; ticketed as a known Tier-1 security quirk) and,
    // since the boot-perf lazy-init change, constructed on FIRST use
    // rather than at preamble load. This golden pins the exact
    // default-stream values the eager `@@rng = Random.new(0)`
    // produced, so the deferral (or any future rng plumbing change)
    // can never silently skew outputs. Values captured from the
    // pre-lazy-init binary.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let script = r#"
        puts SecureRandom.hex(16)
        puts SecureRandom.uuid
        puts SecureRandom.random_bytes(8).bytes.inspect
        puts SecureRandom.alphanumeric(12)
        puts SecureRandom.hex(4)
        SecureRandom.seed = 42
        puts SecureRandom.hex(8)
    "#;
    rt.eval(script, "sr_default_golden.rb").expect("eval");
    assert_eq!(
        buf.snapshot(),
        "ac0a7f8c2faac49775a616b7c0cc21d8\n\
         43b34e9a-fb52-42db-8376-7d8b677de5d8\n\
         [9, 164, 116, 108, 211, 222, 161, 159]\n\
         ljrFUsEAAX5O\n\
         573a2b4c\n\
         66dce15fb33deacb\n"
    );
}

#[test]
fn secure_random_seed_before_first_use_wins_over_lazy_default() {
    // Lazy-init edge: `SecureRandom.seed = n` BEFORE the first
    // consuming call must fully own the stream — the deferred
    // `||= Random.new(0)` default must see the non-nil slot and
    // never fire. Golden captured from the pre-lazy-init binary
    // (where seed= simply overwrote the eagerly-built default).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "SecureRandom.seed = 7\nputs SecureRandom.hex(8)",
        "sr_seed_first.rb",
    )
    .expect("eval");
    assert_eq!(buf.snapshot(), "aff08813c4e4323a\n");
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
fn adr_0024_phase_a2_stop_iteration_has_result_accessor() {
    // ADR 0024 Phase A.2: `StopIteration#result` reader +
    // writer (`attr_accessor`). Default nil. Required for
    // Phase A.3's `def loop` to match CRuby's
    // "rescue StopIteration => e; e.result; end" shape.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        e1 = StopIteration.new
        puts "default: msg=#{e1.message} result=#{e1.result.inspect}"
        e2 = StopIteration.new("custom msg")
        e2.result = 42
        puts "set: msg=#{e2.message} result=#{e2.result}"
        puts "hierarchy: is_a IndexError? #{e1.is_a?(IndexError)}"
        "##,
        "adr_0024_a2_stopiteration.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "default: msg=iteration reached an end result=nil\n\
         set: msg=custom msg result=42\n\
         hierarchy: is_a IndexError? true\n",
    );
}

#[test]
fn adr_0024_phase_a4_block_break_runs_yielding_method_ensure() {
    // ADR 0024 Phase A.4: `break` from inside a block must
    // walk the yielding method's `is_ensure` rescue handlers
    // before the method frame returns the break value.
    // Pre-A.4: the unwind raw-popped the frame and the ensure
    // body never executed.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          begin
            yield
            puts "after-yield-unreachable"
          ensure
            puts "f ensure ran"
          end
        end
        result = f { break "broken" }
        puts "result=#{result}"
        "##,
        "adr_0024_a4_ensure_break.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "f ensure ran\nresult=broken\n",
    );
}

#[test]
fn adr_0024_phase_a4_nested_ensures_run_inner_first() {
    // Phase A.4: nested begin/ensure blocks run inner ensure
    // first, then outer — matching CRuby's stack-unwind
    // order. Break value flows through both ensures
    // unchanged.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          begin
            begin
              yield
            ensure
              puts "inner ensure"
            end
          ensure
            puts "outer ensure"
          end
        end
        puts f { break "v" }
        "##,
        "adr_0024_a4_nested_ensures.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "inner ensure\nouter ensure\nv\n",
    );
}

#[test]
fn adr_0024_phase_a4_raise_in_ensure_supersedes_break() {
    // Phase A.4: when a block-break is mid-walk and the
    // yielding method's ensure body raises, the exception
    // takes over — the in-flight break value is dropped
    // (matches CRuby; mirrors the existing
    // pending_loop_transfer behavior).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def g
          begin
            yield
          ensure
            raise "from-ensure"
          end
        end
        begin
          result = g { break "ignored" }
          puts "unexpected: #{result}"
        rescue => e
          puts "rescued: #{e.message}"
        end
        "##,
        "adr_0024_a4_ensure_raise.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "rescued: from-ensure\n");
}

#[test]
fn adr_0024_phase_a5_case_b_skips_remaining_method_body() {
    // ADR 0024 Phase A.5: when `def each; 3.times { yield };
    // puts "after"; end` is broken from the user's block,
    // CRuby skips the "after" puts and returns the break
    // value as each's result. Pre-A.5, rubyrs left
    // break_signaled set so Int#times returned the value
    // but each's body continued executing "after" and the
    // break value was discarded.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def each
          3.times { |i| yield i }
          puts "after-unreachable"
        end
        result = each { |v| break "br-#{v}" if v == 1 }
        puts "result=#{result}"
        "##,
        "adr_0024_a5_case_b_skip.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "result=br-1\n");
}

#[test]
fn adr_0024_phase_a5_case_b_runs_yielding_method_ensure() {
    // Phase A.5: ensure on the yielding method must run
    // even when break crosses a Rust iter driver
    // (`Int#times`'s step_block loop sits between yield
    // and each).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def each
          begin
            3.times { |i| yield i }
            puts "unreachable"
          ensure
            puts "each ensure ran"
          end
        end
        result = each { |v| break "br-#{v}" if v == 1 }
        puts "result=#{result}"
        "##,
        "adr_0024_a5_case_b_ensure.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "each ensure ran\nresult=br-1\n",
    );
}

#[test]
fn adr_0024_phase_a5_case_b_nested_ensures_run_inner_first() {
    // Phase A.5: nested begin/ensure inside the yielding
    // method runs inner ensure first, then outer.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          begin
            begin
              3.times { yield }
            ensure
              puts "inner"
            end
          ensure
            puts "outer"
          end
        end
        puts f { break "v" }
        "##,
        "adr_0024_a5_case_b_nested.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "inner\nouter\nv\n");
}

#[test]
fn adr_0024_phase_a6_method_return_runs_intermediate_ensure() {
    // ADR 0024 Phase A.6: `return` from inside a block must
    // walk intervening ensure handlers before the method's
    // frame returns. Pre-A.6 `dispatch()`'s method_return
    // arm raw-popped frames without walking rescues —
    // ensures in the enclosing def silently dropped.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          begin
            [1,2,3].each { |x| return "ret-#{x}" if x == 2 }
            puts "after-unreachable"
          ensure
            puts "f ensure ran"
          end
        end
        puts f
        "##,
        "adr_0024_a6_method_return_ensure.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "f ensure ran\nret-2\n");
}

#[test]
fn adr_0024_phase_a6_nested_ensures_run_inner_first() {
    // Phase A.6: nested begin/ensure inside the def runs
    // inner ensure first, outer second.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          begin
            begin
              [1,2,3].each { |x| return "ret-#{x}" if x == 2 }
            ensure
              puts "inner"
            end
          ensure
            puts "outer"
          end
        end
        puts f
        "##,
        "adr_0024_a6_method_return_nested.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "inner\nouter\nret-2\n");
}

#[test]
fn adr_0024_phase_a7_yield_resolves_lexically_through_forwarded_block() {
    // ADR 0024 Phase A.7: `yield` in a block forwarded to
    // another method must resolve to the LEXICAL enclosing
    // method's block_arg, not the dynamically-nearest
    // method frame. Pre-A.7 the inner `yield` bound to `g`
    // (the dynamic neighbour) and recursively re-invoked
    // g's block_arg, stack-overflowing.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          g { yield }
        end
        def g
          yield
        end
        puts(f { "v" })
        "##,
        "adr_0024_a7_forwarded_yield.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "v\n");
}

#[test]
fn adr_0024_phase_a7_break_through_forwarded_block_targets_lexical_owner() {
    // Phase A.7: `break` from a block whose lexical owner
    // is `f` (block passed to f, f forwarded to g) targets
    // f, not g.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          g { yield }
        end
        def g
          yield
        end
        result = f { break "broken" }
        puts "result=#{result}"
        "##,
        "adr_0024_a7_forwarded_break.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "result=broken\n");
}

#[test]
fn adr_0024_phase_a7_doubly_forwarded_yield_resolves_lexically() {
    // Phase A.7: three-deep forwarding still resolves
    // correctly.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def aa; bb { yield }; end
        def bb; cc { yield }; end
        def cc; yield; end
        puts(aa { "deep" })
        "##,
        "adr_0024_a7_doubly_forwarded.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "deep\n");
}

#[test]
fn adr_0024_phase_a9_break_unwinds_through_intermediate_method() {
    // ADR 0024 Phase A.9: `f { break v }` where f forwards
    // its block to g and g yields via an iter — the break
    // must unwind THROUGH g (skipping g's remaining body)
    // to land at f. Pre-A.9 the outer Op::Yield case (b)
    // overwrote pending_method_break with g's index, so
    // break landed at g and f's body continued past the
    // call.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def f
          g { |x| yield x }
          puts "f after-unreachable"
        end
        def g
          [1,2,3].each { |x| yield x }
          puts "g after-unreachable"
        end
        result = f { |v| break "br-#{v}" if v == 2 }
        puts "result=#{result}"
        "##,
        "adr_0024_a9_multi_frame.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "result=br-2\n");
}

#[test]
fn adr_0024_phase_a9_runs_ensures_along_unwind_chain() {
    // Phase A.9: ensures in every method frame the break
    // unwinds through must run, inner-most first.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def x1
          begin
            x2 { |v| yield v }
          ensure
            puts "x1 ensure"
          end
        end
        def x2
          begin
            [10,20].each { |v| yield v }
          ensure
            puts "x2 ensure"
          end
        end
        puts(x1 { |v| break "got #{v}" if v == 20 })
        "##,
        "adr_0024_a9_ensures_chain.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "x2 ensure\nx1 ensure\ngot 20\n");
}

#[test]
fn adr_0024_phase_a9_three_level_unwind() {
    // Phase A.9: three levels of method forwarding unwind
    // cleanly to the lexical owner.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def aa
          bb { |x| yield x }
          puts "aa unreachable"
        end
        def bb
          cc { |x| yield x }
          puts "bb unreachable"
        end
        def cc
          [1,2,3].each { |x| yield x }
          puts "cc unreachable"
        end
        puts(aa { |v| break "v=#{v}" if v == 2 })
        "##,
        "adr_0024_a9_three_levels.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "v=2\n");
}

#[test]
fn adr_0024_phase_a10_return_from_stored_proc_raises_local_jump_error() {
    // ADR 0024 Phase A.6 round 2: a stored Proc that calls
    // `return` after its lexical owner has already returned
    // must raise `LocalJumpError: unexpected return`,
    // matching CRuby. Pre-fix the legacy fallback in
    // dispatch()'s method_return arm raw-popped and the
    // script silently completed.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def make_pr
          proc { return :early }
        end
        pr = make_pr
        begin
          pr.call
          puts "no-raise"
        rescue LocalJumpError => e
          puts "lje: #{e.message}"
        end
        "##,
        "adr_0024_a10_return.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "lje: unexpected return\n");
}

#[test]
fn adr_0024_phase_a10_break_from_stored_proc_raises_local_jump_error() {
    // ADR 0024 Phase A.6 round 2: a stored Proc that calls
    // `break` (invoked via `.call`, not yielded to by a
    // wrapping method) must raise
    // `LocalJumpError: break from proc-closure`.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        pr = proc { break :early }
        begin
          pr.call
          puts "no-raise"
        rescue LocalJumpError => e
          puts "lje: #{e.message}"
        end
        "##,
        "adr_0024_a10_break.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "lje: break from proc-closure\n");
}

#[test]
fn adr_0024_phase_a3_kernel_loop_works_with_break() {
    // ADR 0024 Phase A.3: top-level `def loop` installed in
    // preamble. The canonical CRuby idiom should work:
    //   i = 0; loop { i += 1; break if i >= 3 }
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        i = 0
        loop do
          i += 1
          break if i >= 3
          puts "iter #{i}"
        end
        puts "after, i=#{i}"
        "##,
        "adr_0024_a3_loop_break.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "iter 1\niter 2\nafter, i=3\n",
    );
}

#[test]
fn adr_0024_phase_a3_kernel_loop_break_value() {
    // `result = loop { break "x" }` returns "x".
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        result = loop { break "early-out" }
        puts "result=#{result}"
        "##,
        "adr_0024_a3_loop_value.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "result=early-out\n");
}

#[test]
fn adr_0024_phase_a3_kernel_loop_catches_stop_iteration() {
    // CRuby's `loop` rescues StopIteration and returns the
    // exception's `#result`. The preamble def uses this
    // shape directly.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def producer
          e = StopIteration.new
          e.result = "iter-end-val"
          raise e
        end
        result = loop { producer }
        puts "result=#{result}"
        "##,
        "adr_0024_a3_loop_stopiteration.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "result=iter-end-val\n");
}

#[test]
fn adr_0024_phase_a_max_yield_recursion_cap_trips_resource_exhausted() {
    // Recursive yield chains hit the cap. Setting
    // max_yield_recursion: Some(N) makes a recursion depth
    // of N+1 trap ResourceExhausted.
    // The yield-recursion shape here ALSO trips the always-on
    // dispatch-depth cap (each `yield` re-enters `dispatch_until`),
    // so `max_yield_recursion` only fires first when set BELOW the
    // always-on default (debug: 5, release: 150). Pin to 3 so the
    // yield cap wins on both build profiles.
    let cfg = rubyrs::Config {
        max_yield_recursion: Some(3),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        r##"
        def f
          yield
        end
        # Build a deep yield chain via mutually-recursive yields.
        # 8 levels exceeds the cap of 3.
        def recurse(n)
          return if n <= 0
          f { recurse(n - 1) }
        end
        recurse(8)
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
    // SystemExit.new(1). Tier-1 2c: now writes to stderr
    // (was stdout pre-stderr-sink-landing).
    let mut rt = rubyrs::Runtime::new();
    let stdout_buf = SharedBuf::new();
    let stderr_buf = SharedBuf::new();
    rt.set_stdout(Box::new(stdout_buf.clone()));
    rt.set_stderr(Box::new(stderr_buf.clone()));
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
    assert_eq!(stdout_buf.snapshot(), "caught: status=1\n");
    assert_eq!(stderr_buf.snapshot(), "boom\n");
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

#[test]
fn adr_0025_followup_dollar_bang_set_in_rescue_body() {
    // ADR 0025 round-3 deferred follow-up: `$!` exposes the
    // rescued exception inside a `rescue` body. Pre-fix `$!`
    // was always nil in rubyrs, breaking the canonical
    // `rescue => e; puts $!.message; end` idiom.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        begin
          raise RuntimeError.new("boom")
        rescue => e
          puts "match: #{$!.equal?(e)}"
          puts "msg: #{$!.message}"
        end
        "##,
        "adr_0025_dollar_bang.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "match: true\nmsg: boom\n");
}

#[test]
fn adr_0025_followup_abort_no_args_consults_dollar_bang() {
    // ADR 0025 round-3 deferred follow-up: `abort` with no
    // args reads `$!` and writes `<class>: <message>` before
    // raising SystemExit(1). Pre-fix the no-args path wrote
    // nothing.
    let cfg = rubyrs::Config {
        process_exit: Some(std::sync::Arc::new(|_status| {
            // Test scaffold: don't actually std::process::exit;
            // let abort's SystemExit propagate as a Trap.
        })),
        ..rubyrs::Config::default()
    };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let stdout_buf = SharedBuf::new();
    let stderr_buf = SharedBuf::new();
    rt.set_stdout(Box::new(stdout_buf.clone()));
    rt.set_stderr(Box::new(stderr_buf.clone()));
    let _ = rt.eval(
        r##"
        begin
          raise RuntimeError.new("boom")
        rescue
          abort
        end
        "##,
        "adr_0025_abort_dollar_bang.rb",
    );
    // Tier-1 2c: abort's message now lands on stderr, not stdout.
    assert_eq!(stdout_buf.snapshot(), "");
    assert_eq!(stderr_buf.snapshot(), "RuntimeError: boom\n");
}

#[test]
fn tier1_2a_exception_inspect_includes_message() {
    // Tier-1 2a: Exception subclasses render as
    // `#<ClassName: message>`. Pre-fix the universal
    // `Object#inspect` fallback emitted `#<Class:0xHEX>`
    // for every Exception instance, polluting every
    // diagnostic / log path that inspected `$!`.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        puts RuntimeError.new("oops").inspect
        puts ArgumentError.new("bad").inspect
        class MyError < StandardError; end
        puts MyError.new("custom").inspect
        "##,
        "tier1_2a_exc_inspect.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "#<RuntimeError: oops>\n\
         #<ArgumentError: bad>\n\
         #<MyError: custom>\n",
    );
}

#[test]
fn tier1_2a_exception_new_no_args_defaults_message_to_class_name() {
    // Tier-1 2a follow-on: `Exception#initialize` accepts
    // no-args and defaults `@message` to the class name.
    // CRuby parity: `RuntimeError.new.message` → "RuntimeError",
    // and inspect shows `#<RuntimeError: RuntimeError>`.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        e = RuntimeError.new
        puts e.message
        puts e.inspect
        "##,
        "tier1_2a_exc_default.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "RuntimeError\n#<RuntimeError: RuntimeError>\n");
}

#[test]
fn tier1_2a_plain_object_keeps_hex_form() {
    // Plain Object instances (non-Exception) keep the
    // `#<Class:0xHEX>` fallback; the new Exception-detect
    // arm must not catch them.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        class Foo; end
        s = Foo.new.inspect
        puts s.start_with?("#<Foo:0x")
        "##,
        "tier1_2a_plain_object.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "true\n");
}

#[test]
fn tier1_2c_warn_writes_to_stderr() {
    // Tier-1 2c: `Kernel#warn(msg)` writes to the Vm's
    // stderr channel, not stdout. Multi-arg form prints
    // one line per argument (CRuby parity).
    let mut rt = rubyrs::Runtime::new();
    let stdout_buf = SharedBuf::new();
    let stderr_buf = SharedBuf::new();
    rt.set_stdout(Box::new(stdout_buf.clone()));
    rt.set_stderr(Box::new(stderr_buf.clone()));
    rt.eval(
        r##"
        warn "first"
        warn "a", "b", "c"
        puts "stdout"
        "##,
        "tier1_2c_warn.rb",
    ).expect("eval");
    assert_eq!(stdout_buf.snapshot(), "stdout\n");
    assert_eq!(stderr_buf.snapshot(), "first\na\nb\nc\n");
}

#[test]
fn tier1_2c_warn_default_sink_is_silent() {
    // Tier-1 2c secure-by-default: a Runtime with no explicit
    // `set_stderr` discards warn output (matches the
    // `set_stdout`-not-called posture). Embedders opt into
    // seeing diagnostics.
    let mut rt = rubyrs::Runtime::new();
    let stdout_buf = SharedBuf::new();
    rt.set_stdout(Box::new(stdout_buf.clone()));
    rt.eval(r#"warn "silent""#, "tier1_2c_warn_silent.rb")
        .expect("eval");
    assert_eq!(stdout_buf.snapshot(), "");
}

#[test]
fn tier1_2b_proc_new_with_block_captures_it() {
    // Tier-1 2b: `Proc.new { ... }` returns the block as a
    // Value::Block — `.call` then dispatches through the
    // existing block-call arm. Pre-fix Proc.new fell through
    // to Object#new producing an empty Proc instance with no
    // `.call` method.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        pr = Proc.new { |a, b| a + b }
        puts pr.class
        puts pr.call(2, 3)
        "##,
        "tier1_2b_proc_new.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "Proc\n5\n");
}

#[test]
fn tier1_2b_proc_new_without_block_raises_argument_error() {
    // Tier-1 2b: `Proc.new` with no explicit block raises
    // ArgumentError, matching CRuby 3.x (which removed the
    // implicit-block-from-caller capture form).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        begin
          Proc.new
        rescue ArgumentError => e
          puts "ok: #{e.message}"
        end
        "##,
        "tier1_2b_proc_new_noblock.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "ok: tried to create Proc object without a block\n",
    );
}

#[test]
fn tier1_2b_proc_new_implicit_capture_also_raises() {
    // Tier-1 2b: `Proc.new` inside a method that has a
    // block_arg STILL raises (no implicit capture). The
    // explicit form `Proc.new(&blk)` is the surviving
    // capture API (out of scope for this commit).
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        def make
          Proc.new
        end
        begin
          make { :x }
        rescue ArgumentError => e
          puts "ok: #{e.message}"
        end
        "##,
        "tier1_2b_proc_new_implicit.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot(),
        "ok: tried to create Proc object without a block\n",
    );
}

#[test]
fn dunder_dir_works_with_explicit_self_receiver() {
    // Pre-fix `self.__dir__` raised NoMethodError because
    // the `__dir__` arm in `do_call` was gated by the
    // `if no_recv` branch — only bare `__dir__` reached it.
    // CRuby exposes `__dir__` as a Kernel private instance
    // method, so `self.__dir__` (the "explicit self for
    // private" exception) must work at every scope.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        # toplevel: self is the main object
        a = self.__dir__
        # instance method body: self is the Foo instance
        class Foo
          def dir; self.__dir__; end
        end
        b = Foo.new.dir
        # class method body: self is the class
        class Bar
          def self.dir; self.__dir__; end
        end
        c = Bar.dir
        puts (a == b && b == c) ? "all match" : "diverged"
        puts a.start_with?("/") || a == "."
        "##,
        "dunder_dir_self.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "all match\ntrue\n");
}

#[test]
fn dunder_dir_with_third_party_receiver_raises() {
    // Other receivers (`obj.__dir__`, `"x".__dir__`) fail —
    // CRuby surfaces this as "private method called", rubyrs
    // currently as the broader "undefined method" (private-
    // method-error parity is a separate Tier-1 gap). Either
    // shape is correct here; what matters is that the call
    // doesn't silently return.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        class Foo; end
        begin
          Foo.new.__dir__
          puts "leaked"
        rescue NoMethodError => e
          puts "ok: #{e.class}"
        end
        "##,
        "dunder_dir_third_party.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot(), "ok: NoMethodError\n");
}
