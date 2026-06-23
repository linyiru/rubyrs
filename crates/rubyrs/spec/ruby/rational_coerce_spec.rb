# Adapted from ruby/spec core/rational/coerce_spec.rb at 2026-05.
# Covers the Rational#coerce arm in vm/dispatch.rs: Integer coerces to a
# Rational pair, Rational stays as-is, and non-numeric raises.

describe "Rational#coerce" do
  it "returns [Rational(other), self] for an Integer" do
    assert_eq(Rational(3, 4).coerce(2), [Rational(2, 1), Rational(3, 4)])
  end

  it "returns [other, self] for a Rational" do
    assert_eq(Rational(3, 4).coerce(Rational(1, 2)),
              [Rational(1, 2), Rational(3, 4)])
  end

  it "raises TypeError for a non-numeric argument" do
    assert_raises("TypeError") { Rational(3, 4).coerce("x") }
    assert_raises("TypeError") { Rational(3, 4).coerce(:sym) }
    assert_raises("TypeError") { Rational(3, 4).coerce(nil) }
  end

  bignum_it "bignum: coerces a BigInt to a Rational pair" do
    bn = 2**70
    assert_eq(Rational(3, 4).coerce(bn), [Rational(bn, 1), Rational(3, 4)])
  end
end
