# Adapted from ruby/spec core/integer/downto_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished — same conventions
# as integer_upto_spec.rb (sibling): Float endpoint accepted
# (yields down to ceil); non-numeric raises ArgumentError.
# Both behaviors pinned by
# tests/embed/numeric.rs::int_iter_arity_and_coerce_errors_match_cruby.

describe "Integer#downto [stop] when self and stop are Integers" do
  it "does not yield when stop is greater than self" do
    result = []
    5.downto(6) { |x| result << x }
    assert_eq(result, [])
  end

  it "yields once when stop equals self" do
    result = []
    5.downto(5) { |x| result << x }
    assert_eq(result, [5])
  end

  it "yields while decreasing self until it is less than stop" do
    result = []
    5.downto(2) { |x| result << x }
    assert_eq(result, [5, 4, 3, 2])
  end

  it "yields while decreasing self until it is less than ceil for a Float endpoint" do
    result = []
    9.downto(1.3) {|i| result << i}
    3.downto(-1.3) {|i| result << i}
    assert_eq(result, [9, 8, 7, 6, 5, 4, 3, 2, 3, 2, 1, 0, -1])
  end

  it "raises an ArgumentError for invalid endpoints" do
    assert_raises("ArgumentError") { 1.downto("A") {|x| p x } }
    assert_raises("ArgumentError") { 1.downto(nil) {|x| p x } }
  end

  # skipped (method-not-implemented): no-block Enumerator surface.
  #
  # describe "when no block is given" do
  #   # ...
  # end
end
