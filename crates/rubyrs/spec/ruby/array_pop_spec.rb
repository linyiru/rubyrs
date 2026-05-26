describe "Array#pop" do
  it "removes and returns the last element of the array" do
    a = ["a", 1, nil, true]

    assert_eq(a.pop, true)
    assert_eq(a, ["a", 1, nil])

    assert_eq(a.pop, nil)
    assert_eq(a, ["a", 1])

    assert_eq(a.pop, 1)
    assert_eq(a, ["a"])

    assert_eq(a.pop, "a")
    assert_eq(a, [])
  end

  it "returns nil if there are no more elements" do
    assert_eq([].pop, nil)
  end

  # skipped (fixture): it "properly handles recursive arrays" do

  # skipped (fixture): it "raises a FrozenError on a frozen array" do

  # skipped (fixture): it "raises a FrozenError on an empty frozen array" do

  describe "passed a number n as an argument" do
    # skipped (method-not-implemented): it "removes and returns an array with the last n elements of the array" do

    # skipped (method-not-implemented): it "returns an array with the last n elements even if shift was invoked" do

    # skipped (method-not-implemented): it "returns a new empty array if there are no more elements" do

    # skipped (method-not-implemented): it "returns whole elements if n exceeds size of the array" do

    # skipped (method-not-implemented): it "does not return self even when it returns whole elements" do

    # skipped (method-not-implemented): it "raises an ArgumentError if n is negative" do

    # skipped (mock): it "tries to convert n to an Integer using #to_int" do

    # skipped (method-not-implemented): it "raises a TypeError when the passed n cannot be coerced to Integer" do

    # skipped (method-not-implemented): it "raises an ArgumentError if more arguments are passed" do

    # skipped (fixture): it "does not return subclass instances with Array subclass" do

    # skipped (fixture): it "raises a FrozenError on a frozen array" do
  end
end
