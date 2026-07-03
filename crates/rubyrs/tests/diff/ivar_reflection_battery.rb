# Permanent reflection battery for the flat ivar layout (ADR 0035 Ph4/5):
# per-class union shape tables + slot-indexed instance storage must be
# invisible to every reflection surface. Each section stresses a specific
# consumer: instance_variables ORDER is per-object assignment order (NOT
# the class's union-table slot order), defined? distinguishes assigned-nil
# from undefined, remove_instance_variable really removes (and re-assign
# re-appends to the order), frozen objects raise, dup/clone copy ivars
# (shared refs stay shared), eval-defined ivars work.

class Shape
  def set_xy; @x = 1; @y = 2; self; end
  def set_yx; @y = 20; @x = 10; self; end
  def set_z_only; @z = 99; self; end
end

# --- instance_variables order: per-object assignment order ---
a = Shape.new.set_xy
b = Shape.new.set_yx   # same class, REVERSE assignment order
c = Shape.new.set_z_only # only a later-slot name: holes must not leak
p a.instance_variables
p b.instance_variables
p c.instance_variables
p [a.instance_variable_get(:@x), a.instance_variable_get(:@y)]
p [b.instance_variable_get(:@x), b.instance_variable_get(:@y)]
# undefined ivar on an object whose CLASS knows the name reads nil:
p c.instance_variable_get(:@x)

# --- defined? vs assigned nil ---
d = Shape.new
d.instance_variable_set(:@x, nil)
p d.instance_variable_defined?(:@x)   # true — assigned nil
p d.instance_variable_defined?(:@y)   # false — class knows @y, this object doesn't
p d.instance_variable_defined?(:@never_anywhere) # false — class never saw it
p d.instance_variables

# --- remove_instance_variable: removes, order drops, re-add re-appends ---
e = Shape.new.set_xy
p e.remove_instance_variable(:@x)     # 1
p e.instance_variables                # [:@y]
p e.instance_variable_defined?(:@x)   # false
p e.instance_variable_get(:@x)        # nil
e.instance_variable_set(:@x, 111)     # re-add → goes to the END of the order
p e.instance_variables                # [:@y, :@x]
begin
  e.remove_instance_variable(:@gone)
rescue NameError => ex
  puts "NameError: #{ex.class}"
end

# --- frozen objects: writes raise, reads fine ---
f = Shape.new.set_xy.freeze
p f.instance_variable_get(:@x)
begin
  f.instance_variable_set(:@x, 2)
rescue FrozenError
  puts "FrozenError on ivar set"
end
begin
  def f.poke; @x = 3; end
  f.poke
rescue FrozenError
  puts "FrozenError on @x="
end

# --- dup / clone: ivars copied, shared refs stay shared ---
shared = [1, 2]
g = Shape.new
g.instance_variable_set(:@list, shared)
g.instance_variable_set(:@n, 5)
g2 = g.dup
p g2.instance_variables
p g2.instance_variable_get(:@n)
g2.instance_variable_get(:@list) << 3
p shared                       # dup shares the ref → [1, 2, 3]
g3 = g.clone
p g3.instance_variables
p g3.instance_variable_get(:@list).equal?(shared)

# --- define/read via eval / instance_eval ---
h = Shape.new
h.instance_eval { @via_eval = :ok }
p h.instance_variable_defined?(:@via_eval)
p h.instance_eval { @via_eval }
p h.instance_variables

# --- many ivars (past the inline-4 slots AND the 8-name linear-scan cap) ---
big = Object.new
12.times { |i| big.instance_variable_set(:"@v#{i}", i * i) }
p big.instance_variables
p 12.times.map { |i| big.instance_variable_get(:"@v#{i}") }
big.remove_instance_variable(:@v5)
p big.instance_variables.size
p big.instance_variable_get(:@v5)

# --- polymorphic siblings: same names, per-object order kept straight ---
class Kid1 < Shape; end
class Kid2 < Shape; end
k1 = Kid1.new.set_xy
k2 = Kid2.new.set_yx
p [k1.instance_variables, k2.instance_variables]
p [k1.instance_variable_get(:@y), k2.instance_variable_get(:@y)]
