# Hash#transform_keys / #transform_values — return new Hashes
# with the block applied. Both block forms. transform_keys may
# collide (later wins). transform_values preserves keys.

# Basic transform_keys.
puts({a: 1, b: 2}.transform_keys { |k| k.to_s }.inspect)
# → {"a" => 1, "b" => 2}

# Basic transform_values.
puts({a: 1, b: 2}.transform_values { |v| v * 10 }.inspect)
# → {a: 10, b: 20}

# Empty.
puts({}.transform_keys { |k| k }.inspect)               # {}
puts({}.transform_values { |v| v }.inspect)             # {}

# Key collision — later wins, key under that slot is the
# block's mapped value for the LAST original key (c→same with
# value 3 replaces the earlier mappings).
puts({a: 1, b: 2, c: 3}.transform_keys { |_| :same }.inspect)
# → {same: 3}

# Different key types.
puts({1 => :a, 2 => :b}.transform_values { |v| v.to_s }.inspect)
# → {1 => "a", 2 => "b"}

# Compose-style chaining: transform_keys then transform_values.
result = {x: 1, y: 2}.transform_keys { |k| k.to_s.upcase }.transform_values { |v| v + 100 }
puts result.inspect
# → {"X" => 101, "Y" => 102}

# Original Hash not mutated.
h = {a: 1, b: 2}
h2 = h.transform_keys { |k| k.to_s }
puts h.inspect                                          # {a: 1, b: 2}
puts h2.inspect                                         # {"a" => 1, "b" => 2}
puts h.equal?(h2)                                       # false
