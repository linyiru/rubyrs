//! Public API smoke tests. Locks down the embedding surface
//! (Runtime, register_fn, set_stdout, eval, format_trap) so it can't
//! regress accidentally.
//!
//! Tests are organised into topical submodules under `tests/embed/`
//! to keep this entry file from growing without bound. Shared
//! helpers (`SharedBuf`, `rt_with_buf`) stay here so every sub-mod
//! can reach them via `super::*`. Each sub-mod registers itself with
//! a `mod` declaration below.

// Topical sub-modules — each owns its own #[test] fns, all linked
// into this same test binary so `cargo test --test embed` still
// runs the full suite. Filter by name: `cargo test --test embed
// adr_0017`.
//
// `#[path = "..."]` is needed because Cargo treats this file as a
// crate root, so `mod x;` would normally look for `tests/x.rs`
// rather than `tests/embed/x.rs`. Pointing the path lets the
// sub-files sit under `tests/embed/` (organisational), keeping the
// `tests/` listing flat for the rest of the integration tests.
#[path = "embed/adr_0017.rs"]
mod adr_0017;
#[path = "embed/error_handling.rs"]
mod error_handling;
#[path = "embed/resource_caps.rs"]
mod resource_caps;
#[path = "embed/tier1_capability.rs"]
mod tier1_capability;

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
                s.to_string_lossy(),
            ),
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity / types".into() },
                backtrace: vec![],
            }),
        };
        for (k, v) in h {
            if let Value::Str(ks) = k
                && ks.to_string_lossy() == want
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
            if let Value::Str(s) = k { out.push(s.to_string_lossy()); }
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

// ---------- P1-D: resource caps ----------

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
    assert!(matches!(&pairs[0].0, Value::Str(s) if s.to_string_lossy() == "a"));
    assert!(matches!(&pairs[0].1, Value::Int(1)));
    assert!(matches!(&pairs[1].0, Value::Str(s) if s.to_string_lossy() == "b"));
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
        if let Value::Str(rs) = v { rs.to_string_lossy() } else { String::new() }
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
                rs.to_string_lossy()
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
                    Ok(rs.to_string_lossy())
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
                    Value::Str(rs) => rs.to_string_lossy(),
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

// ADR 0017 host-capability defaults — moved to
// `tests/embed/adr_0017.rs` (the `mod adr_0017;` at the top of
// this file pulls them in).

#[test]
fn interpolated_regex_invalid_pattern_returns_syntax_error_trap() {
    // PR #99 review coverage: the InterpolatedRegex path documents
    // that invalid runtime-assembled patterns surface as SyntaxError
    // traps at `Op::CompileRegex` (mirroring `LoadRegex`). The
    // existing literal-regex path already returns SyntaxError too,
    // so this is a parity check not a divergence acknowledgement.
    //
    // CRuby raises RegexpError here ("end pattern with unmatched
    // parenthesis"); the class differs from rubyrs's SyntaxError
    // for both literal AND interpolated regex paths, which is why
    // this lives as a host-API test rather than in diff_cruby.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        r#"
        bad = "("
        /#{bad}/
        "#,
        "bad_interpolated_regex.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::SyntaxError { .. }),
        "expected SyntaxError trap from invalid interpolated regex, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_respects_max_value_bytes_cap() {
    // Regression cover for PR #103 cycle 13. BigInt#to_s/#inspect
    // produce a decimal-digit string that grows arbitrarily with
    // the magnitude (`(2 ** 1_000_000).to_s` is ~300 KB), so the
    // bigint_primitive path must enforce Config::max_value_bytes
    // the same way primitive_call arms do — otherwise a script
    // could DoS the host by stringifying a huge integer.
    let cfg = rubyrs::Config { max_value_bytes: Some(64), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        r#"
        n = 1
        100.times { n = n * 1_000_000 }   # n has ~600 decimal digits
        n.to_s
        "#,
        "bigint_to_s_size_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted trap from BigInt#to_s exceeding max_value_bytes, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn range_max_with_i64_min_exclusive_returns_nil() {
    // Regression cover for the /code-review finding: Range#max
    // with an exclusive endpoint computes `ei - 1`. Pre-fix this
    // panicked in debug for ei == i64::MIN; treated as an empty
    // range (Nil) now.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // (-2**63 ... -2**63).max  — exclusive, endpoint == i64::MIN
        "puts((-9_223_372_036_854_775_808...-9_223_372_036_854_775_808).max.inspect)",
        "range_max_min_excl.rb",
    ).expect("should succeed without panic");
    assert_eq!(buf.snapshot().trim(), "nil");
}

#[cfg(feature = "bignum")]
#[test]
fn range_size_with_i64_max_width_returns_zero() {
    // Pre-fix `ei - bi + 1` panicked in debug when bi == i64::MIN
    // and ei == i64::MAX (width 2^64). Treat overflow as 0.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (-9_223_372_036_854_775_808..9_223_372_036_854_775_807).size",
        "range_size_max_width.rb",
    ).expect("should succeed without panic");
    assert_eq!(buf.snapshot().trim(), "0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_caps_huge_result() {
    // Phase B.1: `**` with a huge exponent estimates result bits
    // and traps ResourceExhausted before allocating GBs. Default
    // ceiling (no max_value_bytes) is 1 MB; `2 ** 10_000_000`
    // would need ~1.25 MB so it traps.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "2 ** 10_000_000",
        "pow_huge.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted trap from 2**10_000_000, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_honors_max_value_bytes() {
    // The DoS cap respects Config::max_value_bytes when set —
    // a tight 64-byte cap rejects `2 ** 1000`. The estimator
    // bounds the binary magnitude (~126 bytes here; the decimal
    // form would be 302 digits but the cap is on the storable
    // value, not its rendered string).
    let cfg = rubyrs::Config { max_value_bytes: Some(64), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "2 ** 1000",
        "pow_tight_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted under max_value_bytes=64, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_negative_exponent_returns_float() {
    // CRuby returns Rational `(1/4)` for `2 ** -2`; rubyrs uses
    // Float because there's no Rational in the subset
    // (documented SUBSET.md divergence). Pin the Float path here
    // since diff_cruby can't compare the formats.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts (2 ** -2)", "pow_neg.rb").expect("Float reciprocal path");
    assert_eq!(buf.snapshot().trim(), "0.25");
}

#[cfg(feature = "bignum")]
#[test]
fn pow_int_int_identity_bases_skip_numeric_u32_clamp() {
    // 0/±1 bases produce trivial results regardless of exponent
    // size — numeric.rs's `**` arm short-circuits via parity
    // BEFORE the `(*b as u64).min(u32::MAX as u64) as u32`
    // clamp it would otherwise apply. Without those short-
    // circuits `(-1) ** (u32::MAX + 2)` would clamp to the
    // u32::MAX exponent (odd) and silently flip sign for an
    // even input. The inputs here are all Int×Int, so dispatch
    // is owned by numeric.rs and never reaches
    // `Vm::try_bigint_pow` — the BigInt-exponent equivalent of
    // this guarantee lives in
    // `bigint_pow_identity_bases_with_bigint_exponent` below.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let huge = (u32::MAX as i64) + 1; // 4_294_967_296
    rt.eval(
        &format!("puts 1 ** {h}\nputs 0 ** {h}\nputs (-1) ** {h}\nputs (-1) ** ({h} + 1)",
            h = huge),
        "pow_identity_huge.rb",
    ).expect("identity bases must skip the u32 clamp");
    assert_eq!(buf.snapshot().trim(), "1\n0\n1\n-1");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_int_receiver_negative_bigint_exponent_returns_float() {
    // Int receiver + NEGATIVE BigInt exponent had no handler:
    // numeric.rs only covers Int×Int, and try_bigint_pow's
    // recv_is_bigint gate skipped Int receivers — so
    // `2 ** -(2**100)` raised NoMethodError despite
    // `respond_to?(:**)` being true. With the gate widened to
    // `recv OR exp is BigInt`, dispatch produces a Float
    // (which underflows toward 0 for |base|>1 since the BigInt
    // exponent is past f64 range — the helper coerces it to
    // -Inf, and `2 ** -Inf` = 0.0).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Build a negative BigInt via subtraction (BigInt unary
        // `-@` is unshipped Phase B.2).
        "neg_big = 0 - (2 ** 100)\n\
         puts (2 ** neg_big).zero?\n\
         puts (1 ** neg_big)\n\
         puts ((-1) ** neg_big)",
        "int_recv_neg_bigint_exp.rb",
    ).expect("Int recv + negative BigInt exp must not NoMethodError");
    // 2**-2**100 underflows to 0.0; 1**-big = 1.0 exactly;
    // (-1)**-big: big = 2^100 is even, so parity → 1.0.
    assert_eq!(buf.snapshot().trim(), "true\n1.0\n1.0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_bigint_receiver_negative_exponent_returns_float() {
    // BigInt receiver + negative Int exp must not NoMethodError —
    // respond_to?(:**) is true for BigInt, so the dispatch path
    // has to produce *something*. We pick Float (matches the
    // documented Rational divergence for `Int ** -n`).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    // (2 ** 100) ** -2 → 2**-200 ≈ 6.22e-61: a tiny but non-zero
    // Float (well above the smallest f64 subnormal at ~5e-324).
    rt.eval("puts ((2 ** 100) ** -2)", "bigint_pow_neg.rb")
        .expect("BigInt ** negative-Int must return a Float, not NoMethodError");
    let out = buf.snapshot();
    let v: f64 = out.trim().parse().expect("output must parse as Float");
    assert!(v > 0.0 && v < 1e-50, "expected tiny positive Float ~6e-61, got {}", v);
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_bigint_receiver_float_exponent_returns_float() {
    // BigInt receiver + Float exp must also return a Float, not
    // NoMethodError. `(2 ** 100) ** 0.5` ≈ 2**50 ≈ 1.126e15.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval("puts ((2 ** 100) ** 0.5)", "bigint_pow_float_exp.rb")
        .expect("BigInt ** Float must return a Float, not NoMethodError");
    let out = buf.snapshot();
    let v: f64 = out.trim().parse().expect("output must parse as Float");
    let expected = (2.0_f64).powi(50);
    let rel = ((v - expected) / expected).abs();
    assert!(rel < 1e-6, "expected ~{}, got {} (rel error {})", expected, v, rel);
}

#[cfg(feature = "bignum")]
#[test]
fn int_min_abs_promotes_to_bigint() {
    // `i64::MIN.abs` overflows i64 by exactly one (magnitude is
    // 2^63, one past i64::MAX). numeric.rs's `abs` arm now
    // declines under `bignum`, bigint_primitive's unary path
    // materialises the BigInt 2^63 and keeps it as BigInt (since
    // it doesn't fit i64). Same expectation for `-i64::MIN`.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts((-9_223_372_036_854_775_808).abs)\n\
         puts(-(-9_223_372_036_854_775_808))",
        "int_min_unary.rb",
    ).expect("i64::MIN unary must promote, not wrap");
    assert_eq!(buf.snapshot().trim(), "9223372036854775808\n9223372036854775808");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn int_min_abs_wraps_without_bignum() {
    // Without the bignum feature, `i64::MIN.abs` stays as
    // `i64::MIN` (wrapping_abs) — there's no BigInt fallback. Pin
    // the historical behaviour so a future no-bignum build can't
    // silently flip semantics.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts((-9_223_372_036_854_775_808).abs)",
        "int_min_unary_no_bignum.rb",
    ).expect("eval must succeed (wraps to i64::MIN)");
    assert_eq!(buf.snapshot().trim(), "-9223372036854775808");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_unary_plus_returns_same_value_id() {
    // `+@` on BigInt is a no-op clone — the resulting Value is
    // a `Value::BigInt(id)` pointing at the SAME heap entry as
    // the receiver. Numeric `==` would also pass if `+@` silently
    // re-allocated, so capture both values into a 2-element
    // Array and assert on the `Value::BigInt` ids directly.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval(
        "big = 2 ** 100\n[big, +big]",
        "bigint_unary_plus.rb",
    ).expect("+@ on BigInt must produce a Value");
    let elems = rt.resolve_array(&v).expect("expected Value::Array");
    assert_eq!(elems.len(), 2);
    match (&elems[0], &elems[1]) {
        (Value::BigInt(a), Value::BigInt(b)) => assert_eq!(
            a, b,
            "+@ must return a Value::BigInt pointing at the same heap id",
        ),
        other => panic!("expected (Value::BigInt, Value::BigInt), got {:?}", other),
    }
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_unary_neg_demotes_when_result_fits_int() {
    // `-big` where `big` after negation fits i64 must demote to
    // `Value::Int`. `2 ** 63` is exactly i64::MAX + 1
    // (9223372036854775808); negating gives i64::MIN exactly,
    // which fits. Demote-on-fit should produce
    // `Value::Int(i64::MIN)`. Numeric `==` would silently pass
    // even if the result stayed `Value::BigInt`, so assert
    // directly on the Value variant.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval(
        "big = 2 ** 63\n-big",
        "bigint_unary_neg_demote.rb",
    ).expect("eval must succeed");
    assert!(
        matches!(v, Value::Int(i64::MIN)),
        "expected Value::Int(i64::MIN), got {:?}",
        v,
    );
}

#[cfg(not(feature = "bignum"))]
#[test]
fn pow_method_works_under_no_bignum_profile() {
    // Both 1-arg and 2-arg `Integer#pow` must work on the no-bignum
    // profile too — `respond_to?(:pow)` is whitelisted
    // unconditionally, so dispatch needs to match. 1-arg delegates
    // to `**` (numeric.rs alias). 2-arg uses an i128 square-and-
    // multiply since BigInt isn't available.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 5.pow(3)\n\
         puts 5.pow(3, 7)\n\
         puts 7.pow(8, 5)\n\
         puts (-5).pow(3, 7)\n\
         puts 5.pow(3, -7)",
        "pow_no_bignum.rb",
    ).expect("pow must work without bignum");
    // 5³=125; 125 mod 7 = 6; 7⁸ mod 5 = 1; (-5)³=-125, -125 floor-mod 7 = 1
    // (since -125 = 7*-18 + 1); 125 floor-mod -7 = -1.
    assert_eq!(buf.snapshot().trim(), "125\n6\n1\n1\n-1");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn digits_no_bignum_arity_guard_raises_argument_error() {
    // Under no-bignum, `bigint_primitive`'s arity guard doesn't
    // exist — the dispatch.rs Int fast path needs its own guard
    // so `5.digits(10, 2)` raises ArgumentError matching CRuby
    // instead of falling through to NoMethodError despite
    // `respond_to?(:digits)` being true.
    let mut rt = rubyrs::Runtime::new();
    for (script, n) in [
        ("5.digits(10, 2)", 2),
        ("5.digits(10, 2, 3)", 3),
        ("5.digits(10, 2, 3, 4)", 4),
    ] {
        let err = rt.eval(script, "no_bignum_digits_arity.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("wrong number of arguments (given {}, expected 0..1)", n),
            "wrong message for {:?}", script,
        );
    }
}

#[cfg(not(feature = "bignum"))]
#[test]
fn digits_int_path_error_semantics_match_bignum_profile() {
    // Cross-profile parity: the no-bignum Int#digits path
    // (dispatch.rs) must surface the same error class +
    // message text as the bignum BigInt path
    // (Vm::try_integer_digits). Pin the dispatch.rs error arms
    // so a future refactor that flips one side doesn't silently
    // diverge.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_msg) in [
        ("(-5).digits",     "out of domain"),
        ("5.digits(-2)",    "negative radix"),
        ("5.digits(1)",     "invalid radix 1"),
        ("5.digits(0)",     "invalid radix 0"),
    ] {
        let err = rt.eval(script, "no_bignum_digits.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(msg, expected_msg, "wrong message for {:?}", script);
    }
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_radix_bigint_traps_via_pre_alloc_cap() {
    // `'%b' % (2 ** N)` allocates ~N bytes during
    // `to_str_radix`. The post-format cap check in `Kernel#sprintf`
    // / `String#%` only sees the already-allocated result string
    // and can't unwind a host OOM. Pre-alloc cap in
    // `format_radix_any` must trap based on `bits()` BEFORE the
    // alloc runs.
    //
    // Set a 64 KB cap large enough for `2 ** 100_000` to exist
    // as a BigInt (~12.5 KB magnitude) but small enough that
    // its base-2 sprintf form (~100 KB) trips. Pin the trap.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "'%b' % (2 ** 100_000)",
        "sprintf_pre_alloc_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_decimal_bigint_traps_via_pre_alloc_cap() {
    // Companion to `sprintf_radix_bigint_traps_via_pre_alloc_cap`:
    // `'%d' % big` used to call `to_string()` directly with no
    // pre-allocation cap, leaving the most common integer
    // format-spec exposed to the host-OOM scenario the base-N
    // pre-alloc helper defends against.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    // `(2 ** 1_000_000)` is ~301_030 decimal digits — well above
    // the 64 KB cap, well below any reasonable host RAM ceiling
    // (~120 KB of BigInt magnitude). Pre-alloc check must trap
    // before `to_string()` materialises the 300 KB decimal string.
    let err = rt.eval(
        "'%d' % (2 ** 1_000_000)",
        "sprintf_decimal_pre_alloc_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_radix_cap_does_not_false_trap_decimal_at_exact_length() {
    // Regression for cycle 10: earlier the cap estimator used
    // integer `floor(log2(radix))` as the per-digit bit yield,
    // which over-estimated digit count by ~10% for radix 10.
    // `(10 ** 100).to_s` is exactly 101 chars ("1" + 100 "0"s);
    // pre-fix estimate was ceil(333 bits / 3) = 111, so a cap
    // of 105 would have false-trapped despite the rendered
    // value fitting. Post-fix estimate is 101, matching reality.
    let cfg = rubyrs::Config { max_value_bytes: Some(105), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (10 ** 100).to_s",
        "to_s_cap_tight.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let expected = format!("1{}", "0".repeat(100));
    assert_eq!(out.trim(), expected);
}

#[test]
fn sprintf_alt_form_suppresses_prefix_for_zero_value() {
    // CRuby suppresses the alt-form prefix when the value is
    // zero: `'%#x' % 0` → `"0"`, not `"0x0"`. Same for
    // `'%#o' % 0` (`"0"`, not `"00"`), `'%#b' % 0` (`"0"`,
    // not `"0b0"`). All literals here take the Int(0) path;
    // non-zero alt rendering pinned as the negative half of
    // the contract.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%#o' % 0\n\
         puts '%#x' % 0\n\
         puts '%#X' % 0\n\
         puts '%#b' % 0\n\
         puts '%#B' % 0\n\
         puts '%#o' % 7\n\
         puts '%#x' % 255",
        "sprintf_alt_zero.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Zero values: no prefix.
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "0");
    assert_eq!(lines[2], "0");
    assert_eq!(lines[3], "0");
    assert_eq!(lines[4], "0");
    // Non-zero: prefix present.
    assert_eq!(lines[5], "07");
    assert_eq!(lines[6], "0xff");
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_alt_form_zero_via_bignum_arithmetic_still_suppressed() {
    // Regression guard for the bignum profile: expressions
    // that route through the BigInt arithmetic path but reduce
    // to zero (`(2 ** 100) % (2 ** 100)`) demote to Int(0) per
    // the canonical-BigInt invariant, so the formatter sees
    // Int(0) and the alt prefix must still be suppressed.
    // The BigInt(0) formatting arm itself isn't reachable from
    // user code (demote-on-fit), but the `b.sign() != NoSign`
    // guard in `format_radix_any` defends against hand-built
    // BigInt(0) values from FFI / preamble paths; that guard
    // is exercised structurally rather than dynamically here.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%#x' % ((2 ** 100) % (2 ** 100))\n\
         puts '%#o' % ((2 ** 100) % (2 ** 100))\n\
         puts '%#b' % ((2 ** 100) % (2 ** 100))",
        "sprintf_alt_bignum_arith_zero.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert_eq!(out.trim(), "0\n0\n0");
}

#[test]
fn sprintf_alt_form_with_zero_pad_keeps_prefix_before_zeros() {
    // Regression guard: pre-fix `'%#08x' % 255` produced
    // `00000xff` (zero-pad inserted before the `0x` prefix);
    // CRuby produces `0x0000ff` (zeros go between prefix and
    // digits). Same for `%#08X`, `%#08b`, `%#08B`. Octal's `0`
    // alt prefix happens to behave identically under
    // unconditional zero-padding (`'%#08o' % 7` → `00000007`
    // either way), so no special handling there.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%#08x' % 255\n\
         puts '%#08X' % 255\n\
         puts '%#08b' % 7\n\
         puts '%#08B' % 7\n\
         puts '%#08o' % 7\n\
         puts '%#08x' % (2 ** 60)",
        "sprintf_alt_zero_pad.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0x0000ff");
    assert_eq!(lines[1], "0X0000FF");
    assert_eq!(lines[2], "0b000111");
    assert_eq!(lines[3], "0B000111");
    assert_eq!(lines[4], "00000007");
    assert_eq!(lines[5], "0x1000000000000000"); // body > width, no pad
}

#[test]
fn sprintf_radix_int_min_does_not_panic() {
    // Regression guard: `format_radix_int` used to compute the
    // magnitude of a negative i64 via `(-n) as u64`, which panics
    // in debug builds for `n == i64::MIN` (-i64::MIN overflows
    // i64). `'%x' % i64::MIN` is a legitimate Ruby call. Switch
    // to `unsigned_abs()` so the path stays panic-free; pin all
    // four base specifiers at the i64::MIN cell.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "imin = -9_223_372_036_854_775_808\n\
         puts '%x' % imin\n\
         puts '%X' % imin\n\
         puts '%o' % imin\n\
         puts '%b' % imin",
        "sprintf_imin.rb",
    ).expect("i64::MIN sprintf must not panic");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // Documented divergence: we render `-<unsigned magnitude>`,
    // CRuby renders the `..f`-prefixed two's-complement form.
    assert_eq!(lines[0], "-8000000000000000");
    assert_eq!(lines[1], "-8000000000000000");
    assert_eq!(lines[2], "-1000000000000000000000");
    assert_eq!(lines[3], "-1000000000000000000000000000000000000000000000000000000000000000");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_times_upto_downto_iterate_with_demote_on_fit() {
    // Phase B.6: block-form iteration over BigInt operands.
    // Counter lives as a native num_bigint::BigInt; each
    // yielded Value is demoted to `Value::Int` when it fits i64
    // (`(big - 5).upto(big)` yields five BigInts but
    // `(2**65).times { |i| break if i >= 3 }` yields Int(0..3)
    // because the in-range counts fit i64 fine).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // BigInt#times: break early — yields Int because the
        // visited values fit i64.
        "arr = []\n\
         (2 ** 65).times { |i| arr << i; break if i >= 3 }\n\
         puts arr.inspect\n\
         puts arr[0].class.name\n\
         # BigInt#upto: small range across the i64 boundary —\n\
         # all yielded values are BigInt (> i64::MAX).\n\
         out = []\n\
         (2 ** 70).upto(2 ** 70 + 3) { |i| out << i.to_s }\n\
         puts out.inspect\n\
         # BigInt#downto: same but decreasing.\n\
         out2 = []\n\
         (2 ** 70).downto(2 ** 70 - 3) { |i| out2 << i.to_s }\n\
         puts out2.inspect\n\
         # Int recv + BigInt stop: start in-range, break early.\n\
         out3 = []\n\
         5.upto(2 ** 100) { |i| out3 << i; break if i >= 10 }\n\
         puts out3.inspect\n\
         # Negative BigInt#times → 0 iterations (CRuby).\n\
         calls = 0\n\
         (-(2 ** 65)).times { |i| calls += 1 }\n\
         puts \"neg=#{calls}\"\n\
         # Return value: recv when no break, break-value when break.\n\
         ret = (2 ** 65).downto(2 ** 65 - 2) { |_| }\n\
         puts \"ret_class=#{ret.class.name}\"\n\
         br = (2 ** 65).times { |i| break :early if i >= 1 }\n\
         puts \"break=#{br}\"\n\
         # respond_to? gates true for the new methods.\n\
         b = 2 ** 70\n\
         puts b.respond_to?(:times)\n\
         puts b.respond_to?(:upto)\n\
         puts b.respond_to?(:downto)",
        "bigint_iter.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "[0, 1, 2, 3]");
    assert_eq!(lines[1], "Integer"); // demoted
    assert_eq!(
        lines[2],
        "[\"1180591620717411303424\", \"1180591620717411303425\", \"1180591620717411303426\", \"1180591620717411303427\"]"
    );
    assert_eq!(
        lines[3],
        "[\"1180591620717411303424\", \"1180591620717411303423\", \"1180591620717411303422\", \"1180591620717411303421\"]"
    );
    assert_eq!(lines[4], "[5, 6, 7, 8, 9, 10]");
    assert_eq!(lines[5], "neg=0");
    assert_eq!(lines[6], "ret_class=Integer");
    assert_eq!(lines[7], "break=early");
    assert_eq!(lines[8], "true");
    assert_eq!(lines[9], "true");
    assert_eq!(lines[10], "true");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_iter_yield_pinned_across_rest_param_gc_window() {
    // Regression for PR #174 cycle 1: `invoke_block` builds the
    // rest-args Array via heap.alloc, which runs maybe_gc with
    // only the Block pinned — leaving any freshly-allocated
    // yielded Value reachable only from the local args Vec,
    // which GC doesn't see. Without the per-iteration
    // `vm.pinned.push(yield_val)` fix, this would sweep the
    // yielded BigInt and either panic or read garbage into
    // the rest-Array.
    //
    // Reproducer: BigInt counter (`(big - 5).upto(big)` yields
    // five separately-allocated BigInts), block with `|*args|`
    // rest param, allocations inside the block to trigger GC.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // `|*args|` triggers the rest-args allocation path in
        // invoke_block. The body allocates strings to pressure GC.
        // The yielded BigInt must survive into the rest-Array so
        // `args[0].to_s` produces the right value.
        "out = []\n\
         (2 ** 80).upto(2 ** 80 + 4) do |*args|\n\
           50.times { |k| _ = \"alloc#{k}\".dup }\n\
           out << args[0].to_s\n\
         end\n\
         puts out.size\n\
         puts out.first\n\
         puts out.last",
        "bigint_iter_rest_gc.rb",
    ).expect("eval");
    let lines: Vec<String> = buf.snapshot().trim().split('\n').map(String::from).collect();
    assert_eq!(lines[0], "5");
    assert_eq!(lines[1], "1208925819614629174706176"); // 2^80
    assert_eq!(lines[2], "1208925819614629174706180"); // 2^80 + 4
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_iter_survives_gc_inside_block() {
    // GC stress: the yielded BigInt sits in the block-arg slot
    // (a Ruby stack root) during invocation, but the block may
    // allocate strings that trigger maybe_gc. Verify the heap
    // entry stays reachable across collection cycles, with the
    // BigInt recv pinned via PinGuard so it survives too.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // 6 iterations, each allocating 50 small Strings to
        // pressure the heap. If the counter BigInt got swept
        // mid-iteration the to_s call would panic / read garbage.
        "out = []\n\
         (2 ** 80).upto(2 ** 80 + 5) do |i|\n\
           50.times { |k| _ = \"alloc#{k}\".dup }\n\
           out << i.to_s\n\
         end\n\
         puts out.size\n\
         puts out.first\n\
         puts out.last",
        "bigint_iter_gc.rb",
    ).expect("eval");
    let lines: Vec<String> = buf.snapshot().trim().split('\n').map(String::from).collect();
    assert_eq!(lines[0], "6");
    assert_eq!(lines[1], "1208925819614629174706176"); // 2^80
    assert_eq!(lines[2], "1208925819614629174706181"); // 2^80 + 5
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_works_as_hash_key_across_allocation_and_gc() {
    // Phase B.7 contract: the Hash collection's internal key
    // lookup uses `ruby_eq`, which for BigInt does value equality
    // via num_bigint. Two separately-allocated BigInts with the
    // same magnitude must therefore behave as the same key —
    // covering insert / lookup / size accounting / collision
    // semantics, plus survival across a GC stress that reallocates
    // every intermediate.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Insert via one allocation, look up via a fresh
        // allocation of the same magnitude — must hit.
        // Inserting the same key value a second time must NOT
        // grow the hash (overwrite, not duplicate).
        // Different-magnitude BigInts → distinct keys.
        // Mixed-magnitude paths (`2**63 * 2` and `2**64`) compute
        // the same value via different code paths and must hit
        // the same slot.
        // GC stress: alloc enough Strings to push a mark-sweep
        // cycle, then re-look-up the BigInt key — must still hit.
        "h = {}\n\
         h[2 ** 100] = :first\n\
         puts h[2 ** 100]\n\
         puts h.size\n\
         h[2 ** 100] = :second\n\
         puts h.size\n\
         puts h[2 ** 100]\n\
         h[2 ** 64] = :sixty_four\n\
         puts h[2 ** 63 * 2]\n\
         puts h.size\n\
         h[2 ** 200] = :two_hundred\n\
         puts h[2 ** 200]\n\
         puts h.size\n\
         # GC stress between insert and lookup\n\
         1000.times { |i| _ = \"alloc#{i}\".dup }\n\
         puts h[2 ** 100]\n\
         puts h[2 ** 200]",
        "bigint_hash_keys.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "first");      // lookup via separate alloc
    assert_eq!(lines[1], "1");          // single key
    assert_eq!(lines[2], "1");          // overwrite, not grow
    assert_eq!(lines[3], "second");     // value updated
    assert_eq!(lines[4], "sixty_four"); // 2^63*2 finds 2^64
    assert_eq!(lines[5], "2");          // 2^100 + 2^64
    assert_eq!(lines[6], "two_hundred");
    assert_eq!(lines[7], "3");
    assert_eq!(lines[8], "second");     // last write was :second;
                                        // survives 1000 String allocs
    assert_eq!(lines[9], "two_hundred");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_hash_equality_is_order_insensitive_with_bigint_keys() {
    // Phase B.7: `Hash#==` does order-insensitive comparison via
    // ruby_eq on both keys AND values, so two hashes built in
    // different orders with the same {BigInt → Value} mapping
    // must compare equal. Pre-existing behavior; pin it so the
    // BigInt-key path stays correct as the collection evolves.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "h1 = {}\n\
         h1[2 ** 100] = :a\n\
         h1[2 ** 200] = :b\n\
         h2 = {}\n\
         h2[2 ** 200] = :b\n\
         h2[2 ** 100] = :a\n\
         puts h1 == h2\n\
         # Differing values on equal keys → not equal\n\
         h3 = {}\n\
         h3[2 ** 100] = :a\n\
         h3[2 ** 200] = :different\n\
         puts h1 == h3\n\
         # Differing keys → not equal\n\
         h4 = {}\n\
         h4[2 ** 100] = :a\n\
         h4[2 ** 201] = :b\n\
         puts h1 == h4",
        "bigint_hash_eq.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "true");   // same mapping, different order
    assert_eq!(lines[1], "false");  // differing values
    assert_eq!(lines[2], "false");  // differing keys
}

#[cfg(feature = "bignum")]
#[test]
fn array_include_p_handles_bigint_value_equality() {
    // Phase B.7: `Array#include?(needle)` uses ruby_eq, which
    // for BigInt does value equality. A `needle` allocated
    // separately from the array's stored BigInt must still hit.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "arr = [2 ** 100, 5, 2 ** 64]\n\
         puts arr.include?(2 ** 100)\n\
         puts arr.include?(2 ** 64)\n\
         puts arr.include?(2 ** 63 * 2)\n\
         puts arr.include?(2 ** 101)\n\
         puts arr.include?(5)\n\
         # uniq dedups via ==\n\
         puts [2 ** 100, 2 ** 100, 2 ** 100].uniq.size",
        "bigint_array_include.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "true");
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "true");   // 2^63*2 == 2^64 via different path
    assert_eq!(lines[3], "false");
    assert_eq!(lines[4], "true");
    assert_eq!(lines[5], "1");      // all three BigInts coalesce
}

#[cfg(feature = "bignum")]
#[test]
fn integer_hash_is_within_process_stable_and_distinguishes_value() {
    // Phase B.7: `Integer#hash` returns a within-process-stable
    // i64 that satisfies `a.eql?(b) ⇒ a.hash == b.hash`. Pre-fix
    // every Integer receiver raised NoMethodError on `.hash`.
    //
    // The Hash collection itself uses linear scan via ruby_eq, so
    // this method isn't on the internal lookup path — it exists
    // for the user-facing protocol (pure-Ruby code calling
    // `n.hash` for its own bookkeeping). Stability is per-process
    // (DefaultHasher), matching CRuby's per-VM-seeded behaviour.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Same value → same hash (the key invariant).
        // Different value → almost-certainly different hash.
        // Sign matters: `n` and `-n` distinct hashes.
        // Cross-allocation BigInt stability.
        "puts 5.hash == 5.hash\n\
         puts 5.hash == 6.hash\n\
         puts (2 ** 100).hash == (2 ** 100).hash\n\
         puts (2 ** 100).hash == (2 ** 100 + 1).hash\n\
         puts (2 ** 100).hash == (-(2 ** 100)).hash\n\
         puts 5.hash.class.name\n\
         puts (2 ** 100).hash.class.name\n\
         puts 5.respond_to?(:hash)\n\
         puts (2 ** 100).respond_to?(:hash)",
        "integer_hash.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "true\nfalse\ntrue\nfalse\nfalse\nInteger\nInteger\ntrue\ntrue"
    );
}

#[cfg(feature = "bignum")]
#[test]
fn integer_eql_q_is_type_strict_equality() {
    // Phase B.7: `Integer#eql?` is value equality restricted to
    // matching numeric class. CRuby uses this (not `==`) for Hash
    // key matching at the language level, so it must distinguish
    // `5 == 5.0` (true) from `5.eql?(5.0)` (false). Pre-fix
    // rubyrs raised NoMethodError on every `Integer#eql?` call.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Int receiver:
        //   eql?(Int_same) → true
        //   eql?(Int_diff) → false
        //   eql?(Float) → false (type strict, even when values match)
        //   eql?(BigInt) → false (canonical invariant)
        //   eql?(String) → false
        // BigInt receiver:
        //   eql?(BigInt_same_value) → true (separate allocs OK)
        //   eql?(BigInt_diff) → false
        //   eql?(Int) → false (canonical invariant)
        //   eql?(Float) → false (type strict)
        // respond_to? whitelist covers both receivers.
        "puts 5.eql?(5)\n\
         puts 5.eql?(6)\n\
         puts 5.eql?(5.0)\n\
         puts 5.eql?(2 ** 100)\n\
         puts 5.eql?(\"5\")\n\
         puts (2 ** 100).eql?(2 ** 100)\n\
         puts (2 ** 100).eql?(2 ** 100 + 1)\n\
         puts (2 ** 100).eql?(5)\n\
         puts (2 ** 100).eql?(2.0)\n\
         puts 5.respond_to?(:eql?)\n\
         puts (2 ** 100).respond_to?(:eql?)",
        "integer_eql.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert_eq!(
        out.trim(),
        "true\nfalse\nfalse\nfalse\nfalse\ntrue\nfalse\nfalse\nfalse\ntrue\ntrue"
    );
}

#[test]
fn eql_q_and_hash_raise_argumenterror_on_wrong_arity() {
    // Phase B.7 review: pre-fix wrong-arity calls on eql?/hash
    // bypassed the exact-arity per-type arms and surfaced as
    // NoMethodError instead of CRuby's
    // ArgumentError. User code's `rescue ArgumentError` keys on
    // the error class, so the divergence is observable.
    //
    // Universal `eql?` interceptor raises for any non-1 arg
    // count. `hash` arity guard fires only for receivers that
    // actually support hash (gated by responds_to) so unrelated
    // `obj.hash(:x)` for obj without hash still surfaces as
    // NoMethodError per CRuby.
    let mut rt = rubyrs::Runtime::new();
    for (script, expected) in [
        ("5.eql?(1, 2)",          "wrong number of arguments (given 2, expected 1)"),
        ("5.hash(:x)",            "wrong number of arguments (given 1, expected 0)"),
        ("5.0.eql?(1, 2)",        "wrong number of arguments (given 2, expected 1)"),
        ("5.0.hash(:x)",          "wrong number of arguments (given 1, expected 0)"),
        ("(2 ** 100).hash(:x)",   "wrong number of arguments (given 1, expected 0)"),
        ("nil.eql?(1, 2)",        "wrong number of arguments (given 2, expected 1)"),
        ("\"a\".eql?(1, 2)",      "wrong number of arguments (given 2, expected 1)"),
    ] {
        let err = rt.eval(script, "arity.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "ArgumentError", "for {:?}", script);
                assert_eq!(message, expected, "for {:?}", script);
            }
            other => panic!("expected Uncaught ArgumentError for {:?}, got {:?}", script, other),
        }
    }
}

#[test]
fn universal_eql_q_delegates_to_ruby_eq_for_non_numeric_receivers() {
    // Phase B.7 review: pre-fix nil/Sym/Bool/String/Array/Hash/
    // arbitrary-Object all raised NoMethodError on `.eql?(x)`
    // because only Integer (+ Float in this PR) had per-type
    // arms. CRuby's Kernel#eql? defaults to identity for user
    // objects, but Array/Hash/String override it to value
    // equality.
    //
    // Add a universal dispatch interceptor that fires AFTER
    // primitive_call (so per-type type-strict numeric arms still
    // win) and delegates to `ruby_eq`. This matches CRuby for:
    //  - immediates (Sym/Bool/Nil) — identity ≡ value
    //  - String — value equality (via ruby_eq's Str arm)
    //  - Array/Hash/Range — value equality (recursive ruby_eq)
    //  - heap-allocated Objects/Methods/Procs — ObjId identity
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Immediates: identity = value.
        // String: value equality.
        // Arrays / Hashes: value equality across allocations.
        // Cross-type: false.
        // respond_to gated universally.
        "puts nil.eql?(nil)\n\
         puts nil.eql?(false)\n\
         puts :sym.eql?(:sym)\n\
         puts :sym.eql?(:other)\n\
         puts \"a\".eql?(\"a\")\n\
         puts \"a\".eql?(\"b\")\n\
         puts true.eql?(true)\n\
         puts true.eql?(false)\n\
         puts [1, 2].eql?([1, 2])\n\
         puts [1, 2].eql?([1, 3])\n\
         puts({a: 1}.eql?({a: 1}))\n\
         puts({a: 1}.eql?({a: 2}))\n\
         puts nil.respond_to?(:eql?)\n\
         puts :sym.respond_to?(:eql?)\n\
         puts \"x\".respond_to?(:eql?)\n\
         puts [].respond_to?(:eql?)",
        "universal_eql.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "true\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue"
    );
}

#[test]
fn float_eql_and_hash_are_type_strict_siblings_to_integer() {
    // Phase B.7 review: shipping `eql?`/`hash` only on Integer
    // made the canonical `5.eql?(5.0) == false` case
    // unexercisable from the Float side. Add the sibling methods
    // with a distinct hash tag so `5.hash != 5.0.hash` —
    // required by the `a.eql?(b) ⇒ a.hash == b.hash` invariant.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 5.0.eql?(5.0)\n\
         puts 5.0.eql?(5)\n\
         puts 5.eql?(5.0)\n\
         puts 5.0.eql?(6.0)\n\
         puts 5.0.eql?(\"5\")\n\
         puts 5.0.hash == 5.0.hash\n\
         puts 5.0.hash == 5.hash\n\
         puts 5.0.respond_to?(:eql?)\n\
         puts 5.0.respond_to?(:hash)",
        "float_eql_hash.rb",
    ).expect("eval");
    assert_eq!(
        buf.snapshot().trim(),
        "true\nfalse\nfalse\nfalse\nfalse\ntrue\nfalse\ntrue\ntrue"
    );
}

#[test]
fn equal_q_handles_sibling_heap_variants_via_identity() {
    // Phase B.7 drive-by: `Object#equal?` mirrored its BigInt arm
    // pattern for the four other heap-allocated variants that
    // previously fell through to ruby_eq's `_ => false` default
    // and reported `false` even for self-comparison.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "m = 5.method(:succ)\n\
         puts m.equal?(m)\n\
         um = Integer.instance_method(:succ)\n\
         puts um.equal?(um)\n\
         c = proc { |a, b| a + b }.curry\n\
         puts c.equal?(c)\n\
         r = /x/\n\
         puts r.equal?(r)",
        "equal_sibling.rb",
    ).expect("eval");
    assert_eq!(buf.snapshot().trim(), "true\ntrue\ntrue\ntrue");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_equal_q_is_object_identity_not_value_equality() {
    // Phase B.7: `Object#equal?` is BasicObject identity, not
    // value equality. For heap-managed types (Array, Hash, Str,
    // BigInt) two separately-allocated objects with identical
    // value must NOT be `equal?`. Pre-fix BigInt fell through
    // to ruby_eq's value-equality default and `(2**64).equal?(2**64)`
    // wrongly returned true.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Two separate allocs, same value → distinct objects.
        // `a.equal?(a)` is always true (same alloc).
        // `==` (value equality) is still true.
        "a = 2 ** 64\n\
         b = 2 ** 64\n\
         puts a.equal?(b)\n\
         puts a.equal?(a)\n\
         puts (2 ** 64).equal?(2 ** 64)\n\
         puts a == b",
        "bigint_equal.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "false");  // separate allocs
    assert_eq!(lines[1], "true");   // same alloc
    assert_eq!(lines[2], "false");  // separate literals
    assert_eq!(lines[3], "true");   // value equality unchanged
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_bitwise_not_uses_twos_complement_identity() {
    // Phase B.3: BigInt bit ops. `~big` is two's-complement
    // bitwise NOT — equivalent to `-(big + 1)` for any sign.
    // Numeric.rs's `(Int, "~", [])` arm handles Int receivers
    // (since `!i64::MIN == i64::MAX` fits without promotion),
    // but BigInt receivers need bigint_primitive's path.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // - `~(2**100)` = -(2^100 + 1) — stays BigInt.
        // - `~(-(2**100))` = -(-(2^100) + 1) = 2^100 - 1 — stays BigInt.
        // - `~(2**63)` = -(2^63 + 1) — one past i64::MIN, stays BigInt.
        // - `~(2**63 - 1)` = -(2^63) = i64::MIN — demotes to Int via
        //   bigint_to_value's demote-on-fit. Pins that the demote
        //   funnel runs for `~` results too (catches a regression
        //   where the bit-op path bypassed bigint_to_value).
        // - `~~big == big` round-trip (involution).
        "puts (~(2 ** 100)).to_s\n\
         puts (~(-(2 ** 100))).to_s\n\
         puts (~(2 ** 63)).to_s\n\
         puts (~(2 ** 63 - 1)).to_s\n\
         puts (~(2 ** 63 - 1)).class.name\n\
         puts (~~(2 ** 100)).to_s == (2 ** 100).to_s",
        "bigint_bitnot.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "-1267650600228229401496703205377");
    assert_eq!(lines[1], "1267650600228229401496703205375");
    assert_eq!(lines[2], "-9223372036854775809");
    assert_eq!(lines[3], "-9223372036854775808");
    assert_eq!(lines[4], "Integer");
    assert_eq!(lines[5], "true");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_bitwise_and_or_xor_two_complement_semantics() {
    // Phase B.3b: `&` / `|` / `^` with at least one BigInt operand.
    // CRuby uses unbounded two's-complement representation for
    // negatives in bitwise ops. num_bigint's BitAnd/Or/Xor impls
    // perform the conversion internally so we just route through
    // them — but pin the expected results to catch any future
    // regression in either the num_bigint contract or our hook.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Magnitude masks: `(2**100) & 0xff == 0` (low 8 bits of
        // 2^100 are all 0), demotes to Int.
        // Sign extension: `(-1) & (2**100) == 2**100` (-1 is
        // all-ones in two's-complement).
        // Sign extension: `(-256) & 0xff == 0` (low 8 bits of
        // two's-complement -256 are clear).
        // OR with low bit: `(2**100) | 1` lights bit 0 — full
        // BigInt result.
        // Self-XOR: cancels to 0 (Int via demote).
        // Inverse receiver: `5 & (2**100)` — Int recv + BigInt arg,
        // exercises the recv-or-arg guard path.
        // Mixed sign: `(-(2**100)) & 0xff == 0` (bit 0..7 of
        // -(2^100) in two's-complement are 0).
        "puts ((2 ** 100) & 0xff)\n\
         puts ((2 ** 100) & 0xff).class.name\n\
         puts ((-1) & (2 ** 100))\n\
         puts ((-256) & 0xff)\n\
         puts ((2 ** 100) | 1)\n\
         puts ((2 ** 100) ^ (2 ** 100))\n\
         puts ((2 ** 100) ^ (2 ** 100)).class.name\n\
         puts (5 & (2 ** 100))\n\
         puts ((-(2 ** 100)) & 0xff)",
        "bigint_bitops.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "Integer");
    assert_eq!(lines[2], "1267650600228229401496703205376");
    assert_eq!(lines[3], "0");
    assert_eq!(lines[4], "1267650600228229401496703205377");
    assert_eq!(lines[5], "0");
    assert_eq!(lines[6], "Integer");
    assert_eq!(lines[7], "0");
    assert_eq!(lines[8], "0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_left_right_promote_and_collapse() {
    // Phase B.3c: `<<` / `>>` with BigInt-flavoured operands.
    // Covers:
    // - Int recv overflow promote: `1 << 64` was Int 0 pre-fix
    //   (wrapping_shl clamped to 63), now BigInt 2^64.
    // - BigInt magnitude: `1 << 100` produces 2^100.
    // - BigInt recv right-shift: `(2**100) >> 50` = 2^50 (Int demote).
    // - Right-shift collapse: shifting past bit-length returns 0
    //   (non-neg) or -1 (neg) via the early-exit, not a giant alloc.
    // - Negative shift count: `5 << -1 == 5 >> 1 == 2`.
    // - Demote-on-fit: `(2**100) << -100 == 1`.
    // - Identity short-circuit: `1 << 0 == 1` returns recv unchanged.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (1 << 64)\n\
         puts (1 << 64).class.name\n\
         puts (1 << 100)\n\
         puts ((2 ** 100) >> 50)\n\
         puts ((2 ** 100) >> 50).class.name\n\
         puts ((2 ** 100) >> 1000)\n\
         puts ((-(2 ** 100)) >> 1000)\n\
         puts (5 << -1)\n\
         puts ((2 ** 100) << -100)\n\
         puts ((2 ** 100) << -100).class.name\n\
         puts (5 >> 100)\n\
         puts ((-1) >> 100)",
        "bigint_shifts.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "18446744073709551616");
    assert_eq!(lines[1], "Integer");
    assert_eq!(lines[2], "1267650600228229401496703205376");
    assert_eq!(lines[3], "1125899906842624");
    assert_eq!(lines[4], "Integer");
    assert_eq!(lines[5], "0");
    assert_eq!(lines[6], "-1");
    assert_eq!(lines[7], "2");
    assert_eq!(lines[8], "1");
    assert_eq!(lines[9], "Integer");
    assert_eq!(lines[10], "0");
    assert_eq!(lines[11], "-1");
}

#[cfg(feature = "bignum")]
#[test]
fn int_shift_left_promotes_on_value_overflow_not_just_count_overflow() {
    // Regression for PR #159 cycle 1: `i64::checked_shl` only
    // detects shift-count overflow (≥ 64), not value overflow.
    // Pre-fix, `1 << 63` returned `i64::MIN` (sign bit set,
    // wrapping into negative space) instead of promoting to
    // BigInt(2^63). Round-trip check `(a << s) >> s == a`
    // catches bit-loss exactly so these subtler overflow cases
    // promote like the count-overflow path already did for
    // `1 << 64`.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // - `1 << 62` is exactly the largest positive i64 (sign
        //   bit clear) — must stay Int, no false promote.
        // - `1 << 63` is +2^63 in Ruby (positive Bignum), not
        //   `i64::MIN`. Must promote.
        // - `5 << 61` overflows into the sign bit (5 takes 3
        //   bits, +61 = bit 63 set) — must promote.
        // - `1 >> -63` == `1 << 63` via direction swap — same
        //   value-overflow path, must promote.
        // - `(-1) << 1` == -2 stays Int (sign-preserving, no
        //   bit-loss).
        "puts (1 << 62)\n\
         puts (1 << 62).class.name\n\
         puts (1 << 63)\n\
         puts (1 << 63).class.name\n\
         puts (5 << 61)\n\
         puts (5 << 61).class.name\n\
         puts (1 >> -63)\n\
         puts (1 >> -63).class.name\n\
         puts ((-1) << 1)\n\
         puts ((-1) << 1).class.name",
        "int_shift_value_overflow.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "4611686018427387904");
    assert_eq!(lines[1], "Integer");
    assert_eq!(lines[2], "9223372036854775808"); // +2^63, NOT i64::MIN
    assert_eq!(lines[3], "Integer");
    assert_eq!(lines[4], "11529215046068469760"); // 5 * 2^61
    assert_eq!(lines[5], "Integer");
    assert_eq!(lines[6], "9223372036854775808"); // 1 >> -63 == 1 << 63
    assert_eq!(lines[7], "Integer");
    assert_eq!(lines[8], "-2");
    assert_eq!(lines[9], "Integer");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_left_dos_cap_uses_exact_int_bit_length() {
    // Regression for PR #159 cycle 1: `recv_bits` over-counted
    // Int receivers as 64 bits, so small-magnitude shifts under a
    // tight `max_value_bytes` could false-trap even when the
    // rendered BigInt fit. With exact bit-length for Ints, the
    // cap estimator matches the actual storage.
    //
    // `5 << 1_000_000` produces a ~125 KB BigInt. Pre-fix recv_bits
    // = 64 → est_bits = 1_000_064 → est_bytes ≈ 125_040. With
    // a cap of 125_064 bytes (just above the true est) pre-fix
    // would still trap because the 64-bit Int width over-counted
    // by ~61 bits. Post-fix recv_bits = bit_length(5) = 3 →
    // est_bits = 1_000_003 → est_bytes ≈ 125_032, passes.
    let cfg = rubyrs::Config { max_value_bytes: Some(125_064), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // `class` returns Integer for both Int and Bignum, so check
        // a deterministic property: bit_length matches the shift.
        "puts (5 << 1_000_000).bit_length",
        "shift_dos_exact_bits.rb",
    ).expect("eval");
    // `bit_length(5) == 3`, so `(5 << 1_000_000).bit_length == 1_000_003`.
    // Ruby prints integers without underscores.
    assert_eq!(buf.snapshot().trim(), "1000003");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_responds_to_bit_op_names_matches_dispatch() {
    // Regression for PR #159 cycle 2: `Vm::responds_to`'s BigInt
    // whitelist must include every method `bigint_primitive` can
    // dispatch — otherwise `big.respond_to?(:<<)` returns false
    // even though the call succeeds, breaking pure-Ruby code that
    // gates on respond_to?. Phase B.3 adds `~`, `& | ^`, `<< >>`.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "b = 2 ** 100\n\
         puts b.respond_to?(:~)\n\
         puts b.respond_to?(:&)\n\
         puts b.respond_to?(:|)\n\
         puts b.respond_to?(:^)\n\
         puts b.respond_to?(:<<)\n\
         puts b.respond_to?(:>>)",
        "bigint_responds_to_bit_ops.rb",
    ).expect("eval");
    let out = buf.snapshot();
    assert_eq!(out.trim(), "true\ntrue\ntrue\ntrue\ntrue\ntrue");
}

#[cfg(not(feature = "bignum"))]
#[test]
fn int_shift_i64_min_count_does_not_panic_under_no_bignum() {
    // Regression for the no-bignum `<<` / `>>` arms in
    // numeric.rs: pre-fix `(-b) as u32` overflowed when
    // `b == i64::MIN` (debug builds panicked with "attempt to
    // negate with overflow"; release silently wrapped to a
    // 63-bit shift via two-step wrap). Pin clamp semantics so
    // both profiles agree on the result for this corner.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    // `5 << i64::MIN` == `5 >> |i64::MIN|` == `5 >> 63` == 0.
    // `(-1) << i64::MIN` == `(-1) >> |i64::MIN|` == -1 (sign-ext).
    // `5 >> i64::MIN` == `5 << |i64::MIN|` clamped to 63 bits;
    //   `5.wrapping_shl(63)` produces `i64::MIN`-relative bit
    //   pattern (5 << 63 wraps), but the saturating-shift
    //   semantics under no-bignum just want no-panic + matching
    //   the existing wrapping behaviour. Pin the result so
    //   future refactors don't accidentally change it.
    rt.eval(
        "x = -9223372036854775807 - 1\n\
         puts (5 << x)\n\
         puts ((-1) << x)",
        "shift_i64_min_no_bignum.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "-1");
}

#[cfg(feature = "bignum")]
#[test]
fn integer_bit_ops_raise_typeerror_on_non_integer_arg() {
    // Phase B.3 follow-up: pre-fix `try_bigint_bit_binop` and
    // `try_bigint_bit_shift` returned `Ok(None)` when the arg
    // wasn't an Integer, falling through to NoMethodError. CRuby
    // raises TypeError "no implicit conversion of X into Integer"
    // — same shape as the BigInt-arith coerce errors and as the
    // unified `Integer#to_s(non_integer)` arm. Pin that both
    // Int and BigInt receivers route through the same TypeError
    // for every bit-op selector. Covers:
    // - all 5 bit-op selectors (& | ^ << >>)
    // - all 4 non-Integer arg types (Float, String, nil, Symbol)
    // - both Int and BigInt receivers
    // - the special `Int(0)` recv case (which used to short-circuit
    //   ahead of the arg-type guard)
    let mut rt = rubyrs::Runtime::new();
    for (script, expected_arg_type) in [
        // BigInt recv, every selector × Float arg
        ("(2 ** 100) & 1.5", "Float"),
        ("(2 ** 100) | 1.5", "Float"),
        ("(2 ** 100) ^ 1.5", "Float"),
        ("(2 ** 100) << 1.5", "Float"),
        ("(2 ** 100) >> 1.5", "Float"),
        // Int recv, non-Integer args
        ("5 & 1.5", "Float"),
        ("5 << 1.5", "Float"),
        ("5 >> \"foo\"", "String"),
        ("5 << nil", "nil"),
        ("5 << :sym", "Symbol"),
        // Int(0) recv: regression for the swallow-TypeError fix.
        ("0 << 1.5", "Float"),
        ("0 >> :sym", "Symbol"),
        ("0 << nil", "nil"),
    ] {
        let err = rt.eval(script, "bit_op_nonint_arg.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "TypeError", "for {:?}", script);
                assert_eq!(
                    message,
                    format!("no implicit conversion of {} into Integer", expected_arg_type),
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught TypeError for {:?}, got {:?}", script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn int_shift_zero_receiver_never_traps_regardless_of_count() {
    // Regression for PR #159 cycle 2: `0 << anything == 0` and
    // `0 >> anything == 0` in Ruby — should never allocate, never
    // trap on the DoS cap, never trap on the BigInt-count "shift
    // exceeds u32::MAX" guard. Pre-fix `0 << 1_000_000` under a
    // 1024-byte cap would trap because the cap estimator computed
    // `est_bits = 0 + 1_000_000` → 125 KB which exceeds 1 KB.
    let cfg = rubyrs::Config { max_value_bytes: Some(1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // Tight cap, huge shift counts: all should return 0
        // without touching the DoS estimator or the BigInt-count
        // trap.
        "puts (0 << 1_000_000)\n\
         puts (0 << (2 ** 100))\n\
         puts (0 >> 1_000_000)\n\
         puts (0 >> -(2 ** 100))",
        "zero_shift.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "0");
    assert_eq!(lines[2], "0");
    assert_eq!(lines[3], "0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_left_traps_dos_via_max_value_bytes() {
    // Left-shift DoS cap: `1 << 1_000_000` would allocate
    // ~125 KB. With a 64 KB `max_value_bytes`, the pre-cap
    // estimator must trap before BigInt::shl touches the
    // allocator. Honours `max_value_bytes` with the same 1 MB
    // fallback as `try_bigint_pow`.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "1 << 1_000_000",
        "shift_dos.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_shift_by_bigint_count_left_traps_right_collapses() {
    // BigInt shift count: by canonical invariant any BigInt is
    // outside i64, so:
    // - actual-left-shift by BigInt count → trap (would need
    //   > 2^63 bits of storage).
    // - actual-right-shift by BigInt count → collapse to 0 / -1
    //   without touching num_bigint (avoids the impossible alloc).
    let mut rt = rubyrs::Runtime::new();
    // Right-shift by BigInt count: collapses.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts ((2 ** 100) >> (2 ** 100))\n\
         puts ((-(2 ** 100)) >> (2 ** 100))",
        "shift_by_bigint_right.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "-1");
    // Left-shift by BigInt count: traps regardless of cap.
    let err = rt.eval(
        "1 << (2 ** 100)",
        "shift_by_bigint_left.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_radix_negative_uses_minus_magnitude_form() {
    // Two distinct CRuby behaviours for negative integers in
    // non-decimal bases:
    //   - `Integer#to_s(radix)` returns `-<magnitude>`:
    //     `(-256).to_s(16) == "-100"`. We match this exactly.
    //   - `sprintf '%x' % -256` returns `"..f00"` (CRuby's
    //     two's-complement infinite-ones notation). We diverge
    //     here and render `-<magnitude>` instead — documented
    //     in the sibling
    //     `sprintf_bigint_radix_negative_uses_minus_magnitude_divergence`
    //     test and in `format_radix_int`'s source comment.
    //
    // This test pins the `to_s` half — Int and BigInt receivers
    // both produce `-<magnitude>` for negative inputs, matching
    // CRuby byte-for-byte.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (-256).to_s(16)\n\
         puts (0 - (2 ** 100)).to_s(16)\n\
         puts (0 - (2 ** 64)).to_s(2).start_with?(\"-1\")",
        "bigint_to_s_neg.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "-100");
    // 2^100 in hex = 0x10000000000000000000000000 (1 followed by 25 zeros)
    assert!(lines[1].starts_with("-1") && lines[1].len() == 27,
        "expected -10000... (27 chars), got {:?}", lines[1]);
    assert_eq!(lines[2], "true");
}

#[cfg(feature = "bignum")]
#[test]
fn sprintf_bigint_radix_negative_uses_minus_magnitude_divergence() {
    // Documented divergence shared with the Int sprintf path:
    // CRuby renders `'%x' % -256` as `..f00` (two's-complement
    // infinite-ones notation), we render `-100`. Same shape for
    // negative BigInt. Pin our behaviour so a future "fix" that
    // adds CRuby compat is an opt-in upgrade rather than a silent
    // regression.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts '%x' % (0 - 256)\n\
         puts '%x' % (0 - (2 ** 100))\n\
         puts '%b' % (0 - (2 ** 16))",
        "sprintf_bigint_neg.rb",
    ).expect("eval");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines[0], "-100");
    assert!(lines[1].starts_with("-1") && lines[1].len() == 27);
    assert!(lines[2].starts_with("-1"));
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_to_s_radix_traps_under_max_value_bytes() {
    // Like the 0-arg to_s arm, the radix form's string output must
    // be capped against `max_value_bytes` to prevent a hostile
    // script from DoSing the host via `(2 ** 1_000_000).to_s(2)`.
    // `(2 ** 10_000).to_s(2)` is exactly 10_001 chars; pin under
    // a 4 KB cap so the trap fires.
    let cfg = rubyrs::Config { max_value_bytes: Some(4 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "(2 ** 10_000).to_s(2)",
        "to_s_radix_cap.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[test]
fn integer_to_s_non_integer_radix_raises_typeerror_on_int_path() {
    // Regression for cycle 13: the BigInt arm of `Integer#to_s(radix)`
    // raised `TypeError` for non-Integer radix, but the Int arm only
    // matched `Value::Int(radix)` and fell through to `NoMethodError`,
    // diverging from CRuby and from the BigInt path. Pin parity on
    // both sides — the unified `Integer#to_s` API should raise the
    // same `TypeError` regardless of receiver size.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.to_s(\"x\")", "int_to_s_typeerr.rb").unwrap_err();
    match err.err {
        rubyrs::RubyError::Uncaught { class_name, message } => {
            assert_eq!(class_name, "TypeError");
            assert_eq!(message, "no implicit conversion of String into Integer");
        }
        other => panic!("expected Uncaught TypeError, got {:?}", other),
    }
    // `Float` should error the same way (matches BigInt-path coercion).
    let err = rt.eval("5.to_s(1.0)", "int_to_s_typeerr_float.rb").unwrap_err();
    assert!(matches!(
        err.err,
        rubyrs::RubyError::Uncaught { ref class_name, .. } if class_name == "TypeError"
    ));
}

#[cfg(feature = "bignum")]
#[test]
fn integer_to_s_bigint_radix_raises_rangeerror_not_self_referential_typeerror() {
    // Pre-fix the catch-all `(Value::Int(_), "to_s", [other])` arm
    // intercepted `5.to_s(2**100)` (BigInt radix) and emitted
    // TypeError "no implicit conversion of Integer into Integer"
    // — `type_name_for_coerce` maps BigInt → "Integer" so the
    // wording was self-referential nonsense. CRuby raises
    // `RangeError: bignum too big to convert into 'long'` for this
    // shape (any BigInt is by canonical-BigInt invariant outside
    // i64, hence outside the 2..=36 radix range, but it IS an
    // Integer so TypeError is the wrong error class).
    let mut rt = rubyrs::Runtime::new();
    for script in ["5.to_s(2 ** 100)", "(2 ** 100).to_s(2 ** 100)"] {
        let err = rt.eval(script, "to_s_bigint_radix.rb").unwrap_err();
        match err.err {
            rubyrs::RubyError::Uncaught { class_name, message } => {
                assert_eq!(class_name, "RangeError", "for {:?}", script);
                assert_eq!(
                    message, "bignum too big to convert into `long'",
                    "for {:?}", script,
                );
            }
            other => panic!("expected Uncaught RangeError for {:?}, got {:?}", script, other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn integer_to_s_non_integer_radix_typeerror_message_matches_bigint_path() {
    // Cross-check the parity guard above against the BigInt path
    // so future drift between the two arms is caught immediately.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "(2 ** 100).to_s(\"x\")",
        "bigint_to_s_typeerr.rb",
    ).unwrap_err();
    match err.err {
        rubyrs::RubyError::Uncaught { class_name, message } => {
            assert_eq!(class_name, "TypeError");
            assert_eq!(message, "no implicit conversion of String into Integer");
        }
        other => panic!("expected Uncaught TypeError, got {:?}", other),
    }
}

#[test]
fn digits_negative_recv_takes_precedence_over_arity_and_base_errors() {
    // CRuby precedence: a negative `Integer#digits` receiver
    // raises Math::DomainError BEFORE any arity / base validation.
    // Pre-fix rubyrs checked arity / base type / base sign / base
    // < 2 first, so each shape surfaced a different error class.
    // Match CRuby's precedence so user code's `rescue ArgumentError`
    // catches the negative-recv path regardless of the other args'
    // shapes. Substitute is ArgumentError "out of domain" (same
    // convention as other numeric-out-of-domain arms in
    // Vm::do_call). Runs in both profiles.
    let mut rt = rubyrs::Runtime::new();
    for script in [
        "(-5).digits(10, 2)",     // would have been arity error
        "(-5).digits(-2)",        // would have been "negative radix"
        "(-5).digits(\"foo\")",   // would have been TypeError
        "(-5).digits(1)",         // would have been "invalid radix 1"
        "(-5).digits",            // pure negative-recv, no other badness
    ] {
        let err = rt.eval(script, "digits_precedence.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError (out-of-domain substitute) for {:?}, got {:?}",
            script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg, "out of domain",
            "wrong message for {:?} — expected the negative-recv check to fire first",
            script,
        );
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_negative_recv_raises_argument_error_substitute() {
    // CRuby raises `Math::DomainError: out of domain` for
    // `(-5).digits` (and the same shape for negative BigInt).
    // The established subset pattern (same convention as other
    // numeric-out-of-domain arms in Vm::do_call) substitutes
    // `ArgumentError` because `Math::DomainError` isn't modelled.
    // Pin the divergence so a future Math::DomainError addition
    // is an opt-in upgrade rather than a silent regression.
    let mut rt = rubyrs::Runtime::new();
    for script in [
        "(-5).digits",
        "(0 - (2 ** 100)).digits",
        "(-1).digits(16)",
    ] {
        let err = rt.eval(script, "digits_neg.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(msg, "out of domain", "wrong message for {:?}", script);
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_bigint_radix_survives_stress_gc() {
    // GC rooting regression guard. For a BigInt radix (e.g. base
    // = 2 ** 70), each digit produced is itself a heap-backed
    // `Value::BigInt(id)`. Every `bigint_to_value` call inside
    // the loop invokes `maybe_gc()`; without PinGuard rooting,
    // a sweep mid-loop could deallocate already-pushed digits,
    // leaving dangling ObjIds in the returned Array. Run under
    // forced GC (`stress_gc: true`) so every alloc triggers a
    // full mark — pre-fix this test panicked / produced wrong
    // values; with PinGuard around the loop it stays sound.
    let cfg = rubyrs::Config { stress_gc: true, ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let v = rt.eval(
        // (2 ** 200).digits(2 ** 70) — 3 digits, each potentially
        // BigInt-backed (top digit fits below 2^60 → demotes; the
        // other two could be BigInts). Verify all elements are
        // valid Integer values (no dangling refs / no panic).
        "(2 ** 200).digits(2 ** 70).map { |d| d.bit_length }",
        "digits_stress_gc.rb",
    ).expect("BigInt-radix digits must survive STRESS_GC");
    let elems = rt.resolve_array(&v).expect("expected Value::Array");
    // Each element is bit_length of a digit; values bounded by
    // log2(2^70) = 70. Just confirm we have a populated array of
    // small Ints — exact values are an implementation detail.
    assert!(!elems.is_empty(), "expected non-empty digits array");
    for e in &elems {
        match e {
            rubyrs::Value::Int(n) => assert!(*n >= 0 && *n <= 70, "bit_length out of range: {}", n),
            other => panic!("expected Value::Int (bit_length), got {:?}", other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_estimator_uses_log2_base_not_just_bits() {
    // Tighter estimator (`(recv_bits - 1) / (base.bits() - 1) + 1`)
    // means a `recv` whose base-2 expansion would exceed the cap
    // can still succeed in base-10 / base-16 — the actual digit
    // count for those bases is far smaller. Pin this so a future
    // refactor that drops the log-2 division and reverts to a
    // base-independent bound fails immediately.
    //
    // `(2 ** 1000).digits` is 302 decimal digits; at 16 B per
    // Value that's ~4.8 KB, well under an 8 KB cap. The base-2
    // form of the same recv would estimate 1001 elements
    // (~16 KB) and would correctly TRAP the 8 KB cap — exactly
    // the shape the sibling `digits_huge_bigint_in_base_2_traps_under_tight_cap`
    // test pins. So this test exercises the estimator's
    // base-awareness rather than its trap path.
    let cfg = rubyrs::Config { max_value_bytes: Some(8 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let v = rt.eval(
        "(2 ** 1000).digits.length",
        "digits_base10_fits.rb",
    ).expect("base-10 estimate must fit 8 KB cap for 2**1000");
    match v {
        rubyrs::Value::Int(n) => {
            // 2**1000 has 302 decimal digits.
            assert_eq!(n, 302, "expected 302 decimal digits, got {}", n);
        }
        other => panic!("expected Value::Int, got {:?}", other),
    }
}

#[cfg(feature = "bignum")]
#[test]
fn digits_huge_bigint_in_base_2_traps_under_tight_cap() {
    // `(2 ** 100_000).digits(2)` would produce a 100_001-element
    // array (~1.6 MB at 16 B per Value). Under a tight cap, the
    // helper traps ResourceExhausted before allocating. Pin the
    // pre-allocation bound so a future refactor that drops the
    // estimator-trip fails immediately.
    let cfg = rubyrs::Config { max_value_bytes: Some(16 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    let err = rt.eval(
        "(2 ** 100_000).digits(2)",
        "digits_huge.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn digits_returns_value_array_with_int_elements() {
    // Embedding-facing contract: result is `Value::Array` of
    // `Value::Int` digits (each digit fits i64 since base fits
    // i64). Lock the public-API shape rather than just the
    // printed form.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval("12345.digits", "digits_shape.rb").expect("eval");
    let elems = rt.resolve_array(&v).expect("expected Value::Array");
    let nums: Vec<i64> = elems.iter().map(|e| match e {
        rubyrs::Value::Int(n) => *n,
        other => panic!("expected Value::Int, got {:?}", other),
    }).collect();
    assert_eq!(nums, vec![5, 4, 3, 2, 1]);
}

#[cfg(feature = "bignum")]
#[test]
fn bit_length_bigint_two_complement_semantics() {
    // Embedding-facing contract: `bit_length` on BigInt returns
    // `Value::Int`. Verify both signs across boundary cases.
    let mut rt = rubyrs::Runtime::new();
    let cases: &[(&str, i64)] = &[
        ("(2 ** 100).bit_length", 101),
        ("(2 ** 200).bit_length", 201),
        ("(0 - (2 ** 100)).bit_length", 100),  // bit_length(-2^100) = 100
        ("(0 - (2 ** 100) - 1).bit_length", 101),  // bit_length(-2^100 - 1) = bit_length(2^100) = 101
    ];
    for (script, expected) in cases {
        let v = rt.eval(script, "bit_length.rb").expect(script);
        match v {
            rubyrs::Value::Int(n) => assert_eq!(n, *expected, "{} → {}", script, n),
            other => panic!("expected Value::Int, got {:?}", other),
        }
    }
}

#[cfg(feature = "bignum")]
#[test]
fn pow_arity_guard_fires_for_bigint_receiver() {
    // numeric.rs's arity guard only catches Int receivers — BigInt
    // receivers go through bigint_primitive's separate dispatch
    // path. Mirror the guard there so `big.pow` / `big.pow(1,2,3)`
    // raise CRuby's exact ArgumentError instead of NoMethodError.
    let mut rt = rubyrs::Runtime::new();
    for (script, n) in [
        ("big = 2 ** 100; big.pow", 0),
        ("big = 2 ** 100; big.pow(1, 2, 3)", 3),
    ] {
        let err = rt.eval(script, "bigint_pow_arity.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("wrong number of arguments (given {}, expected 1..2)", n),
            "wrong message for {:?}", script,
        );
    }
}

#[test]
fn pow_one_arg_non_numeric_raises_type_error() {
    // CRuby: `5.pow("x")` raises `TypeError: String can't be
    // coerced into Integer`. Pre-fix the 1-arg pow alias
    // recursed unconditionally to `**`, which (separately) only
    // surfaces NoMethodError for non-numeric args — so pow's
    // delegate inherited that wrong error class. Validate the
    // arg type at the pow boundary and raise TypeError directly.
    let mut rt = rubyrs::Runtime::new();
    for (script, class_name) in [
        ("5.pow(\"x\")", "String"),
        ("5.pow(nil)", "nil"),
        ("5.pow(true)", "true"),
        ("5.pow([1])", "Array"),
        ("5.pow({a: 1})", "Hash"),
    ] {
        let err = rt.eval(script, "pow_typeerr.rb").unwrap_err();
        assert!(
            err.err.is("TypeError"),
            "expected TypeError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::TypeError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("{} can't be coerced into Integer", class_name),
            "wrong message for {:?}", script,
        );
    }
}

#[cfg(feature = "bignum")]
#[test]
fn pow_one_arg_non_numeric_raises_type_error_for_bigint_receiver() {
    // Same fix on the BigInt receiver path — `(2 ** 100).pow("x")`
    // routes through `try_bigint_pow_method`'s 1-arg branch, which
    // mirrors the Int-side guard.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "(2 ** 100).pow(\"x\")",
        "bigint_pow_typeerr.rb",
    ).unwrap_err();
    assert!(err.err.is("TypeError"), "got {:?}", err.err);
    let msg = match &err.err {
        rubyrs::RubyError::TypeError { msg } => msg.clone(),
        rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
        _ => unreachable!(),
    };
    assert_eq!(msg, "String can't be coerced into Integer");
}

#[test]
fn pow_arity_zero_or_too_many_args_raise_argument_error() {
    // CRuby: `5.pow` and `5.pow(1, 2, 3)` raise ArgumentError
    // ("wrong number of arguments (given N, expected 1..2)").
    // Without the explicit arity guard those shapes fall through
    // to NoMethodError despite `respond_to?(:pow)` being true.
    let mut rt = rubyrs::Runtime::new();
    for (script, n) in [("5.pow", 0), ("5.pow(1, 2, 3)", 3), ("5.pow(1, 2, 3, 4, 5)", 5)] {
        let err = rt.eval(script, "pow_arity.rb").unwrap_err();
        assert!(
            err.err.is("ArgumentError"),
            "expected ArgumentError for {:?}, got {:?}", script, err.err,
        );
        let msg = match &err.err {
            rubyrs::RubyError::ArgumentError { msg } => msg.clone(),
            rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            msg,
            format!("wrong number of arguments (given {}, expected 1..2)", n),
            "wrong message for {:?}", script,
        );
    }
}

#[test]
fn pow_one_arg_accepts_float_exponent() {
    // `5.pow(1.5)` must mirror `5 ** 1.5` — both routes through
    // the same `**` arm. Previously the `pow` alias only fired
    // for `[Int]` exponents, so Float exp NoMethodErrored despite
    // being supported by `**`. Pin across both profiles.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 5.pow(1.5)\nputs 9.pow(0.5)",
        "pow_float_exp.rb",
    ).expect("Int#pow(Float) must work");
    let out = buf.snapshot();
    let lines: Vec<&str> = out.trim().split('\n').collect();
    let a: f64 = lines[0].parse().expect("Float output");
    let b: f64 = lines[1].parse().expect("Float output");
    // 5^1.5 ≈ 11.180339887; 9^0.5 = 3.0.
    assert!((a - 11.180_339_887).abs() < 1e-6);
    assert!((b - 3.0).abs() < 1e-12);
}

#[cfg(not(feature = "bignum"))]
#[test]
fn pow_no_bignum_two_arg_distinguishes_exp_vs_mod_type_errors() {
    // CRuby uses two distinct TypeError messages depending on
    // which arg is non-Integer: "...1st argument is integer" when
    // the exp is non-Int, "...all arguments are integers" when the
    // mod is non-Int. The no-bignum 2-arg path must match exactly
    // (the bignum path already does).
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(1.5, 7)", "exp_float.rb").unwrap_err();
    let msg = match &err.err {
        rubyrs::RubyError::TypeError { msg } => msg.clone(),
        rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
        other => panic!("expected TypeError, got {:?}", other),
    };
    assert!(
        msg.contains("a 1st argument is integer"),
        "wrong message for non-Int exp: {}",
        msg,
    );
    let err = rt.eval("5.pow(3, 1.5)", "mod_float.rb").unwrap_err();
    let msg = match &err.err {
        rubyrs::RubyError::TypeError { msg } => msg.clone(),
        rubyrs::RubyError::Uncaught { message, .. } => message.clone(),
        other => panic!("expected TypeError, got {:?}", other),
    };
    assert!(
        msg.contains("all arguments are integers"),
        "wrong message for non-Int mod: {}",
        msg,
    );
}

#[cfg(not(feature = "bignum"))]
#[test]
fn pow_no_bignum_error_shapes_match_cruby() {
    let mut rt = rubyrs::Runtime::new();
    assert!(
        rt.eval("5.pow(-1, 7)", "no_bignum_neg_exp.rb").unwrap_err().err.is("RangeError"),
    );
    assert!(
        rt.eval("5.pow(3, 0)", "no_bignum_zero_mod.rb").unwrap_err().err.is("ZeroDivisionError"),
    );
    assert!(
        rt.eval("5.pow(1.5, 7)", "no_bignum_float_exp.rb").unwrap_err().err.is("TypeError"),
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_huge_exponent_skips_dos_cap() {
    // `2.pow(huge_exp, mod)` must succeed even when `2 ** huge_exp`
    // would blow far past any reasonable max_value_bytes — modpow
    // never materialises the intermediate, so the cap on the
    // pre-modulo `**` path doesn't apply. Pin under a tight 1 KB
    // cap that `2 ** 100_000` would trip (12.5 KB real magnitude).
    let cfg = rubyrs::Config { max_value_bytes: Some(1024), ..Default::default() };
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts 2.pow(100_000, 1_000_000_007)",
        "pow_mod_huge.rb",
    ).expect("modpow must not trip the unmodulated `**` DoS cap");
    let v: i64 = buf.snapshot().trim().parse().expect("result must be Int");
    assert!((0..1_000_000_007).contains(&v),
        "result {} not in [0, mod)", v);
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_negative_exponent_raises_range_error() {
    // CRuby: `5.pow(-1, 7)` raises RangeError. Modular inverse may
    // not exist and we don't compute it — match by raising rather
    // than silently producing an unrelated value.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(-1, 7)", "pow_neg_exp_with_mod.rb").unwrap_err();
    assert!(
        err.err.is("RangeError"),
        "expected RangeError, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_zero_modulus_raises_zero_division() {
    // CRuby: `5.pow(3, 0)` raises ZeroDivisionError ("divided by 0").
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(3, 0)", "pow_zero_mod.rb").unwrap_err();
    assert!(
        err.err.is("ZeroDivisionError"),
        "expected ZeroDivisionError, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_non_integer_args_raise_type_error() {
    // CRuby: `5.pow(1.5, 7)` raises TypeError. Same for
    // `5.pow(3, 1.5)`. Pin the type-shape contract.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("5.pow(1.5, 7)", "pow_float_exp.rb").unwrap_err();
    assert!(err.err.is("TypeError"), "expected TypeError, got {:?}", err.err);
    let err = rt.eval("5.pow(3, 1.5)", "pow_float_mod.rb").unwrap_err();
    assert!(err.err.is("TypeError"), "expected TypeError, got {:?}", err.err);
}

#[cfg(feature = "bignum")]
#[test]
fn pow_mod_result_demotes_when_fits_int() {
    // The result is always strictly bounded by |mod|. When |mod|
    // fits i64, the result fits too — `bigint_to_value` should
    // demote so the embedding-facing `Value` is `Value::Int`, not
    // `Value::BigInt`. Pins demote-on-fit through the modpow path.
    let mut rt = rubyrs::Runtime::new();
    let v = rt.eval(
        "(2 ** 100).pow(50, 1_000_000_007)",
        "pow_mod_demote.rb",
    ).expect("eval must succeed");
    assert!(
        matches!(v, Value::Int(_)),
        "expected Value::Int (mod fits i64), got {:?}", v,
    );
}

#[test]
fn pow_zero_to_negative_exponent_raises_zero_division() {
    // CRuby: `0 ** -1` raises `ZeroDivisionError: divided by 0`
    // because the reciprocal of 0 is undefined. Previous rubyrs
    // routed through `(0_u64 as f64).powf(-1.0) = +Infinity` and
    // silently returned `Float::INFINITY`, poisoning downstream
    // arithmetic. Match CRuby and raise instead.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval("0 ** -1", "pow_zero_neg.rb").unwrap_err();
    assert!(
        err.err.is("ZeroDivisionError"),
        "expected ZeroDivisionError (direct or Uncaught-wrapped), got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn pow_zero_to_negative_bigint_exponent_raises_zero_division() {
    // Same divergence fix on the BigInt-flavoured path: when the
    // exponent is a (negative) BigInt and recv is Int(0), dispatch
    // goes through try_bigint_pow's |base|≤1 short-circuit. That
    // arm previously returned `Float::INFINITY` for BigInt-flavoured
    // operands. Now it raises ZeroDivisionError uniformly with the
    // Int×Int path.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "neg_big = 0 - (2 ** 100); 0 ** neg_big",
        "pow_zero_neg_bigint.rb",
    ).unwrap_err();
    assert!(
        err.err.is("ZeroDivisionError"),
        "expected ZeroDivisionError (direct or Uncaught-wrapped), got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_zero_and_one_exponent_skip_estimator() {
    // `big ** 0` must always return 1 and `big ** 1` must return
    // the receiver, regardless of cap. With the previous flow the
    // estimator added a 32-byte BigInt-header overhead to
    // est_bytes, so a sub-32-byte cap would trap `big ** 0` even
    // though no allocation is actually needed. Pin both shapes
    // under a minimal 16-byte cap.
    let cfg = rubyrs::Config { max_value_bytes: Some(16), ..Default::default() };
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    // Build a `big` BigInt under a larger cap-free runtime first
    // would change scope; instead use a small Int receiver where
    // the demoted result still hits the identity short-circuits.
    rt.eval(
        "puts 7 ** 0\nputs 7 ** 1\nputs (-3) ** 0\nputs (-3) ** 1",
        "pow_exp_identities.rb",
    ).expect("** 0 and ** 1 must short-circuit before the cap check");
    assert_eq!(buf.snapshot().trim(), "1\n7\n1\n-3");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_pow2_estimator_avoids_2x_overshoot() {
    // The DoS estimator must use `(base_bits - 1) * exp + 1` for
    // power-of-two bases, not `base_bits * exp` — otherwise a
    // factor-of-2 overestimate falsely rejects allocations that
    // fit. `2 ** 100_000` produces ~12.5 KB of magnitude; the
    // tight bound estimates ~12.5 KB and fits under a 16 KB cap.
    // The old `base_bits * exp` would have estimated ~25 KB and
    // trapped, even though the real value fits comfortably.
    let cfg = rubyrs::Config { max_value_bytes: Some(16 * 1024), ..Default::default() };
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.eval("2 ** 100_000", "pow2_tight_estimate.rb")
        .expect("tight pow-of-2 estimate must allow values that fit the cap");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_huge_bigint_float_coercion_skips_string_alloc() {
    // BigInt → f64 must NOT materialise a decimal string for
    // BigInts past f64 range — Copilot flagged that a script
    // could trigger an unbounded allocation via `huge ** 0.5`.
    // The bits()-based pre-check (> 1024 ⇒ ±∞ directly) caps
    // any intermediate string at ~310 digits. Build a BigInt
    // far past 2**1024, then exercise the Float and negative-Int
    // exp paths. Both must produce ±∞ Floats without trapping.
    let cfg = rubyrs::Config { max_value_bytes: Some(64 * 1024), ..Default::default() };
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::with_config(cfg);
    rt.set_stdout(Box::new(buf.clone()));
    // 2 ** 5000 ≈ 625 bytes of magnitude, fits the 64 KB cap; its
    // bits() == 5001 puts it well past the 1024 f64 threshold.
    rt.eval(
        "big = 2 ** 5000\n\
         puts (big ** 0.5).infinite?\n\
         puts (big ** -1).zero?",
        "bigint_huge_to_f64.rb",
    ).expect("must not trap or NoMethodError");
    // 0.5 of +∞ is still +∞; -1 reciprocal of +∞ is 0.0.
    assert_eq!(buf.snapshot().trim(), "1\ntrue");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_identity_bases_with_bigint_exponent() {
    // |base| ≤ 1 must not trap on BigInt exponents — results are
    // constant-size. Pin `1 ** big`, `0 ** big`, `(-1) ** big`
    // (even and odd via parity-preserving bit(0)).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "big_even = 2 ** 100\n\
         big_odd  = big_even + 1\n\
         puts 1 ** big_even\n\
         puts 0 ** big_even\n\
         puts (-1) ** big_even\n\
         puts (-1) ** big_odd",
        "pow_bigint_exp_identity.rb",
    ).expect("identity bases must accept BigInt exponents");
    assert_eq!(buf.snapshot().trim(), "1\n0\n1\n-1");
}

#[cfg(feature = "bignum")]
#[test]
fn pow_neg_exponent_negative_base_preserves_parity_via_abs_powf() {
    // Negative-base + large-magnitude negative-exp must keep
    // the sign decided by i64 parity rather than relying on
    // f64-rounded `powf(neg, non-int-as-int)` which can NaN
    // (or flip sign) on some libm impls. `(-2) ** -3` is a
    // small enough case to assert exactly: -1/8 = -0.125.
    // Then `(-2) ** -(2**60 + 1)` (odd huge) — past 2**53
    // f64-mantissa — must stay non-positive (underflows to
    // -0.0 or a tiny negative Float).
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let odd_huge = (1_i64 << 60) | 1;
    rt.eval(
        &format!("puts (-2) ** -3\nv = (-2) ** -{odd}\nputs v <= 0.0\nputs !v.nan?",
            odd = odd_huge),
        "pow_neg_base_parity.rb",
    ).expect("negative-base negative-exp must not NaN");
    assert_eq!(buf.snapshot().trim(), "-0.125\ntrue\ntrue");
}

#[cfg(feature = "bignum")]
#[test]
fn pow_neg_exponent_minus_one_preserves_parity_beyond_f64_mantissa() {
    // (-1) ** (-huge_odd) must remain -1.0; casting the i64
    // exponent through f64 loses parity past 2**53, so the
    // negative-exp arm has to short-circuit ±1 bases before powf.
    let buf = SharedBuf::new();
    let mut rt = rubyrs::Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    let odd = (1_i64 << 60) | 1; // 2**60 + 1: way past f64 mantissa
    rt.eval(
        &format!("puts (-1) ** (-{odd})\nputs (-1) ** (-({odd} - 1))", odd = odd),
        "pow_neg_exp_parity.rb",
    ).expect("parity must survive f64 cast");
    assert_eq!(buf.snapshot().trim(), "-1.0\n1.0");
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_bigint_exponent_traps() {
    // `2 ** (2**63)` (BigInt exponent) must trap ResourceExhausted
    // instead of falling through to NoMethodError. The doc comment
    // promises a clean error.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        "big = 2 ** 100; 2 ** big",
        "pow_bigint_exp.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted for BigInt exponent, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn bigint_pow_oversize_exponent_traps_for_real_bases() {
    // For bases with |a| > 1, an exponent that doesn't fit u32
    // must trap (the result would be astronomically large) —
    // verifies numeric_call declines on u32-overflow so
    // bigint_primitive can issue the trap.
    let mut rt = rubyrs::Runtime::new();
    let huge = (u32::MAX as i64) + 1;
    let err = rt.eval(
        &format!("2 ** {}", huge),
        "pow_oversize_exp.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted for u32-overflow exp, got {:?}",
        err.err,
    );
}

// Tier 1 capability-injection tests (Random / SecureRandom /
// Time) moved to `tests/embed/tier1_capability.rs`.

#[test]
fn array_first_last_non_int_n_raises_no_method_error_today() {
    // Pin the current rubyrs divergence from CRuby on
    // `Array#first(n)` / `Array#last(n)` when `n` isn't an
    // `Int`.
    //
    // CRuby behaviour (2026-05):
    //   - `[1,2,3].first(2.0)` returns `[1, 2]` — Float's
    //     `to_int` coerces to 2.
    //   - `[1,2,3].last(:x)`   raises `TypeError: no implicit
    //     conversion of Symbol into Integer`.
    //
    // rubyrs behaviour: both raise `NoMethodError: undefined
    // method 'first'/'last' for Array` because the match arms
    // in `vm/array.rs` only bind `Value::Int(n)`, so Float /
    // Sym / BigInt / etc. fall past the `(n)` arms to the
    // generic NoMethodError catch-all.
    //
    // This test is NOT a diff_cruby fixture because the
    // divergence would make the harness fail. The point is to
    // make the divergence VISIBLE in tree: a future contributor
    // who fixes Float coercion (or wires `to_int` more
    // generally) will see this test fail, get directed to
    // re-classify Array#first(n) / Array#last(n), and either
    // remove or update this test. Without it, the divergence
    // is invisible — there's no failing breadcrumb when
    // someone partially implements coercion in a way that
    // changes the behaviour here.
    //
    // The `take` / `drop` arms in the same file have the same
    // shape; widening to_int coercion across all Int-taking
    // Array methods would be a separable change.
    // RubyError + Runtime are already in scope from the file-level
    // `use rubyrs::{Config, HostCtx, Runtime, RubyError, Trap, Value};`
    // at the top — no extra import needed.

    fn assert_no_method(src: &str) {
        let mut rt = Runtime::new();
        let err = rt.eval(src, "non_int_n.rb")
            .expect_err("expected error");
        // `RubyError::is()` handles both the direct
        // NoMethodError variant and the Uncaught wrapper that
        // some dispatch paths route through (they both surface
        // as `NoMethodError` to the script).
        assert!(
            err.err.is("NoMethodError"),
            "expected NoMethodError for `{src}`, got {:?}",
            err.err,
        );
    }

    assert_no_method("[1,2,3].first(2.0)");
    assert_no_method("[1,2,3].last(2.0)");
    assert_no_method("[1,2,3].first(:x)");
    assert_no_method("[1,2,3].last(:x)");
    assert_no_method("[1,2,3].first('2')");
    assert_no_method("[1,2,3].last('2')");
}

#[test]
fn range_first_last_non_int_n_raises_no_method_error_today() {
    // Companion to `array_first_last_non_int_n_raises_no_method_error_today`.
    // Pin the current rubyrs divergence from CRuby on
    // `Range#first(n)` / `Range#last(n)` when `n` isn't an
    // `Int`.
    //
    // CRuby behaviour (2026-05):
    //   - `(1..5).first(2.0)` returns `[1, 2]` — Float's
    //     `to_int` coerces to 2.
    //   - `(1..5).last(:x)`   raises `TypeError: no implicit
    //     conversion of Symbol into Integer`.
    //
    // rubyrs behaviour: both raise NoMethodError because the
    // match arms in `vm/range.rs` only bind `Value::Int(n)`.
    // Float / Sym / String fall past the `(n)` arms to the
    // generic NoMethodError catch-all.
    //
    // This test mirrors the Array sibling rather than being a
    // diff_cruby fixture, for the same reason: a diff_cruby
    // fixture would fail the harness because CRuby's output
    // disagrees with rubyrs's. The embed test creates a
    // breadcrumb so a future contributor who wires `to_int`
    // coercion (or adds a Float / BigInt arm) gets a failing
    // test and is forced to re-classify Range#first(n) /
    // Range#last(n) intentionally.

    fn assert_no_method(src: &str) {
        let mut rt = Runtime::new();
        let err = rt.eval(src, "non_int_n.rb")
            .expect_err("expected error");
        assert!(
            err.err.is("NoMethodError"),
            "expected NoMethodError for `{src}`, got {:?}",
            err.err,
        );
    }

    assert_no_method("(1..5).first(2.0)");
    assert_no_method("(1..5).last(2.0)");
    assert_no_method("(1..5).first(:x)");
    assert_no_method("(1..5).last(:x)");
    assert_no_method("(1..5).first('2')");
    assert_no_method("(1..5).last('2')");
    // Endless range too — the endless first(n) arm also only
    // matches Value::Int(n).
    assert_no_method("(1..).first(2.0)");
}
