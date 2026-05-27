# Adapted from ruby/spec core/array/any_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — upstream uses
# nested `describe` groups + shared bodies. Two flattened
# describes here:
#   - block-form `any? { |v| ... }` (covered)
#   - no-block default (skipped — `[].any?` raises NoMethodError;
#     rubyrs implements only the with-block path)

describe "Array#any?" do
  it "is false if the array is empty (with block)" do
    assert_eq([].any? { |v| 1 == 1 }, false)
  end

  it "is true if the block returns true for any member of the array" do
    array_with_members = [false, false, true, false]
    assert_eq(array_with_members.any? { |v| v == true }, true)
  end

  it "is false if the block returns false for all members of the array" do
    array_with_members = [false, false, true, false]
    assert_eq(array_with_members.any? { |v| v == 42 }, false)
  end

  # skipped (method-not-implemented): describe 'with no block given' do ... end
  #   `[].any?` / `[false, nil].any?` / `[nil, "x"].any?` — the
  #   no-block default-truthy-check form raises NoMethodError in
  #   rubyrs (only `("any?", [])` block-dispatch arm in
  #   iter.rs:2273 is wired; no-block path falls through).
  # skipped (fixture): it_behaves_like :array_iterable_and_tolerating_size_increasing
  #   Uses `ScratchPad` mspec helper.
  # skipped (method-not-implemented): describe 'when given a pattern argument' do
  #   Uses `should complain(/.../)` matcher (mspec internals).
end
