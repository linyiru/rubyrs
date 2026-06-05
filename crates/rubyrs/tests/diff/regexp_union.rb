# `Regexp.union(*patterns)` — combine String / Regexp args into
# one alternation Regexp. Required by Rack 3
# `rack/utils.rb:607`:
#   PATH_SEPS = Regexp.union(*[::File::SEPARATOR, ::File::ALT_SEPARATOR].compact)
# evaluated at class-body load time during the P3 Sinatra
# spike. Pre-fix this raised
# `NoMethodError: undefined method 'union' for Class`.

# Shape 1: one String — escapes metacharacters.
r1 = Regexp.union("a.b")
puts "matches_literal=#{!!('a.b' =~ r1)}"
puts "no_metachar=#{!('axb' =~ r1)}"

# Shape 2: multiple Strings — alternation.
r2 = Regexp.union("foo", "bar")
puts "alt_foo=#{!!('foo' =~ r2)}"
puts "alt_bar=#{!!('bar' =~ r2)}"
puts "alt_miss=#{!('baz' =~ r2)}"

# Shape 3: single Array arg — splatted.
r3 = Regexp.union(["x", "y"])
puts "splat=#{!!('x' =~ r3)}"

# Shape 4: Regexp args — sources combined.
r4 = Regexp.union(/\d+/, /[a-z]+/)
puts "re_digit=#{!!('123' =~ r4)}"
puts "re_alpha=#{!!('abc' =~ r4)}"

# Shape 5: no args — never-matching pattern.
r5 = Regexp.union
puts "empty_no_match=#{!('anything' =~ r5)}"

# Shape 6: Rack's exact use shape.
paths = [::File::SEPARATOR, ::File::ALT_SEPARATOR].compact
rack = Regexp.union(*paths)
puts "rack_slash=#{!!('/' =~ rack)}"
