# Adapted from ruby/spec core/integer/coerce_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should raise_error` → `assert_raises`.
# - `bignum_value` → `(2**64)`; bignum cases gated on `bignum_it`.
# - upstream nests `context "fixnum"/"bignum"`; flatten into
#   descriptive top-level it / bignum_it names since the micro-
#   runner has no `context`.
# - skipped (method-not-implemented): the `Rational(x, y)` coerce
#   cases — Rational isn't modeled in rubyrs (Phase C). Same
#   rationale for the `Complex(...)` cases.

describe "Integer#coerce" do
  it "fixnum: returns [other, self] when other is an Integer" do
    assert_eq(1.coerce(2), [2, 1])
    assert_eq(10.coerce(-5), [-5, 10])
    assert_eq(0.coerce(0), [0, 0])
  end

  it "fixnum: returns [Float(other), Float(self)] when other is a Float" do
    assert_eq(1.coerce(2.5), [2.5, 1.0])
    assert_eq(5.coerce(-3.14), [-3.14, 5.0])
  end

  it "fixnum: raises a TypeError when other is a String" do
    assert_raises("TypeError") { 1.coerce("2") }
  end

  it "fixnum: raises a TypeError when other is nil" do
    assert_raises("TypeError") { 1.coerce(nil) }
  end

  it "fixnum: raises a TypeError when other is a Symbol" do
    assert_raises("TypeError") { 1.coerce(:sym) }
  end

  bignum_it "bignum: returns [other, self] when other is an Integer" do
    bn = 2**64
    assert_eq(bn.coerce(1), [1, bn])
    assert_eq(bn.coerce(-bn), [-bn, bn])
    assert_eq(1.coerce(bn), [bn, 1])
  end

  bignum_it "bignum: returns [Float(other), Float(self)] when other is a Float" do
    bn = 2**64
    assert_eq(bn.coerce(2.5), [2.5, bn.to_f])
  end

  bignum_it "bignum: raises a TypeError when other is a String" do
    bn = 2**64
    assert_raises("TypeError") { bn.coerce("x") }
  end

  bignum_it "bignum: raises a TypeError when other is nil" do
    bn = 2**64
    assert_raises("TypeError") { bn.coerce(nil) }
  end

  # skipped (method-not-implemented): the `Rational(x, y)` and
  # `Complex(...)` coerce cases. Rational / Complex aren't
  # modeled in rubyrs (Phase C — Numeric#coerce is the gate, now
  # in place; Rational and Complex classes themselves are
  # tracked as a follow-up).
end
