# Ivar-reflection battery for COLLECTION values (2026-07 correctness
# fix): `instance_variable_get/set/defined?` / `instance_variables` /
# `remove_instance_variable` on Array / Hash / String values.
#
# Pre-fix, Array/Hash receivers fell into the reflection catch-all and
# raised a MISLEADING FrozenError on UNFROZEN receivers; now they wire
# to the heap-side ivar tables (`ArrayObj::ivars` / `HashObj::ivars` —
# the same slots the marshal and subclass StoreIvar paths use), with a
# real frozen guard. String dup/clone now copy the `str_ivars`
# side-table entry (CRuby's generic-copy rule).
#
# NOTE on ordering: rubyrs reports Array/Hash/Str/Class ivar lists
# ALPHABETICALLY (unordered backing maps) where CRuby reports insertion
# order — every multi-ivar assertion here either inserts in
# alphabetical order or sorts, so both engines print identical bytes.

# ---- Array ---------------------------------------------------------------
a = [1, 2]
p a.instance_variables
p a.instance_variable_get(:@x)
p a.instance_variable_defined?(:@x)
p a.instance_variable_set(:@x, 5)      # returns the assigned value
p a.instance_variable_get(:@x)
p a.instance_variable_defined?(:@x)
p a.instance_variables

# Nested heap values survive (GC edge under STRESS_GC).
a.instance_variable_set(:@y, [10, { z: 1 }])
50.times { [1] * 3 }                    # churn allocations
p a.instance_variable_get(:@y)
p a.instance_variables.sort

# dup AND clone copy ivars (CRuby generic-copy rule); the copies are
# independent tables.
d = a.dup
p d.instance_variables.sort
d.instance_variable_set(:@x, :changed)
p a.instance_variable_get(:@x)
c = a.clone
p c.instance_variables.sort
p c.instance_variable_get(:@x)

# remove returns the value; NameError when absent.
p a.remove_instance_variable(:@x)
p a.instance_variables
begin
  a.remove_instance_variable(:@nope)
rescue NameError => e
  puts "arr-remove-missing: #{e.message}"
end

# Frozen Array: reads stay fine; set/remove raise a REAL FrozenError
# (remove raises even when the ivar was never set — CRuby order).
fa = [1, 2]
fa.instance_variable_set(:@keep, :v)
fa.freeze
p fa.instance_variable_get(:@keep)
p fa.instance_variable_defined?(:@keep)
p fa.instance_variables
begin
  fa.instance_variable_set(:@x, 1)
rescue FrozenError => e
  puts "arr-frozen-set: #{e.message}"
end
begin
  fa.remove_instance_variable(:@absent)
rescue FrozenError => e
  puts "arr-frozen-remove: #{e.message}"
end

# ---- Hash ----------------------------------------------------------------
h = { k: 1 }
p h.instance_variable_get(:@hx)
p h.instance_variable_set(:@hx, "hv")
p h.instance_variable_get(:@hx)
p h.instance_variable_defined?(:@hx)
p h.instance_variables
hd = h.dup
p hd.instance_variables
hc = h.clone
p hc.instance_variable_get(:@hx)
p h.remove_instance_variable(:@hx)
p h.instance_variables

fh = {}.freeze
begin
  fh.instance_variable_set(:@x, 1)
rescue FrozenError => e
  puts "hash-frozen-set: #{e.message}"
end
p fh.instance_variable_get(:@x)
p fh.instance_variable_defined?(:@x)
p fh.instance_variables

# ---- String --------------------------------------------------------------
s = +"str"
p s.instance_variable_set(:@w, 9)
p s.instance_variable_get(:@w)
p s.instance_variables

# dup/clone copy the side-table entry now (was: silently dropped).
sd = s.dup
p sd.instance_variable_get(:@w)
sd.instance_variable_set(:@w, :other)
p s.instance_variable_get(:@w)
sc = s.clone
p sc.instance_variables

# Frozen string: set AND remove raise FrozenError (inspect rendering).
sf = s.dup
sf.freeze
begin
  sf.instance_variable_set(:@z, 1)
rescue FrozenError => e
  puts "str-frozen-set: #{e.message}"
end
begin
  sf.remove_instance_variable(:@w)
rescue FrozenError => e
  puts "str-frozen-remove: #{e.message}"
end
p s.remove_instance_variable(:@w)
p s.instance_variables

# ---- immediates keep CRuby's frozen answers -------------------------------
p 5.instance_variable_get(:@x)
begin
  5.instance_variable_set(:@x, 1)
rescue FrozenError => e
  puts "int-set: #{e.message}"
end
begin
  :sym.instance_variable_set(:@x, 1)
rescue FrozenError => e
  puts "sym-set: #{e.message}"
end
begin
  nil.instance_variable_set(:@x, 1)
rescue FrozenError => e
  puts "nil-set: #{e.message}"
end

# ---- name validation stays shared -----------------------------------------
begin
  [].instance_variable_set(:x, 1)
rescue NameError => e
  puts "badname-set: #{e.message}"
end
begin
  {}.instance_variable_get("y")
rescue NameError => e
  puts "badname-get: #{e.message}"
end
p [].instance_variable_get("@ok")

# ---- warm loop: the reflection arms under repeated dispatch ---------------
acc = nil
60.times do |i|
  arr = [i]
  arr.instance_variable_set(:@n, i)
  acc = arr.instance_variable_get(:@n)
end
p acc
