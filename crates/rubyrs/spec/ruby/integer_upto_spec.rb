# Adapted from ruby/spec core/integer/upto_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should.raise(X)` → `assert_raises`.
# - Float endpoint accepted (yields up to floor); non-numeric
#   endpoint raises ArgumentError ("comparison of Integer with
#   X failed"). Both gaps surfaced as `divergent` in earlier
#   batches and are now closed — pinned by
#   tests/embed/numeric.rs::int_iter_arity_and_coerce_errors_match_cruby.
# - skipped (method-not-implemented): the no-block Enumerator
#   surface and `Enumerator#size` assertions. Same rationale as
#   integer_times_spec.

describe "Integer#upto [stop] when self and stop are Integers" do
  it "does not yield when stop is less than self" do
    result = []
    5.upto(4) { |x| result << x }
    assert_eq(result, [])
  end

  it "yields once when stop equals self" do
    result = []
    5.upto(5) { |x| result << x }
    assert_eq(result, [5])
  end

  it "yields each value from self up to and including stop" do
    result = []
    2.upto(5) { |x| result << x }
    assert_eq(result, [2, 3, 4, 5])
  end

  it "yields while increasing self until it is greater than floor of a Float endpoint" do
    result = []
    9.upto(13.3) {|i| result << i}
    -5.upto(-1.3) {|i| result << i}
    assert_eq(result, [9,10,11,12,13,-5,-4,-3,-2])
  end

  it "raises an ArgumentError for non-numeric endpoints" do
    assert_raises("ArgumentError") { 1.upto("A") {|x| p x} }
    assert_raises("ArgumentError") { 1.upto(nil) {|x| p x} }
  end

  # skipped (method-not-implemented): no-block Enumerator surface.
  #
  # describe "when no block is given" do
  #   it "returns an Enumerator" do
  #     result = []
  #     enum = 2.upto(5)
  #     enum.each { |i| result << i }
  #     assert_eq(result, [2, 3, 4, 5])
  #   end
  #
  #   describe "returned Enumerator" do
  #     describe "size" do
  #       # ...
  #     end
  #   end
  # end
end
