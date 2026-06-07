# Array#insert(index, *objs): non-negative inserts before the index
# (padding with nils past the end); negative inserts after; returns self.
p [1, 2, 3].insert(1, :a)
p [1, 2, 3].insert(1, :a, :b)
p [1, 2, 3].insert(-1, :z)
p [1, 2, 3].insert(-2, :y)
p [1, 2, 3].insert(5, :pad)
p [1, 2, 3].insert(0, :head)
a = [1, 2, 3]; a.insert(1, :x); p a   # mutates
begin; [1, 2].insert(-5, :x); rescue => e; p [e.class, e.message]; end
