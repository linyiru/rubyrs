# Adapted from ruby/spec core/hash/each_slice_spec.rb /
# each_cons_spec.rb / chunk_while_spec.rb (Enumerable-inherited
# behaviour) at upstream commit 448cb340 (2026-05).
# Hand-translated — the no-block/no-arg Enumerator form is
# folded into a single "returns the same shape as .to_a" test
# (rubyrs returns the materialised Array directly; see comment
# at hash.rs:432).

describe "Hash#each_slice" do
  it "yields each consecutive group of n [k,v] pair Arrays as one Array" do
    seen = []
    {a: 1, b: 2, c: 3, d: 4, e: 5}.each_slice(2) { |s| seen << s }
    assert_eq(seen, [[[:a, 1], [:b, 2]], [[:c, 3], [:d, 4]], [[:e, 5]]])
  end

  it "returns the receiver Hash (block form)" do
    h = {a: 1, b: 2}
    assert(h.each_slice(1) { |_| }.equal?(h))
  end

  it "yields a single-element slice for a single-pair receiver" do
    seen = []
    {a: 1}.each_slice(3) { |s| seen << s }
    assert_eq(seen, [[[:a, 1]]])
  end

  it "is a no-op on an empty Hash" do
    h = {}
    seen = []
    r = h.each_slice(2) { |s| seen << s }
    assert_eq(seen, [])
    assert(r.equal?(h))
  end

  it "raises ArgumentError when n <= 0" do
    assert_raises("ArgumentError") { {a: 1}.each_slice(0) { } }
    assert_raises("ArgumentError") { {a: 1}.each_slice(-1) { } }
  end

  it "honours break inside the block" do
    r = {a: 1, b: 2, c: 3}.each_slice(2) { |_| break :early }
    assert_eq(r, :early)
  end

  # Skipped (method-not-implemented): no-block-form returns an
  # Enumerator in CRuby; rubyrs returns the materialised Array
  # directly. `.to_a` on the result is a no-op so the canonical
  # `h.each_slice(2).to_a` idiom still works.
end

describe "Hash#each_cons" do
  it "yields each sliding window of n [k,v] pair Arrays" do
    seen = []
    {a: 1, b: 2, c: 3}.each_cons(2) { |w| seen << w }
    assert_eq(seen, [[[:a, 1], [:b, 2]], [[:b, 2], [:c, 3]]])
  end

  it "returns the receiver Hash (block form)" do
    h = {a: 1, b: 2, c: 3}
    assert(h.each_cons(2) { |_| }.equal?(h))
  end

  it "yields nothing when receiver has fewer than n pairs" do
    seen = []
    {a: 1}.each_cons(2) { |w| seen << w }
    assert_eq(seen, [])
  end

  it "raises ArgumentError when n <= 0" do
    assert_raises("ArgumentError") { {a: 1}.each_cons(0) { } }
    assert_raises("ArgumentError") { {a: 1}.each_cons(-1) { } }
  end

  it "honours break inside the block" do
    r = {a: 1, b: 2, c: 3}.each_cons(2) { |_| break :early }
    assert_eq(r, :early)
  end
end

describe "Hash#chunk_while" do
  it "partitions into runs where the block (called with adjacent pair Arrays) is truthy" do
    h = {a: 1, b: 2, c: 5, d: 6}
    r = h.chunk_while { |prev, cur| cur[1] - prev[1] == 1 }
    assert_eq(r, [[[:a, 1], [:b, 2]], [[:c, 5], [:d, 6]]])
  end

  it "yields prev=pair[i] and cur=pair[i+1] as two separate args" do
    seen = []
    {a: 1, b: 2, c: 3}.chunk_while { |a, b| seen << [a, b]; true }
    assert_eq(seen, [[[:a, 1], [:b, 2]], [[:b, 2], [:c, 3]]])
  end

  it "returns a single chunk when the block is always truthy" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.chunk_while { true }, [[[:a, 1], [:b, 2], [:c, 3]]])
  end

  it "returns a chunk per pair when the block is always falsy" do
    h = {a: 1, b: 2, c: 3}
    assert_eq(h.chunk_while { false }, [[[:a, 1]], [[:b, 2]], [[:c, 3]]])
  end

  it "returns [] on empty Hash" do
    assert_eq({}.chunk_while { true }, [])
  end

  it "returns [[[k,v]]] on single-pair Hash (block never invoked)" do
    assert_eq({a: 1}.chunk_while { false }, [[[:a, 1]]])
  end
end
