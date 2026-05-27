# Adapted from ruby/spec core/string/rjust_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — mirror image of
# string_ljust_spec.rb: pads on the LEFT with the given char.

describe "String#rjust" do
  it "pads on the left with spaces when no pad string is given" do
    assert_eq("hello".rjust(10), "     hello")
  end

  it "pads with the given pad string" do
    assert_eq("hello".rjust(10, "0"), "00000hello")
  end

  it "returns self unchanged when width is less than or equal to length" do
    assert_eq("hello".rjust(2), "hello")
    assert_eq("hello".rjust(5), "hello")
  end

  it "cycles a multi-char pad string" do
    assert_eq("x".rjust(7, "ab"), "abababx")
  end

  # skipped (fixture): subclass-return-type variant.
end
