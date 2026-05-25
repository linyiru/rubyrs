//! Public API smoke tests. Locks down the embedding surface
//! (Runtime, register_fn, set_stdout, eval, format_trap) so it can't
//! regress accidentally.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use rubyrs::{Config, HostCtx, Runtime, RubyError, Trap, Value};

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
fn register_fn_v2_reads_array_arg_via_host_ctx() {
    // v2 signature gives the closure a `HostCtx` so it can unpack
    // `Value::Array` directly — the heap-y shape that v1's
    // `&[Value]`-only signature couldn't reach without going back
    // through the (cloning) `Runtime::resolve_array`. This is the
    // gap PR #35's Gemfile demo hit and worked around with a
    // Ruby-side prelude that flattened `*args` to a `|`-joined
    // String.
    let (mut rt, buf) = rt_with_buf();
    rt.register_fn_v2("sum_array", |ctx: &HostCtx, args: &[Value]| {
        let arr = match args {
            [v] => ctx.resolve_array(v).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "expected Array".into() },
                backtrace: vec![],
            })?,
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity".into() },
                backtrace: vec![],
            }),
        };
        let mut total: i64 = 0;
        for v in arr {
            if let Value::Int(n) = v { total += n; } else {
                return Err(Trap {
                    err: RubyError::TypeError { msg: "expected Integer element".into() },
                    backtrace: vec![],
                });
            }
        }
        Ok(Value::Int(total))
    });
    rt.eval(r#"puts sum_array([1, 2, 3, 4, 5])"#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "15\n");
}

#[test]
fn register_fn_v2_reads_hash_arg_via_host_ctx() {
    // Two checks against the same Hash:
    //   1. key lookup returns the right Value (exercises the
    //      resolve_hash → (k, v) pair shape end-to-end).
    //   2. iteration order matches insertion order. CRuby's
    //      Hash guarantees this since 1.9; rubyrs's
    //      `Vec<(Value, Value)>` representation preserves it
    //      mechanically. The `hash_keys` host fn below joins
    //      keys with `|` so a regression that switched to a
    //      `HashMap` or any unordered shape would fail with a
    //      different concrete output string.
    let mut rt = Runtime::new();
    rt.register_fn_v2("hash_lookup", |ctx: &HostCtx, args: &[Value]| {
        let (h, want) = match args {
            [h, Value::Str(s)] => (
                ctx.resolve_hash(h).ok_or_else(|| Trap {
                    err: RubyError::ArgumentError { msg: "expected Hash".into() },
                    backtrace: vec![],
                })?,
                s.borrow().clone(),
            ),
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity / types".into() },
                backtrace: vec![],
            }),
        };
        for (k, v) in h {
            if let Value::Str(ks) = k
                && *ks.borrow() == want
            {
                return Ok(v.clone());
            }
        }
        Ok(Value::Nil)
    });
    // Captures the iteration order Rust-side so the assertion
    // below can read an actual String (sidesteps `Value::Str`'s
    // non-public constructor).
    let captured_keys: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![]));
    let captured_for_fn = captured_keys.clone();
    rt.register_fn_v2("hash_keys", move |ctx: &HostCtx, args: &[Value]| {
        let h = match args {
            [h] => ctx.resolve_hash(h).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "expected Hash".into() },
                backtrace: vec![],
            })?,
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity".into() },
                backtrace: vec![],
            }),
        };
        let mut out = captured_for_fn.borrow_mut();
        out.clear();
        for (k, _) in h {
            if let Value::Str(s) = k { out.push(s.borrow().clone()); }
        }
        Ok(Value::Nil)
    });

    // Key lookup.
    let v = rt.eval(
        r#"hash_lookup({ "a" => 1, "b" => 2, "c" => 3 }, "b")"#,
        "t.rb",
    ).unwrap();
    assert!(matches!(v, Value::Int(2)), "expected Int(2), got {:?}", v);

    // Iteration order. Insertion order is `a, b, c`; if the
    // underlying representation ever became unordered (e.g.
    // HashMap) this assertion would fail with a different
    // permutation rather than passing silently.
    rt.eval(
        r#"hash_keys({ "a" => 1, "b" => 2, "c" => 3 })"#,
        "t.rb",
    ).unwrap();
    assert_eq!(&*captured_keys.borrow(), &["a", "b", "c"],
        "hash iteration order should match insertion");
}

