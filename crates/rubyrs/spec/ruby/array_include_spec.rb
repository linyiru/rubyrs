describe "Array#include?" do
  it "returns true if object is present, false otherwise" do
    assert_eq([1, 2, "a", "b"].include?("c"), false)
    assert_eq([1, 2, "a", "b"].include?("a"), true)
  end

  # skipped (mock): it "determines presence by using element == obj" do

  # skipped (mock): it "calls == on elements from left to right until success" do
end
