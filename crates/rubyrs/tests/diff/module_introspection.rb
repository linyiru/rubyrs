# `Module#instance_methods` / `superclass` / `constants`
# introspection. Pre-fix these all raised NoMethodError
# because the Class arms in `do_call` didn't implement
# them.
#
# Documented Tier 1 divergences (NOT exercised here):
#   - rubyrs's `instance_methods` walk bottoms out at the
#     first class with no superclass. CRuby's chain always
#     terminates at BasicObject, so `Foo.instance_methods`
#     (no arg) includes ~50 Object-provided methods rubyrs
#     doesn't add. Fixture uses the `false` arg to skip
#     inherited methods, which both sides agree on.
#   - rubyrs `Foo.superclass` returns `nil` when no
#     explicit parent was specified; CRuby returns Object.
#     Fixture only tests `superclass` on explicitly-
#     parented classes.
#   - `Module#include?(other)` requires `other` to be a
#     Module in CRuby (raises TypeError on Class).
#     rubyrs accepts both. Fixture passes a Module on the
#     RHS.
#   - `private_instance_methods` etc. don't model
#     visibility filtering — all four variants return the
#     same list. Fixture stays away from the visibility-
#     diverging path.

class Animal
  def name_method; "animal"; end
  def sound; "..."; end
end

class Dog < Animal
  def bark; "woof"; end
end

# Own methods only (`false` arg) — both sides agree.
puts Animal.instance_methods(false).sort.inspect
puts Dog.instance_methods(false).sort.inspect

# `superclass` on explicitly-parented class.
puts Dog.superclass.name

# Modules — `instance_methods` walks the module's own
# table.
module Greeter
  def hello; "hi"; end
  def bye; "bye"; end
end
puts Greeter.instance_methods(false).sort.inspect

# Inclusion: `include?` accepts a Module on the RHS in
# both CRuby and rubyrs.
class Hello
  include Greeter
  def own_method; end
end
puts Hello.include?(Greeter)

# `instance_methods` after include picks up the module's
# methods. We check membership rather than full equality
# because rubyrs's chain misses the Object-provided
# tail.
methods_with_inherited = Hello.instance_methods.sort
expected = [:bye, :hello, :own_method]
expected.each { |m| puts methods_with_inherited.include?(m) }

# `method_defined?` agrees with `instance_methods`.
puts Hello.method_defined?(:hello)
puts Hello.method_defined?(:own_method)
puts Hello.method_defined?(:nope)

# Each method_name is a Symbol.
puts Hello.instance_methods(false).first.class
