
describe "Array#size" do
  it "returns the number of elements" do
    assert_eq([].send(:size), 0)
    assert_eq([1, 2, 3].send(:size), 3)
  end

  # skipped (fixture): it "properly handles recursive arrays" do
end
