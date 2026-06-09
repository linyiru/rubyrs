# Inline-constant-cache invalidation: a warmed read must see every kind
# of mutation that changes resolution (const_set shadowing, reopen
# defining a nested const, include changing the ancestor walk, anon
# class naming).
FOO = 1
3.times { p FOO }                       # warm the flat cache

class A1; V = 1; end
class B1 < A1; def g; V; end; end
b = B1.new
p b.g                                   # 1 via ancestor A1 (warms chain)
p b.g
B1.const_set(:V, 2)                     # closer shadow appears
p b.g                                   # 2 — cache must invalidate

class K; X = 1; def get; X; end; end
k = K.new
p k.get; p k.get                        # warm
module M; CCC = 99; end
class L; def get; CCC; end; end
l = L.new
class L; include M; end
p l.get                                 # 99 (via include)

c = Class.new
c.const_set(:Z, 7)
Holder = c                              # anon naming re-homes Z
p Holder::Z

module N1; end
begin; p N1::W; rescue NameError; puts "w miss"; end
module N1; W = 5; end                   # reopen adds the const
p N1::W

class P1; Q = 1; def g; Q; end; end
p P1.new.g
P1.const_set(:Q2, 42)
p P1::Q2

s = 0; 2000.times { s += FOO }; p s     # steady-state hit path
