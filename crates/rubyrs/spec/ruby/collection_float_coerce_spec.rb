# Adapted from ruby/spec core/array/each_slice_spec.rb (Float coerce semantics).

# Float-arg coercion for the each_slice / each_cons (1-arg)
# family added in PRs #311 / #312 / #316 / #323. CRuby
# truncates `each_slice(2.5)` to 2 (Integer cast). NaN /
# ±Infinity raise RangeError with the "float <label> out of
# range of integer" wording (note: short labels "NaN" / "Inf"
# / "-Inf", NOT the FloatDomainError-class "Infinity").
# Documented divergence in PR #330: this PR adds the missing
# coercion path so the family stays CRuby-compatible.

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "Array#each_slice / #each_cons Float coercion" do
  it "truncates Float arg toward zero (2.5 → 2)" do
    seen = []
    [1, 2, 3, 4, 5].each_slice(2.5) { |s| seen << s }
    assert_eq(seen, [[1, 2], [3, 4], [5]])
  end

  it "truncates not rounds (2.9 → 2)" do
    seen = []
    [1, 2, 3, 4, 5].each_slice(2.9) { |s| seen << s }
    assert_eq(seen, [[1, 2], [3, 4], [5]])
  end

  it "Float arg in no-block form: .to_a yields the same shape" do
    assert_eq([1, 2, 3, 4, 5].each_slice(2.5).to_a, [[1, 2], [3, 4], [5]])
  end

  it "rejects NaN with RangeError" do
    klass, msg = caught_pair { [1].each_slice(0.0 / 0.0) { |_| } }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float NaN out of range of integer")
  end

  it "rejects +Infinity with RangeError (short 'Inf' label)" do
    klass, msg = caught_pair { [1].each_slice(1.0 / 0.0) { |_| } }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float Inf out of range of integer")
  end

  it "rejects -Infinity with RangeError (short '-Inf' label)" do
    klass, msg = caught_pair { [1].each_cons(-1.0 / 0.0) { |_| } }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float -Inf out of range of integer")
  end
end

describe "Hash#each_slice / #each_cons Float coercion" do
  it "truncates Float arg toward zero (2.5 → 2)" do
    seen = []
    {a: 1, b: 2, c: 3, d: 4, e: 5}.each_slice(2.5) { |s| seen << s }
    assert_eq(seen.size, 3)
  end

  it "rejects NaN with RangeError (no-block)" do
    klass, msg = caught_pair { {a: 1}.each_slice(0.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float NaN out of range of integer")
  end
end

describe "Range#each_slice / #each_cons Float coercion" do
  it "truncates Float arg toward zero (2.5 → 2)" do
    seen = []
    (1..5).each_slice(2.5) { |s| seen << s }
    assert_eq(seen, [[1, 2], [3, 4], [5]])
  end

  it "rejects +Infinity with RangeError (no-block)" do
    klass, msg = caught_pair { (1..3).each_cons(1.0 / 0.0) }
    assert_eq(klass, "RangeError")
    assert_eq(msg, "float Inf out of range of integer")
  end
end
