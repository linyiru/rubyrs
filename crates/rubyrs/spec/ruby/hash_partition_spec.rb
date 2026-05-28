# Adapted from ruby/spec core/hash/partition_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline shape covers the (k, v) yield and the
# `[truthy_pairs, falsy_pairs]` return where each inner
# Array is a fresh `[k, v]` pair.

describe "Hash#partition" do
  it "yields a single [k, v] Array per entry (matches Hash#each)" do
    # Single-param block should receive the whole pair; the
    # predicate `pair[1].even?` would error if `pair` were
    # bound to just the key. Verifies the auto-splat
    # convention shared with flat_map / sum / one?.
    out = {a: 1, b: 2, c: 3}.partition { |pair| pair[1].even? }
    assert_eq(out, [[[:b, 2]], [[:a, 1], [:c, 3]]])
  end

  it "splits entries into [truthy_pairs, falsy_pairs]" do
    h = {a: 1, b: 2, c: 3, d: 4}
    out = h.partition { |k, v| v > 2 }
    assert_eq(out, [[[:c, 3], [:d, 4]], [[:a, 1], [:b, 2]]])
  end

  it "returns [[], []] on an empty Hash" do
    assert_eq({}.partition { |k, v| true }, [[], []])
  end

  it "places all entries in the truthy bucket when block is constantly true" do
    h = {a: 1, b: 2}
    assert_eq(h.partition { |k, v| true }, [[[:a, 1], [:b, 2]], []])
  end

  it "places all entries in the falsy bucket when block is constantly false" do
    h = {a: 1, b: 2}
    assert_eq(h.partition { |k, v| false }, [[], [[:a, 1], [:b, 2]]])
  end

  it "honours `break` with the break value" do
    h = {a: 1, b: 2}
    out = h.partition { |k, v| break :stop }
    assert_eq(out, :stop)
  end
end
