# Adapted from ruby/spec core/hash/any_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the no-block
# and with-block forms are inlined. The pattern-arg form
# (`h.any?(MatchPattern)`) is dropped.

describe "Hash#any?" do
  it "returns true when the hash has at least one pair" do
    assert_eq({ a: 1 }.any?, true)
    assert_eq({ a: 1, b: 2 }.any?, true)
  end

  it "returns false when the hash is empty" do
    assert_eq({}.any?, false)
  end

  it "returns true when the block is truthy for some pair" do
    assert_eq({ a: 1, b: 2 }.any? { |k, v| v > 1 }, true)
  end

  it "returns false when the block is falsy for all pairs" do
    assert_eq({ a: 1, b: 2 }.any? { |k, v| v > 5 }, false)
  end

  it "yields a single [k, v] Array per entry (Enumerable shape)" do
    # Hash#any? inherits from Enumerable, so the block
    # receives a single `[k, v]` pair Array. `|k, v|`
    # blocks auto-splat from it; `|pair|` blocks bind to
    # the whole Array. (Contrast with Hash#select /
    # #reject which override Enumerable and yield two
    # separate args — see hash_select_spec.)
    h = { a: 1, b: 2 }
    seen = []
    h.any? { |pair| seen << pair; false }
    assert_eq(seen, [[:a, 1], [:b, 2]])
    # Two-param block still works via auto-splat:
    seen2 = []
    h.any? { |k, v| seen2 << [k, v]; false }
    assert_eq(seen2, [[:a, 1], [:b, 2]])
  end

  # skipped (method-not-implemented): pattern-arg form `h.any?(pat)`.
end
