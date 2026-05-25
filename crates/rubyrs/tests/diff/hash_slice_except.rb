# Hash#except and Hash#slice — non-mutating subset operations.
# except: drop listed keys.
# slice: keep only listed keys.
# Both preserve the receiver's insertion order and silently
# skip keys not present.

# Basic.
puts({a: 1, b: 2, c: 3}.except(:a).inspect)            # {b: 2, c: 3}
puts({a: 1, b: 2, c: 3}.except(:a, :c).inspect)        # {b: 2}
puts({a: 1, b: 2, c: 3}.except.inspect)                # {a: 1, b: 2, c: 3}

puts({a: 1, b: 2, c: 3}.slice(:a, :c).inspect)         # {a: 1, c: 3}
puts({a: 1, b: 2, c: 3}.slice(:b).inspect)             # {b: 2}
puts({a: 1, b: 2, c: 3}.slice.inspect)                 # {}

# Missing keys silently skipped.
puts({a: 1, b: 2}.except(:nonexistent).inspect)        # {a: 1, b: 2}
puts({a: 1, b: 2}.slice(:nonexistent).inspect)         # {}
puts({a: 1, b: 2}.slice(:a, :nonexistent).inspect)     # {a: 1}

# Slice preserves receiver order (not argument order).
puts({a: 1, b: 2, c: 3}.slice(:c, :a).inspect)         # {a: 1, c: 3}

# Non-mutating.
h = {a: 1, b: 2, c: 3}
h.except(:a)
h.slice(:a)
puts h.inspect                                          # {a: 1, b: 2, c: 3}

# Empty receiver.
puts({}.except(:a).inspect)                            # {}
puts({}.slice(:a).inspect)                             # {}

# Mixed key types.
puts({1 => :a, "k" => :b, :c => 3}.except(1).inspect)  # {"k" => :b, c: 3}
puts({1 => :a, "k" => :b, :c => 3}.slice("k", :c).inspect)  # {"k" => :b, c: 3}
