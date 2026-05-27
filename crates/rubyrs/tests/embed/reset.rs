//! `Runtime::reset` contract tests.
//!
//! `reset()` rewinds a Runtime to its post-preamble baseline:
//! everything the preamble installed (Exception hierarchy, Object,
//! Comparable, String/Integer/... method tables, host fns, compiled
//! protos) survives; everything a user `eval()` introduced (globals,
//! constants, user classes, user methods, heap allocations, user-
//! interned symbols, control-flow signal flags) is wiped.
//!
//! These tests pin both halves of that contract:
//!
//! - "user state is gone after reset" — globals, constants, classes,
//!   methods-on-preamble-classes, heap live_count, interner length.
//! - "preamble state survives reset" — Exception class still
//!   resolvable, primitive method dispatch still works.
//! - "post-reset Runtime is healthy" — eval succeeds, resource caps
//!   still in force, control-flow signals from a prior trapped eval
//!   don't leak.

use rubyrs::{Config, RubyError, Runtime};

#[test]
fn reset_clears_user_globals() {
    let mut rt = Runtime::new();
    rt.eval("$g = 42", "set.rb").expect("set $g");
    // Before reset: $g visible.
    let before = rt.eval("$g", "read.rb").expect("read $g");
    assert!(
        matches!(before, rubyrs::Value::Int(42)),
        "expected Int(42) before reset, got {:?}",
        before,
    );
    rt.reset();
    // After reset: $g is nil (default for unset globals).
    let after = rt.eval("$g.nil?", "post.rb").expect("read $g post-reset");
    assert!(
        matches!(after, rubyrs::Value::Bool(true)),
        "expected $g to be nil post-reset, got {:?}",
        after,
    );
}

#[test]
fn reset_clears_user_constants() {
    let mut rt = Runtime::new();
    rt.eval("FOO = 99", "set.rb").expect("set FOO");
    // After reset, referencing FOO raises NameError. The error
    // is delivered as a Ruby-side `Uncaught { class_name:
    // "NameError" }` (the constant-lookup site raises an
    // Exception subclass) rather than the Rust-side
    // `RubyError::NameError` variant — match the user-observable
    // shape.
    rt.reset();
    let err = rt.eval("FOO", "read.rb").expect_err("FOO must be undefined");
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "NameError"),
        "expected NameError-shape Uncaught post-reset, got {:?}",
        err.err,
    );
}

#[test]
fn reset_clears_user_classes() {
    let mut rt = Runtime::new();
    rt.eval(
        "class Greeter; def hi; 'hello'; end; end",
        "define.rb",
    ).expect("define class");
    // Pre-reset: Greeter is callable.
    rt.eval("Greeter.new.hi", "use.rb").expect("call Greeter#hi");
    rt.reset();
    // Post-reset: referencing Greeter raises a `NameError`-shape
    // Uncaught (same form as the user-constant test above).
    let err = rt.eval("Greeter", "post.rb").expect_err("Greeter undefined");
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "NameError"),
        "expected NameError-shape Uncaught post-reset, got {:?}",
        err.err,
    );
}

#[test]
fn reset_clears_user_methods_added_to_preamble_class() {
    let mut rt = Runtime::new();
    // Add a method to the preamble's String class.
    rt.eval(
        "class String; def shout; self + '!'; end; end",
        "augment.rb",
    ).expect("define String#shout");
    // Pre-reset: shout works.
    let pre = rt.eval(r#""hi".shout"#, "use.rb").expect("call shout");
    assert!(
        matches!(&pre, rubyrs::Value::Str(s) if &*s.borrow() == b"hi!"),
        "expected 'hi!' before reset, got {:?}",
        pre,
    );
    rt.reset();
    // Post-reset: shout is gone — `NoMethodError`-shape Uncaught.
    // String the class still exists (preamble), but the method
    // table has been pruned back to its preamble baseline.
    let err = rt.eval(r#""hi".shout"#, "post.rb").expect_err("shout gone");
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "NoMethodError"),
        "expected NoMethodError-shape Uncaught post-reset, got {:?}",
        err.err,
    );
}

