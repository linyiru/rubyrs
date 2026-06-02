# Adapted from ruby/spec language/literal/numeric_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - Phase C.4.4 wires `1/2r`-style Rational literals to a real
#   `Value::Rational` (replacing the pre-C.4.4 lowering to
#   `Float`). Both `class` and exact arithmetic now match CRuby.

describe "Rational literal" do
  it "evaluates to a canonical-form Rational" do
    assert_eq((1/2r).class.to_s, "Rational")
    assert_eq((1/2r).numerator, 1)
    assert_eq((1/2r).denominator, 2)
  end

  it "evaluates to a Rational in canonical form (gcd-reduced)" do
    # 3/9r → (1/3), 6/4r → (3/2). Bignum tier reduces at parse
    # time; no-bignum reduces in `make_rational` at load time —
    # observable behavior is identical from the user's view.
    assert_eq((3/9r).numerator, 1)
    assert_eq((3/9r).denominator, 3)
    assert_eq((6/4r).numerator, 3)
    assert_eq((6/4r).denominator, 2)
  end

  it "supports negative literals" do
    assert_eq((-1/2r).numerator, -1)
    assert_eq((-1/2r).denominator, 2)
  end

  it "supports the .0r decimal form (integer Rationals)" do
    # `1000.0r` parses as Rational(1000, 1).
    assert_eq((1000.0r).class.to_s, "Rational")
    assert_eq((1000.0r).numerator, 1000)
    assert_eq((1000.0r).denominator, 1)
  end

  it "is equal to the corresponding Rational(num, den) value" do
    assert_eq((1/2r), Rational(1, 2))
    assert_eq((-3/4r), Rational(-3, 4))
    assert_eq((1000.0r), Rational(1000, 1))
  end

  it "supports arithmetic via the standard Rational operators" do
    assert_eq((1/2r + 1/3r), Rational(5, 6))
    assert_eq((1/2r * 4), Rational(2, 1))
    assert_eq((1/2r - 1/4r), Rational(1, 4))
  end
end
