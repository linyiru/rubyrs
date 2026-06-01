# Adapted from ruby/spec core/array/each_slice_spec.rb (error-class assertions).

# Cross-receiver wrong-arity / non-Int-arg sweep for the
# each_slice / each_cons (1-arg) and chunk_while (0-arg)
# family added in PRs #311 / #312 / #316 / #323. Before this
# sweep all six methods fell through to NoMethodError for
# wrong shapes, contradicting `respond_to?` which returns
# true unconditionally. CRuby raises ArgumentError /
# TypeError; rubyrs now matches both class and message
# wording (Float arg remains divergent — CRuby coerces,
# rubyrs raises TypeError; a follow-up PR can add coercion).

def caught_pair(&blk)
  blk.call
  [nil, nil]
rescue => e
  [e.class.to_s, e.message]
end

describe "Array#each_slice / #each_cons arity & type guards" do
  it "raises ArgumentError on multi-arg (block form)" do
    klass, msg = caught_pair { [1].each_slice(2, 3) { |_| } }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 2, expected 1)")
  end

  it "raises ArgumentError on zero-arg (no-block form)" do
    klass, msg = caught_pair { [1].each_slice }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 0, expected 1)")
  end

  it "raises TypeError on String arg" do
    klass, msg = caught_pair { [1].each_slice("2") { |_| } }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end

  it "raises TypeError on Symbol arg (each_cons)" do
    klass, msg = caught_pair { [1].each_cons(:two) { |_| } }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Symbol into Integer")
  end

  it "raises TypeError on nil arg with different wording" do
    klass, msg = caught_pair { [1].each_slice(nil) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion from nil to integer")
  end
end

describe "Hash#each_slice / #each_cons arity & type guards" do
  it "raises ArgumentError on multi-arg" do
    klass, msg = caught_pair { {a: 1}.each_slice(2, 3) { |_| } }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 2, expected 1)")
  end

  it "raises TypeError on String arg (no-block)" do
    klass, msg = caught_pair { {a: 1}.each_cons("2") }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of String into Integer")
  end
end

describe "Range#each_slice / #each_cons arity & type guards" do
  it "raises ArgumentError on multi-arg" do
    klass, msg = caught_pair { (1..3).each_slice(2, 3) { |_| } }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 2, expected 1)")
  end

  it "raises TypeError on Symbol arg (no-block)" do
    klass, msg = caught_pair { (1..3).each_cons(:two) }
    assert_eq(klass, "TypeError")
    assert_eq(msg, "no implicit conversion of Symbol into Integer")
  end
end

describe "chunk_while arity guards (0-arg method)" do
  it "raises ArgumentError on Array#chunk_while(1)" do
    klass, msg = caught_pair { [1].chunk_while(2) { |_a, _b| true } }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 1, expected 0)")
  end

  it "raises ArgumentError on Hash#chunk_while(1)" do
    klass, msg = caught_pair { {a: 1}.chunk_while(2) { |_a, _b| true } }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 1, expected 0)")
  end

  it "raises ArgumentError on Range#chunk_while(1)" do
    klass, msg = caught_pair { (1..3).chunk_while(2) { |_a, _b| true } }
    assert_eq(klass, "ArgumentError")
    assert_eq(msg, "wrong number of arguments (given 1, expected 0)")
  end
end
