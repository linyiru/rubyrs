describe "Array#length" do
  it "diverges from a wrong expectation" do
    [1, 2, 3].length.should_not == 99
    [].length.should_not == 1
  end
end
