# Adapted from ruby/spec core/hash/first_spec.rb / core/range/first_spec.rb / last_spec.rb (arity + Float coerce).

# Cross-receiver wrong-arity / non-Int / Float-coerce sweep
# extending PR #349 to Hash#first and Range#first/#last. Pre-PR
# all three fell to NoMethodError on wrong shapes despite
# `respond_to?` returning true. Same lockstep contract sweep
# from PRs #330/#338/#340/#345/#349.

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "Hash#first arity & type guards" do
  it "raises TypeError on String arg" do
    klass, msg = caught_pair { {a: 1, b: 2}.first("1") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "raises TypeError on Symbol arg" do
    klass, msg = caught_pair { {a: 1, b: 2}.first(:x) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Symbol into Integer")
  end

  it "truncates Float arg toward zero" do
    assert_eq({a: 1, b: 2, c: 3, d: 4}.first(2.5), [[:a, 1], [:b, 2]])
  end

  it "raises RangeError on NaN" do
    klass, msg = caught_pair { {a: 1}.first(0.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float NaN out of range of integer")
  end
end

describe "Range#first / #last arity & type guards" do
  it "raises TypeError on String arg (first)" do
    klass, msg = caught_pair { (1..5).first("1") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "raises TypeError on nil arg (last)" do
    klass, msg = caught_pair { (1..5).last(nil) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion from nil to integer")
  end

  it "truncates Float arg toward zero (first)" do
    assert_eq((1..5).first(2.5), [1, 2])
  end

  it "truncates Float arg toward zero (last)" do
    assert_eq((1..5).last(2.5), [4, 5])
  end

  it "raises RangeError on +Infinity (first)" do
    klass, msg = caught_pair { (1..5).first(1.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float Inf out of range of integer")
  end

  it "raises RangeError on BigInt arg (parity with Array/Hash first/last)" do
    # Without the cfg-gated BigInt arm, BigInt falls into
    # arity_error_arg0_or_1_int and renders as the
    # nonsensical "no implicit conversion of Integer into
    # Integer" TypeError because type_name_for_coerce(BigInt)
    # returns "Integer". Matches the BigInt arms already in
    # Array#first/#last (array.rs:581/647) and Hash#first
    # (hash.rs:338).
    klass, msg = caught_pair { (1..5).first(2**70) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "bignum too big to convert into `long'")
  end

  it "raises ArgumentError on multi-arg (Range uses 'expected 1', not '0..1' — CRuby quirk)" do
    # CRuby Range#first / #last use "expected 1" for multi-arg
    # while Array uses "expected 0..1". Match CRuby's exact
    # wording per-receiver.
    klass, msg = caught_pair { (1..5).first(1, 2) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 2, expected 1)")
  end
end
