# `Array#delete(obj)` — value-based delete (uses `==`).
#
# Removes EVERY element equal to `obj`, returns the last deleted
# element, or nil if `obj` wasn't found. In-place mutation.
#
# Motivating consumer: tilt-2.7.0 lib/tilt/template.rb:378
#
#   assignments = local_keys.map { |k| "#{k} = locals[#{k.inspect}]" }
#   s = "locals = locals[:locals]"
#   if assignments.delete(s)
#     assignments << s
#   end
#
# tilt uses the truthy return value to decide whether to re-append
# the `locals`-key assignment LAST (so it doesn't shadow the
# `locals` method-argument while the other assignments are still
# reading from it).
#
# DIVERGENCE (documented at the impl site): the block form
# `arr.delete(obj) { yield-if-not-found }` reaches the impl (via
# the `collection_call_block` delegation), but rubyrs silently
# drops the block instead of yielding `obj` on no-match. CRuby
# returns the block's result on no-match; rubyrs returns nil.

# --- Single match: returns the matched value, mutates in place ---
a = [1, 2, 3]
puts a.delete(2).inspect                # 2
puts a.inspect                          # [1, 3]

# --- Multiple matches: removes ALL, returns the LAST deleted ---
a = [1, 2, 3, 2, 4, 2]
puts a.delete(2).inspect                # 2
puts a.inspect                          # [1, 3, 4]

# --- Distinct values, all `==` to the arg: returns the LAST in
#     array order (not the first / not arbitrary). `1 == 1.0`
#     holds in CRuby, so both match and we must surface `1.0`.
a = [1, 1.0]
puts a.delete(1).inspect                # 1.0
puts a.inspect                          # []

a = [1.0, 1]
puts a.delete(1).inspect                # 1
puts a.inspect                          # []

# --- Not found: returns nil, array unchanged ---
a = [1, 2, 3]
puts a.delete(99).inspect               # nil
puts a.inspect                          # [1, 2, 3]

# --- String element (the tilt shape) ---
a = ["a = locals[:a]", "locals = locals[:locals]", "b = locals[:b]"]
s = "locals = locals[:locals]"
puts a.delete(s).inspect                # "locals = locals[:locals]"
puts a.inspect                          # ["a = locals[:a]", "b = locals[:b]"]

# --- Empty array → nil ---
puts [].delete(:anything).inspect       # nil

# --- Symbol equality (`==` on Symbols) ---
a = [:x, :y, :x, :z]
puts a.delete(:x).inspect               # :x
puts a.inspect                          # [:y, :z]

# --- Nil is a valid element to delete ---
a = [1, nil, 2, nil]
puts a.delete(nil).inspect              # nil
puts a.inspect                          # [1, 2]

# --- Wrong arity raises ArgumentError (parity with CRuby) ---
begin
  [1, 2].delete
rescue ArgumentError => e
  puts "delete() → ArgumentError"
end
begin
  [1, 2].delete(1, 2)
rescue ArgumentError => e
  puts "delete(1, 2) → ArgumentError"
end

# --- tilt-shape conditional: truthy return drives append ---
assignments = ["a = locals[:a]", "locals = locals[:locals]", "b = locals[:b]"]
s = "locals = locals[:locals]"
if assignments.delete(s)
  assignments << s
end
puts assignments.inspect                # ["a = locals[:a]", "b = locals[:b]", "locals = locals[:locals]"]
