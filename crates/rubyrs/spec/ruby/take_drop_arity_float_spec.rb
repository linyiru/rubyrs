# Adapted from ruby/spec core/array/take_spec.rb / drop_spec.rb (arity + Float coerce).

# Cross-receiver wrong-arity / non-Int / Float-coerce sweep for
# `take(n)` / `drop(n)` on Array and Hash — extends the
# pattern PRs #330 and #338 established for the
# each_slice/each_cons family. Before this sweep:
#   - Array#take("2") → NoMethodError (CRuby: TypeError)
#   - Hash#take(:x) → ArgumentError "given 1, expected 1"
#     (CRuby: TypeError)
#   - Hash#take(2.5) → NoMethodError (CRuby: [{...}, {...}])
# All three now match CRuby byte-identical via the shared
# `arity_error_arg1_int` + `float_to_int_arg` helpers.

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "Array#take / #drop arity & type guards" do
  it "raises ArgumentError on zero-arg take" do
    klass, msg = caught_pair { [1].take }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 0, expected 1)")
  end

  it "raises ArgumentError on multi-arg take" do
    klass, msg = caught_pair { [1].take(2, 3) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 2, expected 1)")
  end

  it "raises TypeError on String arg" do
    klass, msg = caught_pair { [1].take("2") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "raises TypeError on Symbol arg (drop)" do
    klass, msg = caught_pair { [1].drop(:two) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Symbol into Integer")
  end

  it "truncates Float arg toward zero (take)" do
    assert_eq([1, 2, 3, 4].take(2.5), [1, 2])
  end

  it "truncates Float arg toward zero (drop)" do
    assert_eq([1, 2, 3, 4].drop(2.5), [3, 4])
  end

  it "raises RangeError on NaN" do
    klass, msg = caught_pair { [1].take(0.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float NaN out of range of integer")
  end

  it "raises ArgumentError on negative Int arg (take)" do
    # Pre-fix used `(*n).max(0) as usize` which silently
    # swallowed negatives and returned []. Hash#take got it
    # right; Array#take was a pre-existing divergence that
    # this PR's Float arm propagated (Float -2.5 → Int -2).
    klass, msg = caught_pair { [1, 2, 3].take(-1) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "attempt to take negative size")
  end

  it "raises ArgumentError on negative Int arg (drop)" do
    klass, msg = caught_pair { [1, 2, 3].drop(-1) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "attempt to drop negative size")
  end

  it "raises ArgumentError on negative Float arg (truncates to negative Int)" do
    klass, msg = caught_pair { [1, 2, 3, 4].take(-2.5) }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "attempt to take negative size")
  end

  it "raises RangeError on BigInt arg (parity with Hash#take/#drop)" do
    # Without the cfg-gated BigInt arm in array.rs, BigInt
    # would fall into the take/drop catch-all and render as
    # "no implicit conversion of Integer into Integer" —
    # nonsensical because type_name_for_coerce(BigInt) is
    # "Integer". Hash had the arm at hash.rs:378; Array now
    # matches.
    klass, msg = caught_pair { [1].take(2**70) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "bignum too big to convert into `long'")
  end
end

describe "Hash#take / #drop arity & type guards" do
  it "raises ArgumentError on zero-arg take" do
    klass, msg = caught_pair { {a: 1}.take }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 0, expected 1)")
  end

  it "raises TypeError on String arg (was bad ArgumentError pre-fix)" do
    # Pre-#330 Hash had a catch-all that lumped non-Int args
    # under ArgumentError "given 1, expected 1" — wrong class
    # and nonsensical message. Now routes through the shared
    # arity_error_arg1_int helper.
    klass, msg = caught_pair { {a: 1}.take("2") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "truncates Float arg toward zero (take)" do
    assert_eq({a: 1, b: 2, c: 3, d: 4}.take(2.5), [[:a, 1], [:b, 2]])
  end

  it "raises RangeError on +Infinity (drop)" do
    klass, msg = caught_pair { {a: 1}.drop(1.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float Inf out of range of integer")
  end
end
