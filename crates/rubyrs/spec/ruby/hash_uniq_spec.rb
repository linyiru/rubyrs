# Adapted from ruby/spec core/hash/uniq_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — Hash#uniq
# inherits from Enumerable; returns Array<[k, v]>.

describe "Hash#uniq" do
  it "without a block returns all entries (Hash keys are eql?-unique by construction)" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.uniq, [[:a, 1], [:b, 2], [:c, 3]])
  end

  it "returns [] on an empty receiver" do
    assert_eq({}.uniq, [])
  end

  it "with a block uses the block return as the uniqueness key" do
    # First-seen wins. `{a:1, b:1, c:2}.uniq { |k, v| v }`
    # collapses :a and :b (both yield 1), keeping :a.
    h = {a: 1, b: 1, c: 2}
    assert_eq(h.uniq { |k, v| v }, [[:a, 1], [:c, 2]])
  end

  it "yields a single [k, v] Array per entry (single-param block)" do
    h = {a: 1, b: 2}
    pairs = []
    h.uniq { |pair| pairs << pair; pair }
    assert_eq(pairs, [[:a, 1], [:b, 2]])
  end

  it "honours `break` with the break value" do
    out = {a: 1, b: 2}.uniq { |pair| break :early }
    assert_eq(out, :early)
  end

  it "raises ArgumentError when called with positional args (no-block)" do
    assert_raises("ArgumentError") { {a: 1}.uniq(1) }
  end

  it "raises ArgumentError when called with positional args + block" do
    assert_raises("ArgumentError") { {a: 1}.uniq(1) { |p| p } }
  end

  it "accepts a Symbol#to_proc block" do
    # &:last on a pair Array `[k, v]` returns v — common
    # idiom for uniqueness-by-value. Locks in Symbol#to_proc
    # compatibility with the block-form arm.
    h = {a: 1, b: 1, c: 2}
    assert_eq(h.uniq(&:last), [[:a, 1], [:c, 2]])
  end

  it "does NOT dedupe Float::NAN keys (rubyrs ruby_eql divergence)" do
    # Divergent from CRuby: Float#eql?(NaN, NaN) is true in
    # CRuby (structural equality), so a Hash whose block
    # returns NaN for every entry deduplicates to one. In
    # rubyrs, ruby_eql for two Floats falls through to Rust
    # `a == b`, which is false for NaN — so every NaN-keyed
    # entry survives. Pinning the current behaviour; the
    # broader ruby_eql Float-NaN gap is tracked as a
    # separate follow-up.
    nan = 0.0 / 0.0
    r = {a: nan, b: nan}.uniq { |pair| pair.last }
    assert_eq(r.size, 2)  # CRuby returns 1
  end
end
