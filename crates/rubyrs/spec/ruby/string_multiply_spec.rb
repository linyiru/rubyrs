# Adapted from ruby/spec core/string/multiply_spec.rb (negative-arg parity).

# String#* (repeat) — Int arg, returns receiver repeated n
# times. Negative n is an ArgumentError in CRuby ("negative
# argument"); rubyrs previously used `(*n).max(0) as usize`
# and silently returned "". Same `max(0)`-swallows-negative
# pattern that PR #340 cycle-14 fixed for Array#take/#drop.

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "String#*" do
  it "repeats the receiver n times" do
    assert_eq("abc" * 3, "abcabcabc")
  end

  it "returns empty string for n = 0" do
    assert_eq("abc" * 0, "")
  end

  it "returns empty string for empty receiver regardless of n" do
    assert_eq("" * 5, "")
  end

  it "raises ArgumentError on negative arg" do
    klass, msg = caught_pair { "abc" * -1 }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "negative argument")
  end

  it "raises ArgumentError on large negative arg" do
    klass, msg = caught_pair { "abc" * -100 }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "negative argument")
  end
end
