# Adapted from ruby/spec core/string/sub_spec.rb at 2026-05
# (subset). Drops upstream's `.should ==` matcher chain for our
# `assert_eq` shim; drops `.should_not.equal?` for `equal?`
# inverted manually. Skipped (out of subset / out of master);
# see docs/SUBSET.md → "String built-in methods" for the
# canonical list of what sub/gsub support today:
#   - /i case-insensitive flag (not yet implemented; sub fails
#     to match `Hello` against /h/i)
#   - \1 / \& backref replacement strings
#   - encoding/Unicode normalisation
#   - subclass / fixtures-based String specialisations

describe "String#sub with pattern, replacement" do
  it "returns a new String (not self) when no modification is made" do
    a = "hello"
    b = a.sub(/w.*$/, "*")
    # upstream: `b.should_not.equal?(a)` — inverted to a direct
    # identity check via `equal?`, which the existing String
    # primitive_call dispatch supports.
    assert_eq(b.equal?(a), false)
    assert_eq(b, "hello")
  end

  it "returns a copy with the first occurrence replaced" do
    assert_eq("hello".sub(/[aeiou]/, '*'), "h*llo")
    assert_eq("hello".sub(//, "."), ".hello")
  end

  it "ignores a block if a replacement string is supplied" do
    # The replacement string wins; the block never runs. An
    # observable side effect proves the block was actually
    # skipped — a return-value-only assertion would still pass
    # if the block ran and was silently discarded.
    ran = false
    out = "food".sub(/f/, "g") { ran = true; "w" }
    assert_eq(out, "good")
    assert_eq(ran, false)
  end

  it "doesn't interpret regexp metacharacters when pattern is a String" do
    # Literal `\d` as a String pattern matches the four-char
    # sequence "\\d", not the regex meta. "12345" has no
    # literal backslash so the sub is a no-op.
    assert_eq("12345".sub('\d', 'a'), "12345")
    assert_eq('\d'.sub('\d', 'a'), "a")
  end

  it "returns self unchanged for a pattern that doesn't match" do
    s = "hello"
    assert_eq(s.sub(/z/, "x"), "hello")
    # Sanity: the original was not mutated.
    assert_eq(s, "hello")
  end
end

describe "String#sub with pattern and block" do
  it "calls the block with each match and substitutes the result" do
    assert_eq("hello".sub(/[aeiou]/) { |m| m.upcase }, "hEllo")
  end

  it "only invokes the block once even with a global-style regex" do
    # `sub` replaces only the first match; the block runs once.
    count = 0
    out = "abc".sub(/./) do |m|
      count = count + 1
      m.upcase
    end
    assert_eq(out, "Abc")
    assert_eq(count, 1)
  end

  it "uses the block's return value as the replacement String" do
    assert_eq("hi".sub(/i/) { "!" }, "h!")
  end
  # Upstream also covers `"hello".sub("world") { ... }` (String
  # pattern under block form), but rubyrs currently routes the
  # block-form sub through the Regex path only — String-pattern
  # + block raises NoMethodError. See docs/SUBSET.md → "String
  # built-in methods" for the full gap list.
end

describe "String#sub!" do
  it "modifies self in place and returns self on a match" do
    s = "hello"
    r = s.sub!("l", "L")
    assert(r.equal?(s))
    assert_eq(s, "heLlo")
  end

  it "returns nil if no substitutions were made" do
    s = "hello"
    assert_eq(s.sub!("xyz", "Q"), nil)
    # Sanity: the original was not mutated.
    assert_eq(s, "hello")
  end

  it "returns self when a match occurred even if the replacement bytes are identical" do
    # CRuby gates nil-vs-self on match presence, not on byte
    # equality — `s.sub!("l", "l")` matches and returns self
    # despite the result being byte-identical to the input.
    s = "hello"
    r = s.sub!("l", "l")
    assert(r.equal?(s))
    assert_eq(s, "hello")
  end

  it "handles an empty pattern by prepending the replacement" do
    s = "hello"
    s.sub!("", "X")
    assert_eq(s, "Xhello")
  end

  it "supports a Regexp pattern" do
    s = "hello"
    s.sub!(/l+/, "L")
    assert_eq(s, "heLo")
    assert_eq("hello".sub!(/z/, "Q"), nil)
  end

  it "honours Ruby-style numeric backrefs (\\0, \\1) in the replacement" do
    # Guards the `ruby_backref_to_dollar` translation on the
    # destructive Regexp arm. `\0` references the whole match
    # and `\1` references the first capture group.
    s = "hello"
    s.sub!(/(l)/, "<\\1>")
    assert_eq(s, "he<l>lo")
    s = "abc"
    s.sub!(/b/, "[\\0]")
    assert_eq(s, "a[b]c")
  end

  it "raises a FrozenError on a frozen instance that is modified" do
    s = "hi".freeze
    assert_raises("FrozenError") { s.sub!("h", "H") }
  end
end

