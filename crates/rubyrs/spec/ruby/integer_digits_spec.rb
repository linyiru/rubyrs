# Adapted from ruby/spec core/integer/digits_spec.rb at
# 2026-05 (upstream commit 448cb340) — FIRST landed extractor
# output. The bulk of the file came out of
# `rubyrs-spec-extract` v0.1 in one shot (the `assert_eq(...)`
# calls below are byte-for-byte what the extractor produced).
# What v0.1 couldn't yet handle:
#   - `-> { ... }.should.raise(X)` lambdas (v0.2's job;
#     hand-translated to `assert_raises("X") { ... }` here)
#   - `mock_int(2)` (no mock library; the upstream `it` block
#     is preserved as a comment, not run)
#   - one Math::DomainError divergence (rubyrs raises
#     ArgumentError on Integer#digits over a negative
#     receiver — see docs/SUBSET.md → "Integer built-in
#     methods"; the `it` block stays as a comment until
#     alignment lands)

describe "Integer#digits" do
  it "returns an array of place values in base-10 by default" do
    assert_eq(12345.digits, [5,4,3,2,1])
  end

  it "returns digits by place value of a given radix" do
    assert_eq(12345.digits(7), [4,6,6,0,5])
  end

  # Skipped — upstream digits_spec.rb:12-14 uses mspec's
  # `mock_int(2)` to exercise the `to_int` coercion protocol.
  # rubyrs has no mock library and Integer#digits(int_like)
  # coercion is a separate spec area.
  #
  # it "converts the radix with #to_int" do
  #   assert_eq(12345.digits(mock_int(2)), [1, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 1])
  # end

  it "returns [0] when called on 0, regardless of base" do
    assert_eq(0.digits, [0])
    assert_eq(0.digits(7), [0])
  end

  it "raises ArgumentError when calling with a radix less than 2" do
    # Hand-translated from upstream's
    # `-> { 12345.digits(1) }.should.raise(ArgumentError)`.
    # v0.2 of the extractor will produce this form
    # automatically.
    assert_raises("ArgumentError") do
      12345.digits(1)
    end
  end

  it "raises ArgumentError when calling with a negative radix" do
    assert_raises("ArgumentError") do
      12345.digits(-2)
    end
  end

  # Skipped (divergence) — upstream digits_spec.rb:29-31.
  # CRuby raises Math::DomainError on Integer#digits called on
  # a negative receiver; rubyrs raises ArgumentError. Tracked
  # in docs/SUBSET.md → "Integer built-in methods". Un-skip
  # when alignment lands.
  #
  # it "raises Math::DomainError when calling digits on a negative number" do
  #   assert_raises("Math::DomainError") do
  #     -12345.digits(7)
  #   end
  # end

  it "returns integer values > 9 when base is above 10" do
    assert_eq(1234.digits(16), [2, 13, 4])
  end

  it "can be used with base > 37" do
    assert_eq(1234.digits(100), [34, 12])
    assert_eq(980099.digits(100), [99, 0, 98])
  end
end
