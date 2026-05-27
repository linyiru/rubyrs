# Adapted from ruby/spec core/integer/upto_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`; `should.raise(X)` → `assert_raises`.
# - skipped (divergent): upstream uses `should.raise(ArgumentError)`
#   for non-numeric endpoint, but rubyrs's iter.rs raises
#   TypeError ("no implicit conversion of X into Integer") instead.
#   Pinned by tests/embed/numeric.rs::int_iter_arity_and_coerce_errors_match_cruby.
#   The error-class divergence is a known gap from PR #174 (the
#   "CRuby coerce error" comment in iter.rs is technically wrong;
#   CRuby's #upto/#downto compare endpoints rather than coerce,
#   so non-numeric goes through Comparable and raises
#   ArgumentError). Out of B.6 scope.
# - skipped (divergent): Float endpoint (`9.upto(13.3)`). rubyrs
#   raises TypeError via the [other] coerce arm; CRuby walks
#   while self <=> stop with Comparable up to floor of stop.
#   Implementing parity would need a Float-comparison branch on
#   the Int side of #upto. Out of B.6 scope.
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

  # skipped (divergent): Float endpoint. CRuby yields up to floor;
  # rubyrs raises TypeError via the [other]-arm coerce guard.
  #
  # it "yields while increasing self until it is greater than floor of a Float endpoint" do
  #   result = []
  #   9.upto(13.3) {|i| result << i}
  #   -5.upto(-1.3) {|i| result << i}
  #   assert_eq(result, [9,10,11,12,13,-5,-4,-3,-2])
  # end

  # skipped (divergent): upstream expects ArgumentError;
  # rubyrs raises TypeError. See file header.
  #
  # it "raises an ArgumentError for non-numeric endpoints" do
  #   assert_raises("ArgumentError") { 1.upto("A") {|x| p x} }
  #   assert_raises("ArgumentError") { 1.upto(nil) {|x| p x} }
  # end

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
