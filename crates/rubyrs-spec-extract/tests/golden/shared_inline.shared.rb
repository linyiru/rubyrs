describe :string_length, shared: true do
  it "returns the length of self" do
    "".send(@method).should == 0
    "one".send(@method).should == 3
  end

  it "round-trips through send" do
    "abc".send(@method).should == 3
  end
end
