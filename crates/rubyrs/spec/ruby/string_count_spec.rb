# Adapted from ruby/spec core/string/count_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — baseline shapes
# for single-set char counting PLUS range / negation shorthand
# (which rubyrs's `count` set-parser DOES expand correctly —
# unlike `String#tr`, where the same shorthand is divergent;
# see string_tr_spec.rb). The two methods have separate
# set-parser implementations.

describe "String#count" do
  it "counts characters in the given set" do
    assert_eq("hello".count("l"), 2)
    assert_eq("hello".count("aeiou"), 2)
  end

  it "treats the set as a union of single chars" do
    # "lo" matches every l or o; 2 + 1 = 3.
    assert_eq("hello".count("lo"), 3)
  end

  it "returns 0 for an empty source" do
    assert_eq("".count("a"), 0)
  end

  it "returns 0 for an empty set" do
    assert_eq("hello".count(""), 0)
  end

  it "returns 0 when no characters match" do
    assert_eq("hello".count("xyz"), 0)
  end

  it "expands range shorthand `a-y`" do
    # All 5 chars in "hello" fall inside a..y.
    assert_eq("hello".count("a-y"), 5)
  end

  it "supports leading `^` for negation" do
    # Count chars NOT in aeiou: h, l, l → 3.
    assert_eq("hello".count("^aeiou"), 3)
  end
end
