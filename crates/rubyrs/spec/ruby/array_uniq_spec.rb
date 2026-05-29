# Adapted from ruby/spec core/array/uniq_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — baseline
# shapes for the no-block form. Block form lives in
# iter.rs (separate spec).

describe "Array#uniq" do
  it "returns a new Array with duplicates removed (first-seen wins)" do
    assert_eq([1, 1, 2, 3, 3, 4].uniq, [1, 2, 3, 4])
  end

  it "returns an empty Array for an empty receiver" do
    assert_eq([].uniq, [])
  end

  it "uses `eql?` (strict) — does NOT coerce Int and Float" do
    # CRuby parity: `Array#uniq` dedupes via `eql?`, which
    # is strict on numeric type (1.eql?(1.0) == false).
    # Pinned to prevent regression to `==`-based dedup
    # which would collapse [1, 1.0] to [1].
    assert_eq([1, 1.0].uniq, [1, 1.0])
    assert_eq([1, "1"].uniq, [1, "1"])
  end

  it "preserves insertion order" do
    assert_eq([3, 1, 2, 1, 3, 2].uniq, [3, 1, 2])
  end

  it "dedupes bit-identical Float::NAN (same-NaN identity shortcut)" do
    # CRuby's Float#eql?(NaN, NaN) is actually false, but
    # Array#uniq dedup uses an identity shortcut (same
    # object short-circuits to equal). rubyrs has
    # value-based Floats so bit-identical NaN is treated
    # as eql? for dedup purposes — matches the common
    # CRuby same-object case.
    #
    # `assert_eq` compares via `==`, which is false for
    # NaN-vs-NaN, so we assert via `.size` + element-type
    # checks instead of direct Array equality.
    nan = 0.0 / 0.0
    out = [nan, nan, 1, 2].uniq
    assert_eq(out.size, 3)
    assert(out[0].is_a?(Float) && out[0].nan?)
    assert_eq(out[1], 1)
    assert_eq(out[2], 2)
  end

  # skipped (method-not-implemented): it "with a block uses block return as the uniqueness key" do
  #   `[1, 2, 3].uniq { |x| x.odd? }` — block form lives
  #   in iter.rs and has separate spec coverage.
end
