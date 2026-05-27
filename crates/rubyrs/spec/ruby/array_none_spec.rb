# Adapted from ruby/spec core/array/none_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — same shape as
# array_all_spec.rb: upstream uses only the shared body, which
# depends on ScratchPad. Bedrock with-block cases inlined.

describe "Array#none?" do
  it "is true if the block returns false for every member" do
    assert_eq([1, 2, 3].none? { |x| x > 10 }, true)
  end

  it "is false if the block returns true for any member" do
    assert_eq([1, 2, 3].none? { |x| x > 2 }, false)
  end

  it "is true on an empty array (vacuous truth)" do
    assert_eq([].none? { |v| true }, true)
  end

  # skipped (fixture): it_behaves_like :array_iterable_and_tolerating_size_increasing
  # skipped (method-not-implemented): it "ignores the block if there is an argument" do
  # skipped (method-not-implemented): no-block default form (`[].none?` raises NoMethodError)
end