#[test]
fn reset_preserves_preamble_source_locations() {
    // `Method#source_location` on preamble-defined methods
    // (e.g. `Exception#message`, defined in
    // `<rubyrs:preamble:exceptions>`) must keep resolving to its
    // real `[filename, line]` after `reset()`. Pre-fix, reset()
    // called `vm.sources.clear()`, dropping every preamble
    // filename→source-text entry; source_location then fell
    // back to line 0 (dispatch.rs:1036), giving fuzz / per-
    // request hosts increasingly degraded backtraces from the
    // first reset onward.
    let mut rt = Runtime::new();
    // Source-location query helper: returns the line number for
    // `Exception#message`, or panics if the lookup doesn't yield
    // a `[filename, Int(line)]` shape.
    let line_of_message = |rt: &mut Runtime, tag: &'static str| -> i64 {
        let v = rt
            .eval(
                "Exception.instance_method(:message).source_location",
                tag,
            )
            .unwrap_or_else(|e| panic!("{} eval failed: {:?}", tag, e));
        let arr = rt
            .resolve_array(&v)
            .unwrap_or_else(|| panic!("{} expected Array, got {:?}", tag, v));
        match arr.get(1) {
            Some(rubyrs::Value::Int(n)) => *n,
            other => panic!("{} expected Array with Int line at [1], got {:?}", tag, other),
        }
    };
    let before = line_of_message(&mut rt, "before.rb");
    assert!(
        before > 0,
        "preamble source_location line must be > 0 before reset, got {}",
        before,
    );
    rt.reset();
    let after = line_of_message(&mut rt, "after.rb");
    assert_eq!(
        before, after,
        "preamble source_location line must survive reset (pre-fix dropped to 0)",
    );
}

#[test]
fn eval_after_reset_gets_fresh_fuel_budget() {
    // Specific eval-reset-eval shape of the broader "fuel is
    // per-eval" contract — companion to
    // `resource_caps::fuel_resets_between_eval_calls` which
    // exercises eval-eval (no reset between). Together they pin
    // that fuel refills regardless of whether reset is called.
    //
    // Mechanism (post-PR #222 + per-eval-fuel-budget refactor):
    // `Runtime::eval` at entry re-anchors `vm.fuel` from
    // `Runtime::fuel_budget` (set by `apply_config`). `reset()`
    // doesn't touch `vm.fuel` at all; the next eval refills it.
    //
    // Pre-PR-#222 `vm.fuel` decremented monotonically across the
    // Runtime's lifetime and `reset()` didn't touch it, so the
    // second eval here would trap with "out of fuel".
    let cfg = Config { fuel: Some(10_000), ..Default::default() };
    let mut rt = Runtime::with_config(cfg);
    // First eval: ~3k ops, comfortably within the 10k budget.
    let v1 = rt
        .eval(
            "a = []; i = 0; while i < 500; a << i; i = i + 1; end; a.length",
            "first.rb",
        )
        .expect("first eval under budget");
    assert!(matches!(v1, rubyrs::Value::Int(500)));
    rt.reset();
    // Second eval, same script. Re-anchoring at eval entry
    // restores the full 10k budget regardless of how much the
    // first eval consumed.
    let v2 = rt
        .eval(
            "a = []; i = 0; while i < 500; a << i; i = i + 1; end; a.length",
            "second.rb",
        )
        .expect("second eval gets a fresh fuel budget");
    assert!(matches!(v2, rubyrs::Value::Int(500)));
}

#[test]
fn reset_undoes_redefinition_of_preamble_method() {
    // Override a method the PREAMBLE Ruby code defined (not a
    // primitive fast-path one — those bypass user override
    // entirely, see PR #156's documented divergence). The
    // preamble's `mutex.rb` defines `Mutex#synchronize` in Ruby,
    // so it's truly user-overridable.
    //
    // A key-only snapshot (the shape this PR's first commit
    // used) would have left this override in place: `synchronize`
    // is in the preamble's Mutex method-keyset, so a
    // `retain(|m| ...)` doesn't remove it. The value-restore
    // pattern that landed in the C1/C3/C6 fix clones the
    // preamble's original Method back into place, so the
    // override is gone after reset.
    let mut rt = Runtime::new();
    rt.eval(
        "class Mutex; def synchronize; 'OVERRIDDEN'; end; end",
        "override.rb",
    ).expect("override Mutex#synchronize");
    let pre = rt
        .eval("Mutex.new.synchronize { 'block' }", "use.rb")
        .expect("call override");
    assert!(
        matches!(&pre, rubyrs::Value::Str(s) if &*s.borrow() == b"OVERRIDDEN"),
        "expected user override before reset, got {:?}",
        pre,
    );
    rt.reset();
    // Post-reset: preamble's original `synchronize` is back.
    // The preamble's implementation yields to the block and
    // returns the block's value — `'block'` here.
    let post = rt
        .eval("Mutex.new.synchronize { 'block' }", "post.rb")
        .expect("call restored synchronize");
    assert!(
        matches!(&post, rubyrs::Value::Str(s) if &*s.borrow() == b"block"),
        "expected preamble synchronize (yields 'block') post-reset, got {:?}",
        post,
    );
}

