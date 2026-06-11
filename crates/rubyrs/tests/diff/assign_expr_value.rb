# Assignment-syntax expression value (Op::CallAset): CRuby evaluates
# `recv.attr = v` / `recv[k] = v` (and the op-assign desugars) to the
# RHS, discarding the writer's return value; `send(:attr=, v)` keeps
# the return (the rule is purely syntactic — prism ATTRIBUTE_WRITE).
# Covers: user []= override, Hash subclass, attr_writer, send form,
# implicit-return position, +=/||=/&&=, safe-nav, method_missing.
# matrix: assignment-syntax RHS rule
class H2 < Hash; def []=(k,v); "sub"; end; end
h2 = H2.new
p (h2[1] = 7)         # subclass override: still RHS
class C; def foo=(v); "W"; end; attr_writer :bar; end
c = C.new
p (c.foo = 9)
p (c.bar = 8)         # attr_writer
p c.send(:foo=, 3)    # send keeps return
x = (c.foo = [1,2])
p x
def setit(c); c.foo = "ret"; end   # implicit return position
p setit(c)
h = {}
p (h["k"] = 5)        # plain hash (fast path)
p (h["k"] += 2)       # op-assign: value = computed
p (h["z"] ||= 4)      # or-assign
p (h["z"] &&= 6)      # and-assign
o = nil
p (o&.foo = 5) rescue p "safe-nav-err"
class M; def method_missing(n, *a); "mm"; end; def respond_to_missing?(*); true; end; end
m = M.new
p (m.zzz = 11)        # method_missing assignment: still RHS
