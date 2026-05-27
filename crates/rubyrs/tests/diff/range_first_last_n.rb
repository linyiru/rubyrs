# `Range#first(n)` and `Range#last(n)` — variadic forms.
#
# Before issue #143, closed `(b..e).first(n)` / `(b..e).last(n)`
# raised NoMethodError (no arm in vm/range.rs for the closed
# branch), and endless `(b..).first(n)` silently clamped
# negative n via `(*n).max(0)` instead of raising
# ArgumentError. Beginless `(..e).first(n)` fell through to
# NoMethodError too; CRuby raises RangeError there.
#
# Companion to `array_first_last_n.rb` — same shape of fixture,
# different receiver. Covers closed (inclusive + exclusive),
# endless, and beginless ranges across in-bounds / oversized
# / zero / negative `n`.

# Closed range, inclusive
puts (1..5).first(0).inspect      # []
puts (1..5).first(2).inspect      # [1, 2]
puts (1..5).first(5).inspect      # [1, 2, 3, 4, 5]
puts (1..5).first(10).inspect     # [1, 2, 3, 4, 5]  (capped at size)
puts (1..5).last(0).inspect       # []
puts (1..5).last(2).inspect       # [4, 5]
puts (1..5).last(5).inspect       # [1, 2, 3, 4, 5]
puts (1..5).last(10).inspect      # [1, 2, 3, 4, 5]

# Closed range, exclusive — drops the endpoint.
puts (1...5).first(2).inspect     # [1, 2]
puts (1...5).first(10).inspect    # [1, 2, 3, 4]
puts (1...5).last(2).inspect      # [3, 4]
puts (1...5).last(10).inspect     # [1, 2, 3, 4]

# Empty range (begin > end) — first/last(n) return [].
puts (5..1).first(2).inspect      # []
puts (5..1).last(2).inspect       # []
puts (5...5).first(2).inspect     # []   exclusive, begin == end → empty
puts (5...5).last(2).inspect      # []

# i64::MIN boundary — exposes an off-by-one in the previous
# `start = end_inc.saturating_sub(n_taken).saturating_add(1)`
# shape, where `i64::MIN.saturating_sub(1)` clamps and the
# subsequent `+1` produced `[i64::MIN + 1]` instead of
# `[i64::MIN]`. `bi + (count - n_taken)` avoids the saturation
# artifact. Constructed at runtime because Ruby literals don't
# carry a clean `i64::MIN` token (would lex as `0 - 2**63` →
# BigInt promotion). The subtraction here stays inside i64.
neg_min = (-9223372036854775807) - 1
puts neg_min                      # -9223372036854775808
puts (neg_min..neg_min).last(1).inspect  # [-9223372036854775808]
puts (neg_min..neg_min).first(1).inspect # [-9223372036854775808]

# Endless range — first(n) generates n consecutive ints.
puts (1..).first(0).inspect       # []
puts (1..).first(3).inspect       # [1, 2, 3]
puts (10..).first(4).inspect      # [10, 11, 12, 13]
puts (1...).first(3).inspect      # [1, 2, 3]   exclusive doesn't matter (no endpoint)

# Negative n raises ArgumentError — wording differs slightly
# between first ("negative array size (or size too big)") and
# last ("negative array size"). Match both verbatim against
# CRuby.
begin
  (1..5).first(-1)
rescue ArgumentError => e
  puts "(1..5).first(-1): #{e.message}"   # negative array size (or size too big)
end
begin
  (1..5).last(-1)
rescue ArgumentError => e
  puts "(1..5).last(-1): #{e.message}"    # negative array size
end
begin
  (1..).first(-1)
rescue ArgumentError => e
  puts "(1..).first(-1): #{e.message}"    # negative array size (or size too big)
end

# Beginless `(..e)` — first(n) raises RangeError per CRuby
# (no anchor to walk from). last(no-arg) still works since
# `e` is the explicit end. last(n) on beginless is undefined
# in CRuby (would need an anchor); rubyrs does not implement
# it either — out of scope for #143.
begin
  (..5).first(2)
rescue RangeError => e
  puts "(..5).first(2): #{e.message}"     # cannot get the first element of beginless range
end
puts (..5).last.inspect                    # 5

# No-arg form (regression guard — was already supported before).
puts (1..5).first.inspect                  # 1
puts (1..5).last.inspect                   # 5
puts (1..).first.inspect                   # 1

# Block attached — CRuby silently discards (first/last don't
# yield). Without a delegating arm in vm/iter.rs's block-aware
# dispatcher, these would NoMethodError as #146 originally
# shipped. Same pattern as the Array fix in #140.
puts((1..5).first(2) { puts "should-never-run" }.inspect)  # [1, 2]
puts((1..5).last(2)  { puts "should-never-run" }.inspect)  # [4, 5]
puts((1..5).first    { puts "should-never-run" }.inspect)  # 1
puts((1..5).last     { puts "should-never-run" }.inspect)  # 5
puts((1..).first(3)  { puts "should-never-run" }.inspect)  # [1, 2, 3]
