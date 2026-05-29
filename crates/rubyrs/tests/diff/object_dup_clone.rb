# Object#dup / Object#clone — universal shallow-copy arms for
# receivers without a specialized primitive arm. Primitive
# arms in vm/string.rs / vm/array.rs / vm/hash.rs handle their
# own receivers; this fixture covers what the universal arm
# catches: immediates and plain Object instances.

# Immediates — CRuby (since Ruby 2.4) returns self for
# every immediate variant. Before this commit each one raised
# NoMethodError.
puts 5.dup
puts 5.clone
puts nil.dup.inspect
puts true.dup
puts false.dup
puts :foo.dup
puts 1.5.dup
puts 1.5.clone

# Same-identity guarantee for immediates
puts 42.dup.equal?(42)
puts :foo.clone.equal?(:foo)
puts nil.clone.equal?(nil)

# Plain Object — fresh Instance, shallow-cloned ivars
class C
  def initialize
    @x = 1
    @y = [1, 2]
  end
  attr_accessor :x, :y
end

c = C.new
d = c.dup
puts d.x                      # 1 — ivar copied
puts d.equal?(c)              # false — distinct object
puts d.y.equal?(c.y)          # true — shallow (Array ref shared)
c.x = 99
puts d.x                      # 1 — ivar tables independent

# clone works the same way
e = c.clone
puts e.x
puts e.equal?(c)
puts e.y.equal?(c.y)

# Subclasses preserve their class
class D < C; end
d2 = D.new
puts d2.dup.class.name
puts d2.clone.class.name

# Arity guard — CRuby ArgumentError for extra positional args
# (clone(freeze:) kwarg routing is a Tier-2 follow-up, see PR
# commit message; for now extra positionals raise).
begin
  5.dup(1)
rescue ArgumentError
  puts "dup-extra-arg"
end
begin
  5.clone(1)
rescue ArgumentError
  puts "clone-extra-arg"
end

# respond_to? must agree with dispatch
puts 42.respond_to?(:dup)
puts 42.respond_to?(:clone)
puts nil.respond_to?(:dup)
puts Object.new.respond_to?(:clone)
