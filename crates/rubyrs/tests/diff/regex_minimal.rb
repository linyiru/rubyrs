# Regex minimal subset — /pattern/ literal, =~, String#match?,
# Regexp#=== (powering case/when over patterns). Backed by
# Rust's `regex` crate; small divergences from Onigmo (no
# possessive quantifiers, no `\k<name>` named backrefs) are
# documented in SUBSET.md.

re = /hello/
puts re.source
puts re.inspect

# =~ returns the byte offset of the first match, or nil.
puts "hello world" =~ re
puts ("no match" =~ re).inspect
puts re =~ "world hello"

# String#match? — boolean.
puts "hello world".match?(re)
puts "abc".match?(re)

# Regex#match? — symmetric.
puts re.match?("hello")
puts re.match?("nope")

# Anchors.
puts /\A\d+\z/.match?("12345")
puts /\A\d+\z/.match?("12a")
puts /\A\d+/.match?("12a")
puts /\d+\z/.match?("a12")

# Character classes.
puts /[a-z]+/.match?("hello")
puts /[A-Z]+/.match?("hello")
puts /[a-zA-Z]+/.match?("Hello")

# Quantifiers.
puts /\d{3}/.match?("12345")
puts /\d{3}/.match?("12")
puts /\d{2,4}/.match?("123")

# Escape sequences \d \w \s.
puts /\d/.match?("hello 42")
puts /\w+/.match?("foo_bar")
puts /\s+/.match?("  ")

# case/when with regex (regex#=== powers the dispatch).
def kind(s)
  case s
  when /\A\d+\z/ then "number"
  when /\A[a-z]+\z/ then "lower"
  when /\A[A-Z]+\z/ then "upper"
  else "mixed"
  end
end
puts kind("42")
puts kind("hello")
puts kind("HELLO")
puts kind("Hello123")
puts kind("")

# Many strings against one regex.
ip_like = /\A\d+\.\d+\.\d+\.\d+\z/
[
  "127.0.0.1",
  "10.0.0.1",
  "not-an-ip",
  "1.2.3",
  "1.2.3.4.5",
].each { |s| puts "#{s}: #{ip_like.match?(s)}" }

# Counting matches via =~ in a loop.
def count_digits(s)
  n = 0
  i = 0
  while i < s.length
    if /\A\d\z/.match?(s[i])
      n += 1
    end
    i += 1
  end
  n
end
puts count_digits("hello 42 world 7")
puts count_digits("plain text")

# Regex inside a class (using a method instead of a constant
# since `PATTERN = /.../` would need ConstantWrite).
class LexerOK
  def pattern
    /\A\s*([a-z_]+)\s*=\s*(\d+)\s*\z/
  end
  def parse(line)
    if pattern.match?(line)
      "valid"
    else
      "invalid"
    end
  end
end

lex = LexerOK.new
puts lex.parse("x = 42")
puts lex.parse("invalid input")
puts lex.parse("  count = 100  ")

# Empty regex matches every position.
puts //.match?("anything")
puts //.match?("")
