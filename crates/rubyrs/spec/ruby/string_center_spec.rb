# Adapted from ruby/spec core/string/center_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — three baseline
# shapes: default space pad, custom pad char, no-op when width
# is shorter than self.

describe "String#center" do
  it "pads with spaces when no pad string is given" do
    assert_eq("hello".center(11), "   hello   ")
  end

  it "pads with the given pad string" do
    assert_eq("hello".center(11, "*"), "***hello***")
  end

  it "returns self unchanged when width is less than or equal to length" do
    assert_eq("hello".center(3), "hello")
    assert_eq("hello".center(5), "hello")
  end

  it "raises ArgumentError on zero-width pad" do
    assert_raises("ArgumentError") { "hello".center(11, "") }
  end

  # skipped (method-not-implemented): integer-coerce on width arg (`String#center(width.to_int)`).
  # skipped (fixture): subclass-return-type variant.
end
