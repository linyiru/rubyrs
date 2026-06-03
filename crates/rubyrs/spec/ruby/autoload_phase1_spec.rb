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
    assert_eq(msg, "no implicit conversion of Integer into Symbol")
  end

  it "raises TypeError on non-String path" do
    klass, msg = caught_pair { autoload(:X, 42) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Integer into String")
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
    assert_eq(msg, "no implicit conversion of Integer into Symbol")
  end
end
