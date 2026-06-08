# A user-defined singleton method overrides the built-in Module/Class
# `name` / `to_s` / `inspect` (CRuby parity). rouge's Token DSL relies on
# this: `class << self; attr_reader :name; end` makes `Token.name` read
# the @name ivar, not the structural class name.

# direct def self.name
class Direct
  def self.name; "OVERRIDDEN"; end
end
p Direct.name                                  # "OVERRIDDEN"

# inherited class<<self attr_reader (the rouge shape)
class Base
  class << self
    attr_reader :name
  end
end
class Sub < Base
  @name = :hello
end
p Sub.name                                     # :hello
Base.instance_variable_set(:@name, :base_name)
p Base.name                                    # :base_name

# anonymous Class.new(Base) with @name set in the body
k = Class.new(Base) do
  @name = :anon
end
p k.name                                       # :anon

# to_s / inspect overrides
class Stringy
  def self.to_s; "stringy-to-s"; end
  def self.inspect; "stringy-inspect"; end
end
p Stringy.to_s                                 # "stringy-to-s"
p Stringy.inspect                              # "stringy-inspect"
puts "interp: #{Stringy}"                      # interp: stringy-to-s

# control: no override → structural name still works
class Normal; end
p Normal.name                                  # "Normal"
p Normal.to_s                                  # "Normal"
module Modz; end
p Modz.name                                    # "Modz"

# a token-chain style mini-DSL (the rouge mechanism distilled)
class Tok
  class << self
    attr_reader :tname
    attr_reader :name
  end
  def self.make(n)
    Class.new(self) do
      @name = n
      @tname = n
    end
  end
end
Leaf = Tok.make(:Leaf)
p Leaf.name                                     # :Leaf
p Leaf.ancestors.take_while { |x| x != Tok }.reverse.map(&:name)  # [:Leaf]
