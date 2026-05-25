# String#match? / scan / index / rindex — literal-substring only.
# Regexp forms are out of scope for this subset.

s = "hello world"

# match? — true iff the substring appears.
puts s.match?("world")
puts s.match?("xyz")
puts s.match?("o")
puts s.match?("")            # empty pattern matches everywhere
puts "".match?("anything")
puts "".match?("")

# scan — Array of every non-overlapping occurrence.
puts s.scan("o").inspect
puts s.scan("hello").inspect
puts s.scan("xyz").inspect
puts "abcabcabc".scan("abc").inspect
puts "aaaa".scan("aa").inspect       # non-overlapping: 2 hits
puts "banana".scan("ana").inspect    # non-overlapping: 1 hit

# index — byte offset of first occurrence, or nil.
puts s.index("hello")
puts s.index("world")
puts s.index("o")
puts s.index("xyz").inspect
puts "".index("anything").inspect
puts "".index("").inspect            # empty match at 0
puts s.index("")

# rindex — byte offset of last occurrence, or nil.
puts s.rindex("o")
puts s.rindex("l")
puts s.rindex("hello")
puts s.rindex("world")
puts s.rindex("xyz").inspect
puts s.rindex("")

# Common parsing idiom: split-then-scan-for-words.
log = "ERROR: bad input ERROR: bad output WARN: slow path"
errors = log.scan("ERROR")
puts errors.length
puts errors.inspect

# Index gives us a position we can pass to split-style logic
# without needing String#[].
header = "Content-Type: text/plain"
idx = header.index(":")
puts idx
puts header.length - idx - 2

# match? inside a conditional + counter idiom.
lines = [
  "INFO: ok",
  "WARN: degraded",
  "ERROR: crash",
  "INFO: ok",
  "ERROR: timeout",
]
counts = {}
lines.each do |line|
  ["INFO", "WARN", "ERROR"].each do |level|
    if line.match?(level)
      counts[level] ||= 0
      counts[level] += 1
    end
  end
end
puts counts["INFO"]
puts counts["WARN"]
puts counts["ERROR"]

# scan to extract repeated tokens.
csv = "a,b,c,d,e"
commas = csv.scan(",")
puts commas.length

# Class wrapping scan/index.
class Tokenizer
  def initialize(src)
    @src = src
  end
  def words
    @src.scan(" ").length + 1
  end
  def first_at(needle)
    @src.index(needle)
  end
  def last_at(needle)
    @src.rindex(needle)
  end
end

t = Tokenizer.new("the quick brown fox jumps over")
puts t.words
puts t.first_at("o")
puts t.last_at("o")
puts t.first_at("missing").inspect

# index returns nil → falsy → flows through unless / if.
needle = "missing"
hay = "abcdef"
if hay.index(needle).nil?
  puts "absent"
else
  puts "present"
end
