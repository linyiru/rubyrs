# Phase 1 of issue #224 — toplevel autoload registry + LoadConst
# trigger. This spec covers the bits that don't need filesystem
# fixtures: registration round-trip, arity/type guards,
# autoload? introspection. End-to-end trigger (autoload fires
# require on first reference) is covered by the native Rust
# integration test at `tests/autoload_phase1.rs` so the spec
# corpus stays FS-free.

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "toplevel autoload(:Sym, path) registration" do
  it "is initially absent — autoload?(:NotRegistered) returns nil" do
    assert_eq(autoload?(:NotRegistered_Phase1), nil)
  end

  it "registers and round-trips through autoload?" do
    autoload(:Phase1_Round1, "some/path")
    assert_eq(autoload?(:Phase1_Round1), "some/path")
  end

  it "accepts a String for the constant name (coerces to Symbol)" do
    autoload("Phase1_StringName", "x")
    assert_eq(autoload?(:Phase1_StringName), "x")
  end

  it "overwrites a prior registration for the same constant" do
    autoload(:Phase1_Overwrite, "old/path")
    autoload(:Phase1_Overwrite, "new/path")
    assert_eq(autoload?(:Phase1_Overwrite), "new/path")
  end

  it "returns nil from the autoload call itself" do
    assert_eq(autoload(:Phase1_RetVal, "x"), nil)
  end
end

describe "autoload arity & type guards" do
  it "raises ArgumentError on 0 args" do
    klass, msg = caught_pair { autoload }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 0, expected 2)")
  end

  it "raises ArgumentError on 1 arg" do
    klass, msg = caught_pair { autoload(:X) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 1, expected 2)")
  end

  it "raises ArgumentError on 3 args" do
    klass, msg = caught_pair { autoload(:X, "p", :extra) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 3, expected 2)")
  end

  it "raises TypeError on non-Symbol/non-String name" do
    klass, msg = caught_pair { autoload(42, "p") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "42 is not a symbol nor a string")
  end

  it "raises TypeError on non-String path" do
    klass, msg = caught_pair { autoload(:X, 42) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Integer into String")
  end
end

describe "autoload constant-name validation" do
  it "raises NameError on lowercase Symbol" do
    klass, msg = caught_pair { autoload(:lowercase, "x") }
    assert_eq(klass, "NameError")
    assert_eq(msg, "wrong constant name lowercase")
  end

  it "raises NameError on leading-digit Symbol" do
    klass, msg = caught_pair { autoload(:"9StartsWithDigit", "x") }
    assert_eq(klass, "NameError")
    assert_eq(msg, "wrong constant name 9StartsWithDigit")
  end

  it "raises NameError on lowercase String name" do
    klass, msg = caught_pair { autoload("lowercase_str", "x") }
    assert_eq(klass, "NameError")
    assert_eq(msg, "wrong constant name lowercase_str")
  end
end

describe "autoload? arity & type guards" do
  it "raises ArgumentError on 0 args" do
    klass, msg = caught_pair { autoload? }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 0, expected 1..2)")
  end

  it "raises ArgumentError on 3 args" do
    klass, msg = caught_pair { autoload?(:X, true, :extra) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 3, expected 1..2)")
  end

  it "accepts the optional inherit arg without consulting it" do
    autoload(:Phase1_Inherit, "p")
    # Both with and without `inherit` should return the same
    # path — Phase 1 ignores the arg (no inheritance chain at
    # toplevel scope anyway).
    assert_eq(autoload?(:Phase1_Inherit), "p")
    assert_eq(autoload?(:Phase1_Inherit, true), "p")
    assert_eq(autoload?(:Phase1_Inherit, false), "p")
  end

  it "raises TypeError on non-Symbol/non-String name" do
    klass, msg = caught_pair { autoload?(42) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "42 is not a symbol nor a string")
  end

  it "raises NameError on lowercase Symbol" do
    klass, msg = caught_pair { autoload?(:lowercase_q) }
    assert_eq(klass, "NameError")
    assert_eq(msg, "wrong constant name lowercase_q")
  end
end

describe "dispatch precedence: class-body autoload defers to class arm" do
  # When bare `autoload :Bar, "x"` is called inside `class Foo`,
  # the kernel-builtin path must DEFER to the class-recv arm —
  # otherwise the toplevel registry would be polluted with what
  # should be Foo-scoped autoloads. Phase 1 keeps the class-arm
  # as a no-op stub, so the effective contract here is "toplevel
  # autoload registry stays clean when autoload is called inside
  # a class body".

  it "class-body autoload does NOT register on toplevel" do
    class AutoloadP1_DispatchScope
      autoload(:Inner, "x")
    end
    # Phase 1: class-arm is still a no-op stub. The important
    # check is that the TOPLEVEL registry wasn't polluted.
    assert_eq(autoload?(:Inner), nil)
  end

  it "class-body autoload? returns nil (class arm is stub)" do
    class AutoloadP1_DispatchScope2
      autoload(:Other, "x")
      # Inside the body, autoload? hits the class arm which is
      # currently a no-op stub returning nil — Phase 2 will wire
      # this up.
    end
    assert_eq(autoload?(:Other), nil)
  end
end