#[test]
fn reset_restores_heap_next_gc_to_preamble_baseline() {
    // GC's resize heuristic in heap.rs ratchets `next_gc`
    // upward whenever live_count approaches it. Without
    // snapshotting next_gc, a long-lived Runtime + many user
    // evals drives next_gc to large values, so post-reset
    // (where live_count is restored to baseline) GC stops
    // firing at the expected threshold. Pin the contract:
    // next_gc returns to the post-preamble value after reset.
    let mut rt = Runtime::new();
    let baseline = rt.vm_heap_next_gc();
    // Heavy allocation pushes next_gc up via GC's resize.
    rt.eval(
        "arrs = []; i = 0; while i < 2000; arrs << [i]; i = i + 1; end",
        "alloc.rb",
    ).expect("alloc heavy");
    assert!(
        rt.vm_heap_next_gc() > baseline,
        "heavy alloc should have ratcheted next_gc above baseline {}; got {}",
        baseline, rt.vm_heap_next_gc(),
    );
    rt.reset();
    assert_eq!(
        rt.vm_heap_next_gc(), baseline,
        "reset must restore next_gc to baseline {}",
        baseline,
    );
}

#[test]
fn reset_truncates_protos_to_preamble_baseline() {
    // Without this, every user `eval()` appends compiled
    // bytecode to `vm.protos` and the Vec grows monotonically
    // across resets. Pin: protos.len() returns to the
    // post-preamble baseline after reset.
    let mut rt = Runtime::new();
    let baseline = rt.vm_protos_len();
    // Each `eval` compiles new protos (every `def` is one,
    // every block another). A handful of methods adds enough
    // entries to make the check meaningful.
    rt.eval(
        "def a; 1; end; def b; 2; end; def c; 3; end; [1,2,3].each { |x| x * 2 }",
        "compile.rb",
    ).expect("compile bytecode");
    assert!(
        rt.vm_protos_len() > baseline,
        "user eval should have grown protos past baseline {}; got {}",
        baseline, rt.vm_protos_len(),
    );
    rt.reset();
    assert_eq!(
        rt.vm_protos_len(), baseline,
        "reset must truncate protos to baseline {}",
        baseline,
    );
    // And running multiple cycles must keep protos bounded —
    // a fuzz harness shape.
    for _ in 0..20 {
        rt.eval("def x; 1; end", "tmp.rb").expect("compile");
        rt.reset();
    }
    assert_eq!(
        rt.vm_protos_len(), baseline,
        "after 20 cycles, protos must still be at baseline",
    );
}

#[test]
fn reset_keeps_method_gen_bounded_across_cycles() {
    // Pre-fix: `reset()` did `method_gen.wrapping_add(1)`,
    // growing the counter unbounded; at ~10k resets/sec the
    // u32 wraps in ~5 days. Post-fix: reset clamps the counter
    // at `snapshot.method_gen + 1`, so 100 resets all land at
    // the same value.
    let mut rt = Runtime::new();
    let baseline = rt.vm_method_gen();
    // First reset: post-construction state means method_gen
    // should already be at snapshot.method_gen (no user defs
    // bumped it). Reset bumps to baseline+1.
    rt.reset();
    let after_first = rt.vm_method_gen();
    assert_eq!(after_first, baseline.wrapping_add(1));
    // Many resets must NOT keep advancing — the counter is
    // capped at baseline+1 every cycle.
    for _ in 0..100 {
        rt.reset();
    }
    assert_eq!(
        rt.vm_method_gen(),
        baseline.wrapping_add(1),
        "method_gen must stay clamped at baseline+1 across resets",
    );
}

