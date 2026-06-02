# MatchData#inspect format parity with CRuby. Pre-fix the
# implementation emitted the simplified `<MatchData hello>`
# shape (preamble/match_data.rb header comment cited "Rust raw
# string delimiter conflicts" but the file was always loaded
# via include_str! — no embedding conflict to dodge). CRuby's
# canonical form is `#<MatchData "<whole>" 1:"<cap1>" 2:"<cap2>" ...>`
# with String#inspect-escaped strings and `N:nil` for non-
# participating groups.

# With captures.
puts "hello".match(/(.)(.)/).inspect

# Single capture.
puts "hello".match(/h(e)/).inspect

# Non-participating group via alternation.
puts "hello".match(/x(.)|h(.)/).inspect

# No groups — trailing list omitted.
puts "hello world".match(/hello/).inspect

# Special characters in matches — escape parity through
# String#inspect.
puts 'a"b'.match(/(.)"(.)/).inspect
puts "a\tb".match(/(.)\t(.)/).inspect

# Multi-byte (UTF-8) — capture stays bytes through inspect.
puts "héllo".match(/h(.)/).inspect

# $~ from a gsub round-trip (exercises the per-match
# last_match update we shipped at d2c88679, then inspect.)
"hello world".gsub(/(\w+)/) { "#{$1}" }
puts $~.inspect
