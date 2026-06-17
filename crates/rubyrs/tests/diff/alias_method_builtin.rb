# alias_method / alias of a UNIVERSAL Object/Kernel builtin (dup, clone, hash,
# freeze, inspect, class, object_id, …). These live in native dispatch, not any
# method table, so aliasing them used to raise "undefined method" — both the
# literal path (Op::AliasMethod) and the runtime `alias_method "#{m}!", m` form.
# Surfaced by ostruct (`give_access = instance_methods; alias_method "#{m}!", m`).

# literal alias_method (compiler Op path)
class A
  alias_method :dup2, :dup
  alias_method :klass, :class
  alias_method :frozen2, :frozen?
end
a = A.new
p a.dup2.class                 # A
p a.klass                      # A
p a.frozen2                    # false

# `alias` keyword (also Op path)
class B
  alias inspect2 inspect
end
p B.new.inspect2.start_with?("#<B")  # true

# runtime alias_method with computed names over instance_methods (ostruct shape)
class C
  give_access = instance_methods
  give_access.each do |m|
    next if m.match(/\W$/)
    alias_method "#{m}!", m
  end
end
c = C.new
p c.send(:class!)              # C
p c.send(:hash!).is_a?(Integer)# true
p c.send(:object_id!).is_a?(Integer) # true

# a genuinely-undefined source still raises NameError
begin
  Class.new { alias_method :x, :totally_not_a_method }
  puts "no error"
rescue NameError => e
  puts "NameError"
end
