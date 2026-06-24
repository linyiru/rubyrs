# define_method can take a Bound/UnboundMethod whose source is a BUILT-IN
# native method with no Proto body — install a name-forwarding method that
# re-dispatches the original builtin on the new receiver. dry-struct does
# `define_method(:prepend, ::Module.method(:prepend))` ON THE SINGLETON to
# restore the native prepend over dry-types' Builder#prepend (the
# eigenclass has Module among its ancestors, so the bind is valid).
class Foo
  def self.prepend(*a) = "shadowed"
  class << self
    define_method(:prepend, ::Module.method(:prepend))
  end
end
module M; def hi = "hi"; end
Foo.prepend(M)
p Foo.new.hi
p Foo.ancestors.first(2)

# Same shape with const_set (another native Module method) on the singleton.
class Baz
  def self.const_set(*a) = "shadowed"
  class << self
    define_method(:const_set, ::Module.method(:const_set))
  end
end
Baz.const_set(:ANSWER, 42)
p Baz::ANSWER
