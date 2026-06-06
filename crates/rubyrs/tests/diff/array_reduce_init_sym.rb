# `Array#reduce(init, :op)` / `#inject(init, :op)` — the two-arg
# seed+operator form. Pre-fix only `reduce(:op)` (single symbol)
# and `inject(init){block}` worked; `reduce(10, :+)` fell through
# to NoMethodError.
#
# Discovery: P3 Sinatra spike discovery-map (Array#reduce gap).

# Seed + operator over a non-empty array.
puts "add=#{[1, 2, 3].reduce(10, :+)}"
puts "mul=#{[1, 2, 3, 4].inject(1, :*)}"
puts "sub=#{[1, 2, 3].reduce(100, :-)}"

# Empty receiver returns the seed unchanged.
puts "empty=#{[].reduce(5, :+)}"
puts "empty_mul=#{[].inject(1, :*)}"

# The single-symbol and block forms still work (regression).
puts "sym_only=#{[1, 2, 3, 4].reduce(:+)}"
puts "block_init=#{[1, 2, 3].inject(10) { |s, x| s + x }}"
puts "sym_empty=#{[].reduce(:+).inspect}"
# NB: a Float seed (`reduce(0.5, :+)`) is NOT asserted — the
# symbol-fold fast path is Integer/BigInt only in both the
# single-symbol and seed arms (a separate pre-existing gap).
