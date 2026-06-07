# An anonymous class assigned to a constant (Struct.new) holds its
# attr list as a class-level ivar (@__struct_attrs), and its
# `initialize` is a define_method closure with `*args`. Both must
# survive GC. Exercises the class-ivar rooting + define_method-init
# *args pinning fixes (under STRESS_GC in CI).
S = Struct.new(:a, :b, :c)
out = []
200.times { |i| s = S.new(i, i * 2, i * 3); out << s.a + s.b + s.c }
p out.first(3)
p out.last(3)
p out.sum
p S.members rescue p S.new(1, 2, 3).members
