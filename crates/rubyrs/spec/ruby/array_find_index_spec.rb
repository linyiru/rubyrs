# Adapted from ruby/spec core/array/index_spec.rb +
# find_index_spec.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — Array#index and Array#find_index are
# aliases (CRuby parity); the no-arg-no-block form returns an
# Enumerator (modelled via make_enum_for), exercised below.

describe "Array#find_index" do
  it "returns the Int index of the first element == the argument" do
    assert_eq([1, 2, 3, 4].find_index(2), 1)
    assert_eq([1, 2, 3, 2].find_index(2), 1)
  end

  it "returns nil when no element matches" do
    assert_eq([1, 2, 3].find_index(99), nil)
    assert_eq([].find_index(:anything), nil)
  end

  it "uses == (not eql?) — Int/Float cross-type matches" do
    # 1 == 1.0 is true; eql? would be false. CRuby uses ==.
    assert_eq([1, 2, 3].find_index(1.0), 0)
  end

  it "matches nil elements" do
    assert_eq([nil, 1, nil].find_index(nil), 0)
    assert_eq([1, 2].find_index(nil), nil)
  end

  it "compares nested arrays structurally" do
    assert_eq([[1], [2], [3]].find_index([2]), 1)
  end

  it "with a block: returns the index of the first truthy block result" do
    assert_eq([1, 2, 3, 4].find_index { |x| x > 2 }, 2)
    assert_eq([1, 2, 3, 4].find_index { |x| x > 100 }, nil)
  end

  it "with both an arg and a block: uses the arg (block discarded)" do
    # CRuby emits `warning: given block not used` but still
    # honours the positional arg. rubyrs skips the warning
    # but preserves the return-value behaviour.
    assert_eq([1, 2, 3].find_index(2) { |x| x > 99 }, 1)
  end

  it "honours break inside the block" do
    assert_eq([1, 2, 3].find_index { |x| break :early if x == 2 }, :early)
  end

  it "raises ArgumentError when given more than one argument" do
    assert_raises("ArgumentError") { [1].find_index(1, 2) }
  end

  it "returns an Enumerator when called with no arg and no block" do
    # CRuby (and now rubyrs, via make_enum_for) returns an Enumerator;
    # driving it with a block reports the first truthy index, exactly
    # like the direct block form.
    e = [10, 20, 30].find_index
    assert_eq(e.class.to_s, "Enumerator")
    assert_eq(e.each { |x| x == 20 }, 1)
    assert_eq([10, 20, 30].find_index.each { |x| x > 100 }, nil)
  end
end

describe "Array#index" do
  it "is an alias for find_index (positional form)" do
    assert_eq([1, 2, 3, 2].index(2), 1)
    assert_eq([1, 2, 3].index(99), nil)
  end

  it "is an alias for find_index (block form)" do
    assert_eq([10, 20, 30].index { |x| x >= 20 }, 1)
  end
end
