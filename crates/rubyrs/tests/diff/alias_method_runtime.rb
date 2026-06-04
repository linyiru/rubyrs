# `alias_method` runtime dispatch — the path that fires when the
# arguments aren't both Symbol literals (compile-time intercept
# only catches the literal-literal shape).
#
# Surfaced by rack-protection's
#   def self.default_reaction(reaction)
#     alias_method(:default_reaction, reaction)
#   end
# where `reaction` is a method parameter, not a literal.

# Bareword from inside a class method (self is the Class).
class Foo
  def hello; "world"; end
  def self.make_alias(new_name, old_name)
    alias_method(new_name, old_name)
  end
  make_alias(:greet, :hello)
end
puts Foo.new.greet

# Explicit receiver form — Klass.alias_method(:new, :old).
class Bar
  def shout; "BAR"; end
end
Bar.alias_method(:yell, :shout)
puts Bar.new.yell

# Aliases share the underlying Rc<Method> — the alias is
# semantically identical to the original.
class Baz
  def color; "red"; end
end
Baz.alias_method(:colour, :color)
puts Baz.new.colour == Baz.new.color

# String args work too (CRuby's contract is Symbol or String).
class Qux
  def name; "qux"; end
end
Qux.alias_method("title", "name")
puts Qux.new.title

# Unknown old name raises NameError.
class Mug
  def cup; "drink"; end
end
begin
  Mug.alias_method(:missing, :nope)
rescue NameError => e
  puts "NameError: #{e.message}"
end

# Returns the new method name as a Symbol (CRuby's Ruby 3.x
# contract — older docs said "returns the receiver" but the
# actual return is the alias name Symbol).
class Vase
  def water; "wet"; end
end
result = Vase.alias_method(:aqua, :water)
p result
p result.class
