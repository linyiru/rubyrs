describe "String#sub" do
  it "returns a copy with the first occurrence replaced" do
    "hello".sub(/[aeiou]/, '*').should == "h*llo"
    "hello".sub(//, ".").should == ".hello"
  end

  it "ignores a block if a replacement string is supplied" do
    "food".sub(/f/, "g") { "w" }.should == "good"
  end
end
