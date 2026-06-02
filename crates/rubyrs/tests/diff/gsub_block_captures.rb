# `$1` / `$2` / `$~` / `$&` populate inside `String#gsub` / `#sub`
# block bodies. Pre-fix the block could only see the full match
# (via the block arg `|m|`); `$1.upcase` raised NoMethodError on
# NilClass. Drove the ActiveSupport-lite canon (menu item 3) to
# carry a `m[1]`-slice workaround in every Regex-using method
# (camelize / underscore). Fixed by switching gsub's iter from
# `re.find_iter` to `re.captures_iter` and populating
# `vm.last_match` per match before invoking the block.

# Basic single-group capture.
puts "active_record".gsub(/_([a-z])/) { $1.upcase }

# Multi-group, $1 and $2 distinct.
puts "hello world".gsub(/(\w+) (\w+)/) { "#{$2}-#{$1}" }

# $~ (MatchData) — index access mirrors CRuby's MatchData#[].
puts "hello".gsub(/(.)(.)/) { "#{$~[1]}#{$~[2]}" }

# $& — the full match (same as the block arg).
puts "abc".gsub(/./) { "#{$&}#{$&}" }

# Non-participating groups stay nil.
"abc".gsub(/(a)|(b)/) { puts "#{$1.inspect} #{$2.inspect}"; "X" }

# `sub` (single match) also populates.
puts "hello".sub(/(h)(e)/) { "#{$2}#{$1}" }

# `$~` survives past the gsub call (matches CRuby's
# thread-local-current-match semantics) when the call matched.
"foo".gsub(/o/) { "" }
puts $~[0]
puts $~.nil?

# No-match case: CRuby clears `$~` to nil after a gsub call
# that didn't match anything. Document the surface.
"aaa".gsub(/x/) { "X" }
puts $~.nil?
