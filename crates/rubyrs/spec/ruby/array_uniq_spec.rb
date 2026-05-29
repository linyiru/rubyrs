# Adapted from ruby/spec core/array/uniq_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — covers
# both the no-block and block forms.

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

  it "raises ArgumentError when called with positional args" do
    assert_raises("ArgumentError") { [1].uniq(99) }
  end

  it "with a block uses the block return as the uniqueness key" do
    # First-seen wins on collision. `|x| x.odd?` collapses
    # all odd numbers onto the first odd seen, and all even
    # onto the first even.
    assert_eq([1, 2, 3, 4].uniq { |x| x.odd? }, [1, 2])
  end

  it "with a block returns [] for an empty receiver" do
    assert_eq([].uniq { |x| x }, [])
  end

  it "with a block honours `break` with the break value" do
    out = [1, 2].uniq { |x| break :early }
    assert_eq(out, :early)
  end
end

describe "Array#uniq!" do
  it "mutates self in place and returns self when something was deduped" do
    a = [1, 2, 1, 3]
    r = a.uniq!
    assert(r.equal?(a))
    assert_eq(a, [1, 2, 3])
  end

  it "returns nil when nothing was deduped" do
    a = [1, 2, 3]
    assert_eq(a.uniq!, nil)
    # Sanity: no mutation.
    assert_eq(a, [1, 2, 3])
  end

  it "uses `eql?` (strict) — does NOT coerce Int and Float" do
    # Mirror Array#uniq's parity fix; uniq! must use the
    # same predicate.
    a = [1, 1.0]
    assert_eq(a.uniq!, nil)
    assert_eq(a, [1, 1.0])
  end

  it "raises ArgumentError when called with positional args" do
    assert_raises("ArgumentError") { [1].uniq!(99) }
  end
end

