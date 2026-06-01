# Adapted from ruby/spec core/range/chunk_while_spec.rb
# (Enumerable-inherited behaviour) at upstream commit 448cb340
# (2026-06). Hand-translated — only Int+Int endpoints are
# supported (matches Range#each_slice/#each_cons from PR #316
# and the iter_range_filter convention); Str+Str raises
# RuntimeError per the lockstep contract.

describe "Range#chunk_while" do
  it "partitions into runs where the block (called with adjacent Ints) is truthy" do
    r = (1..6).chunk_while { |a, b| b - a == 1 }
    assert_eq(r, [[1, 2, 3, 4, 5, 6]])
  end

  it "returns a chunk per element when the block is always falsy" do
    r = (1..4).chunk_while { |_a, _b| false }
    assert_eq(r, [[1], [2], [3], [4]])
  end

  it "returns a single chunk when the block is always truthy" do
    r = (1..4).chunk_while { |_a, _b| true }
    assert_eq(r, [[1, 2, 3, 4]])
  end

  it "yields prev=a and cur=b as two separate args" do
    seen = []
    (1..3).chunk_while { |a, b| seen << [a, b]; true }
    assert_eq(seen, [[1, 2], [2, 3]])
  end

  it "honours an exclusive end" do
    r = (1...4).chunk_while { |a, b| b - a == 1 }
    assert_eq(r, [[1, 2, 3]])
  end

  it "returns [] on an empty (descending) Range" do
    assert_eq((5..1).chunk_while { true }, [])
  end

  it "returns [[elem]] on a single-element Range (block never invoked)" do
    seen = []
    r = (1..1).chunk_while { |a, b| seen << [a, b]; false }
    assert_eq(r, [[1]])
    assert_eq(seen, [])
  end

  it "honours break inside the block" do
    r = (1..10).chunk_while { |_a, _b| break :early }
    assert_eq(r, :early)
  end

  it "propagates non-local return from inside the block" do
    def self.range_cw_with_return
      (1..10).chunk_while { |_a, _b| return :returned }
      :unreached
    end
    assert_eq(range_cw_with_return, :returned)
  end

  it "non-Int endpoints raise (not NoMethodError) to keep respond_to? consistent" do
    # `('a'..'z').respond_to?(:chunk_while)` is true via the
    # Range whitelist. Same fallback as each_slice/each_cons
    # (PR #316) — raise RuntimeError so the surface matches
    # respond_to?.
    caught = nil
    begin
      ('a'..'e').chunk_while { |_a, _b| true }
    rescue => e
      caught = e.class.to_s
    end
    assert_eq(caught, "RuntimeError")
  end
end
