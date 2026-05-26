# `Module.new` and `Module.new { |m| ... }` — anonymous modules.
# ADR 0017 Tier 1 fill-out: scripts can now build fresh
# Module shells at runtime, mostly for `include`-into-class
# DSL patterns.
#
# Documented divergence NOT exercised (Tier 1 follow-up):
#   - `.to_s` / `.inspect` on anonymous: CRuby renders
#     `"#<Module:0x...address>"`; rubyrs renders the bare
#     `"#<Module>"` because ADR 0017 keeps object-id rendering
#     out of Tier 1's deterministic surface.
#   - `M = Module.new` does NOT promote the module's name to
#     "M" in rubyrs (that needs a StoreConst hook). Fixture
#     stays off the name-promote path.

# No-block form: fresh anonymous module.
m = Module.new
puts m.class.name                       # "Module"
puts m.is_a?(Module)                    # true
puts m.name.inspect                     # nil
puts m.empty? rescue puts "no empty?"   # no empty? — sanity

# Two separate Module.new calls produce distinct objects.
a = Module.new
b = Module.new
puts a == b                             # false
puts a.equal?(b)                        # false
puts a == a                             # true

# Block form — body evaluated as the module body, so `def`
# inside lands on the module's methods table.
mixin = Module.new do
  def announce; "from-mixin"; end
  def repeat(x); x * 2; end
end

# `include` the anonymous module into a class — the canonical
# use case.
class Receiver
end
Receiver.include(mixin)
r = Receiver.new
puts r.announce                         # "from-mixin"
puts r.repeat(7)                        # 14
puts r.is_a?(Receiver)                  # true

# Block receives the module as its sole positional arg —
# useful for ref-based shapes.
captured = nil
m2 = Module.new do |inner|
  captured = inner
end
puts captured.equal?(m2)                # true (block-arg is the same module)

# Multiple includes — each anonymous module is independent.
a_mod = Module.new { def kind; :a; end }
b_mod = Module.new { def kind; :b; end }
class MultiHost
end
MultiHost.include(a_mod)
puts MultiHost.new.kind                 # :a (first include in lookup order)
# Re-include b on top to override:
MultiHost.include(b_mod)
puts MultiHost.new.kind                 # :b — second include wins

# Arity guard — `Module.new(42)` raises ArgumentError.
begin
  Module.new(42)
rescue ArgumentError => e
  puts "AE: #{e.message}"
end

# Mixed: build, include, dispatch through chain.
# (`Class.new` anonymous-class form would go here too, but it
# falls back to the generic allocator in rubyrs today — Tier 1
# follow-up. Stick to a named Class for now.)
greet_mod = Module.new do
  def hello(name); "hello, #{name}"; end
end
class GreetHost
end
GreetHost.include(greet_mod)
puts GreetHost.new.hello("world")       # "hello, world"