#[test]
fn register_fn_v2_resolves_sym_arg_via_host_ctx() {
    // `HostCtx::resolve_sym` lets the host borrow the interned name
    // of a `Value::Sym` arg without going through the prelude. This
    // closes the gap PR #40 noted: a Bundler-style kwarg Hash with
    // Symbol keys (`require:`, `platforms:`) can be consumed
    // host-side without a Ruby `k.to_s` rebuild.
    let captured: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![]));
    let captured_for_fn = captured.clone();
    let mut rt = Runtime::new();
    rt.register_fn_v2("sym_names", move |ctx: &HostCtx, args: &[Value]| {
        let mut out = captured_for_fn.borrow_mut();
        out.clear();
        for v in args {
            let name = ctx.resolve_sym(v).ok_or_else(|| Trap {
                err: RubyError::ArgumentError {
                    msg: "expected all args to be Symbols".into(),
                },
                backtrace: vec![],
            })?;
            out.push(name.to_string());
        }
        Ok(Value::Nil)
    });

    rt.eval(r#"sym_names(:require, :platforms, :mri)"#, "t.rb").unwrap();
    assert_eq!(&*captured.borrow(), &["require", "platforms", "mri"]);

    // Negative case: a non-Symbol arg surfaces the explicit
    // ArgumentError, not a silent skip.
    let err = rt.eval(r#"sym_names(:ok, "not a sym")"#, "t.rb").unwrap_err();
    assert!(err.err.is("ArgumentError"),
        "expected ArgumentError on non-Sym arg, got {:?}", err.err);
}

#[test]
fn register_fn_v2_replaces_prior_v1_registration() {
    // Re-registering under the same name swaps the slot —
    // pinning this so a future refactor that uses separate v1/v2
    // maps would have to keep this guarantee explicit.
    let mut rt = Runtime::new();
    rt.register_fn("answer", |_| Ok(Value::Int(42)));
    let v1 = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(v1, Value::Int(42)));

    rt.register_fn_v2("answer", |_ctx, _args| Ok(Value::Int(99)));
    let v2 = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(v2, Value::Int(99)));
}

#[test]
fn register_fn_replaces_prior_v2_registration() {
    // Symmetric direction. Both register_fn / register_fn_v2 doc
    // strings promise replacement either way; without this test a
    // future map-split refactor (separate v1_fns / v2_fns maps)
    // could keep a stale v2 closure live after the embedder
    // re-registered with v1. Existing v1→v2 test above wouldn't
    // catch that direction.
    let mut rt = Runtime::new();
    rt.register_fn_v2("answer", |_ctx, _args| Ok(Value::Int(99)));
    let first = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(first, Value::Int(99)));

    rt.register_fn("answer", |_| Ok(Value::Int(42)));
    let second = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(second, Value::Int(42)),
        "v1 registration should have replaced the prior v2 slot, got {:?}",
        second);
}

