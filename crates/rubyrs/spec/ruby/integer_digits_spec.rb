# Adapted from ruby/spec core/integer/digits_spec.rb at
# upstream commit 448cb340 (2026-05). RE-EXTRACTED with
# `rubyrs-spec-extract` v0.2; replaces the PR #61 landing
# (which used v0.1 + hand-translated raise blocks).
#
# Diff vs PR #61:
#   - The `should.raise(ArgumentError)` blocks now come
#     verbatim from the extractor (v0.2 added lambda-raise
#     lowering); no more hand polish.
#   - The two skipped blocks (mock_int / Math::DomainError)
#     stay commented out for the same reasons as before:
#       * `mock_int` (digits_spec.rb:12-14) — no mock library
#         in the micro-runner; coercion via `to_int` is a
#         separate spec area.
#       * Math::DomainError on negative receiver
#         (digits_spec.rb:29-31) — rubyrs raises ArgumentError
#         instead. See docs/SUBSET.md → "Integer built-in
#         methods" for the documented divergence.
#
# After this update only those two `it` blocks need hand
# input; everything else is byte-identical to what the
# extractor produced.

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
