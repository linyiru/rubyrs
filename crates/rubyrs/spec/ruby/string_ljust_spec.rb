# Adapted from ruby/spec core/string/ljust_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — three baseline
# shapes: default space pad, custom pad char, no-op when width
# is shorter than self.

describe "String#ljust" do
  it "pads on the right with spaces when no pad string is given" do
    assert_eq("hello".ljust(10), "hello     ")
  end

  it "pads with the given pad string" do
    assert_eq("hello".ljust(10, "."), "hello.....")
  end

  it "returns self unchanged when width is less than or equal to length" do
    assert_eq("hello".ljust(2), "hello")
    assert_eq("hello".ljust(5), "hello")
  end

  it "cycles a multi-char pad string" do
    assert_eq("x".ljust(7, "ab"), "xababab")
  end

  # skipped (fixture): subclass-return-type variant.
end
