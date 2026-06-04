# Bare `super` from a method with a single splat parameter
# splat-forwards the captured args to the parent. Hit by the
# `def initialize(*); super; end` shape in
# rack-protection-4.2.1's HostAuthorization and EscapedParams
# middlewares.
#
# CRuby semantics: `def m(*); super; end` and `def m(*args);
# super; end` both pass the ORIGINAL positional args unchanged
# to the parent. The parent's named parameters receive the
# individual values, not a single Array.

# Anonymous splat — the bare `def m(*)` form. The parent here
# declares two named parameters; bare super must spread the
# captured args back to them.
class A
  attr_reader :app, :opts
  def initialize(app, opts = {})
    @app = app
    @opts = opts
  end
end
class B < A
  def initialize(*)
    super
    @marker = :b_init_ran
  end
  attr_reader :marker
end
b = B.new(:my_app, key: "value")
puts "anon_app=#{b.app}"
puts "anon_opts=#{b.opts.inspect}"
puts "anon_marker=#{b.marker}"

# Named splat — `def m(*args); super; end`. Same semantics: the
# named splat collects all positional args, then bare super
# forwards them splatted.
class C < A
  def initialize(*args)
    super
  end
end
c = C.new(:other_app, retries: 3, backoff: 1.5)
puts "named_app=#{c.app}"
puts "named_opts=#{c.opts.inspect}"

# Single splat with a single arg — degenerates to passing one
# positional arg through to the parent's first slot.
d = B.new(:lone)
puts "lone_app=#{d.app}"
puts "lone_opts=#{d.opts.inspect}"

# Multi-level inheritance — bare super at each level forwards
# through the whole chain.
class E < B
  def initialize(*)
    super
  end
end
e = E.new(:e_app, mode: :prod)
puts "multilevel_app=#{e.app}"
puts "multilevel_opts=#{e.opts.inspect}"
puts "multilevel_marker=#{e.marker}"

# Bare super in a non-rest method still pushes individual
# locals (the existing forwarding path), not an Array. This
# scenario locks in that the new single-splat fast path doesn't
# regress the fixed-arity case.
class F
  def m(a, b)
    a + b
  end
end
class G < F
  def m(a, b)
    super + 100
  end
end
puts "fixed_arity=#{G.new.m(1, 2)}"
