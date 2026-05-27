# Adapted from ruby/spec core/array/map_spec.rb +
# shared/collect.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — upstream delegates via `it_behaves_like
# :array_collect, :map` to shared/collect.rb. Three blocks from
# the shared body land here; the rest depend on `ArraySpecs::MyArray`
# subclass fixture, `Enumerator` (no-block form), or
# `it_should_behave_like :enumeratorized_with_origin_size`.
# The bang variant `Array#map!` is in a sibling describe upstream
# (`shared/collect_b.rb`) and not in this subset.

describe "Array#map" do
  it "returns a copy of array with each element replaced by the value returned by block" do
    a = ['a', 'b', 'c', 'd']
    b = a.map { |i| i + '!' }
    assert_eq(b, ["a!", "b!", "c!", "d!"])
    assert(!b.equal?(a))
  end

  it "does not change self" do
    a = ['a', 'b', 'c', 'd']
    a.map { |i| i + '!' }
    assert_eq(a, ['a', 'b', 'c', 'd'])
  end

  it "returns the evaluated value of block if it broke in the block" do
    a = ['a', 'b', 'c', 'd']
    b = a.map { |i|
      if i == 'c'
        break 0
      else
        i + '!'
      end
    }
    assert_eq(b, 0)
  end

  # skipped (fixture): it "does not return subclass instance" do
  #   Uses `ArraySpecs::MyArray` subclass fixture.
  # skipped (method-not-implemented): it "returns an Enumerator when no block given" do
  #   No-block form returns Enumerator (not in subset).
  # skipped (method-not-implemented): it "raises an ArgumentError when no block and with arguments" do
  #   Same shape — no-block path not in subset.
end
