describe "Array#length" do
  it "diverges from a wrong expectation" do
    assert_neq([1, 2, 3].length, 99)
    assert_neq([].length, 1)
  end
end