#[test]
fn reset_clears_user_singleton_method_on_preamble_class() {
    // `def self.foo` inside a class body installs into the
    // class's `singleton_methods` RefCell, not `methods`. A
    // snapshot that only restored `methods` would leak this
    // singleton across resets. Pins the expanded
    // `ClassStateSnapshot` (lib.rs) actually restores
    // `singleton_methods` too.
    let mut rt = Runtime::new();
    rt.eval(
        "class Mutex; def self.echo; 'leaked'; end; end",
        "augment.rb",
    ).expect("define singleton");
    // Pre-reset: singleton callable.
    let pre = rt.eval("Mutex.echo", "use.rb").expect("call singleton");
    assert!(
        matches!(&pre, rubyrs::Value::Str(s) if &*s.borrow() == b"leaked"),
        "expected 'leaked' before reset, got {:?}", pre,
    );
    rt.reset();
    // Post-reset: singleton gone — NoMethodError on Mutex
    // because `echo` was never a preamble Mutex singleton.
    let err = rt.eval("Mutex.echo", "post.rb").expect_err("singleton gone");
    assert!(
        matches!(&err.err, RubyError::Uncaught { class_name, .. } if class_name == "NoMethodError"),
        "expected NoMethodError post-reset, got {:?}", err.err,
    );
}

#[test]
fn reset_clears_user_class_ivar_on_preamble_class() {
    // `@foo = bar` in a class body sets the class-instance ivar
    // via the `ivars` RefCell, not the regular instance heap.
    // Same reset-leak concern as singleton_methods.
    let mut rt = Runtime::new();
    rt.eval(
        "class Mutex; @stash = 'leaked'; end",
        "store.rb",
    ).expect("set class ivar");
    // Pre-reset: ivar readable via `instance_variable_get`.
    let pre = rt
        .eval("Mutex.instance_variable_get(:@stash)", "read.rb")
        .expect("read class ivar pre-reset");
    assert!(
        matches!(&pre, rubyrs::Value::Str(s) if &*s.borrow() == b"leaked"),
        "expected 'leaked' before reset, got {:?}", pre,
    );
    rt.reset();
    // Post-reset: ivar wiped — `instance_variable_get` returns
    // nil for an unset ivar (matching CRuby semantics).
    let post = rt
        .eval("Mutex.instance_variable_get(:@stash)", "post.rb")
        .expect("read class ivar post-reset");
    assert!(
        matches!(post, rubyrs::Value::Nil),
        "expected nil (cleared ivar) post-reset, got {:?}", post,
    );
}

// Note: a `reset_undoes_redefinition_of_preamble_constant` test
// was attempted but rubyrs's current constant-assignment
// semantics make it untestable today — `Exception = 1` evaluates
// without actually updating the constant in `vm.constants`
// (raises a warning instead, similar to CRuby's
// "already initialized constant" path; the read post-assignment
// still returns the Class). When that gap is closed, an analogous
// test using `Exception = 1` belongs here. The value-restore
// in `reset()` already handles the case correctly — the snapshot
// stores the original Value, so when const reassignment starts
// working, reset will rewind it.

#[test]
fn reset_preserves_preamble_classes() {
    let mut rt = Runtime::new();
    // Populate user state to ensure reset actually does work.
    rt.eval("$g = 1; class Foo; end; FOO = 1", "populate.rb").expect("populate");
    rt.reset();
    // Preamble Exception hierarchy must still be resolvable.
    rt.eval(
        "raise StandardError, 'boom' rescue StandardError",
        "exc.rb",
    ).expect("Exception class survived");
    // Preamble Comparable + primitive ops still work.
    rt.eval("5 <=> 3", "cmp.rb").expect("Comparable survived");
    // Preamble String methods still dispatch.
    let len = rt.eval(r#""abc".length"#, "str.rb").expect("String#length");
    assert!(
        matches!(len, rubyrs::Value::Int(3)),
        "expected 3 from preamble String#length, got {:?}",
        len,
    );
}

#[test]
fn reset_preserves_resource_caps() {
    let cfg = Config { fuel: Some(50_000), ..Default::default() };
    let mut rt = Runtime::with_config(cfg);
    // Burn some ops, then reset, then prove the cap still bites.
    rt.eval("1 + 1", "warm.rb").expect("trivial eval works");
    rt.reset();
    let err = rt
        .eval(
            "i = 0; while i < 1_000_000; i = i + 1; end",
            "spin.rb",
        )
        .expect_err("cap should still trap post-reset");
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted post-reset, got {:?}",
        err.err,
    );
}

