# Adapted from ruby/spec core/hash/zip_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — Hash#zip
# inherits from Enumerable; pairs each `[k, v]` entry with
# the corresponding element from each arg Array.

describe "Hash#zip" do
  it "pairs each [k, v] entry with the corresponding arg element" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.zip([10, 20, 30]), [[[:a, 1], 10], [[:b, 2], 20], [[:c, 3], 30]])
  end

  it "fills nil when an arg is shorter than the receiver" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.zip([10, 20]), [[[:a, 1], 10], [[:b, 2], 20], [[:c, 3], nil]])
  end

  it "truncates at the receiver length when an arg is longer" do
    h = {a: 1, b: 2}
    assert_eq(h.zip([10, 20, 30, 40]), [[[:a, 1], 10], [[:b, 2], 20]])
  end

  it "wraps each pair in a singleton Array when called with no args" do
    h = {a: 1, b: 2}
    assert_eq(h.zip, [[[:a, 1]], [[:b, 2]]])
  end

  it "accepts multiple arg Arrays, filling per-arg nils on overrun" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(
      h.zip([10], [100]),
      [[[:a, 1], 10, 100], [[:b, 2], nil, nil], [[:c, 3], nil, nil]],
    )
  end

  it "returns [] on an empty Hash" do
    assert_eq({}.zip([1, 2, 3]), [])
  end

  # skipped (method-not-implemented): it "accepts a block (yields each tuple)" do
  #   `h.zip(args) { |tuple| ... }` — the block-form's
  #   return is nil and the block runs once per tuple.
  #   Out of subset; the no-block form covers the common
  #   case.

  # skipped (method-not-implemented): it "accepts Enumerable args (Range, Enumerator)" do
  #   `h.zip(1..3)` etc. requires Tier-2 Enumerator
  #   support; we currently restrict args to Array.
end
