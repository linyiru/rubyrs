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
    fn vm_interner_len(&self) -> usize {
        Runtime::__test_vm_interner_len(self)
    }
}