#[test]
fn singleton_class_closures_do_not_cycle_leak() {
    // Regression for PR #31 review (vm/step.rs:521): Method's
    // `defining_class` used to be `Rc<Class>`. For singleton
    // methods that pointed back at the eigenclass, which held
    // the Method in its `methods` table — a strong cycle. For
    // regular classes the cycle is masked because `Vm.classes`
    // pins every class for the program's lifetime; for
    // eigenclasses there's no such anchor, so each short-lived
    // object with a singleton method would leak its eigenclass
    // + the Method + the Method's captured Rc → forever.
    //
    // The fix downgraded `Method.defining_class` to `Weak<Class>`
    // (Frame upgrades at frame push, keeping the eigenclass
    // alive for the duration of the call). This test creates
    // 1000 short-lived objects, each receiving a fresh
    // `define_singleton_method` closure that captures an Array
    // — so a per-Instance leak would retain 1000 Arrays plus
    // their inner Hashes in the heap. The tight `max_heap_objects`
    // cap (200) would trigger ResourceExhausted under the old
    // (Rc-cycle) shape; under the fixed shape, GC reclaims the
    // closures as Instances are swept, and the program runs
    // to completion.
    let mut rt = Runtime::with_config(Config {
        max_heap_objects: Some(200),
        stress_gc: true,
        ..Default::default()
    });
    let res = rt.eval(r#"
        class Container
        end
        i = 0
        while i < 1000
          obj = Container.new
          # Each call captures a fresh Array literal via the
          # block's closure. If the singleton method keeps the
          # eigenclass + Method + captured-Rc alive past the
          # object's lifetime, this loop grows unboundedly.
          obj.define_singleton_method(:carry) { [i, i + 1, i + 2] }
          i = i + 1
        end
        "done"
    "#, "t.rb");
    match res {
        Ok(_) => {}
        Err(trap) => panic!(
            "expected loop to complete; got {:?} — likely the Method/eigenclass cycle leak regressed",
            trap.err,
        ),
    }
}

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
    // false here. Documented behaviour, not a bug.
    let runtime = rt.eval(r#"raise "boom""#, "t.rb").unwrap_err();
    assert!(runtime.err.is("RuntimeError"));
    assert!(!runtime.err.is("StandardError"));
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
    // exit path. The assertion fired in `rt.eval` is the real test;
    // reaching this line is the success signal.
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
        # Constant-path write `Foo::Bar = 1` — still unsupported,
        # used as the canary for "AST translation cannot handle
        # this node".
        class Foo; end
        Foo::Bar = 1
        "#,
        "const_path_write.rb",
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

// ---------- Metaprogramming PoCs ----------

#[test]
fn alias_method_copies_method_under_new_name() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"
        class Greeter
          def hello
            "hi"
          end
          alias_method :greet, :hello
        end
        g = Greeter.new
        puts g.hello
        puts g.greet
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "hi\nhi\n");
}

#[test]
fn alias_method_multiple_per_class_body_stays_stack_balanced() {
    // Regression for PR #8 review (compiler.rs:486): a stray
    // `LoadNil` after `Op::AliasMethod` left one Nil on the operand
    // stack per alias. Three aliases in one body would have
    // accumulated three leftover Nils that only got swept on class
    // body return — and would have surfaced as a real imbalance
    // had the body returned a value. Make sure the body's actual
    // return value is correct.
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"
        class M
          def a; 1; end
          def b; 2; end
          def c; 3; end
          alias_method :x, :a
          alias_method :y, :b
          alias_method :z, :c
        end
        m = M.new
        puts m.x + m.y + m.z
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "6\n");
}

#[test]
fn alias_method_raises_name_error_when_source_missing() {
    // Per PR #8 review (vm.rs:3637): CRuby raises NameError
    // ("undefined method ...") when alias_method's source name
    // doesn't resolve. Previously we raised NoMethodError with a
    // misleading `recv_type: "Class"`.
    let mut rt = Runtime::new();
    let err = rt.eval(r#"
        class Foo
          alias_method :a, :nonexistent
        end
    "#, "t.rb").unwrap_err();
    assert!(err.err.is("NameError"), "expected NameError, got {:?}", err.err);
}

#[test]
fn alias_method_can_alias_inherited_method() {
    // Regression for PR #8 review (vm.rs:3621): Op::AliasMethod
    // used to look up `old_id` only in the immediate class's
    // method table, missing inherited methods. CRuby's
    // `alias_method` walks the ancestor chain to find the source
    // and installs the alias on the *current* class — so the alias
    // can name an inherited method.
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"
        class Parent
          def parent_method
            "from-parent"
          end
        end
        class Child < Parent
          alias_method :inherited_alias, :parent_method
        end
        puts Child.new.inherited_alias
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "from-parent\n");
}

