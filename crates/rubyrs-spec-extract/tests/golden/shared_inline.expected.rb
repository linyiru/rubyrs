describe "String#length" do
  it "returns the length of self" do
    assert_eq("".send(:length), 0)
    assert_eq("one".send(:length), 3)
  end

  it "round-trips through send" do
    assert_eq("abc".send(:length), 3)
  end
end
