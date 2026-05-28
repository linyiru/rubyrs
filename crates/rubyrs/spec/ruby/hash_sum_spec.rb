# Adapted from ruby/spec core/hash/sum_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — baseline
# shape covers the (k, v) yield, the default Int(0) seed,
# and an explicit Int initial value. The no-block form
# (`{}.sum`) raises in CRuby because Hash entries are
# `[k, v]` pairs and adding Arrays-of-mixed-type to 0
# doesn't make sense — out of subset here too.

describe "Hash#sum" do
  it "yields each (k, v) and sums the block return values" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.sum { |k, v| v }, 6)
  end

  it "supports an Int initial value" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.sum(100) { |k, v| v }, 106)
  end

  it "returns the initial value on an empty Hash" do
    assert_eq({}.sum { |k, v| v }, 0)
    assert_eq({}.sum(42) { |k, v| v }, 42)
  end

  bignum_it "promotes Int to BigInt on overflow" do
    # `apply_int_promote` path: when an Int+Int sum
    # overflows i64, the result widens to BigInt. Use a
    # block that yields a large per-entry contribution.
    h = {a: 1, b: 2}
    out = h.sum { |k, v| 4_611_686_018_427_387_904 }  # 2^62
    # 2 * 2^62 = 2^63 — overflows i64::MAX (= 2^63 - 1).
    assert_eq(out, 9_223_372_036_854_775_808)
  end

  it "honours `break` with the break value" do
    h = {a: 1, b: 2}
    out = h.sum { |k, v| break :ss }
    assert_eq(out, :ss)
  end

  # skipped (method-not-implemented): it "without a block, sums the [k, v] pairs directly" do
  #   `{a: 1}.sum` raises TypeError in CRuby because
  #   `0 + [:a, 1]` is undefined; the no-block form only
  #   works for Hashes-of-Numerics-pairs, which is an
  #   edge case not modelled here.
end
