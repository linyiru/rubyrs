
describe "Array#empty?" do
  it "returns true if the array has no elements" do
    assert([].empty?)
    assert(![1].empty?)
    assert(![1, 2].empty?)
  end
end
