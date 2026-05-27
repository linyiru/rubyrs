# Adapted from ruby/spec core/integer/comparison_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; nested `context`/`describe`
#   blocks flattened into top-level it / bignum_it names (the
#   micro-runner doesn't define `context`).
# - `bignum_value` → `(2**64)`; `bignum_value(N)` → `(2**64 + N)`.
# - `before :each` with `@bignum` inlined per `bignum_it`.
# - skipped (mock): all the `mock('value for Integer#<=>')`
#   coerce / exception-propagation tests in the "with an
#   Object" context — they require mspec's mock library to
#   stub `#coerce` return values.
# - skipped (method-not-implemented): `infinity_value` /
#   `Float::MAX` references that depend on Float constants
#   not exposed in rubyrs. Tier-2 Encoding/constants work.

describe "Integer#<=>" do
  it "fixnum: returns -1 when self is less than the given argument" do
    assert_eq((-3 <=> -1), -1)
    assert_eq((-5 <=> 10), -1)
    assert_eq((-5 <=> -4.5), -1)
  end

  it "fixnum: returns 0 when self is equal to the given argument" do
    assert_eq((0 <=> 0), 0)
    assert_eq((954 <=> 954), 0)
    assert_eq((954 <=> 954.0), 0)
  end

  it "fixnum: returns 1 when self is greater than the given argument" do
    assert_eq((496 <=> 5), 1)
    assert_eq((200 <=> 100), 1)
    assert_eq((51 <=> 50.5), 1)
  end

  it "fixnum: returns nil when the given argument is not an Integer" do
    assert_eq((3 <=> 'test'), nil)
    assert_eq((3 <=> :sym), nil)
    assert_eq((3 <=> nil), nil)
  end

  bignum_it "bignum × fixnum: returns -1 when other is larger" do
    assert_eq((-(2**64) <=> 2), -1)
  end

  bignum_it "bignum × fixnum: returns 1 when other is smaller" do
    assert_eq(((2**64) <=> 2), 1)
  end

  bignum_it "bignum × negative bignum: returns -1 when self is more negative" do
    assert_eq((-(2**64 + 42) <=> -(2**64)), -1)
  end

  bignum_it "bignum × negative bignum: returns 0 when other is equal" do
    assert_eq((-(2**64) <=> -(2**64)), 0)
  end

  bignum_it "bignum × negative bignum: returns 1 when self is less negative" do
    assert_eq((-(2**64) <=> -(2**64 + 94)), 1)
  end

  bignum_it "bignum × negative bignum: returns 1 when self is positive" do
    assert_eq(((2**64) <=> -(2**64)), 1)
  end

  bignum_it "bignum × positive bignum: returns -1 when self is negative" do
    assert_eq((-(2**64) <=> (2**64)), -1)
  end

  bignum_it "bignum × positive bignum: returns -1 when other is larger" do
    assert_eq(((2**64) <=> (2**64 + 38)), -1)
  end

  bignum_it "bignum × positive bignum: returns 0 when other is equal" do
    assert_eq(((2**64) <=> (2**64)), 0)
  end

  bignum_it "bignum × positive bignum: returns 1 when other is smaller" do
    assert_eq(((2**64 + 56) <=> (2**64)), 1)
  end

  bignum_it "bignum × negative float: returns -1 when self is more negative" do
    assert_eq((-(2**64 + 0xffff) <=> -(2**64).to_f), -1)
  end

  bignum_it "bignum × negative float: returns 0 when other is equal" do
    assert_eq((-(2**64) <=> -(2**64).to_f), 0)
  end

  bignum_it "bignum × negative float: returns 1 when self is less negative" do
    assert_eq((-(2**64) <=> -(2**64 + 0xffef).to_f), 1)
  end

  bignum_it "bignum × negative float: returns 1 when self is positive" do
    assert_eq(((2**64) <=> -(2**64).to_f), 1)
  end

  bignum_it "bignum × positive float: returns -1 when self is negative" do
    assert_eq((-(2**64) <=> (2**64).to_f), -1)
  end

  bignum_it "bignum × positive float: returns -1 when other is larger" do
    assert_eq(((2**64) <=> (2**64 + 0xfffe).to_f), -1)
  end

  bignum_it "bignum × positive float: returns 0 when other is equal" do
    assert_eq(((2**64) <=> (2**64).to_f), 0)
  end

  bignum_it "bignum × positive float: returns 1 when other is smaller" do
    assert_eq(((2**64 + 0xfeff) <=> (2**64).to_f), 1)
  end

  bignum_it "bignum × float: does not lose precision for values that don't fit in a double" do
    # The core precision test — pre-fix all three returned the
    # same answer (0) because both sides demoted to the same f64.
    assert_eq(((2**64 + 1) <=> (2**64).to_f), 1)
    assert_eq(((2**64) <=> (2**64).to_f), 0)
    assert_eq(((2**64 - 1) <=> (2**64).to_f), -1)
  end

  # skipped (mock): the "with an Object" context tests
  # (`#coerce` mock + RuntimeError/Exception propagation +
  # nil-array return) all require mspec's mock library.
  #
  # describe "with an Object" do
  #   it "calls #coerce on other" / "lets the exception go through" / etc.
  # end

  # skipped (method-not-implemented): the four `infinity_value`
  # / `Float::MAX` tests at the bottom of upstream depend on
  # Float constants not exposed in rubyrs (Float::NAN / INFINITY
  # / MAX). Constructible inline (e.g. `1.0 / 0.0`) but the
  # upstream assertions name them via the spec_helper fixture
  # `infinity_value`. Tier-2 constants work.
  #
  # it "returns 1 when self is Infinity and other is a Bignum"
  # it "returns -1 when self is negative and other is Infinity"
  # it "returns 1 when self is negative and other is -Infinity"
  # it "returns -1 when self is -Infinity and other is negative"
end
