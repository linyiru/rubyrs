# Adapted from ruby/spec core/array/compact_spec.rb at
# upstream commit 448cb340 (2026-05). 4th extractor-derived
# spec — produced by `rubyrs-spec-extract` v0.3. Two blocks
# commented for fixture-class references the micro-runner
# can't resolve (`ArraySpecs::MyArray`, `ArraySpecs.frozen_array`).

describe "Array#compact" do
  it "returns a copy of array with all nil elements removed" do
    a = [1, 2, 4]
    assert_eq(a.compact, [1, 2, 4])
    a = [1, nil, 2, 4]
    assert_eq(a.compact, [1, 2, 4])
    a = [1, 2, 4, nil]
    assert_eq(a.compact, [1, 2, 4])
    a = [nil, 1, 2, 4]
    assert_eq(a.compact, [1, 2, 4])
  end

  it "does not return self" do
    a = [1, 2, 3]
    assert(!a.compact.equal?(a))
  end

  # Skipped — upstream compact_spec.rb:23 uses
  # `ArraySpecs::MyArray` subclass fixture; not vendored.
  #
  # it "does not return subclass instance for Array subclasses" do
  #   assert(ArraySpecs::MyArray[1, 2, 3, nil].compact.instance_of?(Array))
  # end
end

describe "Array#compact!" do
  it "removes all nil elements" do
    a = ['a', nil, 'b', false, 'c']
    assert(a.compact!.equal?(a))
    assert_eq(a, ["a", "b", false, "c"])
    a = [nil, 'a', 'b', false, 'c']
    assert(a.compact!.equal?(a))
    assert_eq(a, ["a", "b", false, "c"])
    a = ['a', 'b', false, 'c', nil]
    assert(a.compact!.equal?(a))
    assert_eq(a, ["a", "b", false, "c"])
  end

  it "returns self if some nil elements are removed" do
    a = ['a', nil, 'b', false, 'c']
    assert(a.compact!.equal? a)
  end

  it "returns nil if there are no nil elements to remove" do
    assert_eq([1, 2, false, 3].compact!, nil)
  end

  # Skipped — upstream compact_spec.rb:51 uses
  # `ArraySpecs.frozen_array` fixture; not vendored.
  #
  # it "raises a FrozenError on a frozen array" do
  #   assert_raises("FrozenError") do
  #     ArraySpecs.frozen_array.compact!
  #   end
  # end
end
