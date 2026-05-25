# encoding: utf-8
# frozen_string_literal: false


describe "String#reverse" do
  it "returns a new string with the characters of self in reverse order" do
    assert_eq("stressed".reverse, "desserts")
    assert_eq("m".reverse, "m")
    assert_eq("".reverse, "")
  end

  it "returns String instances when called on a subclass" do
    assert(StringSpecs::MyString.new("stressed").reverse.instance_of?(String))
    assert(StringSpecs::MyString.new("m").reverse.instance_of?(String))
    assert(StringSpecs::MyString.new("").reverse.instance_of?(String))
  end

  it "reverses a string with multi byte characters" do
    assert_eq("微軟正黑體".reverse, "體黑正軟微")
  end

  it "works with a broken string" do
    str = "微軟\xDF\xDE正黑體".force_encoding(Encoding::UTF_8)

    assert_eq(str.valid_encoding?, false)

    assert_eq(str.reverse, "體黑正\xDE\xDF軟微")
  end

  it "returns a String in the same encoding as self" do
    assert_eq("stressed".encode("US-ASCII").reverse.encoding, Encoding::US_ASCII)
  end
end

describe "String#reverse!" do
  it "reverses self in place and always returns self" do
    a = "stressed"
    assert(a.reverse!.equal?(a))
    assert_eq(a, "desserts")

    assert_eq("".reverse!, "")
  end

  it "raises a FrozenError on a frozen instance that is modified" do
    assert_raises("FrozenError") do
      "anna".freeze.reverse!
    end
    assert_raises("FrozenError") do
      "hello".freeze.reverse!
    end
  end

  # see [ruby-core:23666]
  it "raises a FrozenError on a frozen instance that would not be modified" do
    assert_raises("FrozenError") do
      "".freeze.reverse!
    end
  end

  it "reverses a string with multi byte characters" do
    str = "微軟正黑體"
    str.reverse!
    assert_eq(str, "體黑正軟微")
  end

  it "works with a broken string" do
    str = "微軟\xDF\xDE正黑體".force_encoding(Encoding::UTF_8)

    assert_eq(str.valid_encoding?, false)
    str.reverse!

    assert_eq(str, "體黑正\xDE\xDF軟微")
  end
end
