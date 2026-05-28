# Adapted from ruby/spec core/hash/flat_map_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated —
# baseline shape covers the (k, v) yield, the
# one-level Array flatten, and the `collect_concat` alias.
# Skipped upstream's shared-examples form and the no-block
# Enumerator return (ADR 0017 Tier-2).

describe "Hash#flat_map" do
  it "yields each (key, value) pair and one-level-flattens the result" do
    h = {a: 1, b: 2}
    assert_eq(h.flat_map { |k, v| [k, v] }, [:a, 1, :b, 2])
  end

  it "yields a single [k, v] Array per entry (matches Hash#each)" do
    # Single-param block should receive the whole pair, not
    # just the key. CRuby yields a single Array; `|k, v|`
    # auto-splats from it. Pins the convention pinned by
    # the Hash#each implementation in vm/iter.rs.
    h = {a: 1, b: 2}
    pairs = h.flat_map { |pair| [pair] }
    assert_eq(pairs, [[:a, 1], [:b, 2]])
  end

  it "pushes non-Array block returns as single elements" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.flat_map { |k, v| v * 2 }, [2, 4, 6])
  end

  it "returns an empty Array for an empty Hash" do
    assert_eq({}.flat_map { |k, v| [k] }, [])
  end

  it "is aliased as collect_concat" do
    h = {a: 1, b: 2}
    assert_eq(h.collect_concat { |k, v| [v] }, [1, 2])
  end

  it "honours `break` to bail out early with the break value" do
    h = {a: 1, b: 2}
    out = h.flat_map { |k, v| break :early }
    assert_eq(out, :early)
  end

  # skipped (method-not-implemented): it "returns an Enumerator when no block is given" do
  #   ADR 0017 Tier-2 — Enumerator not modelled.
end