#[test]
fn reset_clears_heap_after_user_allocation() {
    let mut rt = Runtime::new();
    let baseline = rt.vm_live_count();
    // Allocate user heap objects via a plain `while` loop so the
    // test doesn't depend on whether `Integer#times` is in the
    // supported subset. Each iteration adds an Array.
    rt.eval(
        "arrs = []; i = 0; while i < 50; arrs << [i, i+1, i+2]; i = i + 1; end",
        "alloc.rb",
    ).expect("populate heap");
    let post_alloc = rt.vm_live_count();
    assert!(
        post_alloc > baseline,
        "expected heap to grow past preamble baseline ({}), got {}",
        baseline, post_alloc,
    );
    rt.reset();
    let post_reset = rt.vm_live_count();
    assert_eq!(
        post_reset, baseline,
        "expected heap live_count to match preamble baseline after reset",
    );
}

#[test]
fn reset_after_trapped_eval_recovers() {
    let mut rt = Runtime::new();
    // Cause a trap mid-execution to leave control-flow signals
    // potentially in a dirty state.
    let _trap = rt
        .eval("raise 'boom'", "fail.rb")
        .expect_err("raise must trap");
    rt.reset();
    // Next eval must succeed cleanly — break/return signals,
    // frames stack, value stack all cleared.
    let v = rt.eval("1 + 2", "ok.rb").expect("post-reset eval works");
    assert!(
        matches!(v, rubyrs::Value::Int(3)),
        "expected 3, got {:?}",
        v,
    );
}

// (`reset_clears_loaded_features` would have asserted that the Vm's
// `loaded_features` HashSet is emptied by reset(), but it's an
// internal field rather than a script-visible `$LOADED_FEATURES`
// global. The behaviour is exercised indirectly by the other reset
// tests — every fresh-Runtime test starts with an empty
// loaded_features set, and reset puts the Runtime back to that
// shape.)

#[test]
fn reset_idempotent_when_called_twice() {
    let mut rt = Runtime::new();
    rt.eval("$g = 42; class Foo; end", "populate.rb").expect("populate");
    rt.reset();
    rt.reset();
    rt.eval("1 + 1", "post.rb").expect("Runtime still works");
}

#[test]
fn reset_clears_user_interned_symbols() {
    let mut rt = Runtime::new();
    // Force user-side symbol interning. `String#to_sym` runs at
    // eval time and grows the interner past its post-preamble
    // high-water.
    rt.eval(
        "100.times { |i| (\"sym_\" + i.to_s).to_sym }",
        "intern.rb",
    ).expect("intern user symbols");
    let before = rt.vm_interner_len();
    rt.reset();
    let after = rt.vm_interner_len();
    assert!(
        after < before,
        "expected interner to shrink post-reset (before={}, after={})",
        before, after,
    );
}

// --- helpers ---
//
// These reach into private Vm state through the test-only Runtime
// methods added to `lib.rs`'s test surface. Kept here rather than
// in `embed.rs` because no other test needs them yet.

trait RuntimeInternals {
    fn vm_live_count(&self) -> usize;
    fn vm_interner_len(&self) -> usize;
    fn vm_heap_next_gc(&self) -> usize;
    fn vm_method_gen(&self) -> u32;
    fn vm_protos_len(&self) -> usize;
}

// Note: these accessors are `pub fn` on `Runtime` with the
// `__test_` prefix + `#[doc(hidden)]`. They aren't gated behind
// `#[cfg(test)]` because Cargo's `cfg(test)` doesn't reach the
// lib when integration tests build against it as a normal
// dependency — see the doc-comment on the impls in `lib.rs` for
// the full rationale and why a real `test-internals` Cargo
// feature would cost more in build/CI plumbing than the de-facto
// `__test_` + `#[doc(hidden)]` convention does.
impl RuntimeInternals for Runtime {
    fn vm_live_count(&self) -> usize {
        Runtime::__test_vm_live_count(self)
    }
    fn vm_heap_next_gc(&self) -> usize {
        Runtime::__test_vm_heap_next_gc(self)
    }
    fn vm_method_gen(&self) -> u32 {
        Runtime::__test_vm_method_gen(self)
    }
    fn vm_protos_len(&self) -> usize {
        Runtime::__test_vm_protos_len(self)
    }
    fn vm_interner_len(&self) -> usize {
        Runtime::__test_vm_interner_len(self)
    }
}
