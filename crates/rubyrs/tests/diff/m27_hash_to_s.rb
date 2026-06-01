# M27 C1: Hash#to_s is an alias of Hash#inspect (since 1.9); pre-fix
# `to_s` fell through to the universal `Object#to_s` `#<Hash:0xHEX>`
# fallback, so `puts h` and `"#{h}"` rendered the hex form instead of
# the canonical `{k: v}` shape. CRuby is the oracle.

h = {a: 1, b: 2}
puts h.to_s
puts h.inspect
puts "interp=#{h}"

# Empty
puts({}.to_s)
puts({}.inspect)

# Nested
n = {x: {y: 3}, z: [1, 2]}
puts n.to_s
puts n.inspect
