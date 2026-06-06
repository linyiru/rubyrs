# `alias_method :new, :old` (method-call form) inside `class << self`
# aliases a CLASS method, landing on the singleton table — same as
# the `alias` keyword. Discovery: P3 Jekyll spike — addressable's
# uri.rb uses `class << self; alias_method :escape_component,
# :encode_component; end` (and several more).
class Foo
  def self.encode_component(x); "enc:#{x}"; end
  class << self
    alias_method :escape_component, :encode_component
  end

  def self.unencode(x); "unenc:#{x}"; end
  # multiple alias_method calls in one singleton body, in order.
  class << self
    alias_method :unescape, :unencode
    alias_method :unencode_component, :unencode
    alias_method :unescape_component, :unencode
  end
end

puts Foo.escape_component("a")
puts Foo.unescape("b")
puts Foo.unencode_component("c")
puts Foo.unescape_component("d")

# the alias is a real class method, callable + introspectable.
puts Foo.respond_to?(:escape_component)
puts Foo.respond_to?(:unescape_component)

# the original still works, and alias shares its behaviour after a
# redefinition of the original does NOT change the alias (alias snapshots).
puts Foo.encode_component("z")
