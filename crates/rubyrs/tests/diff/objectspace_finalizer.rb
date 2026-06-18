# ObjectSpace.define_finalizer / undefine_finalizer — no-op in rubyrs
# (no GC finalizer hook). Registration is accepted; CRuby returns
# `[0, callable]` and the finalizer simply never fires here.
o = Object.new
pr = proc { }
p ObjectSpace.respond_to?(:define_finalizer)
r = ObjectSpace.define_finalizer(o, pr)
p [r.class.name, r.length, r[0], r[1].equal?(pr)]
rb = ObjectSpace.define_finalizer(o) { }
p [rb.class.name, rb.length, rb[0], rb[1].is_a?(Proc)]
p ObjectSpace.undefine_finalizer(o).equal?(o)
