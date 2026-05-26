
describe "Array#length" do
  it "returns the number of elements" do
    assert_eq([].send(:length), 0)
    assert_eq([1, 2, 3].send(:length), 3)
  end

  # skipped (fixture-dependent): it "properly handles recursive arrays" do
end
