//! Public API smoke tests. Locks down the embedding surface
//! (Runtime, register_fn, set_stdout, eval, format_trap) so it can't
//! regress accidentally.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use rubyrs::{Config, Runtime, RubyError, Value};

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self { SharedBuf(Rc::new(RefCell::new(Vec::new()))) }
    fn snapshot(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).expect("non-utf8 stdout")
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn rt_with_buf() -> (Runtime, SharedBuf) {
    let mut rt = Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    (rt, buf)
}

#[test]
fn eval_runs_a_simple_script() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"puts "hi""#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "hi\n");
}

#[test]
fn eval_returns_final_value() {
    let mut rt = Runtime::new();
    let v = rt.eval("1 + 2", "t.rb").unwrap();
    assert!(matches!(v, Value::Int(3)));
}

#[test]
fn host_fn_is_callable_from_ruby() {
    let (mut rt, buf) = rt_with_buf();
    rt.register_fn("triple", |args| match args {
        [Value::Int(n)] => Ok(Value::Int(n * 3)),
        _ => Ok(Value::Nil),
    });
    rt.eval(r#"puts triple(7)"#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "21\n");
}

#[test]
fn host_fn_can_propagate_trap() {
    let mut rt = Runtime::new();
    rt.register_fn("boom", |_| {
        // ArgumentError via the public surface
        Err(rubyrs::Trap {
            err: rubyrs::RubyError::ArgumentError { msg: "no good".into() },
            backtrace: vec![],
        })
    });
    let res = rt.eval(r#"boom"#, "t.rb");
    assert!(res.is_err());
    let formatted = rt.format_trap(&res.unwrap_err());
    assert!(formatted.contains("no good"), "got: {}", formatted);
    assert!(formatted.contains("ArgumentError"), "got: {}", formatted);
}

#[test]
fn definitions_persist_across_eval() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(
        r#"
        class Greeter
          def initialize(name); @name = name; end
          def hello; "hi, #{@name}"; end
        end
        "#,
        "define.rb",
    ).unwrap();
    rt.eval(r#"puts Greeter.new("rubyrs").hello"#, "use.rb").unwrap();
    assert_eq!(buf.snapshot(), "hi, rubyrs\n");
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

// ---------- P1-D: resource caps ----------

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
fn pin_guard_balanced_when_block_raises_inside_iterator() {
    // P0-2 regression: when a block running inside Array#each / #map /
    // any of the iterator drivers raises, the surrounding native code
    // used to leak `pinned` entries because the manual
    // `self.pinned.pop()` came AFTER the `?` early-return.
    //
    // The debug_assert in `Runtime::eval` catches an imbalanced pinned
    // stack at the end of every script. We hammer the path 50 times
    // under stress-GC to make sure the assertion doesn't fire and that
    // GC doesn't end up dragging zombie roots around.
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    for _ in 0..50 {
        let _ = rt.eval(
            r#"
            begin
              [1, 2, 3].map { |x| raise "boom" if x == 2; x * 2 }
            rescue => _e
              # swallow the synthetic RuntimeError so the script returns
              # normally; the *invariant* we're checking is that the
              # native side cleaned up its pins on the way out, not the
              # script's behaviour.
            end
            "#,
            "leak.rb",
        );
    }
    // If we got here without the debug_assert in eval firing, the
    // PinGuard's Drop was wired up correctly for every iterator
    // exit path. The assertion is the real test; this expression
    // just keeps the loop in scope.
    assert!(true);
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
    // `case` is currently outside the supported subset and reaches
    // the unsupported-node fallback. We expect a SyntaxError Trap
    // back, not a SIGABRT.
    let mut rt = Runtime::new();
    let err = rt.eval(
        r#"
        x = 1
        case x
        when 1 then puts "one"
        else        puts "other"
        end
        "#,
        "case.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, RubyError::SyntaxError { .. }),
        "expected SyntaxError, got {:?}",
        err.err,
    );
}

#[test]
fn blocks_are_gc_reclaimed_under_stress() {
    // P2-13 regression: with BlockHandle now in the GC heap, a
    // tight loop that creates many block values must let the GC
    // reclaim each block once the iteration moves on. Before
    // P2-13 blocks were Rc-managed and a (then-theoretical)
    // self-capturing cycle would leak; now they're swept like
    // Array/Hash.
    //
    // We set a small heap cap so any leak surfaces as a
    // ResourceExhausted trap rather than a slow degradation.
    // 200 iterations × {1 Array + 1 Block per iter} = 400 allocs.
    // Steady-state live_count should be O(1), well under 50.
    let mut rt = Runtime::with_config(Config {
        stress_gc: true,
        max_heap_objects: Some(50),
        ..Default::default()
    });
    rt.eval(
        r#"
        i = 0
        while i < 200
          [1, 2, 3].each { |x| i = i + 1 }
        end
        puts i
        "#,
        "many_blocks.rb",
    ).unwrap();
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

// ---------- resolve_* helpers ----------

#[test]
fn resolve_array_unpacks_elements() {
    let mut rt = Runtime::new();
    let val = rt.eval("[10, 20, 30]", "t.rb").unwrap();
    let elems = rt.resolve_array(&val).expect("should be an Array");
    assert_eq!(elems.len(), 3);
    assert!(matches!(elems[0], Value::Int(10)));
    assert!(matches!(elems[1], Value::Int(20)));
    assert!(matches!(elems[2], Value::Int(30)));
}

#[test]
fn resolve_array_returns_none_for_non_array() {
    let rt = Runtime::new();
    assert!(rt.resolve_array(&Value::Int(42)).is_none());
}

#[test]
fn resolve_hash_unpacks_pairs() {
    let mut rt = Runtime::new();
    let val = rt.eval(r#"{ "a" => 1, "b" => 2 }"#, "t.rb").unwrap();
    let pairs = rt.resolve_hash(&val).expect("should be a Hash");
    assert_eq!(pairs.len(), 2);
    assert!(matches!(&pairs[0].0, Value::Str(s) if *s.borrow() == "a"));
    assert!(matches!(&pairs[0].1, Value::Int(1)));
    assert!(matches!(&pairs[1].0, Value::Str(s) if *s.borrow() == "b"));
    assert!(matches!(&pairs[1].1, Value::Int(2)));
}

#[test]
fn resolve_hash_returns_none_for_non_hash() {
    let rt = Runtime::new();
    assert!(rt.resolve_hash(&Value::Nil).is_none());
}

#[test]
fn resolve_sym_roundtrips_symbol() {
    let mut rt = Runtime::new();
    let val = rt.eval(":hello", "t.rb").unwrap();
    if let Value::Sym(id) = val {
        assert_eq!(rt.resolve_sym(id), "hello");
    } else {
        panic!("expected Value::Sym, got {:?}", val);
    }
}
