# Adapted from ruby/spec core/hash/take_while_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# Hash#take_while / #drop_while inherit from Enumerable;
# yield a single `[k, v]` Array per entry (matches the
# Hash#each + flat_map / sum / partition / one? convention).

describe "Hash#take_while" do
  it "returns the prefix where the block is truthy" do
    h = {a: 1, b: 2, c: 3, d: 4}
    assert_eq(h.take_while { |k, v| v < 3 }, [[:a, 1], [:b, 2]])
  end

  it "yields a single [k, v] Array per entry (single-param block)" do
    h = {a: 1, b: 2, c: 3}
    out = h.take_while { |pair| pair[1] < 3 }
    assert_eq(out, [[:a, 1], [:b, 2]])
  end

  it "stops at the first falsy and does not invoke the block after" do
    seen = []
    {a: 1, b: 2, c: 3, d: 4}.take_while { |k, v| seen << k; v < 2 }
    assert_eq(seen, [:a, :b])  # block ran for :a (kept), :b (stopped)
  end

  it "returns [] on an empty Hash" do
    assert_eq({}.take_while { |x| true }, [])
  end

  it "returns the whole Hash when block is always truthy" do
    h = {a: 1, b: 2}
    assert_eq(h.take_while { |k, v| true }, [[:a, 1], [:b, 2]])
  end

  it "honours `break` with the break value" do
    out = {a: 1, b: 2}.take_while { |k, v| break :early }
    assert_eq(out, :early)
  end
end

describe "Hash#drop_while" do
  it "returns the suffix after the first falsy" do
    h = {a: 1, b: 2, c: 3, d: 4}
    assert_eq(h.drop_while { |k, v| v < 3 }, [[:c, 3], [:d, 4]])
  end

  it "yields a single [k, v] Array per entry (single-param block)" do
    h = {a: 1, b: 2, c: 3}
    out = h.drop_while { |pair| pair[1] < 3 }
    assert_eq(out, [[:c, 3]])
  end

  it "returns [] on an empty Hash" do
    assert_eq({}.drop_while { |x| true }, [])
  end

  it "returns the whole Hash when block is always falsy on first entry" do
    h = {a: 1, b: 2}
    assert_eq(h.drop_while { |k, v| false }, [[:a, 1], [:b, 2]])
  end

  it "honours `break` with the break value" do
    out = {a: 1, b: 2}.drop_while { |k, v| break :stop }
    assert_eq(out, :stop)
  end
end
