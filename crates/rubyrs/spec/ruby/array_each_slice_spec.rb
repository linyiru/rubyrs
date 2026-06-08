# Adapted from ruby/spec core/array/each_slice_spec.rb /
# each_cons_spec.rb (Enumerable-inherited behaviour) at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# no-block form returns a real Enumerator (via make_enum_for),
# exercised by the trailing `.to_a` example per group.

describe "Array#each_slice" do
  it "yields each consecutive group of n elements as one Array" do
    seen = []
    [1, 2, 3, 4, 5].each_slice(2) { |s| seen << s }
    assert_eq(seen, [[1, 2], [3, 4], [5]])
  end

  it "returns the receiver Array (block form)" do
    a = [1, 2, 3]
    assert(a.each_slice(1) { |_| }.equal?(a))
  end

  it "yields a single-element slice for a one-element receiver" do
    seen = []
    [42].each_slice(3) { |s| seen << s }
    assert_eq(seen, [[42]])
  end

  it "is a no-op on an empty Array" do
    a = []
    seen = []
    r = a.each_slice(2) { |s| seen << s }
    assert_eq(seen, [])
    assert(r.equal?(a))
  end

  it "raises ArgumentError when n <= 0" do
    assert_raises("ArgumentError") { [1].each_slice(0) { } }
    assert_raises("ArgumentError") { [1].each_slice(-1) { } }
  end

  it "honours break inside the block" do
    r = [1, 2, 3, 4].each_slice(2) { |_| break :early }
    assert_eq(r, :early)
  end

  it "propagates non-local return from inside the block" do
    def self.each_slice_with_return
      [1, 2, 3, 4].each_slice(2) { |_| return :returned }
      :unreached
    end
    assert_eq(each_slice_with_return, :returned)
  end

  it "no-block form: returns an Enumerator whose .to_a matches the block form" do
    assert_eq([1, 2, 3, 4, 5].each_slice(2).class.to_s, "Enumerator")
    assert_eq(
      [1, 2, 3, 4, 5].each_slice(2).to_a,
      [[1, 2], [3, 4], [5]]
    )
  end
end

describe "Array#each_cons" do
  it "yields each sliding window of n consecutive elements" do
    seen = []
    [1, 2, 3, 4].each_cons(2) { |w| seen << w }
    assert_eq(seen, [[1, 2], [2, 3], [3, 4]])
  end

  it "returns the receiver Array (block form)" do
    a = [1, 2, 3, 4]
    assert(a.each_cons(2) { |_| }.equal?(a))
  end

  it "yields nothing when receiver has fewer than n elements" do
    seen = []
    [1].each_cons(2) { |w| seen << w }
    assert_eq(seen, [])
  end

  it "raises ArgumentError when n <= 0" do
    assert_raises("ArgumentError") { [1].each_cons(0) { } }
    assert_raises("ArgumentError") { [1].each_cons(-1) { } }
  end

  it "honours break inside the block" do
    r = [1, 2, 3].each_cons(2) { |_| break :early }
    assert_eq(r, :early)
  end

  it "propagates non-local return from inside the block" do
    def self.each_cons_with_return
      [1, 2, 3].each_cons(2) { |_| return :returned }
      :unreached
    end
    assert_eq(each_cons_with_return, :returned)
  end

  it "no-block form: returns an Enumerator whose .to_a matches the block form" do
    assert_eq([1, 2, 3, 4].each_cons(2).class.to_s, "Enumerator")
    assert_eq(
      [1, 2, 3, 4].each_cons(2).to_a,
      [[1, 2], [2, 3], [3, 4]]
    )
  end
end