#[test]
fn alias_method_shares_super_lookup_chain() {
    let (mut rt, buf) = rt_with_buf();
    // Alias preserves defining_class — `super` from `greet`
    // (aliased to `hello`) still resolves Parent#hello.
    rt.eval(r#"
        class Parent
          def hello
            "parent"
          end
        end
        class Child < Parent
          def hello
            super + "+child"
          end
          alias_method :greet, :hello
        end
        puts Child.new.greet
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "parent+child\n");
}

#[test]
fn method_missing_catches_unknown_call_on_object() {
    let (mut rt, buf) = rt_with_buf();
    // method_missing's real use is as a proxy — accept any name +
    // any arg count, route somewhere. That requires `*args` splat
    // in the method def. Master added splat in a24d7cb; this test
    // locks in the integration (metaprog + splat) so neither side
    // can regress unnoticed.
    rt.eval(r##"
        class Ghost
          def method_missing(name, *args)
            "#{name}(#{args.length}: #{args.inspect})"
          end
        end
        g = Ghost.new
        puts g.poof
        puts g.boo(1)
        puts g.zap(1, 2, 3)
    "##, "t.rb").unwrap();
    assert_eq!(
        buf.snapshot(),
        "poof(0: [])\nboo(1: [1])\nzap(3: [1, 2, 3])\n"
    );
}

#[test]
fn splat_rest_param_survives_stress_gc() {
    // Regression: `invoke_method_with_block` allocates the
    // rest-Array via `heap.alloc(HeapObj::Array(rest_vec))` after
    // a `maybe_gc()`. Before this fix (master a24d7cb,
    // vm.rs:2615-2620), GC ran while `locals` and `rest_vec` were
    // bare Rust Vecs not in any root set — any Object / Array /
    // Hash / Range / Block referenced through them would be
    // swept under `STRESS_GC=1`, leaving dangling ObjIds inside
    // the freshly-built frame.
    //
    // Force the situation: pass heap-allocated values (Arrays) as
    // rest-args, do enough method-internal work that we'd notice
    // a sweep, then read the rest contents back. Without the pin
    // guards the inner Array elements would dangle and `.inspect`
    // would either panic or print garbage.
    let mut rt = Runtime::with_config(Config {
        // `stress_gc` triggers a collection at every alloc check,
        // matching the CI `STRESS_GC=1` mode.
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        def collect(*items)
          # A few extra allocations after the rest-Array is built,
          # so a hypothetical dangling slot has had time to be
          # reused by the time we inspect.
          tmp = []
          i = 0
          while i < 50
            tmp << [i, i + 1]
            i = i + 1
          end
          items
        end
        # Crucially: pass Array LITERALS inline, not via locals.
        # If the rest-args came from local-variable slots, those
        # slots would already be in `frames[0].locals` and the
        # GC would mark through them via the normal root walk —
        # the bug wouldn't reproduce. Inline literals are
        # constructed right before the call, pushed to the
        # operand stack, drained into `args: Vec<Value>`, and held
        # ONLY via that bare Rust Vec by the time `maybe_gc()`
        # runs inside the rest-collect branch.
        result = collect([1, 2], [3, 4], [5, 6])
        puts result.length
        puts result[0].inspect
        puts result[1].inspect
        puts result[2].inspect
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "3\n[1, 2]\n[3, 4]\n[5, 6]\n");
}

#[test]
fn splat_rest_inline_receiver_survives_stress_gc() {
    // Companion regression for the second half of the same
    // PinGuard window — beyond locals/rest_vec, the *receiver*
    // (`self_val`) is also unrooted during the rest-Array alloc.
    // Inline-allocated receivers like `Container.new.collect(...)`
    // hold the Object only as a Rust local; without pinning it,
    // STRESS_GC would sweep the instance mid-call and the method
    // body would see a dangling self.
    let mut rt = Runtime::with_config(Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r##"
        class Container
          def initialize
            @label = "live"
          end
          def collect(*items)
            tmp = []
            i = 0
            while i < 20
              tmp << [i, i + 1]
              i = i + 1
            end
            "#{@label}: #{items.length}"
          end
        end
        # Inline `.new` — the Container Object is held only as a
        # Rust local in do_call's recv slot, never bound to a Ruby
        # variable. Without `self_val` in the rest-alloc PinGuard,
        # STRESS_GC would sweep the instance during the rest-Array
        # alloc and `@label` would land on a dangling ObjId.
        puts Container.new.collect([1, 2], [3, 4], [5, 6])
    "##, "t.rb").unwrap();
    let out = buf.snapshot();
    // The body interpolates `@label` (= "live") and items.length
    // (= 3); both rely on self surviving the alloc window.
    assert_eq!(out, "live: 3\n");
}

#[test]
fn top_level_constant_array_survives_stress_gc() {
    // Regression: `Vm.constants` (the `FOO = expr` table) was added
    // without a corresponding entry in `maybe_gc`'s root walk, so
    // Array/Hash/Object values stored as constants could be swept
    // between the assignment and any later LoadConst. Under
    // STRESS_GC=1 the inner allocations below would trip a sweep
    // before the final `.length` read, and the dangling ObjId would
    // either panic or print garbage.
    let mut rt = Runtime::with_config(Config {
        stress_gc: true,
        ..Default::default()
    });
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(r#"
        LIST = [10, 20, 30]
        MAP = { a: 1, b: 2 }
        # Burn allocations so any unrooted ObjId held by LIST/MAP
        # would be reused by the time we read them back.
        i = 0
        while i < 50
          [i, i + 1]
          { k: i }
          i = i + 1
        end
        puts LIST.length
        puts LIST.first
        puts MAP[:a]
        puts MAP[:b]
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "3\n10\n1\n2\n");
}

#[test]
fn method_missing_inherited_through_superclass() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"
        class Base
          def method_missing(name)
            name.to_s
          end
        end
        class Mid < Base
        end
        puts Mid.new.does_not_exist
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "does_not_exist\n");
}

#[test]
fn missing_without_method_missing_still_raises() {
    let mut rt = Runtime::new();
    let err = rt.eval(r#"
        class Empty; end
        Empty.new.missing_method
    "#, "t.rb").unwrap_err();
    assert!(
        err.err.is("NoMethodError"),
        "expected NoMethodError (direct or Uncaught-wrapped), got {:?}",
        err.err
    );
}

#[test]
fn define_method_installs_a_callable_method() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"
        class Foo
          define_method(:greet) { |name| "hello, " + name }
        end
        puts Foo.new.greet("world")
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "hello, world\n");
}

