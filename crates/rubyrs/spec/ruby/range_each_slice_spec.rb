# Adapted from ruby/spec core/range/each_slice_spec.rb /
# each_cons_spec.rb (Enumerable-inherited behaviour) at
# upstream commit 448cb340 (2026-05). Hand-translated — only
# Int+Int endpoints are supported (matches the
# iter_range_filter convention); Str+Str ranges fall through
# to NoMethodError and aren't covered here.

describe "Range#each_slice" do
  it "yields each consecutive group of n Ints as one Array" do
    seen = []
    (1..5).each_slice(2) { |s| seen << s }
    assert_eq(seen, [[1, 2], [3, 4], [5]])
  end

  it "returns the receiver Range (block form)" do
    r = (1..3)
    assert(r.each_slice(1) { |_| }.equal?(r))
  end

  it "honours an exclusive end" do
    seen = []
    (1...5).each_slice(2) { |s| seen << s }
    assert_eq(seen, [[1, 2], [3, 4]])
  end

  it "is a no-op on an empty (descending) Range" do
    r = (5..1)
    seen = []
    ret = r.each_slice(2) { |s| seen << s }
    assert_eq(seen, [])
    assert(ret.equal?(r))
  end

  it "raises ArgumentError when n <= 0" do
    assert_raises("ArgumentError") { (1..3).each_slice(0) { } }
    assert_raises("ArgumentError") { (1..3).each_slice(-1) { } }
  end

  it "honours break inside the block" do
    r = (1..10).each_slice(3) { |_| break :early }
    assert_eq(r, :early)
  end

  it "propagates non-local return from inside the block" do
    def self.range_es_with_return
      (1..10).each_slice(3) { |_| return :returned }
      :unreached
    end
    assert_eq(range_es_with_return, :returned)
  end

  it "no-block form: .to_a yields the same shape as the block form" do
    assert_eq((1..5).each_slice(2).to_a, [[1, 2], [3, 4], [5]])
  end

  it "exclusive end at i64::MIN is empty (no slice yielded)" do
    # The exclusive bound conversion uses checked_sub — saturating
    # subtraction would underflow back to min and yield one slice.
    m = -(2**63)
    seen = []
    (m...m).each_slice(1) { |s| seen << s }
    assert_eq(seen, [])
    assert_eq((m...m).each_slice(1).to_a, [])
  end
end

describe "Range#each_cons" do
  it "yields each sliding window of n consecutive Ints" do
    seen = []
    (1..4).each_cons(2) { |w| seen << w }
    assert_eq(seen, [[1, 2], [2, 3], [3, 4]])
  end

  it "returns the receiver Range (block form)" do
    r = (1..4)
    assert(r.each_cons(2) { |_| }.equal?(r))
  end

  it "yields nothing when range length < n" do
    seen = []
    (1..1).each_cons(2) { |w| seen << w }
    assert_eq(seen, [])
  end

  it "honours an exclusive end" do
    seen = []
    (1...5).each_cons(3) { |w| seen << w }
    assert_eq(seen, [[1, 2, 3], [2, 3, 4]])
  end

  it "raises ArgumentError when n <= 0" do
    assert_raises("ArgumentError") { (1..3).each_cons(0) { } }
    assert_raises("ArgumentError") { (1..3).each_cons(-1) { } }
  end

  it "honours break inside the block" do
    r = (1..10).each_cons(2) { |_| break :early }
    assert_eq(r, :early)
  end

  it "propagates non-local return from inside the block" do
    def self.range_ec_with_return
      (1..10).each_cons(2) { |_| return :returned }
      :unreached
    end
    assert_eq(range_ec_with_return, :returned)
  end

  it "no-block form: .to_a yields the same shape as the block form" do
    assert_eq((1..4).each_cons(2).to_a, [[1, 2], [2, 3], [3, 4]])
  end

  it "exclusive end at i64::MIN is empty (no window yielded)" do
    m = -(2**63)
    seen = []
    (m...m).each_cons(1) { |w| seen << w }
    assert_eq(seen, [])
    assert_eq((m...m).each_cons(1).to_a, [])
  end
end
