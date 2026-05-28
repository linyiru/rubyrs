# Adapted from ruby/spec core/integer/round_spec.rb +
# shared/to_i.rb + shared/integer_rounding.rb at upstream commit
# 448cb340. Hand-polish:
# - `.should.eql?(x)` → `assert_eq(actual, x)`.
# - skipped (mock): the `mock("Object").should_receive(:to_int)`
#   coerce tests — micro-runner has no mock library and the
#   to_int protocol isn't wired through ceil/floor/round/truncate.
# - `half:` keyword argument (`:up` / `:down` / `:even`) is
#   implemented via the dedicated `Op::CallKw*` kwarg-routing
#   path; both Int and Float receivers route through
#   `int_round_with_half` / `float_round_with_half` for these
#   modes. Default :up matches CRuby's round-half-away-from-zero.
# - skipped (method-not-implemented): `10**70`-magnitude inputs
#   and precision -71 — needs BigInt-aware rounding.
# - skipped (method-not-implemented): `min_long - 1` / `Float::INFINITY`
#   / `1<<31` precision-bound RangeError. The bound-check would
#   slot in beside the existing i64-doesn't-fit decline path
#   in numeric.rs (which today routes through the i128 widening
#   up to `|n| <= 38` and then falls back to NoMethodError
#   when the scaled result overflows i64). Deferred.

describe "Integer#round" do
  it "fixnum: returns self for to_i shape" do
    assert_eq(10.round, 10)
    assert_eq((-15).round, -15)
  end

  bignum_it "bignum: returns self" do
    bn = 2**64
    assert_eq(bn.round, bn)
    assert_eq((-bn).round, -bn)
  end

  it "returns self if not passed a precision" do
    [2, -4].each { |v| assert_eq(v.round, v) }
  end

  it "returns self if passed a precision of zero" do
    [2, -4].each { |v| assert_eq(v.round(0), v) }
  end

  it "returns itself if passed a positive precision" do
    [2, -4].each { |v| assert_eq(v.round(42), v) }
  end

  it "returns itself rounded if passed a negative value" do
    assert_eq(249.round(-2), 200)
    assert_eq((-249).round(-2), -200)
  end

  it "returns itself rounded to nearest if passed a negative value" do
    assert_eq(250.round(-2), 300)
    assert_eq((-250).round(-2), -300)
  end

  it "raises a TypeError when passed a String" do
    assert_raises("TypeError") { 42.round("4") }
  end

  it "raises a TypeError when its argument cannot be converted to an Integer" do
    assert_raises("TypeError") { 42.round(nil) }
    assert_raises("TypeError") { 42.round(:sym) }
  end

  # skipped (mock): `mock("Object").should_receive(:to_int)` —
  # to_int coerce protocol is a separate follow-up.

  it "rounds to nearest, half away from zero by default and with half: :up" do
    assert_eq(25.round(-1), 30)
    assert_eq(25.round(-1, half: :up), 30)
    assert_eq((-25).round(-1, half: :up), -30)
  end

  it "rounds half toward zero with half: :down" do
    assert_eq(25.round(-1, half: :down), 20)
    assert_eq(35.round(-1, half: :down), 30)
    assert_eq((-25).round(-1, half: :down), -20)
  end

  it "rounds half to even with half: :even (banker's rounding)" do
    assert_eq(25.round(-1, half: :even), 20)
    assert_eq(35.round(-1, half: :even), 40)
    assert_eq(45.round(-1, half: :even), 40)
    assert_eq(55.round(-1, half: :even), 60)
  end

  it "raises ArgumentError for unknown half: value" do
    assert_raises("ArgumentError") { 25.round(-1, half: :weird) }
  end

  it "raises ArgumentError for unknown kwarg key" do
    assert_raises("ArgumentError") { 25.round(-1, foo: :bar) }
  end

  # skipped (method-not-implemented): `42.round(min_long - 1)` /
  # `42.round(Float::INFINITY)` / `42.round(1<<31)` RangeError
  # cases — would need a precision-bound check.

  # skipped (method-not-implemented): `(25 * 10**70).round(-71)`
  # BigInt-magnitude inputs — see integer_ceil_spec.rb header.
end
