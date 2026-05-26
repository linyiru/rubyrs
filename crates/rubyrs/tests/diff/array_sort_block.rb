# `Array#sort` / `Array#sort!` with a comparator block.
#
# CRuby: block called with `(a, b)` on every comparison; returns
# negative / zero / positive (Integer) for ordering. The block
# replaces the default `<=>` dispatch path used by the no-arg
# `sort` / `sort!` forms (which call user_cmp under the hood).
#
# tilt/template.rb:252 uses `locals_keys.sort!{|x, y| x.to_s <=>
# y.to_s}` to order the locals-key array for compiled-method
# cache keys; previously this NoMethodError'd because the
# block form fell through `array_collection_call` (no-block
# only) into `collection_call_block` (which had `sort_by` but
# no `sort` / `sort!` arm).

# --- sort with block (returns new Array, receiver unchanged) ---
src = [3, 1, 4, 1, 5, 9, 2, 6]
puts src.sort { |a, b| a <=> b }.inspect        # [1, 1, 2, 3, 4, 5, 6, 9]
puts src.sort { |a, b| b <=> a }.inspect        # reverse
puts src.inspect                                # unchanged

# --- sort! with block (in-place, returns receiver) ---
arr = ["bb", "a", "ccc"]
ret = arr.sort! { |x, y| x.length <=> y.length }
puts arr.inspect                                # ["a", "bb", "ccc"]
# sort! returns the receiver (identity is via .equal?).
puts ret.equal?(arr)                            # true

# --- the tilt-template.rb:252 shape: sort symbols by .to_s ---
keys = [:foo, :bar, :baz]
keys.sort! { |x, y| x.to_s <=> y.to_s }
puts keys.inspect                               # [:bar, :baz, :foo]

# --- already-sorted is a no-op (but still returns receiver) ---
already = [1, 2, 3, 4]
already.sort! { |a, b| a <=> b }
puts already.inspect                            # [1, 2, 3, 4]

# --- empty / single-element edge cases ---
puts [].sort! { |a, b| a <=> b }.inspect        # []
puts [42].sort! { |a, b| a <=> b }.inspect      # [42]
