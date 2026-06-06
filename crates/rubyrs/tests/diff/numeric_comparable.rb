# `Numeric` mixes in `Comparable` (CRuby parity:
# `Integer.include?(Comparable)` is true), supplying `between?`
# and `clamp` for every numeric type. Pre-fix Integer/Float
# lacked both (Comparable was never mixed in).
#
# Discovery: P3 Sinatra spike discovery-map — several gems use
# `n.between?` / `n.clamp` on Integer.

# is_a?(Comparable) now true for the numeric tower.
puts "int_comparable=#{5.is_a?(Comparable)}"
puts "float_comparable=#{2.5.is_a?(Comparable)}"

# between? — inclusive on both ends.
puts "i_btw_t=#{5.between?(1, 10)}"
puts "i_btw_lo=#{1.between?(1, 10)}"
puts "i_btw_hi=#{10.between?(1, 10)}"
puts "i_btw_f=#{15.between?(1, 10)}"
puts "f_btw=#{2.5.between?(1.0, 3.0)}"

# clamp — two-arg form.
puts "clamp_mid=#{5.clamp(1, 10)}"
puts "clamp_lo=#{(-3).clamp(0, 100)}"
puts "clamp_hi=#{150.clamp(0, 100)}"
puts "f_clamp=#{5.5.clamp(0.0, 5.0)}"

# clamp — Range form, including one-sided ranges.
puts "clamp_range=#{15.clamp(1..10)}"
puts "clamp_range_lo=#{(-5).clamp(0..)}"
puts "clamp_range_hi=#{99.clamp(..10)}"

# Cross-type comparison still works through <=>.
puts "mixed_btw=#{3.between?(2.5, 3.5)}"

# Primitive comparison fast path is unaffected.
puts "lt=#{5 < 3}"
puts "gte=#{5 >= 5}"
puts "spaceship=#{(7 <=> 3)}"

# clamp arity error matches CRuby.
begin
  5.clamp(1, 2, 3)
rescue ArgumentError => e
  puts "arity=ok"
end
