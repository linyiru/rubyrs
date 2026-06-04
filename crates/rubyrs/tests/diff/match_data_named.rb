# MatchData named captures — `(?<name>...)` groups exposed via
# `match[:name]` / `match["name"]` and `#named_captures`.
# Extracted from the regex's `capture_names()` iterator at match
# time and stored in the `@named_caps` ivar.

# Named groups via Symbol AND String index.
m = "John Doe 42".match(/(?<first>\w+) (?<last>\w+) (?<age>\d+)/)
p m[:first]
p m[:last]
p m[:age]
p m["first"]
p m["last"]

# Positional indexing still works alongside named.
p m[0]
p m[1]
p m[2]
p m[3]

# `named_captures` returns a real Hash; mutating it doesn't
# affect the MatchData (it's a dup).
nc = m.named_captures
p nc
p nc.keys
p nc.values
nc["first"] = "MUTATED"
p m.named_captures["first"]   # still "John"

# Unknown name raises IndexError on CRuby.
begin
  m[:nope]
rescue IndexError => e
  puts "rescued: #{e.class} #{e.message}"
end

# Pattern with no named groups — `named_captures` is empty,
# any string/symbol lookup raises.
m2 = "hello".match(/(\w+)/)
p m2.named_captures
begin
  m2[:anything]
rescue IndexError => e
  puts "rescued: #{e.class} #{e.message}"
end

# Non-participating named group (alternation arm) — present in
# named_captures with nil value, lookup returns nil (no raise).
m3 = "abc".match(/(?<word>[a-z]+)|(?<num>\d+)/)
p m3.named_captures
p m3[:word]
p m3[:num]

# String-arg `String#match` with named groups in the pattern
# still extracts them.
m4 = "key=value".match("(?<k>\\w+)=(?<v>\\w+)")
p m4[:k]
p m4[:v]
p m4.named_captures
