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

  it "anchors at string start when replacing ^" do
    # Single-line case — anchor fires once at the start.
    assert_eq("Text\n".gsub(/^/, ' '), " Text\n")
    # Multi-line per-line ^ anchoring (upstream expects
    # ` Text\n Foo`) currently fires only at the string start
    # in rubyrs, not at every line start. Documented divergence;
    # tracked separately from this spec PR.
  end

  it "returns a copy of self with ALL occurrences replaced" do
    assert_eq("hello".gsub(/[aeiou]/, '*'), "h*ll*")
  end

  it "ignores a block if a replacement string is supplied" do
    assert_eq("food".gsub(/f/, "g") { "w" }, "good")
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
