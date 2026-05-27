//! Method-dispatch quirks — `alias_method`, `method_missing`,
//! `define_method`, and singleton-class closure semantics.
//! Each is a corner of Ruby's method resolution that the
//! straight superclass / mixin chain doesn't cover:
//!
//!   - `alias_method` — copies the method entry under a new
//!     name. Inherited-method case + super-lookup-chain
//!     preservation are the load-bearing assertions.
//!   - `method_missing` — fallback when normal lookup fails.
//!     Inheritance walk through the chain, and the
//!     "without method_missing, still raises NoMethodError"
//!     contract.
//!   - `define_method` — installs methods at runtime via
//!     block. Closure over the defining scope + arity
//!     validation against the block.
//!   - Singleton class closures — `class << obj; end` blocks
//!     must not create reference cycles that leak the
//!     enclosing object.

use rubyrs::{Config, Runtime};

use super::rt_with_buf;

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

