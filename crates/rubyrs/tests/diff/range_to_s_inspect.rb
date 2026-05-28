# Range#to_s and Range#inspect — added when the universal
# Object#to_s/inspect arm in PR #272 (object_id/hash/frozen?/
# to_s/inspect) was found to silently render Range as
# `#<Range:0xHEX>` instead of CRuby's `begin..end` form.
#
# Range delegates to_s via #to_s on each endpoint and inspect
# via #inspect on each endpoint, so String endpoints come out
# quoted under inspect but bare under to_s.

# Inclusive
puts (1..5).to_s
puts (1..5).inspect

# Exclusive (...)
puts (1...5).to_s
puts (1...5).inspect

# String endpoints — inspect quotes them, to_s doesn't
puts ("a".."z").to_s
puts ("a".."z").inspect

# Endless
puts (1..).to_s
puts (1..).inspect

# Beginless (CRuby renders the missing endpoint as empty)
puts (..5).to_s
puts (..5).inspect

# Interpolation uses to_s
puts "#{1..3}"
puts "#{1...3}"

# Array#inspect routes each element through to_inspect, which
# means Range elements should pick up the quote-aware form.
puts [1..3, "a".."c"].inspect
