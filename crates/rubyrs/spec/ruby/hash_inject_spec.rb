# Adapted from ruby/spec core/hash/inject_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline shape covers both arity forms of the block-given
# variant. The Symbol-only form (`reduce(:+)` /
# `reduce(0, :+)`) is dropped — rubyrs doesn't yet route
# Symbol#to_proc on Enumerable reductions.

describe "Hash#inject" do
  it "with initial value, threads acc through every (k, v) pair" do
    h = {a: 1, b: 2, c: 3}
    out = h.inject(0) { |acc, (k, v)| acc + v }
    assert_eq(out, 6)
  end

  it "with initial value and a non-numeric seed" do
    h = {a: 1, b: 2}
    out = h.inject([]) { |acc, (k, v)| acc << k }
    assert_eq(out, [:a, :b])
  end

  it "without initial value, seeds acc from the first (k, v) pair as a [k, v] Array" do
    h = {a: 1, b: 2}
    out = h.inject { |acc, (k, v)| acc }
    assert_eq(out, [:a, 1])
  end

  it "returns nil on an empty Hash without an initial value" do
    assert_eq({}.inject { |a, b| a }, nil)
  end

  it "returns the initial value on an empty Hash" do
    assert_eq({}.inject(42) { |a, b| a + 1 }, 42)
  end

  it "is aliased as #reduce" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.reduce(0) { |acc, (k, v)| acc + v }, 6)
  end

  it "honours `break` with the break value" do
    h = {a: 1, b: 2}
    out = h.reduce(0) { |a, (k, v)| break :stop }
    assert_eq(out, :stop)
  end

  # skipped (method-not-implemented): it "with a Symbol method-name argument" do
  #   `h.reduce(:+)` / `h.reduce(0, :+)`. Requires Symbol
  #   #to_proc on the reduction path — separate slice.
end
