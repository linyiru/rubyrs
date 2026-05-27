# Adapted from ruby/spec core/array/all_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — upstream uses
# only `it_behaves_like` against shared/iterable_and_tolerating_size_increasing.rb
# (`ScratchPad` mspec helper, not in subset). Bedrock with-block
# cases inlined here.

describe "Array#all?" do
  it "is true if the block returns true for every member" do
    assert_eq([1, 2, 3].all? { |x| x > 0 }, true)
  end

  it "is false if the block returns false for any member" do
    assert_eq([1, 2, 3].all? { |x| x > 1 }, false)
  end

  it "is true on an empty array (vacuous truth)" do
    assert_eq([].all? { |v| false }, true)
  end

  # skipped (fixture): it_behaves_like :array_iterable_and_tolerating_size_increasing
  #   Uses `ScratchPad` mspec helper.
  # skipped (method-not-implemented): it "ignores the block if there is an argument" do
  #   Pattern arg + `should complain(/.../)` matcher not in subset.
  # skipped (method-not-implemented): no-block default form
  #   `[].all?` / `[1, nil].all?` raises NoMethodError; only
  #   with-block path is dispatched.
end
