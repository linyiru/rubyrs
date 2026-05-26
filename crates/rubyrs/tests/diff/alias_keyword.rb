# `alias` keyword form (`alias new old`) — the syntactic
# counterpart to the call form `alias_method :new, :old`.
# Prism parses each operand as a SymbolNode; rubyrs desugars
# the keyword node into a synthetic `alias_method` call so
# the existing compile-time intercept (Op::AliasMethod) does
# the actual work. Both forms should now behave identically.

class Greeter
  def hello
    "hello from Greeter"
  end
  # Keyword form — bare identifiers, no colon, no commas.
  alias hi hello
  # Multiple aliases of the same source method.
  alias hey hello
end

g = Greeter.new
puts g.hello
puts g.hi
puts g.hey

# Aliasing then overriding the original: the alias keeps
# pointing at the original Method (CRuby semantics — alias
# captures by identity, not by name). This matches rubyrs's
# Op::AliasMethod copy-the-Rc<Method>-entry behaviour.
class Greeter
  alias original_hello hello
  def hello
    "OVERRIDDEN"
  end
end

g2 = Greeter.new
puts g2.hello          # OVERRIDDEN
puts g2.original_hello # still "hello from Greeter"

# Inside a class body, alias works on instance methods. Mix
# with attr_accessor expansions and `def` to prove the call
# integrates with the rest of the class-body machinery.
class Counter
  attr_accessor :count
  def initialize
    @count = 0
  end
  def increment
    @count += 1
  end
  alias inc increment
end

c = Counter.new
3.times { c.inc }
puts c.count

# Alias of an inherited method — alias should resolve `old`
# by walking the ancestor chain.
class Base
  def speak; "base"; end
end
class Child < Base
  alias say speak
end
puts Child.new.say