#[test]
fn define_method_closes_over_outer_scope() {
    let (mut rt, buf) = rt_with_buf();
    // The block captures `counter` from the surrounding class-body
    // scope; each invocation reads & writes the same slot. This is
    // the closure semantic that distinguishes `define_method` from
    // `def`.
    rt.eval(r#"
        class Counter
          counter = 0
          define_method(:bump) { counter = counter + 1; counter }
        end
        c = Counter.new
        puts c.bump
        puts c.bump
        puts c.bump
    "#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "1\n2\n3\n");
}

#[test]
fn define_method_validates_arity() {
    let mut rt = Runtime::new();
    let err = rt.eval(r#"
        class Foo
          define_method(:two) { |a, b| a + b }
        end
        Foo.new.two(1)
    "#, "t.rb").unwrap_err();
    assert!(err.err.is("ArgumentError"), "expected ArgumentError, got {:?}", err.err);
}

#[test]
fn gemfile_dsl_real_hosting_end_to_end() {
    // Locks in the `examples/gemfile/` demo at integration-test
    // shape: prelude + unmodified Gemfile + the same Rust host
    // surface, all driven through the public Runtime API.
    // Asserts the gem-count + group bucketing the demo produces
    // so any regression in (kwargs / splat receive / group block
    // yielding / `if RUBY_VERSION` conditional / `**opts` Hash
    // unpacking in the prelude) shows up here, not just when
    // someone happens to re-run the example binary.
    use std::cell::RefCell;
    use std::rc::Rc;

    // Mirror the example's GemfileState shape — small enough to
    // dup here and keeps the test self-contained. Named fields
    // (rather than a positional tuple) so the assertions below
    // read as `puma.require_kw` not `puma.3` — much harder to
    // mis-order when the schema grows.
    #[derive(Default)]
    struct Gem {
        name: String,
        reqs: Vec<String>,
        groups: Vec<String>,
        require_kw: String,
        platforms_kw: String,
        platforms_scope: Vec<String>,
        source_override: Option<(String, String)>,
    }
    #[derive(Default)]
    struct State {
        source: Option<String>,
        ruby_version: Option<String>,
        gems: Vec<Gem>,
        group_stack: Vec<String>,
        platforms_stack: Vec<String>,
        // Unified source-override stack — matches the example's
        // shape so `git` / `path` precedence is push-order, not
        // type-priority. See `examples/gemfile.rs::GemfileState`.
        source_stack: Vec<(String, String)>,
    }
    let state = Rc::new(RefCell::new(State::default()));
    let mut rt = Runtime::new();

    fn s(v: &Value) -> String {
        if let Value::Str(rs) = v { rs.borrow().clone() } else { String::new() }
    }

    {
        let st = state.clone();
        rt.register_fn("__gemfile_source", move |args| {
            if let [u] = args { st.borrow_mut().source = Some(s(u)); }
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_ruby", move |args| {
            if let [v] = args { st.borrow_mut().ruby_version = Some(s(v)); }
            Ok(Value::Nil)
        });
    }
    // v2 form — mirrors examples/gemfile.rs::__gemfile_gem_v2.
    // Fail-fast shape validation: matches the demo's pattern and
    // the earlier register_fn_v2_reads_* unit tests. A regression
    // in the prelude (sending the wrong shape) surfaces as an
    // ArgumentError here, not as a silent partial GemfileState
    // that fails 200 lines later in `.gems.len() != 18`.
    {
        let st = state.clone();
        rt.register_fn_v2("__gemfile_gem_v2", move |ctx, args| {
            let [name, requirements, opts] = args else {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!("__gemfile_gem_v2: expected 3 args, got {}", args.len()),
                    },
                    backtrace: vec![],
                });
            };
            let name = if let Value::Str(rs) = name {
                rs.borrow().clone()
            } else {
                return Err(Trap {
                    err: RubyError::ArgumentError { msg: "name must be a String".into() },
                    backtrace: vec![],
                });
            };
            let reqs_slice = ctx.resolve_array(requirements).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "requirements must be an Array".into() },
                backtrace: vec![],
            })?;
            let opts_slice = ctx.resolve_hash(opts).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "opts must be a Hash".into() },
                backtrace: vec![],
            })?;

            let reqs_vec: Vec<String> = reqs_slice.iter()
                .map(|v| if let Value::Str(rs) = v {
                    Ok(rs.borrow().clone())
                } else {
                    Err(Trap {
                        err: RubyError::ArgumentError {
                            msg: "requirements element must be a String".into(),
                        },
                        backtrace: vec![],
                    })
                })
                .collect::<Result<_, _>>()?;
            // Bundler kwargs Hash: Symbol keys, mixed values (Bool /
            // Sym / String). Mirrors examples/gemfile.rs.
            let mut require_kw = String::new();
            let mut platforms_kw = String::new();
            for (k, v) in opts_slice {
                let key = ctx.resolve_sym(k).ok_or_else(|| Trap {
                    err: RubyError::ArgumentError {
                        msg: "opts keys must be Symbols".into(),
                    },
                    backtrace: vec![],
                })?;
                let vs = match v {
                    Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
                    Value::Str(rs) => rs.borrow().clone(),
                    // The outer match already filtered on `Value::Sym`,
                    // so `resolve_sym` is guaranteed to return Some.
                    // `expect` rather than `unwrap_or("")` so a future
                    // interner-contract regression surfaces loudly.
                    Value::Sym(_) => ctx.resolve_sym(v)
                        .expect("resolve_sym on Value::Sym arm must return Some")
                        .to_string(),
                    _ => return Err(Trap {
                        err: RubyError::ArgumentError {
                            msg: format!("opts[{key}] must be a Bool, Symbol, or String"),
                        },
                        backtrace: vec![],
                    }),
                };
                match key {
                    "require"   => require_kw   = vs,
                    "platforms" => platforms_kw = vs,
                    _ => {}
                }
            }

            let mut sm = st.borrow_mut();
            let groups: Vec<String> = sm.group_stack.last()
                .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            let platforms_scope: Vec<String> = sm.platforms_stack.last()
                .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            let source_override = sm.source_stack.last().cloned();
            sm.gems.push(Gem {
                name,
                reqs: reqs_vec,
                groups,
                require_kw,
                platforms_kw,
                platforms_scope,
                source_override,
            });
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_push_groups", move |args| {
            if let [v] = args { st.borrow_mut().group_stack.push(s(v)); }
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_pop_groups", move |_args| {
            st.borrow_mut().group_stack.pop();
            Ok(Value::Nil)
        });
    }
    // Real push/pop wiring for platforms / git / path so a
    // regression in those scope blocks (block-yield ordering,
    // ensure-pop pairing, source-precedence) actually fails
    // the test instead of silently no-op'ing.
    {
        let st = state.clone();
        rt.register_fn("__gemfile_push_platforms", move |args| {
            if let [v] = args { st.borrow_mut().platforms_stack.push(s(v)); }
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_pop_platforms", move |_args| {
            st.borrow_mut().platforms_stack.pop();
            Ok(Value::Nil)
        });
    }
    for (push_name, pop_name, kind) in [
        ("__gemfile_push_git",  "__gemfile_pop_git",  "git"),
        ("__gemfile_push_path", "__gemfile_pop_path", "path"),
    ] {
        let st = state.clone();
        let kind_s: String = kind.into();
        rt.register_fn(push_name, move |args| {
            if let [v] = args {
                st.borrow_mut().source_stack.push((kind_s.clone(), s(v)));
            }
            Ok(Value::Nil)
        });
        let st = state.clone();
        rt.register_fn(pop_name, move |_args| {
            st.borrow_mut().source_stack.pop();
            Ok(Value::Nil)
        });
    }

    // Read the actual prelude + Gemfile from the repo. That's
    // the point: the demo and the test exercise the same files.
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/gemfile");
    let prelude_src = std::fs::read_to_string(base.join("dsl_prelude.rb"))
        .expect("dsl_prelude.rb missing — examples/gemfile/ removed?");
    let gemfile_src = std::fs::read_to_string(base.join("Gemfile"))
        .expect("Gemfile missing — examples/gemfile/ removed?");

    rt.eval(&prelude_src, "dsl_prelude.rb").expect("prelude eval");
    rt.eval(&gemfile_src, "Gemfile").expect("Gemfile eval");

    let st = state.borrow();
    assert_eq!(st.source.as_deref(), Some("https://rubygems.org"));
    assert_eq!(st.ruby_version.as_deref(), Some("3.4.0"));
    // 15 from the original list + rb-readline + forked-gem +
    // vendored-gem = 18. The negative `if RUBY_VERSION >=
    // "99.0.0"` branch must NOT contribute `future-only-gem`.
    assert_eq!(st.gems.len(), 18,
        "expected 18 gems from examples/gemfile/Gemfile, got {}",
        st.gems.len());

    let find = |n: &str| st.gems.iter().find(|g| g.name == n)
        .unwrap_or_else(|| panic!("{n} missing"));

    // Spot-check the splat-receive case: rack should have 2
    // version constraints, not 1.
    assert_eq!(find("rack").reqs, vec![">= 3.0", "< 4.0"]);

    // Spot-check the multi-group block: rspec-rails should be
    // tagged with BOTH `:development` and `:test`.
    assert_eq!(find("rspec-rails").groups, vec!["development", "test"]);

    // Conditional truthy branch: with prelude setting
    // RUBY_VERSION = "3.4.0", `csv` (guarded by >= "3.4.0")
    // should be present.
    assert!(st.gems.iter().any(|g| g.name == "csv"),
        "csv should be present when RUBY_VERSION >= 3.4.0");
    // Conditional falsy branch: `future-only-gem` is guarded by
    // `if RUBY_VERSION >= "99.0.0"`. If String `>=` inverted or
    // `if` polarity flipped, this gem would sneak in.
    assert!(!st.gems.iter().any(|g| g.name == "future-only-gem"),
        "future-only-gem must NOT appear under RUBY_VERSION 3.4.0");

    // `**kwargs` Hash round-trip into our named fields. A
    // regression in Hash receive / Symbol-key / `.to_s` would
    // blank these out.
    let puma = find("puma");
    assert_eq!(puma.require_kw, "false", "puma's require: false should round-trip");
    assert_eq!(puma.platforms_kw, "", "puma has no platforms: kwarg");

    let sidekiq = find("sidekiq");
    assert_eq!(sidekiq.require_kw, "sidekiq", "sidekiq's require: 'sidekiq' should round-trip");
    assert_eq!(sidekiq.platforms_kw, "mri", "sidekiq's platforms: :mri should round-trip");

    let pry = find("pry-byebug");
    assert_eq!(pry.require_kw, "pry-byebug");
    assert_eq!(pry.platforms_kw, "mri");

    // Bare gem — no kwargs, both slots empty.
    let rake = find("rake");
    assert_eq!(rake.require_kw, "");
    assert_eq!(rake.platforms_kw, "");

    // `platforms :mri do ... end` block — rb-readline picks up
    // the platforms_scope via the push/pop wiring above.
    let rb_readline = find("rb-readline");
    assert_eq!(rb_readline.platforms_scope, vec!["mri"],
        "rb-readline should inherit platforms-scope from its block");

    // `git "url" do ... end` block — forked-gem picks up the
    // source override. If git/path used separate stacks with
    // git-then-path precedence this would still work for a
    // bare git block, but nested git/path would mis-tag.
    let forked = find("forked-gem");
    assert_eq!(forked.source_override,
        Some(("git".into(), "https://github.com/example/forked-gem.git".into())),
        "forked-gem should be tagged with its enclosing git source");

    // `path "..." do ... end` block — vendored-gem.
    let vendored = find("vendored-gem");
    assert_eq!(vendored.source_override,
        Some(("path".into(), "vendor/cache".into())),
        "vendored-gem should be tagged with its enclosing path source");

    // None of the gems declared outside a source block should
    // have a stale source_override. If pop_git/pop_path leaked
    // or the unified stack didn't drain, a later gem would
    // pick up an override it shouldn't have.
    assert_eq!(rake.source_override, None,
        "rake (top-level) should have no source override; \
         a non-None here means pop didn't pair with push");
}
