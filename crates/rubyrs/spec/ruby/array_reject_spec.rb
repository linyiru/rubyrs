# Adapted from ruby/spec core/array/reject_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — first block of
# the main describe is inlined. The rest depend on
# `ArraySpecs.empty_recursive_array` / `MyArray`,
# `instance_variable_set` (not in subset), Enumerator, or the
# `reject!` sibling describe.

describe "Array#reject" do
  it "returns a new array without elements for which block is true" do
    ary = [1, 2, 3, 4, 5]
    assert_eq(ary.reject { true }, [])
    assert_eq(ary.reject { false }, ary)
    assert(!ary.reject { false }.equal?(ary))
    assert_eq(ary.reject { nil }, ary)
    assert(!ary.reject { nil }.equal?(ary))
    assert_eq(ary.reject { 5 }, [])
    assert_eq(ary.reject { |i| i < 3 }, [3, 4, 5])
    assert_eq(ary.reject { |i| i % 2 == 0 }, [1, 3, 5])
  end

  it "returns self when called on an Array emptied with #shift" do
    array = [1]
    array.shift
    assert_eq(array.reject { |x| true }, [])
  end

  # skipped (fixture): it "properly handles recursive arrays" do
  # skipped (fixture): it "does not return subclass instance on Array subclasses" do
  # skipped (method-not-implemented): it "does not retain instance variables" do
  #   `instance_variable_set` / `instance_variable_get` not in subset.
  # skipped (method-not-implemented): describe "Array#reject!" do ... end
  #   In-place mutator path not in subset.
end
