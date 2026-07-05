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
fn reset_clears_singleton_class_class_eval_installs() {
    // PR #253 layer #23: `cls.singleton_class.class_eval do
    // define_method(:x) { ... } end` redirects the install into
    // `cls.singleton_methods` via the eigenclass shell. `reset()`
    // drops the cached shell (sets `singleton_view = None` rather
    // than preserving the Rc — which would preserve the shell's
    // internal RefCells via the shared allocation, leaking
    // session-time state into the post-reset baseline). The
    // redirected method itself lands on `cls.singleton_methods`,
    // which `reset()` snapshots and restores — both halves of the
    // contract are exercised here. (Code-review #253 round 8 #2.)
    let mut rt = Runtime::new();
    // Use a preamble class — String survives reset (the class
    // itself is part of the baseline); user-defined classes would
    // be dropped wholesale, which doesn't isolate the
    // singleton_view-drop behavior we want to verify.
    rt.eval(
        r#"
        String.singleton_class.class_eval do
          define_method(:rubyrs_layer_23_marker) { "marker-present" }
        end
        "#,
        "install.rb",
    ).expect("install via singleton_class.class_eval");
    // Pre-reset: the redirected install dispatches via String's
    // singleton-method chain.
    let pre = rt.eval(
        "String.rubyrs_layer_23_marker",
        "use-pre.rb",
    ).expect("call shell-installed method");
    assert!(
        matches!(&pre, rubyrs::Value::Str(s) if &*s.borrow() == b"marker-present"),
        "expected marker-present before reset, got {:?}",
        pre,
    );
    rt.reset();
    // Post-reset: both the redirected method (via
    // singleton_methods snapshot/restore) AND the cached shell
    // (via singleton_view drop) are gone. Calling the method
    // raises NoMethodError; rebuilding via `singleton_class`
    // produces a fresh shell with a fresh identity.
    let err = rt.eval(
        "String.rubyrs_layer_23_marker",
        "use-post.rb",
    ).expect_err("singleton method gone");
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
    // Source-location query helper: returns `(filename, line)`
    // for `Exception#message`, or panics if the lookup doesn't
    // yield a `[Str(filename), Int(line)]` shape. Both elements
    // are checked — a regression that returns line 23 from a
    // *different* file (e.g., a `"".source_location`-style
    // shortcut that synthesises a path) would slip past a
    // line-only assertion.
    let source_location_of_message = |rt: &mut Runtime, tag: &'static str| -> (String, i64) {
        let v = rt
            .eval(
                "Exception.instance_method(:message).source_location",
                tag,
            )
            .unwrap_or_else(|e| panic!("{} eval failed: {:?}", tag, e));
        let arr = rt
            .resolve_array(&v)
            .unwrap_or_else(|| panic!("{} expected Array, got {:?}", tag, v));
        let filename = match arr.first() {
            // `to_string_lossy()` matches the convention used elsewhere
            // in tests/embed (e.g. misc.rs) — drops the manual
            // `from_utf8 + to_vec` round-trip and gracefully handles
            // a hypothetical non-UTF8 preamble filename.
            Some(rubyrs::Value::Str(s)) => s.to_string_lossy(),
            other => panic!("{} expected Str filename at [0], got {:?}", tag, other),
        };
        let line = match arr.get(1) {
            Some(rubyrs::Value::Int(n)) => *n,
            other => panic!("{} expected Int line at [1], got {:?}", tag, other),
        };
        (filename, line)
    };
    let before = source_location_of_message(&mut rt, "before.rb");
    assert!(
        before.0.starts_with("<rubyrs:preamble:"),
        "preamble source_location filename must start with `<rubyrs:preamble:` before reset, got {:?}",
        before.0,
    );
    assert!(
        before.1 > 0,
        "preamble source_location line must be > 0 before reset, got {}",
        before.1,
    );
    rt.reset();
    let after = source_location_of_message(&mut rt, "after.rb");
    assert_eq!(
        before, after,
        "preamble source_location (filename, line) must survive reset (pre-fix dropped to line 0)",
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
    // Loop count history: 2000 → 12_000 when the floor moved
    // `max 1024` → `max 4096` (json_bench round_trip sweep
    // tuning); 12_000 → 40_000 when the floor moved again to
    // `live*2 max 32768` (the require-rubocop collection-storm
    // fix) — the RETAINED live set must cross the floor so a
    // sweep fires and `live * growth` exceeds it, or `next_gc`
    // never budges from the post-preamble baseline and the
    // ratchet this test pins never engages.
    rt.eval(
        "arrs = []; i = 0; while i < 40000; arrs << [i]; i = i + 1; end",
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
fn reset_restores_adaptive_gc_floor_to_preamble_baseline() {
    // The GC floor (heap.rs `gc_floor` + `last_sweep_us`) adapts to
    // measured sweep cost during user evals. Like `next_gc` above, it
    // must rewind with reset() or a floor raised by one user eval's
    // expensive sweeps leaks a bigger RSS window into every later
    // eval. A REAL raise needs two consecutive >256µs sweeps — not
    // something a test can time deterministically — so perturb the
    // captured-and-restored state directly through the test hook.
    let mut rt = Runtime::new();
    let baseline = rt.vm_heap_gc_floor();
    rt.__test_vm_set_heap_gc_floor(baseline + 12_288, 9_999);
    assert_eq!(rt.vm_heap_gc_floor(), baseline + 12_288);
    rt.reset();
    assert_eq!(
        rt.vm_heap_gc_floor(), baseline,
        "reset must restore the adaptive GC floor to baseline {}",
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

// === Runtime::reset_between_requests ===
//
// Lightweight per-request reset specified by ADR 0022 v5.
// Differs from `reset()`: only clears per-request transient
// state (globals, control-flow signals, pinned, class_stack,
// last_match). Class definitions, heap, interner, loaded_features
// all PERSIST so a long-running server keeps its app loaded.

#[test]
fn reset_between_requests_clears_user_globals() {
    let mut rt = Runtime::new();
    rt.eval("$secret = 'abc123'", "set.rb").expect("set global");
    let before = rt.eval("$secret", "read.rb").expect("read before");
    assert!(
        matches!(&before, rubyrs::Value::Str(s) if s.to_string_lossy() == "abc123"),
        "$secret should be 'abc123' before reset, got {:?}",
        before,
    );
    rt.reset_between_requests();
    let after = rt.eval("$secret", "read_after.rb").expect("read after");
    assert!(
        matches!(after, rubyrs::Value::Nil),
        "$secret should be nil after reset_between_requests, got {:?}",
        after,
    );
}

#[test]
fn reset_between_requests_preserves_user_class_definitions() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        class Greeter
          def initialize(name)
            @name = name
          end
          def hello
            "Hello, #{@name}!"
          end
        end
        "#,
        "define.rb",
    )
    .expect("define class");

    rt.reset_between_requests();

    // Class survives — calling .new + .hello works.
    let result = rt
        .eval(r#"Greeter.new("World").hello"#, "call.rb")
        .expect("call class method after reset");
    assert!(
        matches!(&result, rubyrs::Value::Str(s) if s.to_string_lossy() == "Hello, World!"),
        "Greeter class should survive reset_between_requests; got {:?}",
        result,
    );
}

#[test]
fn reset_between_requests_preserves_constants() {
    let mut rt = Runtime::new();
    rt.eval("FOO = 99", "define_const.rb").expect("define const");
    rt.reset_between_requests();
    let v = rt.eval("FOO", "read_const.rb").expect("read const after reset");
    assert!(
        matches!(v, rubyrs::Value::Int(99)),
        "constants must persist across reset_between_requests; got {:?}",
        v,
    );
}

#[test]
fn reset_between_requests_does_not_truncate_heap() {
    let mut rt = Runtime::new();
    // Allocate some heap-bound state inside a class constant
    // so it stays rooted across the reset.
    rt.eval(
        r#"
        class Cache
          ENTRIES = (1..50).map { |i| "entry_#{i}" }
        end
        "#,
        "alloc.rb",
    )
    .expect("populate");
    let before = rt.vm_live_count();
    rt.reset_between_requests();
    // reset_between_requests doesn't truncate the heap. Some
    // heap objects (like the constant Array) stay reachable;
    // a future GC sweep might trim transient ones, but the
    // method itself doesn't touch the heap.
    let after = rt.vm_live_count();
    assert!(
        after >= before,
        "reset_between_requests must not shrink heap; before={before}, after={after}",
    );
}

#[test]
fn reset_between_requests_keeps_loaded_features() {
    let mut rt = Runtime::new();
    // Doesn't matter what we require — `require` returns
    // true on first load, false on subsequent loads (because
    // loaded_features dedups). Reset between MUST NOT clear
    // loaded_features, else require would re-execute every
    // dep per request (catastrophic perf hit for real apps).
    //
    // Use a known require name. The 4 vendored stdlib modules
    // (set / pathname / stringio / strscan) are guaranteed to
    // exist under the lenient stub path even with `stdlib`
    // feature off (they short-circuit before file lookup).
    rt.eval("require \"pathname\"", "first.rb").expect("first require");
    rt.reset_between_requests();
    let v = rt
        .eval("require \"pathname\"", "second.rb")
        .expect("second require");
    // require returns false when already loaded
    assert!(
        matches!(v, rubyrs::Value::Bool(false)),
        "loaded_features must persist across reset_between_requests; \
         second require should return false but got {:?}",
        v,
    );
}

#[test]
fn reset_between_requests_lets_post_reset_eval_work() {
    let mut rt = Runtime::new();
    rt.eval("$x = 1", "set.rb").expect("set");
    rt.reset_between_requests();
    // After reset, $x is nil (global cleared) — but we can
    // set it again and have arithmetic work as normal.
    let v = rt.eval("$x = 10; $x + 5", "post.rb").expect("post-reset eval");
    assert!(matches!(v, rubyrs::Value::Int(15)), "got {:?}", v);
}

#[test]
fn reset_between_requests_idempotent() {
    let mut rt = Runtime::new();
    rt.eval("$g = 42", "set.rb").expect("set");
    rt.reset_between_requests();
    rt.reset_between_requests();
    rt.reset_between_requests();
    let v = rt.eval("$g", "read.rb").expect("read");
    assert!(matches!(v, rubyrs::Value::Nil), "got {:?}", v);
}

#[test]
fn reset_between_requests_clears_method_added_to_preamble_class() {
    // Adding a method to a preamble class (e.g. `class String;
    // def secret; ...; end; end`) IS NOT cleared by
    // reset_between_requests — it stays in the class's method
    // table until full `reset()`. This is intentional: a Rack
    // app that monkey-patches String at boot expects the patch
    // to survive across requests.
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        class String
          def screaming
            self.upcase + "!"
          end
        end
        "#,
        "patch.rb",
    )
    .expect("patch String");
    rt.reset_between_requests();
    // Patch survives.
    let v = rt
        .eval(r#""hello".screaming"#, "use.rb")
        .expect("call patched method");
    assert!(
        matches!(&v, rubyrs::Value::Str(s) if s.to_string_lossy() == "HELLO!"),
        "monkey-patched String method must persist across reset_between_requests; got {:?}",
        v,
    );
}

// === Runtime::refill_fuel ===
//
// Per-request fuel re-anchor (ADR 0022 v5). 3 cases:
//   (a) Some(n) -> vm.fuel = Some(n)
//   (b) None + Config::fuel = Some(c) -> vm.fuel = Some(c)
//   (c) None + Config::fuel = None -> vm.fuel = None
//
// Plus the integration use case: a tight loop that exhausts
// fuel ONCE, then refill, runs again without ResourceExhausted.

#[test]
fn refill_fuel_some_sets_to_provided_value() {
    let mut rt = Runtime::with_config(Config { fuel: Some(1_000), ..Default::default() });
    // After construction, fuel_budget = 1000 but vm.fuel
    // hasn't run an eval yet. Refill with explicit Some(500):
    rt.refill_fuel(Some(500));
    // Now eval; if vm.fuel was 500, a tight 1000-iter loop
    // should trap with ResourceExhausted.
    let result = rt.eval("1000.times { 1 + 1 }", "loop.rb");
    assert!(
        matches!(&result, Err(t) if matches!(t.err, RubyError::ResourceExhausted { .. })),
        "expected ResourceExhausted with 500-fuel budget, got {:?}",
        result.as_ref().map_err(|e| &e.err),
    );
}

#[test]
fn refill_fuel_none_with_some_config_uses_config_value() {
    let mut rt = Runtime::with_config(Config { fuel: Some(10_000_000), ..Default::default() });
    // Burn most of the fuel
    rt.eval("100_000.times { 1 + 1 }", "burn.rb").expect("burn ok");
    // Now refill with None — should re-anchor to Config 10M.
    rt.refill_fuel(None);
    // Same 100k-iter loop should fit again (vm.fuel back to 10M).
    let v = rt.eval("100_000.times { 1 + 1 }", "refilled.rb")
        .expect("post-refill eval should fit in re-anchored budget");
    let _ = v; // value not asserted; just verifying no trap
}

#[test]
fn refill_fuel_none_with_none_config_is_unbounded() {
    let mut rt = Runtime::with_config(Config { fuel: None, ..Default::default() });
    rt.refill_fuel(None);
    // Unbounded — 100k iters fine.
    rt.eval("100_000.times { 1 + 1 }", "unbounded.rb")
        .expect("unbounded eval");
}

#[test]
fn refill_fuel_does_not_clamp_against_config() {
    // Per ADR 0022 v5: per-request budget can be LARGER than
    // Config::fuel. Embedders may use a smaller config (for
    // long-running lifetime cap when not http_server) but a
    // larger per-request cap. No clamping.
    let mut rt = Runtime::with_config(Config { fuel: Some(100), ..Default::default() });
    rt.refill_fuel(Some(10_000_000));
    // 100k loop should fit fine with the larger per-request budget.
    rt.eval("100_000.times { 1 + 1 }", "larger_per_request.rb")
        .expect("per-request budget should NOT be clamped against Config::fuel");
}

#[test]
fn refill_fuel_then_reset_between_then_refill_simulates_server_loop() {
    // Simulates the per-request shape the _http_server
    // battery will follow: reset, refill, eval, repeat.
    let mut rt = Runtime::with_config(Config { fuel: Some(100_000), ..Default::default() });
    for i in 0..5 {
        rt.reset_between_requests();
        rt.refill_fuel(Some(50_000));
        // Each "request" runs a tight loop that fits in
        // 50k fuel. Globals from one request don't leak to
        // the next (reset_between_requests clears them).
        rt.eval(
            &format!("$req = {i}; 5_000.times {{ $req }}"),
            "per_request.rb",
        )
        .unwrap_or_else(|e| panic!("iter {i} should fit in 50k fuel, got {:?}", e.err));
    }
}

#[test]
fn refill_fuel_lets_runaway_trap_then_recover() {
    // Models the "long-running server: one runaway request
    // traps, worker survives" scenario. After
    // ResourceExhausted, the next eval-with-refill should
    // succeed.
    let mut rt = Runtime::with_config(Config { fuel: Some(10_000_000), ..Default::default() });
    rt.refill_fuel(Some(1_000)); // tight per-request budget
    let trap = rt.eval("1_000_000.times { 1 + 1 }", "runaway.rb")
        .expect_err("runaway loop should trap");
    assert!(
        matches!(trap.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}",
        trap.err,
    );
    // Recovery: reset + refill with the lifetime budget.
    rt.reset_between_requests();
    rt.refill_fuel(None); // re-anchor to Config 10M
    let v = rt.eval("1 + 2", "post_runaway.rb")
        .expect("post-runaway eval should succeed with re-anchored fuel");
    assert!(matches!(v, rubyrs::Value::Int(3)));
}

/// `reset()` must rewind the generational GC's slot-indexed
/// bookkeeping (`young_slots` / `remembered` / `old` /
/// `minors_since_major`) along with the heap vectors themselves.
///
/// The nightly fuzz harness (one cached Runtime, `reset()` between
/// inputs — see crates/rubyrs/fuzz/src/lib.rs) caught the miss:
/// `young_slots` entries above the post-preamble high-water mark
/// survived reset, and the next MINOR collection's mark-reset
/// (`marks[yi] = false` in heap.rs) indexed the truncated `marks`
/// vector out of bounds — "index out of bounds: the len is 4091
/// but the index is 4095", red on every scheduled fuzz run
/// 2026-06-30 through 2026-07-04.
///
/// `stress_gc` forces a collection at every allocation, so the
/// first post-reset alloc walks the young list immediately; the
/// two grow/reset rounds cover the minor/major cadence wherever
/// the previous eval left it (a stale-counter major on round one
/// would otherwise mask the stale-young-list minor on round two).
#[test]
fn reset_rewinds_generational_gc_young_state() {
    let mut rt = Runtime::with_config(Config { stress_gc: true, ..Default::default() });
    for round in 0..3 {
        // Grow the heap well past the preamble high-water mark;
        // under stress-GC the tail allocations leave high slot
        // indices in `young_slots`.
        rt.eval(
            "arrs = []; i = 0; while i < 300; arrs << [i, i + 1]; i = i + 1; end",
            "grow.rb",
        )
        .unwrap_or_else(|e| panic!("grow eval (round {round}) must succeed: {:?}", e.err));
        rt.reset();
        // Fresh eval allocates + collects immediately. With stale
        // young entries this panicked before the fix.
        rt.eval(
            "xs = []; j = 0; while j < 50; xs << [j]; j = j + 1; end",
            "post_reset.rb",
        )
        .unwrap_or_else(|e| panic!("post-reset eval (round {round}) must succeed: {:?}", e.err));
        rt.reset();
    }
}

/// Harness-shape soak: ONE cached Runtime under the fuzz `parse`
/// target's tight caps, `reset()` between inputs, every repo
/// fixture (`tests/diff` + `tests/fixtures`, recursively) as the
/// input sequence — exactly the seed corpus the nightly fuzz
/// workflow feeds through `rubyrs_fuzz::run`. Scripts trapping
/// (fuel / heap / deadline exhaustion, raises) is expected and
/// ignored; the failure mode this pins is a host-side Rust panic
/// from cross-input state the reset failed to rewind (the
/// 2026-06-30..07-04 nightly-fuzz reds: stale generational-GC
/// young slots, free-list-recycled zombie objects, dangling
/// `main_obj`).
///
/// Runs the corpus in sorted order for determinism. Kept as a
/// regular (non-#[ignore]) test: one pass over ~1k fixtures under
/// a 50k-op fuel cap is seconds, and it's the only in-repo
/// coverage of the cached-Runtime + reset() usage pattern the
/// fuzz harness and per-request embedders rely on.
#[test]
fn reset_survives_fixture_corpus_soak_under_tight_caps() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rb") {
                out.push(p);
            }
        }
    }
    // Caps::tight() from crates/rubyrs/fuzz/src/lib.rs.
    let mut rt = Runtime::with_config(Config {
        fuel: Some(50_000),
        max_frames: Some(64),
        max_heap_objects: Some(1024),
        max_value_bytes: Some(1 << 16),
        max_symbols: Some(1 << 14),
        deadline: Some(std::time::Duration::from_millis(500)),
        // Honour the repo's STRESS_GC=1 rerun convention: the
        // stressed pass is the one that surfaces reset()-missed
        // dangling heap references deterministically (a collection
        // fires on every allocation, so a stale root can't hide
        // behind GC timing).
        stress_gc: std::env::var_os("STRESS_GC").is_some_and(|v| v == "1"),
        ..Default::default()
    });
    let mut files = Vec::new();
    walk(std::path::Path::new("tests/diff"), &mut files);
    walk(std::path::Path::new("tests/fixtures"), &mut files);
    files.sort();
    assert!(
        files.len() > 100,
        "fixture corpus went missing? found {} .rb files",
        files.len(),
    );
    for f in &files {
        let src = std::fs::read_to_string(f).expect("fixture readable");
        rt.reset();
        // Traps are correct VM behaviour under the tight caps;
        // only a Rust panic (which aborts the test) is a failure.
        let _ = rt.eval(&src, "fuzz.rb");
    }
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
    fn vm_heap_gc_floor(&self) -> usize;
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
    fn vm_heap_gc_floor(&self) -> usize {
        Runtime::__test_vm_heap_gc_floor(self)
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
