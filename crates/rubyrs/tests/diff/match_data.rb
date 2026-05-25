# String#match — returns a MatchData instance (or nil). The
# MatchData wrapper exposes []/captures/to_a/size/to_s/inspect.

# Basic capture extraction.
m = "hello world".match(/(\w+)\s+(\w+)/)
p m[0]
p m[1]
p m[2]
p m.captures
p m.to_a
p m.size
puts m.to_s

# No captures — empty captures array, whole match available.
m2 = "abc123".match(/[a-z]+/)
p m2[0]
p m2.captures
p m2.size

# No match → nil.
p "no nums".match(/\d+/)
p "abc".match(/^\d/)

# A miss does not blow up downstream — chain on nil-safe path.
hit = "x=42".match(/(\w+)=(\d+)/)
if hit
  puts "key=#{hit[1]}, val=#{hit[2]}"
end

miss = "foo".match(/(\d+)/)
if miss
  puts "should not run"
else
  puts "miss"
end

# Note: match returns only the first match. (Regex-arg `scan` for
# multiple matches isn't implemented yet.)

# Combined with case/when patterns (just to verify the wrappers
# stay independent — match? still works on the raw regex).
def kind(s)
  case
  when s.match?(/\A\d+\z/) then "number"
  when s.match?(/\A[a-z]+\z/) then "lower"
  else "other"
  end
end
puts kind("42")
puts kind("hello")
puts kind("Hi42")

# Use match inside an instance method to parse a key=val pair.
class Pair
  def parse(s)
    m = s.match(/(\w+)=(\w+)/)
    return nil unless m
    [m[1], m[2]]
  end
end

p Pair.new.parse("name=alice")
p Pair.new.parse("nothing")

# Multiple capture groups.
m4 = "2026-05-25".match(/(\d{4})-(\d{2})-(\d{2})/)
p m4.captures
puts "#{m4[1]}/#{m4[2]}/#{m4[3]}"

# Optional capture group that didn't match → nil.
m5 = "abc".match(/(\d+)?([a-z]+)/)
p m5[1]
p m5[2]
p m5.captures
