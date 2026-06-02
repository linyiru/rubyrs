# Adapted from ruby/spec core/array/pop_spec.rb / shift_spec.rb / first_spec.rb / last_spec.rb (arity + Float coerce).

# Cross-method wrong-arity / non-Int / Float-coerce sweep for
# Array#pop / #shift / #first / #last — all four take 0..1
# Int args. Continues the lockstep-contract sweep from PRs
# #330 / #338 / #340 / #345. Before this sweep:
#   - Array#pop("1") / .shift / .first / .last  → wrong
#     ArgumentError class (CRuby: TypeError)
#   - Array#pop(2.5) etc. → ArgumentError (CRuby: coerces to 2)
#   - Array#first / .last had no catch-all → NoMethodError on
#     wrong shape
# All four now match CRuby byte-identical via the shared
# `arity_error_arg0_or_1_int` + `float_to_int_arg` helpers.

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "Array#pop arity & type guards" do
  it "raises TypeError on String arg" do
    klass, msg = caught_pair { [1, 2, 3].pop("1") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "raises TypeError on nil arg" do
    klass, msg = caught_pair { [1, 2, 3].pop(nil) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion from nil to integer")
  end

  it "truncates Float arg toward zero" do
    assert_eq([1, 2, 3, 4].pop(2.5), [3, 4])
  end

  it "raises RangeError on NaN arg" do
    klass, msg = caught_pair { [1, 2, 3].pop(0.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float NaN out of range of integer")
  end

  it "raises ArgumentError on multi-arg" do
    klass, msg = caught_pair { [1].pop(1, 2) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 2, expected 0..1)")
  end
end

describe "Array#shift arity & type guards" do
  it "raises TypeError on Symbol arg" do
    klass, msg = caught_pair { [1, 2, 3].shift(:x) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Symbol into Integer")
  end

  it "truncates Float arg toward zero" do
    assert_eq([1, 2, 3, 4].shift(2.5), [1, 2])
  end
end

describe "Array#first arity & type guards" do
  it "raises TypeError on String arg (was NoMethodError pre-fix)" do
    # Pre-PR Array#first/#last had no catch-all and fell
    # through to NoMethodError despite `respond_to?` returning
    # true. Same lockstep violation pattern caught in PR
    # #308 / #316 / #323.
    klass, msg = caught_pair { [1, 2, 3].first("1") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "truncates Float arg toward zero" do
    assert_eq([1, 2, 3, 4].first(2.5), [1, 2])
  end
end

describe "Array#last arity & type guards" do
  it "raises TypeError on Symbol arg (was NoMethodError pre-fix)" do
    klass, msg = caught_pair { [1, 2, 3].last(:x) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Symbol into Integer")
  end

  it "truncates Float arg toward zero" do
    assert_eq([1, 2, 3, 4].last(2.5), [3, 4])
  end

  it "raises RangeError on +Infinity arg" do
    klass, msg = caught_pair { [1, 2, 3].last(1.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float Inf out of range of integer")
  end
end
