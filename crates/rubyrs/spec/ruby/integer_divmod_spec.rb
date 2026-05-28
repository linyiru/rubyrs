# Adapted from ruby/spec core/integer/divmod_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should.raise(X)` → `assert_raises`.
# - `bignum_value` / `bignum_value(N)` → `(2**64)` / `(2**64 + N)`.
# - skipped (mock): the to_int mock test in the fixnum context's
#   TypeError example — the plain non-Integer cases ("10" / :symbol)
#   are kept as the non-mock subset.
# - skipped (mock): the bignum context's `mock('10')` TypeError test.
# - skipped (method-not-implemented): `FloatDomainError` rescue at
#   the script level isn't wired through rubyrs's exception class
#   hierarchy; the raise itself works (the implementation raises
#   `Uncaught { class_name: "FloatDomainError" }`) but the
#   micro-runner's `assert_raises` checks would need ancestor-aware
#   matching. Tracked as a follow-up.

describe "Integer#divmod" do
  it "fixnum: returns an Array containing quotient and modulus" do
    assert_eq(13.divmod(4), [3, 1])
    assert_eq(4.divmod(13), [0, 4])
    assert_eq(13.divmod(4.0), [3, 1.0])
    assert_eq(4.divmod(13.0), [0, 4.0])
    assert_eq(1.divmod(2.0), [0, 1.0])
  end

  bignum_it "fixnum: divmod with bignum divisor" do
    assert_eq(200.divmod(2**64), [0, 200])
  end

  it "fixnum: raises a ZeroDivisionError when the given argument is 0" do
    assert_raises("ZeroDivisionError") { 13.divmod(0) }
    assert_raises("ZeroDivisionError") { 0.divmod(0) }
    assert_raises("ZeroDivisionError") { (-10).divmod(0) }
  end

  it "fixnum: raises a ZeroDivisionError when the given argument is 0.0" do
    assert_raises("ZeroDivisionError") { 0.divmod(0.0) }
    assert_raises("ZeroDivisionError") { 10.divmod(0.0) }
    assert_raises("ZeroDivisionError") { (-10).divmod(0.0) }
  end

  it "fixnum: raises a TypeError when given a non-Integer" do
    assert_raises("TypeError") { 13.divmod("10") }
    assert_raises("TypeError") { 13.divmod(:symbol) }
  end

  bignum_it "bignum: returns an Array containing quotient and modulus" do
    bn = 2**64 + 55
    assert_eq(bn.divmod(4),  [4611686018427387917, 3])
    assert_eq(bn.divmod(13), [1418980313362273205, 6])
  end

  bignum_it "bignum: returns 0 quotient when divisor equals the bignum" do
    bn = 2**64 + 55
    assert_eq(bn.divmod(bn), [1, 0])
  end

  bignum_it "bignum: floor-division semantics for mixed signs (large operands)" do
    # CRuby property: if q, r = a.divmod(b), then
    #   b > 0 ⇒ 0 ≤ r < b
    #   b < 0 ⇒ b < r ≤ 0
    pair = (10**50).divmod(10**40 + 1)
    assert_eq(pair, [9999999999, 9999999999999999999999999999990000000001])
    pair = ((-(10**50))).divmod(10**40 + 1)
    assert_eq(pair, [-10000000000, 10000000000])
  end

  bignum_it "bignum: raises a ZeroDivisionError when the given argument is 0" do
    bn = 2**64
    assert_raises("ZeroDivisionError") { bn.divmod(0) }
    assert_raises("ZeroDivisionError") { (-bn).divmod(0) }
    assert_raises("ZeroDivisionError") { bn.divmod(0.0) }
  end

  bignum_it "bignum: raises a TypeError when the given argument is not an Integer" do
    bn = 2**64
    assert_raises("TypeError") { bn.divmod("10") }
    assert_raises("TypeError") { bn.divmod(:symbol) }
  end

  # skipped (method-not-implemented): `FloatDomainError` rescue at
  # the script level isn't currently wired through rubyrs's
  # exception class hierarchy. The implementation does raise the
  # right error (`Uncaught { class_name: "FloatDomainError" }`)
  # but the micro-runner's `assert_raises` matcher doesn't yet
  # walk the ancestor chain for non-StandardError classes.
  #
  # bignum_it "bignum: raises a FloatDomainError if other is NaN" do
  #   bn = 2**64
  #   assert_raises("FloatDomainError") { bn.divmod(0.0/0.0) }
  # end
end
