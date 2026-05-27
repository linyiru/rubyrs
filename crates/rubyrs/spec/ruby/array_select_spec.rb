# Adapted from ruby/spec core/array/select_spec.rb +
# shared/select.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — upstream delegates via `it_behaves_like
# :array_select, :select` to shared/select.rb. The runnable
# block is inlined; the rest depend on `ArraySpecs::MyArray`
# subclass / fixture-recursive arrays, Enumerator (no-block
# form), or the `select!` mutator sibling describe.

describe "Array#select" do
  it "returns a new array of elements for which block is true" do
    result = [1, 3, 4, 5, 6, 9].select { |i| i % ((i + 1) / 2) == 0 }
    assert_eq(result, [1, 4, 6])
  end

  # skipped (fixture): it "does not return subclass instance on Array subclasses" do
  #   Uses `ArraySpecs::MyArray`.
  # skipped (fixture): it "properly handles recursive arrays" do
  #   Uses `ArraySpecs.empty_recursive_array`.
  # skipped (method-not-implemented): it "returns an Enumerator when no block given" do
  # skipped (method-not-implemented): describe "Array#select!" do ... end
  #   In-place mutator + Enumerator path not in subset.
end
