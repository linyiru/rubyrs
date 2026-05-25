describe "String#sub" do
  it "returns a copy with the first occurrence replaced" do
    assert_eq("hello".sub(/[aeiou]/, '*'), "h*llo")
    assert_eq("hello".sub(//, "."), ".hello")
  end

  it "ignores a block if a replacement string is supplied" do
    assert_eq("food".sub(/f/, "g") { "w" }, "good")
  end
end
