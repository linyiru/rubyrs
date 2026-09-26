# Method resolution is cached per (class, name) until a method-table or
# ancestry change. Every change below must be visible on the next call.
class A; def m = :a; end
class B < A; end
b = B.new
p b.m, b.respond_to?(:n)

module Pre; def m = [:pre, super]; end
module Inc; def n = :inc; end
B.include(Inc)
p b.m, b.n, b.respond_to?(:n)
B.prepend(Pre)
p b.m
class B; def m = :b; end
p b.m
class B; remove_method :m; end
p b.m
class A; undef_method :m; end
p((b.m rescue :undef))
class A; def m = :a2; end
p b.m

# Singleton methods and extend on a class.
class K; def self.s = :k; end
class L < K; end
p L.s
module Ext; def s = :ext; end
L.extend(Ext)
p L.s
L.define_singleton_method(:s) { :own }
p L.s

# Per-object singleton and extend.
o = A.new
p o.m
def o.m = :single
p o.m, A.new.m
o2 = A.new
o2.extend(Module.new { def m = :extended })
p o2.m

# alias / attr / define_method / visibility.
class A
  alias_method :m2, :m
  attr_accessor :v
  define_method(:d) { :d }
end
a = A.new
a.v = 3
p a.m2, a.v, a.d
class A; private :d; end
p((a.d rescue :private))
class A; public :d; end
p a.d

# method_missing and respond_to_missing? appear and disappear.
class Mm; end
mm = Mm.new
p((mm.zz rescue :nm))
class Mm; def method_missing(n, *) = n == :zz ? :mm : super; def respond_to_missing?(n, p = false) = n == :zz || super; end
p mm.zz, mm.respond_to?(:zz)
class Mm; remove_method :method_missing; end
p((mm.zz rescue :nm2))

# Array#each override seen by a native iterator path.
class MyArr < Array; end
x = MyArr[1, 2]
p x.map { _1 * 2 }
class MyArr; def each = yield(:over); end
p x.to_a, x.map { _1 }

# Many short-lived anonymous classes: a freed class must never serve a
# new one's lookups.
res = 2000.times.map do |i|
  c = Class.new { i.even? ? define_method(:q) { :even } : define_method(:r) { :odd } }
  c.new.respond_to?(:q) ? c.new.q : c.new.r
end
GC.start
p res.tally
