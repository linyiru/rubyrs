# Adapted from ruby/spec core/hash/tally_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — Hash#tally
# inherits from Enumerable; returns Hash<element, count>.
# For a Hash receiver, every entry is a unique `[k, v]` pair
# (Hash keys are eql?-unique), so every count is 1.

describe "Hash#tally" do
  it "returns a Hash counting each [k, v] pair" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.tally, {[:a, 1] => 1, [:b, 2] => 1, [:c, 3] => 1})
  end

  it "every count is 1 because Hash entries are eql?-unique by construction" do
    # Even when values collide, the pair (with the unique key)
    # is still unique — tally sees one occurrence each.
    h = {a: 1, b: 1, c: 1}
    assert_eq(h.tally, {[:a, 1] => 1, [:b, 1] => 1, [:c, 1] => 1})
  end

  it "returns an empty Hash on an empty receiver" do
    assert_eq({}.tally, {})
  end

  it "raises ArgumentError when called with an arg (accumulating form unsupported)" do
    # CRuby's Ruby 2.7+ `h.tally(target_hash)` form is out
    # of subset. Explicit guard avoids the silent
    # NoMethodError from respond_to? widening.
    assert_raises("ArgumentError") { {a: 1}.tally({}) }
  end

  # skipped (method-not-implemented): it "accepts a hash argument to fold into" do
  #   `h.tally(target_hash)` (Ruby 2.7+) merges counts into
  #   the given Hash. Out of subset; the no-arg form covers
  #   the common case.
end
