
describe "String#empty?" do
  it "returns true if the string has a length of zero" do
    assert(!"hello".empty?)
    assert(!" ".empty?)
    assert(!"\x00".empty?)
    assert("".empty?)
    assert(StringSpecs::MyString.new("").empty?)
  end
end
