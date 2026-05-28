# Adapted from ruby/spec core/string/gsub_spec.rb at 2026-05
# (subset). Translates upstream's `.should ==` matchers to
# `assert_eq`. Skipped:
#   - /i case-insensitive flag (not yet implemented)
#   - \1 / \& / \k<name> backref replacement (separate spec PR)
#   - Unicode normalisation / multibyte chars
#   - shared-examples form (`describe :string_gsub_named_capture`)
#   - subclass / fixtures-based String specialisations

describe "String#gsub with pattern and replacement" do
  it "inserts the replacement around every character when the pattern collapses" do
    assert_eq("hello".gsub(//, "."), ".h.e.l.l.o.")
  end

  # Upstream's "doesn't freak out when replacing ^" example
  # asserts per-line `^` anchoring:
  #
  #   "Text\n".gsub(/^/, ' ').should == " Text\n"
  #   "Text\nFoo".gsub(/^/, ' ').should == " Text\n Foo"
  #
  # rubyrs's regex engine fires `^` only at the string start,
  # not at every line start. The single-line case happens to
  # pass coincidentally; the multi-line case is a documented
  # divergence. To keep this spec file a faithful mirror of
  # upstream (rather than a check of our narrower behaviour),
  # the whole `it` block is skipped here. Will un-skip once
  # the engine learns per-line anchoring. See docs/SUBSET.md →
  # "Regex literals" for the canonical statement of the gap.

  it "returns a copy of self with ALL occurrences replaced" do
    assert_eq("hello".gsub(/[aeiou]/, '*'), "h*ll*")
  end

  it "ignores a block if a replacement string is supplied" do
    # Mirror sub_spec's pattern: observable side effect proves
    # the block was actually skipped, not silently discarded
    # after running.
    ran = false
    out = "food".gsub(/f/, "g") { ran = true; "w" }
    assert_eq(out, "good")
    assert_eq(ran, false)
  end

  it "doesn't interpret regexp metacharacters when pattern is a String" do
    assert_eq("12345".gsub('\d', 'a'), "12345")
    assert_eq('\d'.gsub('\d', 'a'), "a")
  end

  it "returns self unchanged for a pattern that doesn't match" do
    s = "hello"
    assert_eq(s.gsub(/z/, "x"), "hello")
    assert_eq(s, "hello")
  end
end

describe "String#gsub with pattern and block" do
  it "calls the block once per match and substitutes the result" do
    assert_eq("hello".gsub(/[aeiou]/) { |m| m.upcase }, "hEllO")
  end

  it "invokes the block once per match (not once per call)" do
    count = 0
    out = "aaa".gsub(/a/) do |_m|
      count = count + 1
      "b"
    end
    assert_eq(out, "bbb")
    assert_eq(count, 3)
  end

  it "passes the matched substring to the block" do
    matches = []
    "hello".gsub(/[aeiou]/) do |m|
      matches << m
      m
    end
    assert_eq(matches, ["e", "o"])
  end

  it "uses the block's return value as the replacement String" do
    assert_eq("aaa".gsub(/a/) { "!" }, "!!!")
  end
end

describe "String#gsub!" do
  it "modifies self in place and returns self on a match" do
    s = "hello"
    r = s.gsub!("l", "L")
    assert(r.equal?(s))
    assert_eq(s, "heLLo")
  end

  it "returns nil if no substitutions were made" do
    s = "hello"
    assert_eq(s.gsub!("xyz", "Q"), nil)
    assert_eq(s, "hello")
  end

  it "returns self when a match occurred even if the replacement bytes are identical" do
    # CRuby gates nil-vs-self on match presence, not on byte
    # equality — `s.gsub!("l", "l")` matches and returns
    # self despite the result being byte-identical to the
    # input.
    s = "hello"
    r = s.gsub!("l", "l")
    assert(r.equal?(s))
    assert_eq(s, "hello")
  end

  it "handles an empty pattern by wrapping the replacement around every char" do
    s = "abc"
    s.gsub!("", "X")
    assert_eq(s, "XaXbXcX")
  end

  it "supports a Regexp pattern" do
    s = "hello"
    s.gsub!(/l/, "L")
    assert_eq(s, "heLLo")
    assert_eq("hello".gsub!(/z/, "Q"), nil)
  end

  it "raises a FrozenError on a frozen instance that is modified" do
    s = "hi".freeze
    assert_raises("FrozenError") { s.gsub!("h", "H") }
  end
end

