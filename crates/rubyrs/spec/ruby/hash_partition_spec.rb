# Adapted from ruby/spec core/hash/partition_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline shape covers the (k, v) yield and the
# `[truthy_pairs, falsy_pairs]` return where each inner
# Array is a fresh `[k, v]` pair.

describe "Hash#partition" do
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
