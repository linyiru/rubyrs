# NumberedReferenceReadNode + $~ — `$1`..`$9` and the MatchData
# global, populated by `=~` and `String#match`, cleared on miss.

# `=~` sets captures on hit
"hello world" =~ /(\w+)\s+(\w+)/
puts "#{$1} #{$2}"

# Missing groups are nil
"abc" =~ /(\w)(\w)/
puts "#{$1.inspect} #{$2.inspect} #{$3.inspect}"

# Failed `=~` clears prior captures and `$~`
"hello world" =~ /(\w+)\s+(\w+)/
"xyz" =~ /(\d+)/
puts "after miss: $1=#{$1.inspect} $~=#{$~.inspect}"

# `String#match` populates the same globals
"foo bar".match(/(\w+) (\w+)/)
puts "via match: #{$1} #{$2}"

# `$~` returns a MatchData-shaped object (we test class only;
# CRuby's MatchData inspect format differs from ours)
"a1b2" =~ /(\w)(\d)/
puts $~.class